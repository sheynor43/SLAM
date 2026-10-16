//! Easing and time-based progress for interface animations (ARCHITECTURE §12: fade/slide
//! 150–250 ms, ease-out).

/// Cubic ease-out: fast start, gentle stop. `t` is clamped to `0 ..= 1`.
pub fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

/// Linear progress of an animation that started at `start_ns` and lasts `duration_ns`, at
/// `now_ns`: 0 before the start, 1 from its end on. A zero duration is already done.
pub fn progress(start_ns: u64, now_ns: u64, duration_ns: u64) -> f32 {
    if duration_ns == 0 {
        return 1.0;
    }
    let elapsed = now_ns.saturating_sub(start_ns);
    if elapsed >= duration_ns {
        1.0
    } else {
        (elapsed as f64 / duration_ns as f64) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_cubic_ends_and_shape() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert_eq!(ease_out_cubic(2.0), 1.0);
        // Ease-out is ahead of linear in the middle.
        assert!((ease_out_cubic(0.5) - 0.875).abs() < 1e-6);
        let mut last = 0.0;
        for i in 1..=100 {
            let v = ease_out_cubic(i as f32 / 100.0);
            assert!(v >= last);
            last = v;
        }
    }

    #[test]
    fn progress_clamps_to_the_animation() {
        assert_eq!(progress(100, 50, 200), 0.0);
        assert_eq!(progress(100, 100, 200), 0.0);
        assert_eq!(progress(100, 200, 200), 0.5);
        assert_eq!(progress(100, 300, 200), 1.0);
        assert_eq!(progress(100, u64::MAX, 200), 1.0);
        assert_eq!(progress(100, 100, 0), 1.0);
    }
}
