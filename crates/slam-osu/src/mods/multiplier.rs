//! Score multipliers of the built-in mods.

use crate::dotnet;

/// Which of lazer's osu!standard multiplier tables applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiplierVersion {
    /// `OsuScoreMultiplierCalculatorV1`: scores whose total score version is below 30000017.
    V1,
    /// `OsuScoreMultiplierCalculatorV2`: current scores.
    V2,
}

/// Lazer's `TotalScoreVersion` of the mod multiplier rebalance (`LegacyScoreEncoder`).
pub const TOTAL_SCORE_VERSION_MULTIPLIER_REBALANCE: i32 = 30_000_017;

impl MultiplierVersion {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/OsuRuleset.cs (CreateScoreMultiplierCalculator)
    /// The table for a score with the given total score version; `None` (no score, the
    /// multipliers of a new play) selects V2.
    pub fn for_total_score_version(version: Option<i32>) -> MultiplierVersion {
        match version {
            Some(v) if v < TOTAL_SCORE_VERSION_MULTIPLIER_REBALANCE => MultiplierVersion::V1,
            _ => MultiplierVersion::V2,
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuScoreMultiplierCalculatorV2.cs (easyMultiplier)
pub(crate) fn easy_v2(retries: i32, default_retries: i32) -> f64 {
    // 0.8x base multiplier
    // Reduce by 0.1x per extra life
    let value = 0.8 - dotnet::max(0.0, 0.1 * f64::from(retries - default_retries));

    dotnet::max(0.4, value)
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuScoreMultiplierCalculatorV2.cs (halfTimeMultiplier)
pub(crate) fn half_time_v2(speed_change: f64) -> f64 {
    // 0.2x at 0.5x speed, +0.07x per 0.05x speed increment.
    // Default HT (0.75x) = 0.55
    f64::from((speed_change * 20.0) as i32) / 20.0 * 1.4 - 0.5
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuScoreMultiplierCalculatorV2.cs (doubleTimeMultiplier)
pub(crate) fn double_time_v2(speed_change: f64) -> f64 {
    // Floor to the nearest multiple of 0.1.
    let value = f64::from((speed_change * 10.0) as i32) / 10.0;

    // 0.01 penalty for non-default rates.
    let penalty = if value != 1.5 && value != 1.0 {
        0.01
    } else {
        0.0
    };

    // Linear from 1.0 to 1.46, minus the penalty.
    // Default DT (1.5x) = 1.23
    (value - 1.0) * 0.46 + 1.0 - penalty
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuScoreMultiplierCalculatorV2.cs (hiddenMultiplier)
pub(crate) fn hidden_v2(
    only_fade_approach_circles: bool,
    other_mods_provide_timing_info: bool,
) -> f64 {
    let mut value = 1.04;

    if only_fade_approach_circles {
        value -= 0.02;
    }

    if other_mods_provide_timing_info {
        value -= 0.02;
    }

    value
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuScoreMultiplierCalculatorV1.cs (rateAdjustMultiplier)
pub(crate) fn rate_adjust_v1(speed_change: f64) -> f64 {
    // Round to the nearest multiple of 0.1.
    let mut value = f64::from((speed_change * 10.0) as i32) / 10.0;

    // Offset back to 0.
    value -= 1.0;

    if speed_change >= 1.0 {
        1.0 + value / 5.0
    } else {
        0.6 + value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    /// The values in lazer's comments; exact bits are checked against lazer in
    /// `tests/mods_lazer.rs`.
    #[test]
    fn defaults_match_lazer_comments() {
        assert!(close(double_time_v2(1.5), 1.23));
        assert!(close(half_time_v2(0.75), 0.55));
        assert!(close(half_time_v2(0.5), 0.2));
        assert!(close(easy_v2(2, 2), 0.8));
        assert!(close(easy_v2(0, 2), 0.8));
        assert!(close(easy_v2(10, 2), 0.4));
        assert!(close(hidden_v2(false, false), 1.04));
        assert!(close(rate_adjust_v1(1.5), 1.1));
    }

    #[test]
    fn version_selection() {
        assert_eq!(
            MultiplierVersion::for_total_score_version(None),
            MultiplierVersion::V2
        );
        assert_eq!(
            MultiplierVersion::for_total_score_version(Some(30_000_016)),
            MultiplierVersion::V1
        );
        assert_eq!(
            MultiplierVersion::for_total_score_version(Some(30_000_017)),
            MultiplierVersion::V2
        );
    }
}
