//! The screen stack's steady state does not touch the allocator: ticks and layers without
//! navigation, through and after a transition.

use slam_testkit::assert_no_alloc;
use slam_ui::screen::{Nav, ScreenStack, TRANSITION_NS};

slam_testkit::install_counting_allocator!();

#[test]
fn ticks_and_layers_do_not_allocate() {
    let mut stack = ScreenStack::new(String::from("menu"));
    stack.navigate(Nav::Push(String::from("select")), 0);
    let mut visible = 0;
    assert_no_alloc(|| {
        // Before the transition ends: nothing to drop yet.
        for now in (0..TRANSITION_NS).step_by(1_000_000) {
            stack.tick(now);
            visible += stack.layers(now).into_iter().flatten().count();
            std::hint::black_box(stack.top_mut());
        }
    });
    assert_eq!(visible, 2 * 200);
    // A push leaves nothing to drop at its end either.
    assert_no_alloc(|| {
        for now in (TRANSITION_NS..2 * TRANSITION_NS).step_by(1_000_000) {
            stack.tick(now);
            visible += stack.layers(now).into_iter().flatten().count();
        }
    });
}
