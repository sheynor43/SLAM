//! Mapping of difficulty settings (AR, CS, OD, HP) to gameplay values.

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/IBeatmapDifficultyInfo.cs (DifficultyRange)
/// Values a setting maps to at difficulty 0, 5 and 10 (lazer's `DifficultyRange`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DifficultyRange {
    /// Value at difficulty 0.
    pub min: f64,
    /// Value at difficulty 5.
    pub mid: f64,
    /// Value at difficulty 10.
    pub max: f64,
}

impl DifficultyRange {
    /// Creates a range.
    pub const fn new(min: f64, mid: f64, max: f64) -> Self {
        DifficultyRange { min, mid, max }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/IBeatmapDifficultyInfo.cs (DifficultyRange)
    /// The value at `difficulty`: linear between `min` and `mid` below 5 and between `mid` and
    /// `max` above, extrapolated outside 0..=10.
    pub fn value(&self, difficulty: f64) -> f64 {
        if difficulty > 5.0 {
            return self.mid + (self.max - self.mid) * normalised(difficulty);
        }
        if difficulty < 5.0 {
            return self.mid + (self.mid - self.min) * normalised(difficulty);
        }
        self.mid
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/IBeatmapDifficultyInfo.cs (DifficultyRangeInt)
    /// [`value`](Self::value) truncated towards zero, like a C# `(int)` cast.
    pub fn value_int(&self, difficulty: f64) -> i32 {
        // Since .NET 9 the cast saturates and maps NaN to 0, exactly like `as`.
        self.value(difficulty) as i32
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/IBeatmapDifficultyInfo.cs (DifficultyRange(double))
/// Maps a difficulty of 0..=10 to -1..=1.
pub fn normalised(difficulty: f64) -> f64 {
    (difficulty - 5.0) / 5.0
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/LegacyRulesetExtensions.cs (CalculateScaleFromCircleSize)
/// Object scale for a circle size. With `apply_fudge` the scale is multiplied by osu!stable's
/// allowance for a gamefield rounding bug fixed in 2013, which old replays rely on; lazer's
/// osu!standard objects apply it.
pub fn scale_from_circle_size(circle_size: f32, apply_fudge: bool) -> f32 {
    const BROKEN_GAMEFIELD_ROUNDING_ALLOWANCE: f32 = 1.00041;

    // The inner expression is evaluated in double precision, as in C#.
    let scale = (f64::from(1.0f32) - f64::from(0.7f32) * normalised(f64::from(circle_size))) as f32;
    scale / 2.0
        * if apply_fudge {
            BROKEN_GAMEFIELD_ROUNDING_ALLOWANCE
        } else {
            1.0
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREEMPT: DifficultyRange = DifficultyRange::new(1800.0, 1200.0, 450.0);

    #[test]
    fn range_is_piecewise_linear() {
        assert_eq!(PREEMPT.value(0.0), 1800.0);
        assert_eq!(PREEMPT.value(5.0), 1200.0);
        assert_eq!(PREEMPT.value(10.0), 450.0);
        assert_eq!(PREEMPT.value(9.0), 600.0);
        assert_eq!(PREEMPT.value(11.0), 300.0);
        assert_eq!(PREEMPT.value(-1.0), 1920.0);
    }

    #[test]
    fn range_int_truncates_towards_zero() {
        assert_eq!(DifficultyRange::new(0.0, 0.0, 9.9).value_int(10.0), 9);
        assert_eq!(DifficultyRange::new(0.0, 0.0, -9.9).value_int(10.0), -9);
    }

    #[test]
    fn scale_at_cs_4() {
        // (1 - 0.7 * -0.2) / 2 = 0.57, times the allowance.
        assert_eq!(scale_from_circle_size(4.0, false), 0.57);
        assert_eq!(scale_from_circle_size(4.0, true), 0.57 * 1.00041);
    }
}
