//! The parallax's frame (smoothing and the shifts of the layers) does not touch the allocator.

use slam_app::parallax::{Parallax, layer_map};
use slam_input::{PointerWatch, WindowScale};
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

#[test]
fn the_parallax_frame_does_not_allocate() {
    let pointer = PointerWatch::default();
    pointer.record_move(40.0, 30.0);
    let mut parallax = Parallax::new(pointer.clone(), WindowScale::fixed(1.5));
    let mut total = 0.0_f32;
    assert_no_alloc(|| {
        for i in 0..120_u64 {
            pointer.record_move(i as f32 * 7.0, 2.0 * i as f32);
            if i == 60 {
                pointer.record_outside(true);
            }
            let shown = parallax.update(i * 8_000_000, [1920.0, 1080.0], 0.04);
            let map = layer_map(shown, [1920.0, 1080.0], 0.02, 1.0, true);
            total += map.tx + map.ty;
        }
    });
    assert!(total.is_finite());
}
