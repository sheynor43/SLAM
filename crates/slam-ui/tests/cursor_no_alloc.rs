//! SLAM's cursor and its press animation do not touch the allocator once the batch has room.

use slam_render::Origin;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::hal::Extent;
use slam_render::shape::ShapeBatch;
use slam_render::textures::TextureId;
use slam_testkit::assert_no_alloc;
use slam_ui::cursor::{
    MAX_CURSOR_SHAPES, MenuCursorMotion, push_gameplay_cursor, push_slam_cursor,
};
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
    let mut shapes = ShapeBatch::with_capacity(2 * MAX_CURSOR_SHAPES);
    let mut motion = MenuCursorMotion::default();
    assert_no_alloc(|| {
        for frame in 0..100_u64 {
            shapes.clear();
            let now = frame * 16_000_000;
            let at = [frame as f32 * 9.0, 300.0 + frame as f32 * 4.0];
            motion.update(&theme, 1.0, Some(at), frame % 20 < 10, now);
            let sample = motion.sample(&theme, now);
            push_slam_cursor(&mut shapes, &theme, white, [10.0, 10.0], 28.0, &sample);
            push_gameplay_cursor(
                &mut shapes,
                &theme,
                white,
                [50.0, 50.0],
                28.0,
                1.0 + 0.003 * frame as f32,
            );
        }
    });
    assert!(!shapes.is_empty());
}
