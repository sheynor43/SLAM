use super::*;

fn flash(timing: &[(f64, f64)], effects: &[(f64, bool)]) -> KiaiFlash {
    let max_fade = if timing.is_empty() {
        fade_length(1000.0)
    } else {
        timing
            .iter()
            .map(|&(_, l)| fade_length(l))
            .fold(FADE_MARGIN, f64::max)
    };
    KiaiFlash {
        timing: timing
            .iter()
            .map(|&(time, beat_length)| Timing { time, beat_length })
            .collect(),
        effects: effects.to_vec(),
        max_fade,
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn no_kiai_no_flash() {
    let f = flash(&[(0.0, 500.0)], &[(0.0, false)]);
    for t in [0.0, 1.0, 250.0, 500.0, 10_000.0] {
        assert_eq!(f.alpha(t), 0.0);
    }
    assert_eq!(flash(&[(0.0, 500.0)], &[]).alpha(500.0), 0.0);
}

#[test]
fn every_beat_in_kiai_flashes_and_fades_along_out_sine() {
    let f = flash(&[(100.0, 500.0)], &[(1100.0, true), (3100.0, false)]);
    // Before kiai: nothing.
    assert_eq!(f.alpha(600.0), 0.0);
    assert_eq!(f.alpha(1099.0), 0.0);
    // On the beat: full flash; the fade takes 500 − 80 = 420 ms.
    assert!(close(f.alpha(1100.0), 0.3));
    let half = 1100.0 + 210.0;
    let expected = 0.3 * (1.0 - (std::f64::consts::FRAC_PI_4).sin() as f32);
    assert!(close(f.alpha(half), expected), "{}", f.alpha(half));
    assert_eq!(f.alpha(1100.0 + 420.0), 0.0);
    assert_eq!(f.alpha(1599.0), 0.0);
    // The next beats flash too.
    assert!(close(f.alpha(1600.0), 0.3));
    assert!(close(f.alpha(2600.0), 0.3));
    // Kiai ends at a beat: that beat does not flash.
    assert_eq!(f.alpha(3100.0), 0.0);
    assert_eq!(f.alpha(3600.0), 0.0);
}

#[test]
fn kiai_starting_between_beats_waits_for_the_next_beat() {
    let f = flash(&[(0.0, 400.0)], &[(1000.0, true)]);
    // 1000 is mid-beat (beats at 800 and 1200).
    assert_eq!(f.alpha(1000.0), 0.0);
    assert_eq!(f.alpha(1100.0), 0.0);
    assert!(close(f.alpha(1200.0), 0.3));
}

#[test]
fn a_new_timing_point_starts_a_beat_at_its_time() {
    let f = flash(&[(0.0, 500.0), (1250.0, 300.0)], &[(0.0, true)]);
    // 1250 is off the old grid; the new point fires a beat there.
    assert!(close(f.alpha(1250.0), 0.3));
    assert!(close(f.alpha(1550.0), 0.3));
    // Fade of the new point: 300 − 80 = 220 ms.
    assert_eq!(f.alpha(1250.0 + 220.0), 0.0);
}

#[test]
fn before_the_first_timing_point_beats_follow_it() {
    let f = flash(&[(1000.0, 500.0)], &[(0.0, true)]);
    assert!(close(f.alpha(500.0), 0.3));
    assert!(close(f.alpha(0.0), 0.3));
    // The beat at −500 comes before the effect point that turns kiai on.
    assert_eq!(f.alpha(-250.0), 0.0);
}

#[test]
fn short_beats_fade_over_80_ms_and_a_beat_out_of_kiai_lets_it_finish() {
    // 100 ms beats: the fade is 80 ms... max(80, 20) = 80, shorter than the beat.
    let f = flash(&[(0.0, 100.0)], &[(0.0, true), (250.0, false)]);
    assert!(close(f.alpha(200.0), 0.3));
    assert!(f.alpha(279.0) > 0.0);
    assert_eq!(f.alpha(280.0), 0.0);
    // 60 ms beats: the 80 ms fade outlives the beat; the first beat out of kiai (at 240)
    // leaves the flash of 180 running.
    let f = flash(&[(0.0, 60.0)], &[(0.0, true), (200.0, false)]);
    let at = 250.0;
    let expected = 0.3 * (1.0 - ((at - 180.0) / 80.0 * std::f64::consts::FRAC_PI_2).sin() as f32);
    assert!(
        close(f.alpha(at), expected),
        "{} vs {expected}",
        f.alpha(at)
    );
    assert_eq!(f.alpha(300.0), 0.0);
}

#[test]
fn without_timing_points_beats_are_lazers_default() {
    let f = flash(&[], &[(0.0, true)]);
    assert!(close(f.alpha(1000.0), 0.3));
    assert!(close(f.alpha(2000.0), 0.3));
    // 1000 ms beats fade over 920 ms.
    assert!(f.alpha(1500.0) > 0.0);
    assert_eq!(f.alpha(1920.0), 0.0);
}

#[test]
fn kiai_colour_brightens_the_combo_colour() {
    let c = |r: u8, g: u8, b: u8| Color {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    };
    // Dark: 300 − 60 = 240 added.
    let k = kiai_colour(c(10, 20, 30));
    assert_eq!(
        [k.r, k.g, k.b].map(|v| (v * 255.0).round() as u8),
        [250, 255, 255]
    );
    // Bright: at least 25.
    let k = kiai_colour(c(200, 100, 50));
    assert_eq!(
        [k.r, k.g, k.b].map(|v| (v * 255.0).round() as u8),
        [225, 125, 75]
    );
    assert_eq!(k.a, 1.0);
}

#[test]
fn a_green_line_on_a_fractional_beat_counts_for_that_beat_at_every_frame() {
    // 173.913… ms beats from a fractional offset; kiai turns on and off on beats.
    let (t0, l) = (123.456_789, 60_000.0 / 345.0);
    let on = t0 + 20.0 * l;
    let off = t0 + 30.0 * l;
    let f = flash(&[(t0, l)], &[(on, true), (off, false)]);
    for i in 0..1000 {
        let dt = f64::from(i) / 1000.0 * 50.0;
        assert!(f.alpha(on + dt) > 0.0, "kiai beat at +{dt}");
        // The beat that ends kiai does not flash; the previous flash is long over.
        assert_eq!(f.alpha(off + dt), 0.0, "off beat at +{dt}");
    }
}

#[test]
fn a_red_line_out_of_kiai_lets_the_running_flash_fade_out() {
    // The flash of 1000 fades until 1420; the new point at 1020 ends kiai.
    let f = flash(
        &[(0.0, 500.0), (1020.0, 500.0)],
        &[(0.0, true), (1020.0, false)],
    );
    let expected = 0.3 * (1.0 - (21.0 / 420.0 * std::f64::consts::FRAC_PI_2).sin() as f32);
    assert!(close(f.alpha(1021.0), expected), "{}", f.alpha(1021.0));
    assert!(f.alpha(1400.0) > 0.0);
    assert_eq!(f.alpha(1420.0), 0.0);
    assert_eq!(f.alpha(1520.0), 0.0);
}

#[test]
fn a_new_timing_point_in_kiai_restarts_the_flash_on_its_own_grid() {
    let f = flash(&[(0.0, 1000.0), (1500.0, 250.0)], &[(0.0, true)]);
    // The flash of 1000 (920 ms) is replaced at 1500, which fades over 170 ms.
    assert!(close(f.alpha(1500.0), 0.3));
    assert_eq!(f.alpha(1670.0), 0.0);
    assert!(close(f.alpha(1750.0), 0.3));
}

#[test]
fn very_short_beats_keep_fading_over_several_beats_out_of_kiai() {
    // 30 ms beats: the 80 ms fade of the last kiai beat (at 270) spans the beats after it.
    let f = flash(&[(0.0, 30.0)], &[(0.0, true), (290.0, false)]);
    let at = 330.0;
    let expected = 0.3 * (1.0 - ((at - 270.0) / 80.0 * std::f64::consts::FRAC_PI_2).sin() as f32);
    assert!(
        close(f.alpha(at), expected),
        "{} vs {expected}",
        f.alpha(at)
    );
    assert_eq!(f.alpha(350.0), 0.0);
}

#[test]
fn paused_time_gives_the_same_alpha() {
    let f = flash(&[(0.0, 500.0)], &[(0.0, true)]);
    assert_eq!(f.alpha(1123.0), f.alpha(1123.0));
}
