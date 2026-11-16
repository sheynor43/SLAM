use super::*;

const MS: u64 = 1_000_000;

fn params() -> MarqueeParams {
    // 50 px/s, 1 s pauses, at scale 1.
    MarqueeParams::new(&Theme::default(), 1.0)
}

/// A marquee 100 px too wide, started at time 0.
fn riding() -> Marquee {
    let mut m = Marquee::default();
    m.set_overflow(100.0, 0);
    m
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn a_text_that_fits_stands_still() {
    let p = params();
    let mut m = Marquee::default();
    m.set_overflow(0.3, 0);
    assert!(!m.rides());
    for t in [0, 500, 5_000, 50_000] {
        assert_eq!(m.offset(t * MS, &p), 0.0);
    }
    m.set_by_hand(40.0, 0);
    assert_eq!(m.offset(10 * MS, &p), 0.0);
}

#[test]
fn the_ride_pauses_at_both_ends_and_eases_between() {
    let p = params();
    let mut m = riding();
    // Pause at the start.
    assert_eq!(m.offset(0, &p), 0.0);
    assert_eq!(m.offset(999 * MS, &p), 0.0);
    // 100 px at 50 px/s: two seconds, halfway at one, slow near the ends.
    assert!(near(m.offset(2_000 * MS, &p), 50.0));
    let early = m.offset(1_100 * MS, &p);
    assert!(early > 0.0 && early < 5.0, "{early}");
    assert!(near(m.offset(3_000 * MS, &p), 100.0));
    // Pause at the end, then back.
    assert!(near(m.offset(3_999 * MS, &p), 100.0));
    assert!(near(m.offset(5_000 * MS, &p), 50.0));
    assert!(near(m.offset(6_000 * MS, &p), 0.0));
    // And again: the cycle is 6 s.
    assert!(near(m.offset(8_000 * MS, &p), 50.0));
    assert!(near(m.offset(12_000 * MS, &p), 0.0));
}

#[test]
fn the_offset_depends_on_time_only() {
    let p = params();
    let mut every_frame = riding();
    let mut once = riding();
    let mut last = 0.0;
    for frame in 0..=4_000 {
        last = every_frame.offset(frame * MS, &p);
    }
    assert!(near(last, once.offset(4_000 * MS, &p)));
}

#[test]
fn a_new_overflow_restarts_but_the_same_one_keeps_the_ride() {
    let p = params();
    let mut m = riding();
    assert!(near(m.offset(2_000 * MS, &p), 50.0));
    m.set_overflow(100.1, 2_000 * MS);
    assert!(near(m.offset(2_000 * MS, &p), 50.0));
    m.set_overflow(60.0, 2_000 * MS);
    assert_eq!(m.offset(2_500 * MS, &p), 0.0);
    m.set_overflow(0.0, 2_500 * MS);
    assert!(!m.rides());
}

#[test]
fn a_long_stall_starts_over() {
    let p = params();
    let mut m = riding();
    assert_eq!(m.offset(0, &p), 0.0);
    assert_eq!(m.offset(3_600_000 * MS, &p), 0.0);
    assert_eq!(m.offset(3_600_500 * MS, &p), 0.0);
}

#[test]
fn by_hand_the_text_pauses_then_rides_on_the_way_it_went() {
    let p = params();
    let mut m = riding();
    // Riding forward at 2 s; put back to 20 px.
    m.offset(2_000 * MS, &p);
    m.set_by_hand(20.0, 2_000 * MS);
    assert!(near(m.offset(2_999 * MS, &p), 20.0));
    // Then on to the end: 80 px in 1.6 s.
    assert!(near(m.offset(3_800 * MS, &p), 60.0));
    assert!(near(m.offset(4_600 * MS, &p), 100.0));
    // Clamped to the text.
    m.set_by_hand(500.0, 5_000 * MS);
    assert!(near(m.offset(5_000 * MS, &p), 100.0));
    m.set_by_hand(-5.0, 5_000 * MS);
    assert_eq!(m.offset(5_000 * MS, &p), 0.0);
}

#[test]
fn put_at_the_end_it_headed_for_it_turns_back() {
    let p = params();
    let mut m = riding();
    m.offset(2_000 * MS, &p);
    m.set_by_hand(100.0, 2_000 * MS);
    assert!(near(m.offset(4_000 * MS, &p), 50.0));
}

#[test]
fn a_nudge_moves_from_where_the_text_is() {
    let p = params();
    let mut m = riding();
    m.nudge(p.wheel_step, 0, &p);
    assert!(near(m.offset(0, &p), 40.0));
    m.nudge(-p.wheel_step * 0.5, 100 * MS, &p);
    assert!(near(m.offset(100 * MS, &p), 20.0));
}

fn window(m: &mut Marquee, rect: Rect) {
    m.place(rect);
}

const RECTS: [Rect; 2] = [
    Rect {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 20.0,
    },
    Rect {
        x: 0.0,
        y: 30.0,
        width: 100.0,
        height: 20.0,
    },
];

fn pair() -> [Marquee; 2] {
    let mut ms = [riding(), riding()];
    for (m, r) in ms.iter_mut().zip(RECTS) {
        window(m, r);
    }
    ms
}

#[test]
fn the_wheel_moves_the_text_under_the_pointer_once() {
    let p = params();
    let mut ms = pair();
    let mut tracker = ScrollTracker::default();
    let mut input = TextScroll::default();
    input.pointer([10.0, 40.0]);
    // Notches before the first look do not count.
    input.wheel(3.0);
    tracker.apply(&input, 0, &p, ms.iter_mut());
    input.wheel(1.0);
    tracker.apply(&input, 0, &p, ms.iter_mut());
    // The same snapshot again changes nothing.
    tracker.apply(&input, 0, &p, ms.iter_mut());
    assert_eq!(ms[0].offset(0, &p), 0.0);
    assert!(near(ms[1].offset(0, &p), 40.0));
}

#[test]
fn a_drag_takes_the_text_under_the_press_and_follows_the_pointer() {
    let p = params();
    let mut ms = pair();
    let mut tracker = ScrollTracker::default();
    let mut input = TextScroll::default();
    tracker.apply(&input, 0, &p, ms.iter_mut());
    input.pointer([50.0, 10.0]);
    input.start_drag([50.0, 10.0]);
    input.pointer([20.0, 45.0]);
    tracker.apply(&input, 100 * MS, &p, ms.iter_mut());
    // Dragged left 30 px: the text shows 30 px further on, whatever row the pointer is
    // over now.
    assert!(near(ms[0].offset(100 * MS, &p), 30.0));
    assert_eq!(ms[1].offset(100 * MS, &p), 0.0);
    input.pointer([-100.0, 45.0]);
    tracker.apply(&input, 200 * MS, &p, ms.iter_mut());
    assert!(near(ms[0].offset(200 * MS, &p), 100.0));
    input.end_drag();
    tracker.apply(&input, 300 * MS, &p, ms.iter_mut());
    // Holds a pause from the last move, then turns back from the end.
    assert!(near(ms[0].offset(1_199 * MS, &p), 100.0));
    assert!(ms[0].offset(1_500 * MS, &p) < 100.0);
    // A new drag gets a new id and starts from where the text is.
    input.pointer([50.0, 10.0]);
    input.start_drag([50.0, 10.0]);
    input.pointer([60.0, 10.0]);
    let at = ms[0].offset(1_500 * MS, &p);
    tracker.apply(&input, 1_500 * MS, &p, ms.iter_mut());
    assert!(near(ms[0].offset(1_500 * MS, &p), at - 10.0));
}

#[test]
fn a_text_that_fits_takes_no_drag_or_wheel() {
    let p = params();
    let mut ms = pair();
    ms[0].set_overflow(0.0, 0);
    let mut tracker = ScrollTracker::default();
    let mut input = TextScroll::default();
    tracker.apply(&input, 0, &p, ms.iter_mut());
    input.pointer([10.0, 10.0]);
    input.wheel(2.0);
    input.start_drag([10.0, 10.0]);
    input.pointer([0.0, 10.0]);
    tracker.apply(&input, 0, &p, ms.iter_mut());
    assert_eq!(ms[0].offset(0, &p), 0.0);
    assert_eq!(ms[1].offset(0, &p), 0.0);
}

#[test]
fn a_screen_opened_again_does_not_replay_its_old_sums() {
    let p = params();
    let mut ms = pair();
    let mut tracker = ScrollTracker::default();
    let mut input = TextScroll::default();
    input.pointer([10.0, 10.0]);
    tracker.apply(&input, 0, &p, ms.iter_mut());
    input.wheel(2.0);
    input.start_drag([10.0, 10.0]);
    tracker.apply(&input, 0, &p, ms.iter_mut());
    assert!(near(ms[0].offset(0, &p), 80.0));
    // The screen closes and opens again: new sums from zero, a new first drag.
    let mut again = TextScroll::default();
    again.pointer([10.0, 10.0]);
    tracker.apply(&again, 0, &p, ms.iter_mut());
    again.start_drag([10.0, 40.0]);
    again.pointer([0.0, 40.0]);
    tracker.apply(&again, 0, &p, ms.iter_mut());
    assert!(near(ms[0].offset(0, &p), 80.0));
    assert!(near(ms[1].offset(0, &p), 10.0));
}
