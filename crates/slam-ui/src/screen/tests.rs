use super::*;

const MS: u64 = 1_000_000;

fn names(stack: &ScreenStack<&'static str>, now_ns: u64) -> Vec<(&'static str, Pose)> {
    stack
        .layers(now_ns)
        .into_iter()
        .flatten()
        .map(|(s, pose)| (*s, pose))
        .collect()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn root_is_shown_at_rest() {
    let stack = ScreenStack::new("menu");
    assert_eq!(*stack.top(), "menu");
    assert_eq!(stack.depth(), 1);
    assert_eq!(names(&stack, 0), [("menu", Pose::SHOWN)]);
}

#[test]
fn push_covers_the_screen_below() {
    let mut stack = ScreenStack::new("menu");
    assert_eq!(
        stack.navigate(Nav::Push("select"), 1000 * MS),
        Navigated::Stay
    );
    assert_eq!(*stack.top(), "select");
    assert_eq!(stack.depth(), 2);

    let start = names(&stack, 1000 * MS);
    assert_eq!(start[0].0, "menu");
    assert_eq!(start[0].1, Pose::SHOWN);
    assert_eq!(start[1].0, "select");
    assert_eq!(start[1].1.opacity, 0.0);
    assert!(close(start[1].1.offset, SLIDE));

    let mid = names(&stack, 1100 * MS);
    let p = ease_out_cubic(0.5);
    assert!(close(mid[0].1.opacity, 1.0 - p));
    assert!(close(mid[0].1.offset, -SLIDE * p));
    assert!(close(mid[1].1.opacity, p));
    assert!(close(mid[1].1.offset, SLIDE * (1.0 - p)));

    assert!(stack.in_transition(1199 * MS));
    assert!(!stack.in_transition(1200 * MS));
    stack.tick(1200 * MS);
    assert_eq!(names(&stack, 1200 * MS), [("select", Pose::SHOWN)]);
    // The covered screen stays in the stack.
    assert_eq!(stack.depth(), 2);
}

#[test]
fn pop_returns_to_the_screen_below() {
    let mut stack = ScreenStack::new("menu");
    stack.navigate(Nav::Push("select"), 0);
    stack.tick(TRANSITION_NS);
    assert_eq!(stack.navigate(Nav::Pop, 1000 * MS), Navigated::Stay);
    assert_eq!(*stack.top(), "menu");
    assert_eq!(stack.depth(), 1);

    let start = names(&stack, 1000 * MS);
    assert_eq!(start[0].0, "menu");
    assert_eq!(start[0].1.opacity, 0.0);
    assert!(close(start[0].1.offset, -SLIDE));
    assert_eq!(start[1].0, "select");
    assert_eq!(start[1].1, Pose::SHOWN);

    let end = names(&stack, 1000 * MS + TRANSITION_NS);
    assert_eq!(end[0].1, Pose::SHOWN);
    assert_eq!(end[1].1.opacity, 0.0);
    assert!(close(end[1].1.offset, SLIDE));

    // The leaving screen is drawn until the tick after the transition, then dropped.
    stack.tick(1000 * MS + TRANSITION_NS - 1);
    assert_eq!(names(&stack, 1000 * MS + TRANSITION_NS - 1).len(), 2);
    stack.tick(1000 * MS + TRANSITION_NS);
    assert_eq!(
        names(&stack, 1000 * MS + TRANSITION_NS),
        [("menu", Pose::SHOWN)]
    );
}

#[test]
fn replace_swaps_the_top_screen() {
    let mut stack = ScreenStack::new("menu");
    stack.navigate(Nav::Push("select"), 0);
    stack.tick(TRANSITION_NS);
    stack.navigate(Nav::Replace("play"), 1000 * MS);
    assert_eq!(*stack.top(), "play");
    assert_eq!(stack.depth(), 2);
    let layers = names(&stack, 1000 * MS);
    assert_eq!(layers[0].0, "select");
    assert_eq!(layers[1].0, "play");
    stack.tick(1000 * MS + TRANSITION_NS);
    assert_eq!(
        names(&stack, 1000 * MS + TRANSITION_NS),
        [("play", Pose::SHOWN)]
    );
    // Back goes past the replaced screen.
    stack.navigate(Nav::Pop, 2000 * MS);
    assert_eq!(*stack.top(), "menu");
}

#[test]
fn pop_at_the_root_and_exit_quit() {
    let mut stack = ScreenStack::new("menu");
    assert_eq!(stack.navigate(Nav::Pop, 0), Navigated::Exit);
    assert_eq!(stack.navigate(Nav::Exit, 0), Navigated::Exit);
    assert_eq!(*stack.top(), "menu");
    assert!(!stack.in_transition(0));
    stack.navigate(Nav::Push("select"), 0);
    assert_eq!(stack.navigate(Nav::Exit, 0), Navigated::Exit);
    assert_eq!(*stack.top(), "select");
}

#[test]
fn navigation_mid_transition_restarts_it() {
    let mut stack = ScreenStack::new("menu");
    stack.navigate(Nav::Push("select"), 0);
    // Back halfway through: the pushed screen leaves, the menu returns from rest.
    stack.navigate(Nav::Pop, 100 * MS);
    let layers = names(&stack, 100 * MS);
    assert_eq!(layers[0].0, "menu");
    assert_eq!(layers[0].1.opacity, 0.0);
    assert_eq!(layers[1].0, "select");
    assert_eq!(layers[1].1, Pose::SHOWN);
    assert!(stack.in_transition(100 * MS + TRANSITION_NS - 1));

    // A push during a pop drops the leaving screen at once.
    stack.navigate(Nav::Push("settings"), 150 * MS);
    let layers = names(&stack, 150 * MS);
    assert_eq!(layers[0].0, "menu");
    assert_eq!(layers[1].0, "settings");
    stack.tick(150 * MS + TRANSITION_NS);
    assert_eq!(stack.depth(), 2);
    assert_eq!(
        names(&stack, 150 * MS + TRANSITION_NS),
        [("settings", Pose::SHOWN)]
    );
}

#[test]
fn input_goes_to_the_top_screen_only() {
    let mut stack = ScreenStack::new(0_u32);
    stack.navigate(Nav::Push(10), 0);
    // During the transition the incoming screen already takes input.
    *stack.top_mut() += 1;
    stack.navigate(Nav::Pop, 50 * MS);
    // The leaving screen gets nothing more; the returning one does.
    *stack.top_mut() += 1;
    let layers: Vec<u32> = stack
        .layers(50 * MS)
        .into_iter()
        .flatten()
        .map(|(s, _)| *s)
        .collect();
    assert_eq!(layers, [1, 11]);
}

#[test]
fn push_past_the_maximum_depth_is_ignored() {
    let mut stack = ScreenStack::new(0_usize);
    for i in 1..MAX_DEPTH {
        stack.navigate(Nav::Push(i), 0);
    }
    assert_eq!(stack.depth(), MAX_DEPTH);
    assert_eq!(stack.navigate(Nav::Push(99), 0), Navigated::Full);
    assert_eq!(stack.depth(), MAX_DEPTH);
    assert_eq!(*stack.top(), MAX_DEPTH - 1);
}

#[test]
fn replace_at_the_root_shows_the_replaced_screen_below() {
    let mut stack = ScreenStack::new("menu");
    stack.navigate(Nav::Replace("intro"), 0);
    assert_eq!(stack.depth(), 1);
    let layers = names(&stack, 0);
    assert_eq!(layers[0].0, "menu");
    assert_eq!(layers[1].0, "intro");
}

#[test]
fn replace_mid_transition_drops_the_previous_leaving_screen() {
    let mut stack = ScreenStack::new("menu");
    stack.navigate(Nav::Replace("a"), 0);
    stack.navigate(Nav::Replace("b"), 50 * MS);
    assert_eq!(stack.depth(), 1);
    let layers = names(&stack, 50 * MS);
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0].0, "a");
    assert_eq!(layers[1].0, "b");
}

#[test]
fn pop_mid_replace_shows_the_screen_below() {
    let mut stack = ScreenStack::new("menu");
    stack.navigate(Nav::Push("select"), 0);
    stack.tick(TRANSITION_NS);
    stack.navigate(Nav::Replace("play"), 1000 * MS);
    stack.navigate(Nav::Pop, 1050 * MS);
    assert_eq!(stack.depth(), 1);
    let layers = names(&stack, 1050 * MS);
    assert_eq!(layers[0].0, "menu");
    assert_eq!(layers[1].0, "play");
}

#[test]
fn push_mid_push_covers_the_previous_top() {
    let mut stack = ScreenStack::new("menu");
    stack.navigate(Nav::Push("select"), 0);
    stack.navigate(Nav::Push("settings"), 50 * MS);
    assert_eq!(stack.depth(), 3);
    let layers = names(&stack, 50 * MS);
    assert_eq!(layers[0].0, "select");
    assert_eq!(layers[0].1, Pose::SHOWN);
    assert_eq!(layers[1].0, "settings");
}
