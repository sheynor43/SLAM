use super::*;

const SIZE: [f32; 2] = [1920.0, 1080.0];

fn close(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4
}

#[test]
fn the_centre_does_not_shift() {
    let target = pointer_target(Some([960.0, 540.0]), SIZE);
    assert!(close(target, [0.0, 0.0]));
    let map = layer_map(target, SIZE, 0.02, 1.0, true);

    assert!((map.tx - 960.0 * (1.0 - 1.02)).abs() < 1e-3);
}

#[test]
fn a_corner_shifts_by_the_full_amount() {
    let target = pointer_target(Some([1920.0, 1080.0]), SIZE);
    assert_eq!(target, [1.0, 1.0]);
    let map = layer_map(target, SIZE, 0.02, 1.0, false);
    // A layer that is not grown moves by amount * half the size.
    assert!(close(map.apply([0.0, 0.0]), [0.02 * 960.0, 0.02 * 540.0]));
    let top_left = pointer_target(Some([0.0, 0.0]), SIZE);
    assert_eq!(top_left, [-1.0, -1.0]);
}

#[test]
fn the_pointer_outside_the_window_is_clamped_or_centred() {
    assert_eq!(pointer_target(Some([5000.0, -300.0]), SIZE), [1.0, -1.0]);
    assert_eq!(pointer_target(None, SIZE), [0.0, 0.0]);
    assert_eq!(pointer_target(Some([f32::NAN, 1.0]), SIZE)[0], 0.0);
    assert_eq!(pointer_target(Some([1.0, 1.0]), [0.0, 0.0]), [0.0, 0.0]);
}

#[test]
fn the_shift_scales_with_the_strength() {
    let full = layer_map([1.0, 0.0], SIZE, 0.02, 1.0, false).tx;
    let half = layer_map([1.0, 0.0], SIZE, 0.02, 0.5, false).tx;
    assert!((half - full * 0.5).abs() < 1e-4);
    // The weaker layer shifts half as far.
    let ui = layer_map([1.0, 0.0], SIZE, 0.01, 1.0, false).tx;
    assert!((ui - full * 0.5).abs() < 1e-4);
}

#[test]
fn off_is_the_identity() {
    // Strength 0: the value 0 or the effects off (the caller passes 0).
    assert!(layer_map([1.0, 1.0], SIZE, 0.02, 0.0, true).is_identity());
    assert!(layer_map([1.0, 1.0], SIZE, 0.0, 1.0, true).is_identity());
}

#[test]
fn the_background_never_shows_an_edge() {
    for pointer in [[1.0, 1.0], [-1.0, -1.0], [1.0, -1.0], [0.0, 0.0]] {
        let map = layer_map(pointer, SIZE, 0.02, 1.0, true);
        let top_left = map.apply([0.0, 0.0]);
        let bottom_right = map.apply(SIZE);
        assert!(top_left[0] <= 1e-3 && top_left[1] <= 1e-3, "{pointer:?}");
        assert!(
            bottom_right[0] >= SIZE[0] - 1e-3 && bottom_right[1] >= SIZE[1] - 1e-3,
            "{pointer:?}"
        );
    }
}

#[test]
fn easing_takes_the_same_path_at_any_frame_rate() {
    let target = [1.0, -1.0];
    let run = |frames: u32| {
        let dt = 0.2 / f64::from(frames);
        let mut current = [0.0, 0.0];
        for _ in 0..frames {
            current = ease(current, target, dt, 0.04);
        }
        current
    };
    let reference = run(1);
    for frames in [2, 12, 24, 120, 480] {
        assert!(close(run(frames), reference), "{frames} frames");
    }
    // 0.2 s is five half-lives.
    assert!((reference[0] - (1.0 - 0.5_f32.powi(5))).abs() < 1e-4);
}

#[test]
fn easing_converges_and_a_zero_half_life_snaps() {
    let mut current = [0.0, 0.0];
    for _ in 0..600 {
        current = ease(current, [0.7, -0.3], 1.0 / 240.0, 0.04);
    }
    assert!(close(current, [0.7, -0.3]));
    assert_eq!(ease([0.0, 0.0], [1.0, 1.0], 0.01, 0.0), [1.0, 1.0]);
}

#[test]
fn the_pointer_leaving_eases_back_to_the_middle() {
    let pointer = PointerWatch::default();
    pointer.record_move(1920.0, 1080.0);
    let mut parallax = Parallax::new(pointer.clone(), WindowScale::default());
    let mut now = 0_u64;
    let mut shown = [0.0, 0.0];
    for _ in 0..120 {
        now += 8_333_333;
        shown = parallax.update(now, SIZE, 0.04);
    }
    assert!(close(shown, [1.0, 1.0]));
    pointer.record_outside(true);
    for _ in 0..120 {
        now += 8_333_333;
        shown = parallax.update(now, SIZE, 0.04);
    }
    assert!(close(shown, [0.0, 0.0]));
}

#[test]
fn a_long_stall_counts_as_a_capped_step() {
    let pointer = PointerWatch::default();
    pointer.record_move(1920.0, 540.0);
    let mut parallax = Parallax::new(pointer, WindowScale::default());
    parallax.update(0, SIZE, 0.04);
    // The first frame snaps nothing (dt 0); a stall of an hour counts as 100 ms.
    let shown = parallax.update(3_600_000_000_000, SIZE, 0.04);
    assert!((shown[0] - (1.0 - 0.5_f32.powf(2.5))).abs() < 1e-3);
}
