//! Tolerant float comparisons (osu-framework's `Precision`).

// Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/Precision.cs
/// `Precision.FLOAT_EPSILON`.
pub(crate) const FLOAT_EPSILON: f32 = 1e-3;

/// `Precision.DOUBLE_EPSILON`.
pub(crate) const DOUBLE_EPSILON: f64 = 1e-7;

/// `Precision.AlmostEquals(float, float)` with the default epsilon.
pub(crate) fn almost_equals_f32(a: f32, b: f32) -> bool {
    (a - b).abs() <= FLOAT_EPSILON
}

/// `Precision.AlmostEquals(double, double)` with the default epsilon.
pub(crate) fn almost_equals_f64(a: f64, b: f64) -> bool {
    (a - b).abs() <= DOUBLE_EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epsilon_is_inclusive() {
        assert!(almost_equals_f32(0.0, 0.0009));
        assert!(!almost_equals_f32(0.0, 0.002));
        assert!(almost_equals_f64(1.0, 1.0 + 5e-8));
        assert!(!almost_equals_f64(1.0, 1.0 + 2e-7));
        assert!(!almost_equals_f64(f64::NAN, f64::NAN));
    }
}
