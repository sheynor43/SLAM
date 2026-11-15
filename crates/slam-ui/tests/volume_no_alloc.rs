//! The volume overlay's steady state does not touch the allocator (ADR-0145): the mixer's
//! input on the update side and `VolumeView::push` on the draw side once warm, with every
//! value a meter can show and sparks alive.

use std::sync::Arc;

use slam_render::Extent;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Limits, Origin};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::theme::Theme;
use slam_ui::volume::{
    Edge, MAX_VOLUME_GLYPHS, MAX_VOLUME_SHAPES, MeterStyle, VolumeLabels, VolumeLayout,
    VolumeMixer, VolumeView,
};

slam_testkit::install_counting_allocator!();

const MS: u64 = 1_000_000;

#[test]
fn mixer_input_does_not_allocate() {
    let layout = VolumeLayout::new(
        Edge::Right,
        MeterStyle::Column,
        1280.0,
        720.0,
        &Theme::default(),
    );
    let mut mixer = VolumeMixer::new();
    let mut levels = [0.5; 4];
    assert_no_alloc(|| {
        for k in 0..200u64 {
            let now = k * 20 * MS;
            mixer.adjust(
                &mut levels,
                if k % 3 == 0 { -1.0 } else { 1.0 },
                k % 2 == 0,
                now,
            );
            mixer.step_meter(k % 5 == 0, now);
            mixer.pointer(1200.0, 360.0, &layout, now);
            mixer.wheel_over(&mut levels, 1.0, false, 1200.0, 360.0, &layout, now);
            mixer.tick(now);
            std::hint::black_box(mixer.frame(Edge::Right, MeterStyle::Column, &levels));
        }
        mixer.toggle_mute(5000 * MS);
        std::hint::black_box(mixer.take_save());
    });
}

#[test]
fn the_view_pushes_without_allocating_once_warm() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let limits = Limits {
        max_texture_size: 4096,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits)).unwrap();
    let labels = VolumeLabels {
        meters: ["ХИТСАУНДЫ", "ЭФФЕКТЫ", "ОБЩАЯ", "МУЗЫКА"].map(String::from),
        max: "МАКС".into(),
        muted: "БЕЗ ЗВУКА".into(),
    };
    let mut view = VolumeView::new(registry, Theme::default(), &labels);
    let white = AtlasRegion {
        texture: TextureId(0),
        origin: Origin::new(0, 0),
        size: Extent::new(1, 1),
        uv: UvRect::FULL,
    };
    let (mut shapes, mut text) = (
        ShapeBatch::with_capacity(MAX_VOLUME_SHAPES),
        SpriteBatch::with_capacity(MAX_VOLUME_GLYPHS),
    );
    let mut mixer = VolumeMixer::new();
    let mut levels = [0.0, 0.5, 1.0, 0.33];
    mixer.show(MS);
    view.lay_out_for(1920.0, 1080.0);
    let mut now = 2 * MS;
    // Warm: the glyphs are asked for; the layout was made above.
    shapes.clear();
    text.clear();
    // Warm every style once: their values differ in size.
    for style in MeterStyle::ALL {
        let frame = mixer.frame(Edge::Top, style, &levels);
        view.push(
            &mut shapes,
            &mut text,
            &mut glyphs,
            &frame,
            1920.0,
            1080.0,
            now,
            white,
        );
    }
    let frame = mixer.frame(Edge::Top, MeterStyle::Arc, &levels);
    view.push(
        &mut shapes,
        &mut text,
        &mut glyphs,
        &frame,
        1920.0,
        1080.0,
        now,
        white,
    );
    assert_no_alloc(|| {
        for k in 0..300u64 {
            now += 7 * MS;
            // Every value from 0 to 100, MAX and muted, sparks at every step.
            mixer.adjust(&mut levels, if k < 150 { 1.0 } else { -1.0 }, false, now);
            if k == 200 {
                mixer.toggle_mute(now);
            }
            let style = MeterStyle::ALL[(k / 100) as usize % MeterStyle::ALL.len()];
            let frame = mixer.frame(Edge::Top, style, &levels);
            shapes.clear();
            text.clear();
            view.push(
                &mut shapes,
                &mut text,
                &mut glyphs,
                &frame,
                1920.0,
                1080.0,
                now,
                white,
            );
        }
    });
}
