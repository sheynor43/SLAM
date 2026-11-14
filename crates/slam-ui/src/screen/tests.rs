use super::*;
use crate::anim::ease_out_quint;
use crate::theme::Theme;
use slam_render::shape::ShapeBatch;

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

fn white() -> slam_render::atlas::AtlasRegion {
    slam_render::atlas::AtlasRegion {
        texture: slam_render::textures::TextureId(0),
        origin: slam_render::hal::Origin { x: 0, y: 0 },
        size: slam_render::Extent::new(8, 8),
        uv: slam_render::atlas::UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
    }
}

fn styled(style: TransitionStyle) -> ScreenStack<&'static str> {
    let mut stack = ScreenStack::new("menu");
    stack.set_style(style);
    stack
}

#[test]
fn simple_is_the_default_and_poses_have_no_scale() {
    assert_eq!(TransitionStyle::default(), TransitionStyle::Simple);
    let mut stack = ScreenStack::new("menu");
    stack.navigate(Nav::Push("select"), 0);
    for t in [0, 50, 100, 150, 200] {
        for (_, pose) in names(&stack, t * MS) {
            assert_eq!(pose.scale, 1.0);
        }
    }
    assert!(stack.band(100 * MS).is_none());
}

#[test]
fn none_is_instant_without_a_leaving_layer() {
    let mut stack = styled(TransitionStyle::None);
    stack.navigate(Nav::Push("select"), 0);
    assert!(!stack.in_transition(0));
    assert_eq!(names(&stack, 0), [("select", Pose::SHOWN)]);
    stack.navigate(Nav::Replace("play"), 10);
    assert_eq!(names(&stack, 10), [("play", Pose::SHOWN)]);
    stack.navigate(Nav::Pop, 20);
    assert_eq!(names(&stack, 20), [("menu", Pose::SHOWN)]);
    assert!(stack.band(20).is_none());
}

#[test]
fn fancy_poses_at_the_ends_and_the_middle() {
    use crate::theme::transition::*;
    let mut stack = styled(TransitionStyle::Fancy);
    stack.navigate(Nav::Push("select"), 0);
    let start = names(&stack, 0);
    assert_eq!(start[0].1, Pose::SHOWN);
    assert_eq!(start[1].1.opacity, 0.0);
    assert!(close(start[1].1.offset, FANCY_SLIDE));
    assert!(close(start[1].1.scale, FANCY_SCALE_IN));

    let mid = names(&stack, FANCY_NS / 2);
    let p = ease_out_quint(0.5);
    assert!(close(mid[0].1.opacity, 1.0 - p));
    assert!(close(mid[0].1.scale, 1.0 + (FANCY_SCALE_OUT - 1.0) * p));
    assert!(close(mid[0].1.offset, -FANCY_SLIDE * p));
    assert!(close(mid[1].1.opacity, p));
    assert!(close(
        mid[1].1.scale,
        1.0 + (FANCY_SCALE_IN - 1.0) * (1.0 - p)
    ));

    assert!(stack.in_transition(FANCY_NS - 1));
    assert!(!stack.in_transition(FANCY_NS));
    let end = names(&stack, FANCY_NS);
    assert_eq!(end[1].1, Pose::SHOWN);
    assert_eq!(end[0].1.opacity, 0.0);
    assert!(close(end[0].1.scale, FANCY_SCALE_OUT));
    stack.tick(FANCY_NS);
    assert_eq!(names(&stack, FANCY_NS), [("select", Pose::SHOWN)]);
}

#[test]
fn fancy_poses_move_monotonically() {
    for style in [TransitionStyle::Simple, TransitionStyle::Fancy] {
        for pop in [false, true] {
            let mut stack = styled(style);
            stack.navigate(Nav::Push("select"), 0);
            stack.tick(1_000 * MS);
            let start = 2_000 * MS;
            stack.navigate(if pop { Nav::Pop } else { Nav::Push("x") }, start);
            let top_index = if pop { 0 } else { 1 };
            let mut last: Option<[Pose; 2]> = None;
            for step in 0..=100 {
                let now = start + style.duration_ns() * step / 100;
                let layers = stack.layers(now);
                let (a, b) = (layers[0].unwrap().1, layers[1].unwrap().1);
                let (inc, out) = if top_index == 1 { (b, a) } else { (a, b) };
                if let Some([pi, po]) = last {
                    assert!(inc.opacity >= pi.opacity && out.opacity <= po.opacity);
                    assert!(inc.offset.abs() <= pi.offset.abs() + 1e-6);
                    assert!(out.offset.abs() >= po.offset.abs() - 1e-6);
                    assert!(inc.scale.abs_diff_to_one() <= pi.scale.abs_diff_to_one() + 1e-6);
                    assert!(out.scale.abs_diff_to_one() >= po.scale.abs_diff_to_one() - 1e-6);
                }
                last = Some([inc, out]);
            }
        }
    }
}

trait AbsDiffToOne {
    fn abs_diff_to_one(self) -> f32;
}

impl AbsDiffToOne for f32 {
    fn abs_diff_to_one(self) -> f32 {
        (self - 1.0).abs()
    }
}

#[test]
fn a_transition_keeps_the_style_it_started_in() {
    let mut stack = styled(TransitionStyle::Fancy);
    stack.navigate(Nav::Push("select"), 0);
    stack.set_style(TransitionStyle::None);
    assert!(stack.in_transition(300 * MS));
    assert!(stack.band(100 * MS).is_some());
    assert_eq!(names(&stack, 100 * MS).len(), 2);
    stack.tick(FANCY_NS_FOR_TEST);
    assert_eq!(names(&stack, FANCY_NS_FOR_TEST).len(), 1);
    // The next one takes the new style.
    stack.navigate(Nav::Pop, 1_000 * MS);
    assert!(!stack.in_transition(1_000 * MS));
}

const FANCY_NS_FOR_TEST: u64 = crate::theme::transition::FANCY_NS;

#[test]
fn the_band_crosses_right_to_left_on_push_and_back_on_pop() {
    let theme = Theme::default();
    let mut stack = styled(TransitionStyle::Fancy);
    stack.navigate(Nav::Push("select"), 0);
    let centre = |b: &Band| {
        let s = b.shape(&theme, white(), 1000.0, 500.0);
        s.map(|s| s.position[0] + s.size[0] * 0.5)
    };
    let quarter = stack.band(FANCY_NS_FOR_TEST / 4).unwrap();
    let late = stack.band(FANCY_NS_FOR_TEST * 3 / 4).unwrap();
    assert!(quarter.leftward);
    assert!(centre(&late).unwrap() < centre(&quarter).unwrap());
    assert!(centre(&quarter).unwrap() < 1000.0);

    stack.tick(FANCY_NS_FOR_TEST);
    assert!(stack.band(FANCY_NS_FOR_TEST).is_none());
    stack.navigate(Nav::Pop, 2_000 * MS);
    let at = |f: u64| {
        stack
            .band(2_000 * MS + FANCY_NS_FOR_TEST * f / 100)
            .unwrap()
    };
    let (early, late) = (at(25), at(75));
    assert!(!early.leftward);
    assert!(centre(&late).unwrap() > centre(&early).unwrap());
}

#[test]
fn the_band_peaks_in_the_middle_and_is_accent_coloured_and_skewed() {
    let theme = Theme::default();
    let mut stack = styled(TransitionStyle::Fancy);
    stack.navigate(Nav::Push("select"), 0);
    let strength = |f: u64| stack.band(FANCY_NS_FOR_TEST * f / 100).unwrap().strength;
    assert!(strength(50) > strength(25) && strength(50) > strength(75));
    assert!(strength(50) > 0.99);
    assert!(
        stack
            .band(0)
            .unwrap()
            .shape(&theme, white(), 1000.0, 500.0)
            .is_none()
    );
    let band = stack.band(FANCY_NS_FOR_TEST / 2).unwrap();
    let shape = band.shape(&theme, white(), 1000.0, 500.0).unwrap();
    assert_eq!(shape.skew, theme.button_skew());
    assert_eq!(shape.colour.r, theme.accent.r);
    assert!(close(
        shape.colour.a,
        crate::theme::transition::BAND_PEAK_OPACITY * band.strength
    ));
    let mut shapes = ShapeBatch::with_capacity(4);
    band.push(&mut shapes, &theme, white(), 1000.0, 500.0);
    assert_eq!(shapes.len(), 1);
}

#[test]
fn style_names_round_trip() {
    for style in TransitionStyle::ALL {
        assert_eq!(TransitionStyle::from_name(style.name()), Some(style));
    }
    assert_eq!(TransitionStyle::from_name("flashy"), None);
}
