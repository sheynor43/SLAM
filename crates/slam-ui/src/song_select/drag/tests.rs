use super::*;

const MS: u64 = 1_000_000;

#[test]
fn the_threshold_is_two_dimensional_and_blocking_is_vertical() {
    let mut d = ListDrag::default();
    d.press(100.0, 100.0);
    // 6-8-10: exactly the distance is no drag yet.
    assert_eq!(d.moved(106.0, 108.0, 1.0), 0.0);
    // Past it the drag starts without scrolling what came before.
    assert_eq!(d.moved(120.0, 100.0, 1.0), 0.0);
    assert_eq!(d.moved(120.0, 95.0, 1.0), 5.0);
    assert_eq!(d.release(), Some(Release::Click { x: 100.0, y: 100.0 }));

    d.press(100.0, 100.0);
    d.moved(100.0, 120.0, 1.0);
    assert_eq!(d.moved(100.0, 130.0, 2.0), -5.0);
    assert_eq!(d.release(), Some(Release::Dragged));
    assert_eq!(d.release(), None);
}

#[test]
fn the_fling_distance_matches_lazer() {
    // One drag step of 32 px in 16 ms, released on the same tick: velocity 2 px/ms, and
    // the fling goes velocity / (1 - e^-0.0035) further.
    let mut d = ListDrag::default();
    d.tick(0, 0.0, 1e6);
    d.press(0.0, 500.0);
    d.tick(10 * MS, 0.0, 1e6);
    // The drag starts at the last tick's time, 10 ms.
    d.moved(0.0, 480.0, 1.0);
    d.moved(0.0, 448.0, 1.0);
    d.release();
    let first = d.tick(26 * MS, 32.0, 1e6).unwrap();
    let average_time = 16.0;
    let average_delta = 32.0;
    let distance = (average_delta / average_time) / (1.0 - (-0.0035f64).exp());
    let expected = (32.0 + distance) + (-distance) * (-0.0035f64 * 16.0).exp();
    assert!(
        (f64::from(first) - expected).abs() < 1e-2,
        "{first} vs {expected}"
    );
}

#[test]
fn cancel_forgets_everything_but_the_clock() {
    let mut d = ListDrag::default();
    d.tick(5 * MS, 0.0, 100.0);
    d.press(0.0, 0.0);
    d.cancel();
    assert!(!d.is_pressed());
    assert_eq!(d.release(), None);
    assert_eq!(d.tick(6 * MS, 0.0, 100.0), None);
}
