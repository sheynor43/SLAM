//! The FPS counter over the screens draws its frames without allocating: shown and hidden,
//! in every corner, on the menus and in gameplay (ADR-0060).

use std::sync::Arc;

use slam_app::play::fps::{FpsDisplay, FpsOverlay, FpsSource, MAX_FPS_GLYPHS};
use slam_app::settings::FpsScreens;
use slam_engine::{DrawRateControl, EngineCounters, UpdateRateControl};
use slam_play::{FpsCorner, FpsLabels};
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Extent, Limits, Origin};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};

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
    let source = FpsSource::new(
        Arc::clone(&counters),
        UpdateRateControl::new(),
        DrawRateControl::new(),
    );
    let placeholder = slam_play::hud::OFFSET_PLACEHOLDER;
    let labels = FpsLabels {
        frame_time: format!("{placeholder} ms"),
        fps: format!("{placeholder} fps"),
    };
    let mut overlay = FpsOverlay::new(source, registry, &labels);
    overlay.request_glyphs(&mut glyphs);
    let mut shapes = SpriteBatch::with_capacity(1);
    let mut text = SpriteBatch::with_capacity(MAX_FPS_GLYPHS);
    assert_no_alloc(|| {
        for frame in 0..4000_u64 {
            let display = FpsDisplay {
                show: frame % 1000 < 800,
                corner: FpsCorner::ALL[(frame / 250) as usize % 4],
                screens: FpsScreens::ALL[(frame / 1300) as usize % 3],
            };
            let gameplay = frame % 600 < 300;
            overlay.frame(frame * 1_000_000, 1280.0, 720.0, display, gameplay);
            shapes.clear();
            text.clear();
            overlay.push(&mut shapes, &mut text, &mut glyphs, WHITE);
        }
    });
}
