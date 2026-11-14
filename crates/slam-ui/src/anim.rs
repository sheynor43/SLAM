//! Easing and time-based progress for interface animations (ARCHITECTURE §12: fade/slide
//! 150–250 ms, ease-out).

/// Cubic ease-out: fast start, gentle stop. `t` is clamped to `0 ..= 1`.
pub fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

/// Quintic ease-out: faster start and softer stop than cubic. `t` is clamped to `0 ..= 1`.
pub fn ease_out_quint(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv * inv * inv
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/DefaultEasingFunction.cs (OutElastic)
/// Elastic ease-out: overshoots and settles, ending exactly at 1. `t` is clamped to `0 ..= 1`.
pub fn ease_out_elastic(t: f32) -> f32 {
    elastic(f64::from(t.clamp(0.0, 1.0)), 1.0) as f32
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/DefaultEasingFunction.cs (OutElasticQuarter)
/// Elastic ease-out of a quarter of the wave: a short, tight wobble. `t` is clamped to `0 ..= 1`.
pub fn ease_out_elastic_quarter(t: f32) -> f32 {
    elastic(f64::from(t.clamp(0.0, 1.0)), 0.25) as f32
}

/// The framework's elastic curves share one shape; `part` is the share of the wave used.
fn elastic(t: f64, part: f64) -> f64 {
    const CONST: f64 = 2.0 * std::f64::consts::PI / 0.3;
    const CONST2: f64 = 0.3 / 4.0;
    // Fixes the curve to end at 1 (the full wave's offset is 2^-11).
    let offset = if part == 1.0 {
        2f64.powi(-11)
    } else {
        2f64.powi(-10) * ((part - CONST2) * CONST).sin()
    };
    2f64.powf(-10.0 * t) * ((part * t - CONST2) * CONST).sin() + 1.0 - offset * t
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
    fn elastic_curves_start_at_0_end_at_1_and_overshoot() {
        for ease in [ease_out_elastic, ease_out_elastic_quarter] {
            assert!(ease(0.0).abs() < 1e-5);
            assert!((ease(1.0) - 1.0).abs() < 1e-5);
        }
        let peak = (1..100)
            .map(|i| ease_out_elastic(i as f32 / 100.0))
            .fold(0.0, f32::max);
        assert!(peak > 1.0, "{peak}");
        let quarter = (1..100)
            .map(|i| ease_out_elastic_quarter(i as f32 / 100.0))
            .fold(0.0, f32::max);
        assert!(quarter > 1.0, "{quarter}");
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
