//! The menu cursor's frame (SLAM's and the skin's) does not touch the allocator.

use slam_input::{PointerWatch, WindowScale};
use slam_render::Origin;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::hal::Extent;
use slam_render::shape::ShapeBatch;
use slam_render::textures::TextureId;
use slam_testkit::assert_no_alloc;
use slam_ui::cursor::CursorStyle;
use slam_ui::theme::Theme;

use slam_app::menu_cursor::{
    MAX_MENU_CURSOR_SHAPES, MenuCursorFrame, MenuCursorSkin, MenuCursorView, MenuTrail, SkinImage,
};
use slam_play::circles::SkinSprite;
use slam_play::cursor::CursorSkinConfig;
use slam_render::upload::{TextureKind, TextureLoaderConfig, texture_loader};

slam_testkit::install_counting_allocator!();

#[test]
fn the_menu_cursor_frame_does_not_allocate() {
    let white = AtlasRegion {
        texture: TextureId(1),
        origin: Origin { x: 0, y: 0 },
        size: Extent::new(1, 1),
        uv: UvRect::FULL,
    };
    let pointer = PointerWatch::default();
    pointer.record_move(40.0, 30.0);
    let mut view = MenuCursorView::new(pointer.clone(), WindowScale::fixed(1.5), None);
    let theme = Theme::default();
    let mut shapes = ShapeBatch::with_capacity(MAX_MENU_CURSOR_SHAPES);
    let frames = [
        MenuCursorFrame::default(),
        MenuCursorFrame {
            style: CursorStyle::Skin,
            skin: MenuCursorSkin {
                loaded: true,
                ..MenuCursorSkin::default()
            },
            ..MenuCursorFrame::default()
        },
        MenuCursorFrame {
            style: CursorStyle::System,
            ..MenuCursorFrame::default()
        },
    ];
    assert_no_alloc(|| {
        for i in 0..90_u64 {
            shapes.clear();
            pointer.record_move(i as f32, 2.0 * i as f32);
            view.push(
                &mut shapes,
                &theme,
                white,
                [1024.0, 768.0],
                i * 16_000_000,
                &frames[(i % 3) as usize],
            );
        }
    });
}

#[test]
fn the_skin_trail_frame_does_not_allocate() {
    let limits = slam_render::hal::Limits {
        max_texture_size: 1024,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };
    let (mut requests, _uploads) = texture_loader(&TextureLoaderConfig::new(limits)).unwrap();
    let image = SkinImage {
        handle: requests.load(vec![0], TextureKind::Standalone).unwrap(),
        scale: 1.0,
    };
    let texture = SkinSprite {
        texture: TextureId(2),
        uv: UvRect::FULL,
        size: slam_osu::Vec2::new(32.0, 32.0),
    };
    let mut trail = MenuTrail::new();
    let mut shapes = ShapeBatch::with_capacity(MAX_MENU_CURSOR_SHAPES);
    let configs = [
        CursorSkinConfig::default(),
        CursorSkinConfig {
            disjoint: false,
            ..CursorSkinConfig::default()
        },
    ];
    assert_no_alloc(|| {
        for i in 0..300_u64 {
            shapes.clear();
            let config = &configs[((i / 100) % 2) as usize];
            trail.feed(
                &mut shapes,
                &texture,
                image,
                config,
                [10.0 + 7.0 * (i % 120) as f32, 50.0 + (i % 40) as f32],
                1.5,
                0.8,
                1.0,
                (i % 360) as f32,
                i * 16_000_000,
                [1024.0, 768.0],
            );
            if i % 97 == 0 {
                trail.idle();
            }
        }
    });
}
