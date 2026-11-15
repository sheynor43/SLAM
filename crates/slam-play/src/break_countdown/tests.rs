use super::*;

/// Lazer's period of a break from 1000 to 11 000: its end 325 ms early.
fn period() -> Period {
    Period {
        start: 1_000.0,
        end: 11_000.0 - BREAK_FADE_DURATION,
    }
}

#[test]
fn nothing_shows_before_the_period() {
    let pose = LazerPose::of(period(), 999.0);
    assert_eq!(pose.alpha, 0.0);
    assert_eq!(pose.seconds(), None);
}

#[test]
fn the_overlay_fades_in_and_out_over_the_fade_duration() {
    let p = period();
    assert_eq!(LazerPose::of(p, p.start).alpha, 0.0);
    assert!((LazerPose::of(p, p.start + BREAK_FADE_DURATION / 2.0).alpha - 0.5).abs() < 1e-6);
    assert_eq!(LazerPose::of(p, 5_000.0).alpha, 1.0);
    assert_eq!(LazerPose::of(p, p.end).alpha, 1.0);
    assert!((LazerPose::of(p, p.end + BREAK_FADE_DURATION / 2.0).alpha - 0.5).abs() < 1e-6);
    assert_eq!(LazerPose::of(p, p.end + BREAK_FADE_DURATION).alpha, 0.0);
}

#[test]
fn the_counter_counts_down_to_the_break_end_in_whole_seconds() {
    let p = period();
    // From the duration and the fade at the start to 0 at the break's end (11 000).
    assert_eq!(LazerPose::of(p, p.start).seconds(), Some(10));
    assert_eq!(LazerPose::of(p, 1_001.0).seconds(), Some(10));
    assert_eq!(LazerPose::of(p, 2_000.0).seconds(), Some(9));
    assert_eq!(LazerPose::of(p, 2_001.0).seconds(), Some(9));
    assert_eq!(LazerPose::of(p, 9_999.0).seconds(), Some(2));
    assert_eq!(LazerPose::of(p, 10_999.0).seconds(), Some(1));
    assert_eq!(LazerPose::of(p, 11_000.0).seconds(), Some(0));
}

#[test]
fn the_counter_slides_in_from_the_left() {
    let p = period();
    assert_eq!(LazerPose::of(p, p.start).counter_x, -COUNTER_SLIDE);
    assert_eq!(
        LazerPose::of(p, p.start + BREAK_FADE_DURATION).counter_x,
        0.0
    );
}

#[test]
fn the_bar_opens_then_shrinks_to_the_end() {
    let p = period();
    let bar = |t| LazerPose::of(p, t).bar;
    assert_eq!(bar(p.start), 0.0);
    // Open after the fade: 0.3 of the window times what is left (lazer takes the fade off
    // the end again).
    let t = p.start + BREAK_FADE_DURATION;
    let left = (p.end - t - BREAK_FADE_DURATION) / p.duration();
    assert!((f64::from(bar(t)) - 0.3 * left).abs() < 1e-6);
    assert!(bar(5_000.0) > bar(8_000.0));
    assert_eq!(bar(p.end - BREAK_FADE_DURATION), 0.0);
    assert_eq!(bar(p.end + 100.0), 0.0);
}

#[test]
fn slams_bar_runs_from_full_to_empty_over_the_period() {
    let p = period();
    assert_eq!(slam_left(p, p.start), 1.0);
    assert_eq!(slam_left(p, p.end), 0.0);
    assert!((slam_left(p, (p.start + p.end) / 2.0) - 0.5).abs() < 1e-6);
}

#[test]
fn the_bar_is_gone_while_the_overlay_fades_out() {
    let p = period();
    for t in [
        p.end - BREAK_FADE_DURATION,
        p.end,
        p.end + BREAK_FADE_DURATION / 2.0,
    ] {
        assert_eq!(LazerPose::of(p, t).bar, 0.0, "{t}");
    }
    assert!(LazerPose::of(p, p.end + BREAK_FADE_DURATION / 2.0).alpha > 0.0);
}

#[test]
fn the_shortest_break_fades_in_then_straight_out() {
    // A 650 ms break: lazer's period is as long as the fade.
    let p = Period {
        start: 0.0,
        end: BREAK_FADE_DURATION,
    };
    assert_eq!(LazerPose::of(p, BREAK_FADE_DURATION).alpha, 1.0);
    assert!((LazerPose::of(p, 1.5 * BREAK_FADE_DURATION).alpha - 0.5).abs() < 1e-6);
    assert_eq!(LazerPose::of(p, 2.0 * BREAK_FADE_DURATION).alpha, 0.0);
    assert_eq!(LazerPose::of(p, 0.0).seconds(), Some(1));
}
