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
fn simple_poses_have_no_scale() {
    assert_eq!(TransitionStyle::default(), TransitionStyle::Random);
    let mut stack = styled(TransitionStyle::Simple);
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
    let mut stack = styled(TransitionStyle::Band);
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
    for style in [TransitionStyle::Simple, TransitionStyle::Band] {
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
    let mut stack = styled(TransitionStyle::Band);
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
    let mut stack = styled(TransitionStyle::Band);
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
    let mut stack = styled(TransitionStyle::Band);
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

const FANCY_STYLES: [TransitionStyle; 5] = [
    TransitionStyle::Band,
    TransitionStyle::Slices,
    TransitionStyle::Iris,
    TransitionStyle::Shards,
    TransitionStyle::Shutter,
];

fn overlay(fx: &TransitionFx, width: f32, height: f32) -> Vec<Shape> {
    let mut out = Vec::new();
    fx.emit(&Theme::default(), white(), width, height, |s| out.push(s));
    out
}

fn fx_of(style: TransitionStyle, pop: bool, linear: f32) -> TransitionFx {
    TransitionFx {
        style,
        forward: !pop,
        linear,
        origin: Some([300.0, 200.0]),
    }
}

/// Fraction of a `w` x `h` grid of sample points that lie inside some shape.
fn covered(shapes: &[Shape], w: f32, h: f32) -> f32 {
    let (nx, ny) = (80, 50);
    let mut hit = 0;
    for iy in 0..ny {
        for ix in 0..nx {
            let (x, y) = (
                (ix as f32 + 0.5) * w / nx as f32,
                (iy as f32 + 0.5) * h / ny as f32,
            );
            if shapes.iter().any(|s| inside(s, x, y)) {
                hit += 1;
            }
        }
    }
    hit as f32 / (nx * ny) as f32
}

fn inside(s: &Shape, x: f32, y: f32) -> bool {
    let c = [
        s.position[0] + s.size[0] * 0.5,
        s.position[1] + s.size[1] * 0.5,
    ];
    // Undo the skew about the centre line: positive leans the top right.
    let x = x - (c[1] - y) * s.skew.tan();
    if s.radius >= s.size[0] * 0.5 - 0.01 && s.size[0] == s.size[1] {
        let d = (x - c[0]).hypot(y - c[1]);
        let r = s.size[0] * 0.5;
        return d <= r && (s.outline <= 0.0 || d >= r - s.outline);
    }
    (x - c[0]).abs() <= s.size[0] * 0.5 && (y - c[1]).abs() <= s.size[1] * 0.5
}

#[test]
fn covering_overlays_are_empty_at_the_ends_and_cover_the_middle() {
    let (w, h) = (1000.0, 500.0);
    for style in [
        TransitionStyle::Slices,
        TransitionStyle::Iris,
        TransitionStyle::Shards,
        TransitionStyle::Shutter,
    ] {
        for pop in [false, true] {
            assert!(
                overlay(&fx_of(style, pop, 0.0), w, h).is_empty(),
                "{style:?} start"
            );
            assert!(
                overlay(&fx_of(style, pop, 1.0), w, h).is_empty(),
                "{style:?} end"
            );
            let mid = overlay(&fx_of(style, pop, 0.5), w, h);
            assert!(!mid.is_empty());
            assert!(
                covered(&mid, w, h) > 0.995,
                "{style:?} mid {}",
                covered(&mid, w, h)
            );
            let early = covered(&overlay(&fx_of(style, pop, 0.15), w, h), w, h);
            assert!(early < 0.9, "{style:?} early {early}");
            let late = covered(&overlay(&fx_of(style, pop, 0.9), w, h), w, h);
            assert!(late < 0.9, "{style:?} late {late}");
        }
    }
}

#[test]
fn band_overlay_is_one_translucent_shape_and_empty_at_the_ends() {
    let (w, h) = (1000.0, 500.0);
    assert!(overlay(&fx_of(TransitionStyle::Band, false, 0.0), w, h).is_empty());
    assert!(overlay(&fx_of(TransitionStyle::Band, false, 1.0), w, h).is_empty());
    let mid = overlay(&fx_of(TransitionStyle::Band, false, 0.5), w, h);
    assert_eq!(mid.len(), 1);
    assert!(mid[0].colour.a < 1.0);
}

#[test]
fn iris_opens_from_the_origin_or_the_centre() {
    let (w, h) = (1000.0, 500.0);
    let mut fx = fx_of(TransitionStyle::Iris, false, 0.2);
    let s = overlay(&fx, w, h)[0];
    assert!(close(s.position[0] + s.size[0] * 0.5, 300.0));
    assert!(close(s.position[1] + s.size[1] * 0.5, 200.0));
    fx.origin = None;
    let s = overlay(&fx, w, h)[0];
    assert!(close(s.position[0] + s.size[0] * 0.5, 500.0));
    assert!(close(s.position[1] + s.size[1] * 0.5, 250.0));
    // Past the middle the disc is a ring that opens from the same point.
    fx.linear = 0.75;
    let ring = overlay(&fx, w, h)[0];
    assert!(ring.outline > 0.0);
    assert!(!inside(&ring, 500.0, 250.0) && inside(&ring, 5.0, 5.0));
}

#[test]
fn overlays_are_skewed_by_the_theme_and_stay_within_the_shape_budget() {
    let theme = Theme::default();
    for style in [
        TransitionStyle::Slices,
        TransitionStyle::Shards,
        TransitionStyle::Shutter,
    ] {
        for step in 0..=100 {
            let shapes = overlay(&fx_of(style, false, step as f32 / 100.0), 1280.0, 720.0);
            assert!(shapes.len() <= MAX_FX_SHAPES);
            assert!(shapes.iter().all(|s| s.skew == theme.button_skew()));
        }
    }
    let shards = overlay(&fx_of(TransitionStyle::Shards, false, 0.5), 1280.0, 720.0);
    assert_eq!(
        shards.len(),
        crate::theme::transition::SHARD_COLS * crate::theme::transition::SHARD_ROWS
    );
}

#[test]
fn sweeps_start_at_the_edge_the_new_screen_comes_from() {
    let first_x = |pop: bool| {
        let shapes = overlay(&fx_of(TransitionStyle::Slices, pop, 0.1), 1000.0, 500.0);
        let s = shapes[0];
        s.position[0] + s.size[0] * 0.5
    };
    assert!(first_x(false) > 500.0);
    assert!(first_x(true) < 500.0);
}

#[test]
fn random_never_repeats_and_visits_every_fancy_style() {
    let mut stack = styled(TransitionStyle::Random);
    let mut seen = Vec::new();
    let mut last = None;
    for i in 0..40u64 {
        let now = i * 10_000 * MS;
        stack.navigate(if i % 2 == 0 { Nav::Push("a") } else { Nav::Pop }, now);
        let style = stack.fx(now + 1).unwrap().style;
        assert!(FANCY_STYLES.contains(&style));
        assert_ne!(Some(style), last);
        last = Some(style);
        if !seen.contains(&style) {
            seen.push(style);
        }
        stack.tick(now + 9_000 * MS);
    }
    assert_eq!(seen.len(), FANCY_STYLES.len());
    // Deterministic: a second stack walks the same way.
    let mut again = styled(TransitionStyle::Random);
    again.navigate(Nav::Push("a"), 0);
    let mut first = styled(TransitionStyle::Random);
    first.navigate(Nav::Push("a"), 0);
    assert_eq!(again.fx(1).unwrap().style, first.fx(1).unwrap().style);
}

#[test]
fn a_random_transition_keeps_its_variant_while_running() {
    let mut stack = styled(TransitionStyle::Random);
    stack.navigate(Nav::Push("a"), 0);
    let style = stack.fx(1).unwrap().style;
    stack.set_style(TransitionStyle::Simple);
    assert_eq!(stack.fx(2).unwrap().style, style);
    assert!(stack.in_transition(style.duration_ns() - 1));
}

#[test]
fn covering_poses_swap_the_screens_while_covered() {
    for style in [
        TransitionStyle::Slices,
        TransitionStyle::Iris,
        TransitionStyle::Shards,
        TransitionStyle::Shutter,
    ] {
        let mut stack = styled(style);
        stack.navigate(Nav::Push("select"), 0);
        let d = style.duration_ns();
        let at = |f: u64| names(&stack, d * f / 100);
        assert_eq!(at(0)[1].1.opacity, 0.0);
        assert_eq!(at(30)[0].1.opacity, 1.0);
        assert_eq!(at(70)[1].1.opacity, 1.0);
        assert_eq!(at(70)[0].1.opacity, 0.0);
        assert!(stack.fx(d - 1).is_some() && stack.fx(d).is_none());
        assert!((450_000_000..=600_000_000).contains(&d));
    }
}

#[test]
fn style_names_include_the_old_fancy() {
    assert_eq!(
        TransitionStyle::from_name("fancy"),
        Some(TransitionStyle::Band)
    );
    assert_eq!(TransitionStyle::ALL.len(), 8);
}
