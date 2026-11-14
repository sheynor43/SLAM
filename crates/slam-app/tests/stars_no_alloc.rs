//! The update thread's part of the star ratings (ADR-0134) does not allocate.

use slam_app::stars::StarFocus;
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

#[test]
fn setting_the_focus_does_not_allocate() {
    let focus = StarFocus::default();
    assert_no_alloc(|| {
        for i in 0..1000 {
            focus.set(i, 12);
        }
    });
}
