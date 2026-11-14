//! The FPS counter over the screens draws its frames without allocating: shown and hidden,
//! in every corner and mode, with every thread's counters moving, on the menus and in
//! gameplay (ADR-0060, ADR-0138).

use std::sync::Arc;

use slam_app::play::fps::{
    AudioFeed, AudioMeter, AudioReading, FpsDisplay, FpsOverlay, FpsSource, MAX_FPS_GLYPHS,
    MAX_FPS_SHAPES,
};
use slam_app::settings::FpsScreens;
use slam_audio::{MusicStats, StreamCounters};
use slam_engine::{DrawRateControl, EngineCounters, UpdateRateControl};
use slam_input::InputCounters;
use slam_play::{FpsCorner, FpsLabels, FpsMode};
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Extent, Limits, Origin};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

const LIMITS: Limits = Limits {
    max_texture_size: 4096,
    max_uniform_block_size: 16384,
    max_uniform_buffer_slots: 8,
    max_texture_slots: 8,
};

const WHITE: AtlasRegion = AtlasRegion {
    texture: TextureId(0),
    origin: Origin { x: 0, y: 0 },
    size: Extent {
        width: 4,
        height: 4,
    },
    uv: UvRect::FULL,
};

#[test]
fn overlay_frames_do_not_allocate() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
    let counters = Arc::<EngineCounters>::default();
    let input = Arc::<InputCounters>::default();
    let stream = Arc::<StreamCounters>::default();
    let source = FpsSource::new(
        Arc::clone(&counters),
        UpdateRateControl::new(),
        DrawRateControl::new(),
    )
    .with_input(Arc::clone(&input))
    .with_audio(
        Arc::clone(&stream),
        Arc::<MusicStats>::default(),
        128,
        48_000,
    );
    let labels = FpsLabels::english();
    let mut overlay = FpsOverlay::new(source, registry, &labels);
    overlay.request_glyphs(&mut glyphs);
    let mut shapes = ShapeBatch::with_capacity(MAX_FPS_SHAPES);
    let theme = Theme::default();
    let mut text = SpriteBatch::with_capacity(MAX_FPS_GLYPHS);
    assert_no_alloc(|| {
        for frame in 0..4000_u64 {
            let display = FpsDisplay {
                show: frame % 1000 < 800,
                corner: FpsCorner::ALL[(frame / 250) as usize % 4],
                screens: FpsScreens::ALL[(frame / 1300) as usize % 3],
                mode: FpsMode::ALL[(frame / 90) as usize % 4],
            };
            let gameplay = frame % 600 < 300;
            input.record_poll();
            input.record_queued();
            stream
                .callbacks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            overlay.frame(frame * 1_000_000, 1280.0, 720.0, display, gameplay);
            shapes.clear();
            text.clear();
            overlay.push(&mut shapes, &theme, &mut text, &mut glyphs, WHITE);
            assert!(shapes.len() <= MAX_FPS_SHAPES);
        }
    });
}

/// The shell's way: the update thread feeds a meter across output switches (ADR-0139).
#[test]
fn metered_audio_does_not_allocate() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
    let meter = Arc::<AudioMeter>::default();
    let mut feed = AudioFeed::new(Arc::clone(&meter));
    let streams = [
        Arc::<StreamCounters>::default(),
        Arc::<StreamCounters>::default(),
    ];
    let music = [
        MusicStats::default(),
        MusicStats::default(),
        MusicStats::default(),
    ];
    let source = FpsSource::new(
        Arc::default(),
        UpdateRateControl::new(),
        DrawRateControl::new(),
    )
    .with_input(Arc::default())
    .with_audio_meter(Arc::clone(&meter));
    let labels = FpsLabels::english();
    let mut overlay = FpsOverlay::new(source, registry, &labels);
    overlay.request_glyphs(&mut glyphs);
    let mut shapes = ShapeBatch::with_capacity(MAX_FPS_SHAPES);
    let theme = Theme::default();
    let mut text = SpriteBatch::with_capacity(MAX_FPS_GLYPHS);
    let display = FpsDisplay {
        show: true,
        corner: FpsCorner::default(),
        screens: FpsScreens::Everywhere,
        mode: FpsMode::Full,
    };
    assert_no_alloc(|| {
        for frame in 0..3000_u64 {
            // Two outputs taking turns, with a gap between them.
            let stream = &streams[(frame / 700) as usize % 2];
            stream
                .callbacks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let live = frame % 700 > 50;
            feed.follow(live.then(|| {
                AudioReading::of(stream, &[&music[0], &music[1], &music[2]], 128, 48_000)
            }));
            overlay.frame(frame * 1_000_000, 1280.0, 720.0, display, false);
            shapes.clear();
            text.clear();
            overlay.push(&mut shapes, &theme, &mut text, &mut glyphs, WHITE);
        }
    });
}
