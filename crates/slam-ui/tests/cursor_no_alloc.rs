//! SLAM's cursor and its press animation do not touch the allocator once the batch has room.

use slam_render::Origin;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::hal::Extent;
use slam_render::shape::ShapeBatch;
use slam_render::textures::TextureId;
use slam_testkit::assert_no_alloc;
use slam_ui::cursor::{MAX_CURSOR_SHAPES, PressSqueeze, push_slam_cursor, squeeze_for};
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

#[test]
fn drawing_the_cursor_does_not_allocate() {
    let white = AtlasRegion {
        texture: TextureId(1),
        origin: Origin { x: 0, y: 0 },
        size: Extent::new(1, 1),
        uv: UvRect::FULL,
    };
    let theme = Theme::default();
    let mut shapes = ShapeBatch::with_capacity(MAX_CURSOR_SHAPES);
    let mut press = PressSqueeze::default();
    assert_no_alloc(|| {
        for frame in 0..100_u64 {
            shapes.clear();
            press.update(&theme, frame % 20 < 10, frame * 16_000_000);
            let squeeze = squeeze_for(&theme, &press, frame * 16_000_000);
            push_slam_cursor(&mut shapes, &theme, white, [10.0, 10.0], 28.0, squeeze);
        }
    });
    assert!(!shapes.is_empty());
}
