//! The menu's steady state does not touch the allocator: input on the update side and
//! `MenuView::push` on the draw side once warm.

use std::sync::Arc;

use slam_render::Extent;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Limits, Origin};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::menu::{MainMenu, MenuInput, MenuLabels, MenuLayout, MenuSnapshot, MenuView};
use slam_ui::screen::Pose;
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

#[test]
fn menu_input_does_not_allocate() {
    let layout = MenuLayout::new(1280.0, 720.0, &Theme::default());
    let mut menu = MainMenu::new();
    assert_no_alloc(|| {
        for input in [
            MenuInput::Down,
            MenuInput::Up,
            MenuInput::PointerMove { x: 640.0, y: 400.0 },
            MenuInput::PointerPress { x: 1.0, y: 1.0 },
            MenuInput::Activate,
            MenuInput::Back,
        ] {
            std::hint::black_box(menu.input(input, &layout));
        }
        std::hint::black_box(menu.snapshot());
    });
}

#[test]
fn views_push_without_allocating_once_warm() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let limits = Limits {
        max_texture_size: 4096,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits)).unwrap();
    let labels = MenuLabels {
        title: "SLAM".into(),
        buttons: ["Play".into(), "Settings".into(), "Exit".into()],
    };
    let mut menu = MenuView::new(registry, Theme::default(), &labels);
    let white = AtlasRegion {
        texture: TextureId(0),
        origin: Origin::new(0, 0),
        size: Extent::new(1, 1),
        uv: UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
    };
    let (mut shapes, mut text) = (
        SpriteBatch::with_capacity(64),
        SpriteBatch::with_capacity(64),
    );
    let snapshot = MenuSnapshot { selected: 2 };
    let pose = Pose {
        opacity: 0.5,
        offset: 0.05,
    };
    let mut frame = |width: f32, height: f32, glyphs: &mut _| {
        shapes.clear();
        text.clear();
        menu.push(
            &mut shapes,
            &mut text,
            glyphs,
            width,
            height,
            &snapshot,
            pose,
            white,
        );
    };
    // Warm: the first frames at each size lay out and request glyphs (not allocation-free).
    frame(1280.0, 720.0, &mut glyphs);
    frame(800.0, 600.0, &mut glyphs);
    frame(1280.0, 720.0, &mut glyphs);
    assert_no_alloc(|| {
        for _ in 0..10 {
            frame(1280.0, 720.0, &mut glyphs);
        }
    });
    assert_eq!(shapes.len(), 3);
}
