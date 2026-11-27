use super::*;
use crate::song_select::beat::BeatPoint;

const MS: u64 = 1_000_000;

fn timing() -> BeatTiming {
    let mut timing = BeatTiming::default();
    timing.push_point(BeatPoint {
        time: 0.0,
        beat_length: 500.0,
        meter: 4,
    });
    timing.push_kiai(4000.0, 6000.0);
    timing
}

/// Ticks every ms from `from` to `to` track ms (UI time = track time) with `spectrum`.
fn run(motion: &mut LogoMotion, timing: &BeatTiming, from: u64, to: u64, spectrum: &[f32]) {
    for ms in from..to {
        let audio = LogoAudio {
            track_ms: Some(ms as f64),
            timing: Some(timing),
            spectrum: Some(spectrum),
        };
        motion.tick(ms * MS, &audio, true);
    }
}

#[test]
fn the_logo_dips_on_each_beat_and_recovers() {
    let timing = timing();
    let quiet = [0.0; 256];
    let mut motion = LogoMotion::new();
    run(&mut motion, &timing, 0, 440, &quiet);
    let before = motion.snapshot(439 * MS).scale;
    // The beat at 500 fires at 440 and is lowest at 500.
    run(&mut motion, &timing, 440, 501, &quiet);
    let low = motion.snapshot(500 * MS);
    assert!(low.scale < before, "{} < {before}", low.scale);
    assert!(
        (low.scale - (1.0 - 0.02 * 0.4)).abs() < 1e-3,
        "{}",
        low.scale
    );
    assert!(low.ripple_alpha > 0.0);
    run(&mut motion, &timing, 501, 939, &quiet);
    assert!(motion.snapshot(938 * MS).scale > low.scale);
}

#[test]
fn hover_and_press_scale_the_logo() {
    let mut motion = LogoMotion::new();
    motion.set_hovered(true, 0);
    assert!((motion.snapshot(600 * MS).scale - HOVER_SCALE).abs() < 1e-3);
    motion.press(600 * MS);
    assert!(motion.snapshot(1600 * MS).scale < HOVER_SCALE * 0.95);
    motion.release(1600 * MS, true);
    let flash = motion.snapshot(1600 * MS).flash;
    assert!((flash - CLICK_FLASH).abs() < 1e-3);
    assert!((motion.snapshot(2200 * MS).scale - HOVER_SCALE).abs() < 1e-3);
}

#[test]
fn the_visualiser_follows_the_spectrum_and_decays() {
    let timing = timing();
    let mut loud = [0.0; 256];
    loud[..BARS].fill(0.8);
    let mut motion = LogoMotion::new();
    run(&mut motion, &timing, 1, 60, &loud);
    let bars = motion.snapshot(60 * MS).bars;
    // Halved outside kiai, then decayed a little.
    assert!(bars.iter().all(|&b| b > 0.3 && b <= 0.4), "{}", bars[0]);
    run(&mut motion, &timing, 60, 2000, &[0.0; 256]);
    assert!(motion.snapshot(2000 * MS).bars.iter().all(|&b| b == 0.0));
}

#[test]
fn effects_off_leaves_only_the_beat() {
    let timing = timing();
    let loud = [0.8; 256];
    let mut motion = LogoMotion::new();
    for ms in 0..520u64 {
        let audio = LogoAudio {
            track_ms: Some(ms as f64),
            timing: Some(&timing),
            spectrum: Some(&loud),
        };
        motion.tick(ms * MS, &audio, false);
    }
    let snapshot = motion.snapshot(520 * MS);
    assert!(snapshot.bars.iter().all(|&b| b == 0.0));
    assert!(snapshot.particles.iter().all(|p| p.alpha == 0.0));
    assert!(snapshot.scale < 1.0);
}

#[test]
fn beats_throw_triangles_and_flash_the_sides() {
    let timing = timing();
    let quiet = [0.0; 256];
    let mut motion = LogoMotion::new();
    // The bar's first beat (2000) flashes both sides.
    run(&mut motion, &timing, 1900, 2000, &quiet);
    let snapshot = motion.snapshot(2000 * MS);
    assert!(snapshot.sides[0] > 0.0 && snapshot.sides[1] > 0.0);
    assert!(snapshot.particles.iter().filter(|p| p.alpha > 0.0).count() >= 2);
    // An off beat does not; in kiai they take turns.
    run(&mut motion, &timing, 2000, 2500, &quiet);
    assert_eq!(motion.snapshot(2500 * MS).sides, [0.0; 2]);
    run(&mut motion, &timing, 2500, 4010, &quiet);
    let kiai = motion.snapshot(4010 * MS);
    assert!(
        (kiai.sides[0] > 0.0) != (kiai.sides[1] > 0.0),
        "{:?}",
        kiai.sides
    );
    assert!(kiai.flash > 0.0, "kiai flashes the logo");
}

#[test]
fn joining_the_music_mid_beat_does_not_fire_it() {
    let timing = timing();
    let mut motion = LogoMotion::new();
    run(&mut motion, &timing, 700, 701, &[0.0; 256]);
    assert_eq!(motion.snapshot(700 * MS).ripple_alpha, 0.0);
}
