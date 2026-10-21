//! Song select hotkeys that change mod settings: the speed of the rate mods, as lazer's
//! `ModSpeedHotkeyHandler`, and SLAM's approach rate through Difficulty Adjust (ADR-0056).

use super::customise::DA_DECIMALS;
use super::setting::DifficultyRange;
use super::setting::set_bindable_double;
use super::{
    DA_AR_RANGE, DA_RANGE, DifficultyAdjustSettings, GameplayMod, Mod, ModSet, RateAdjust,
    SPEED_DECIMALS,
};
use super::{SPEED_DOWN, SPEED_UP, is_default_speed};

/// The step of the mod speed hotkeys (lazer's `IncreaseModSpeed`/`DecreaseModSpeed`).
pub const SPEED_STEP: f64 = 0.05;

/// The step of the approach rate hotkeys.
pub const APPROACH_RATE_STEP: f64 = 0.1;

/// Whether a mod is one of lazer's `ModRateAdjust`s.
fn is_rate_adjust(m: &Mod) -> bool {
    matches!(
        m,
        Mod::HalfTime(_) | Mod::DoubleTime(_) | Mod::Daycore { .. } | Mod::Nightcore { .. }
    )
}

/// The speed of a rate mod.
fn speed_change(m: &Mod) -> Option<f64> {
    match *m {
        Mod::HalfTime(r) | Mod::DoubleTime(r) => Some(r.speed_change),
        Mod::Daycore { speed_change } | Mod::Nightcore { speed_change } => Some(speed_change),
        _ => None,
    }
}

impl Mod {
    /// The speed of a rate mod when it differs from the mod's default, as lazer's mod icons
    /// show it. Does not allocate.
    pub fn custom_speed(&self) -> Option<f64> {
        let default = match self {
            Mod::HalfTime(_) | Mod::Daycore { .. } => SPEED_DOWN.0,
            Mod::DoubleTime(_) | Mod::Nightcore { .. } => SPEED_UP.0,
            _ => return None,
        };
        speed_change(self).filter(|&speed| !is_default_speed(speed, default))
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/ModSpeedHotkeyHandler.cs
/// The state of the mod speed hotkeys: the last rate mod the selection had, whose kind
/// (Nightcore and Daycore or Double Time and Half Time) and other settings a new speed keeps.
#[derive(Debug, Clone, Default)]
pub struct SpeedHotkeys {
    last_rate_mod: Option<Mod>,
}

impl SpeedHotkeys {
    /// Remembers the rate mod of `mods`, if it has one (`storeLastActiveRateAdjustMod`). Call
    /// after every change of the selection or of its settings.
    pub fn observe(&mut self, mods: &ModSet) {
        if let Some(m) = mods.mods.iter().find(|m| is_rate_adjust(m)) {
            self.last_rate_mod = Some(m.clone());
        }
    }

    /// Changes the speed of the selection by `delta` (`ChangeSpeed`): a speed of about 1
    /// removes the rate mod, a lower one selects Half Time (Daycore after a Nightcore or
    /// Daycore), a higher one Double Time (Nightcore). The speed goes through the mod's bounds
    /// and precision. Returns `false`, leaving the selection as it was, when the new mod is
    /// incompatible with the others. Does not allocate once the set has room for one more mod.
    pub fn change_speed(&mut self, mods: &mut ModSet, delta: f64) -> bool {
        let position = mods.mods.iter().position(is_rate_adjust);
        let current = position.and_then(|p| speed_change(&mods.mods[p]));
        let target_speed = current.unwrap_or(1.0) + delta;

        // `Precision.AlmostEquals(targetSpeed, 1, 0.005)`.
        if (target_speed - 1.0).abs() <= 0.005 {
            if let Some(p) = position {
                mods.mods.remove(p);
            }
            return true;
        }

        let core = matches!(
            self.last_rate_mod,
            Some(Mod::Daycore { .. } | Mod::Nightcore { .. })
        );
        let up = target_speed >= 1.0;
        let range = if up { SPEED_UP } else { SPEED_DOWN };
        // `SpeedChange.Value = targetSpeed` on a fresh mod.
        let Some(speed) = set_bindable_double(target_speed, range.1, range.2, SPEED_DECIMALS)
        else {
            return false;
        };
        // The other settings come from the last rate mod seen; only Double Time and Half
        // Time have one, `AdjustPitch`.
        let adjust_pitch = match self.last_rate_mod {
            Some(Mod::HalfTime(r) | Mod::DoubleTime(r)) => r.adjust_pitch,
            _ => false,
        };
        let rate = RateAdjust {
            speed_change: speed,
            adjust_pitch,
        };
        let target = match (core, up) {
            (true, true) => Mod::Nightcore {
                speed_change: speed,
            },
            (true, false) => Mod::Daycore {
                speed_change: speed,
            },
            (false, true) => Mod::DoubleTime(rate),
            (false, false) => Mod::HalfTime(rate),
        };

        // The other mods, then the new one (`Where(m => m is not ModRateAdjust).Append`).
        let removed = position.map(|p| mods.mods.remove(p));
        if mods.mods.iter().any(|m| conflicts(m, &target)) {
            if let (Some(p), Some(m)) = (position, removed) {
                mods.mods.insert(p, m);
            }
            return false;
        }
        mods.mods.push(target);
        self.observe(mods);
        true
    }
}

/// Whether two mods cannot be selected together (`ModUtils.CheckCompatibleSet` for a pair).
fn conflicts(a: &Mod, b: &Mod) -> bool {
    a.acronym().eq_ignore_ascii_case(b.acronym())
        || a.lists_incompatible(b)
        || b.lists_incompatible(a)
}

/// Changes the approach rate by `delta` through Difficulty Adjust (ADR-0056): from the
/// adjusted approach rate, or `beatmap` (the difficulty's own, without mods) when none is
/// set, to the next value on the setting's precision within the extended bounds, -10 to 11.
/// Past the normal bounds, 0 to 10, the extended limits come on; coming back within them,
/// they go unless another setting is past its normal bound. Reaching or passing the beatmap's value
/// (which may be off the precision) clears the setting, and a Difficulty Adjust without
/// settings goes. Returns `false`, leaving the selection as it was, when Difficulty Adjust
/// is incompatible with the selection. Does not allocate once the set has room for one
/// more mod.
pub fn change_approach_rate(mods: &mut ModSet, beatmap: f32, delta: f64) -> bool {
    let position = mods
        .mods
        .iter()
        .position(|m| matches!(m, Mod::DifficultyAdjust(_)));
    let mut settings = match position.map(|p| &mods.mods[p]) {
        Some(Mod::DifficultyAdjust(d)) => *d,
        _ => DifficultyAdjustSettings::default(),
    };
    let base = settings.approach_rate.unwrap_or(beatmap);
    let Some(target) = set_bindable_double(
        f64::from(base) + delta,
        f64::from(DA_AR_RANGE.extended_min.unwrap_or(DA_AR_RANGE.min)),
        f64::from(DA_AR_RANGE.extended_max.unwrap_or(DA_AR_RANGE.max)),
        DA_DECIMALS,
    ) else {
        return false;
    };
    let target = target as f32;
    let beyond =
        |v: Option<f32>, range: DifficultyRange| v.is_some_and(|v| v < range.min || v > range.max);
    let was_beyond = beyond(settings.approach_rate, DA_AR_RANGE);
    // From an adjusted value off the beatmap's, reaching or passing the beatmap's goes back
    // to it.
    let back = settings.approach_rate.is_some()
        && base != beatmap
        && (base - beatmap) * (target - beatmap) <= 0.0;
    settings.approach_rate = (!back && target != beatmap).then_some(target);

    // The extended limits follow the approach rate across the normal bounds only, so a
    // flag set otherwise stays.
    if beyond(settings.approach_rate, DA_AR_RANGE) {
        settings.extended_limits = true;
    } else if was_beyond {
        settings.extended_limits = [
            settings.circle_size,
            settings.drain_rate,
            settings.overall_difficulty,
        ]
        .into_iter()
        .any(|v| beyond(v, DA_RANGE));
    }

    let empty = settings == DifficultyAdjustSettings::default();
    match position {
        Some(p) if empty => {
            mods.mods.remove(p);
        }
        Some(p) => mods.mods[p] = Mod::DifficultyAdjust(settings),
        None if empty => {}
        None => {
            let added = Mod::DifficultyAdjust(settings);
            if mods.mods.iter().any(|m| conflicts(m, &added)) {
                return false;
            }
            mods.mods.push(added);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(mods: Vec<Mod>) -> ModSet {
        ModSet::new(mods).unwrap()
    }

    fn dt(speed_change: f64, adjust_pitch: bool) -> Mod {
        Mod::DoubleTime(RateAdjust {
            speed_change,
            adjust_pitch,
        })
    }

    fn ht(speed_change: f64, adjust_pitch: bool) -> Mod {
        Mod::HalfTime(RateAdjust {
            speed_change,
            adjust_pitch,
        })
    }

    fn da(approach_rate: Option<f32>) -> Mod {
        Mod::DifficultyAdjust(DifficultyAdjustSettings {
            approach_rate,
            ..Default::default()
        })
    }

    /// Selects `mods` and presses a speed hotkey `steps` times (negative: down).
    fn press_speed(mods: Vec<Mod>, steps: i32) -> ModSet {
        let mut mods = set(mods);
        let mut hotkeys = SpeedHotkeys::default();
        hotkeys.observe(&mods);
        for _ in 0..steps.abs() {
            assert!(hotkeys.change_speed(&mut mods, SPEED_STEP * f64::from(steps.signum())));
        }
        mods
    }

    #[test]
    fn speed_up_selects_double_time_on_the_precision_grid() {
        assert_eq!(press_speed(vec![], 1).mods(), [dt(1.05, false)]);
        // Each step rounds, so the additions of 0.05 do not drift.
        assert_eq!(press_speed(vec![], 10).mods(), [dt(1.5, false)]);
        assert_eq!(press_speed(vec![], 20).mods(), [dt(2.0, false)]);
        // 2.05 is clamped to the maximum.
        assert_eq!(press_speed(vec![], 25).mods(), [dt(2.0, false)]);
    }

    #[test]
    fn speed_down_selects_half_time_and_clamps() {
        assert_eq!(press_speed(vec![], -1).mods(), [ht(0.95, false)]);
        assert_eq!(press_speed(vec![], -10).mods(), [ht(0.5, false)]);
        assert_eq!(press_speed(vec![], -11).mods(), [ht(0.5, false)]);
    }

    #[test]
    fn speed_of_one_removes_the_rate_mod() {
        assert!(press_speed(vec![dt(1.05, false)], -1).mods().is_empty());
        assert!(press_speed(vec![ht(0.75, false)], 5).mods().is_empty());
        // Then the other side, from 1.
        assert_eq!(
            press_speed(vec![dt(1.05, false)], -2).mods(),
            [ht(0.95, false)]
        );
        // Below the minimum of Double Time the speed crosses to Half Time.
        assert_eq!(
            press_speed(vec![dt(1.01, false)], -1).mods(),
            [ht(0.96, false)]
        );
    }

    #[test]
    fn nightcore_and_daycore_stay_the_kind() {
        let nc = Mod::Nightcore { speed_change: 1.5 };
        assert_eq!(
            press_speed(vec![nc.clone()], 1).mods(),
            [Mod::Nightcore { speed_change: 1.55 }]
        );
        // Through 1 (no rate mod) to Daycore: the last rate mod seen was a Nightcore.
        assert_eq!(
            press_speed(vec![nc], -11).mods(),
            [Mod::Daycore { speed_change: 0.95 }]
        );
    }

    #[test]
    fn adjust_pitch_carries_over() {
        assert_eq!(
            press_speed(vec![dt(1.05, true)], -2).mods(),
            [ht(0.95, true)]
        );
    }

    #[test]
    fn the_rate_mod_moves_to_the_end() {
        let mods = press_speed(vec![dt(1.5, false), Mod::HardRock], 1);
        assert_eq!(mods.mods(), [Mod::HardRock, dt(1.55, false)]);
    }

    #[test]
    fn speed_without_a_rate_mod_on_one_does_nothing() {
        let mut mods = set(vec![Mod::HardRock]);
        let mut hotkeys = SpeedHotkeys::default();
        assert!(hotkeys.change_speed(&mut mods, 0.004));
        assert_eq!(mods.mods(), [Mod::HardRock]);
    }

    fn approach_rate(mods: &ModSet) -> Option<f32> {
        match mods.get("DA") {
            Some(Mod::DifficultyAdjust(d)) => d.approach_rate,
            _ => None,
        }
    }

    #[test]
    fn custom_speed_is_a_speed_off_the_default() {
        assert_eq!(dt(1.5, true).custom_speed(), None);
        assert_eq!(dt(1.55, false).custom_speed(), Some(1.55));
        assert_eq!(Mod::Daycore { speed_change: 0.75 }.custom_speed(), None);
        assert_eq!(Mod::Daycore { speed_change: 0.7 }.custom_speed(), Some(0.7));
        assert_eq!(Mod::HardRock.custom_speed(), None);
    }

    #[test]
    fn approach_rate_adds_difficulty_adjust() {
        let mut mods = set(vec![Mod::Hidden {
            only_fade_approach_circles: false,
        }]);
        assert!(change_approach_rate(&mut mods, 9.3, APPROACH_RATE_STEP));
        assert_eq!(approach_rate(&mods), Some(9.4));
        assert_eq!(mods.mods().len(), 2);
        for _ in 0..6 {
            assert!(change_approach_rate(&mut mods, 9.3, APPROACH_RATE_STEP));
        }
        assert_eq!(approach_rate(&mods), Some(10.0));
        assert!(change_approach_rate(&mut mods, 9.3, -0.7));
        // Back at the beatmap's value.
        assert_eq!(mods.mods().len(), 1);
    }

    #[test]
    fn approach_rate_back_at_the_beatmap_removes_difficulty_adjust() {
        let mut mods = set(vec![da(Some(9.4))]);
        assert!(change_approach_rate(&mut mods, 9.3, -APPROACH_RATE_STEP));
        assert!(mods.mods().is_empty());

        // A Difficulty Adjust with other settings stays.
        let other = DifficultyAdjustSettings {
            circle_size: Some(4.0),
            approach_rate: Some(9.4),
            ..Default::default()
        };
        let mut mods = set(vec![Mod::DifficultyAdjust(other)]);
        assert!(change_approach_rate(&mut mods, 9.3, -APPROACH_RATE_STEP));
        assert_eq!(
            mods.mods(),
            [Mod::DifficultyAdjust(DifficultyAdjustSettings {
                approach_rate: None,
                ..other
            })]
        );
    }

    fn extended_limits(mods: &ModSet) -> bool {
        matches!(mods.get("DA"), Some(Mod::DifficultyAdjust(d)) if d.extended_limits)
    }

    #[test]
    fn approach_rate_past_ten_turns_the_extended_limits_on_up_to_eleven() {
        let mut mods = set(vec![]);
        for _ in 0..15 {
            assert!(change_approach_rate(&mut mods, 9.8, APPROACH_RATE_STEP));
        }
        assert_eq!(approach_rate(&mods), Some(11.0));
        assert!(extended_limits(&mods));
        // Back within 0 to 10 they go again, and so does Difficulty Adjust at the beatmap's.
        for _ in 0..10 {
            assert!(change_approach_rate(&mut mods, 9.8, -APPROACH_RATE_STEP));
        }
        assert_eq!(approach_rate(&mods), Some(10.0));
        assert!(!extended_limits(&mods));
        assert!(change_approach_rate(&mut mods, 9.8, -APPROACH_RATE_STEP));
        assert_eq!(approach_rate(&mods), Some(9.9));
        assert!(change_approach_rate(&mut mods, 9.8, -APPROACH_RATE_STEP));
        assert!(mods.mods().is_empty());
    }

    #[test]
    fn approach_rate_below_zero_goes_down_to_minus_ten() {
        let mut mods = set(vec![]);
        assert!(change_approach_rate(&mut mods, 0.0, -APPROACH_RATE_STEP));
        assert_eq!(approach_rate(&mods), Some(-0.1));
        assert!(extended_limits(&mods));
        assert!(change_approach_rate(&mut mods, 0.0, -20.0));
        assert_eq!(approach_rate(&mods), Some(-10.0));
    }

    #[test]
    fn extended_limits_stay_for_another_setting_past_ten() {
        let settings = DifficultyAdjustSettings {
            circle_size: Some(10.5),
            approach_rate: Some(10.1),
            extended_limits: true,
            ..Default::default()
        };
        let mut mods = set(vec![Mod::DifficultyAdjust(settings)]);
        assert!(change_approach_rate(&mut mods, 9.0, -APPROACH_RATE_STEP));
        assert_eq!(approach_rate(&mods), Some(10.0));
        assert!(extended_limits(&mods));
    }

    #[test]
    fn an_adjusted_approach_rate_equal_to_the_beatmaps_steps() {
        let mut mods = set(vec![da(Some(9.0))]);
        assert!(change_approach_rate(&mut mods, 9.0, APPROACH_RATE_STEP));
        assert_eq!(approach_rate(&mods), Some(9.1));
    }

    #[test]
    fn extended_limits_set_otherwise_stay() {
        let settings = DifficultyAdjustSettings {
            approach_rate: Some(9.1),
            extended_limits: true,
            ..Default::default()
        };
        let mut mods = set(vec![Mod::DifficultyAdjust(settings)]);
        assert!(change_approach_rate(&mut mods, 9.0, APPROACH_RATE_STEP));
        assert!(extended_limits(&mods));
        // Back at the beatmap's, Difficulty Adjust stays for the flag.
        for _ in 0..2 {
            assert!(change_approach_rate(&mut mods, 9.0, -APPROACH_RATE_STEP));
        }
        assert_eq!(approach_rate(&mods), None);
        assert!(extended_limits(&mods));
    }

    #[test]
    fn approach_rate_bounds_widen_up_to_the_extended_ones() {
        let mut mods = set(vec![]);
        assert!(change_approach_rate(&mut mods, 12.0, -APPROACH_RATE_STEP));
        assert_eq!(approach_rate(&mods), Some(11.0));
    }

    #[test]
    fn approach_rate_off_the_precision_goes_back_to_the_beatmap() {
        let mut mods = set(vec![]);
        assert!(change_approach_rate(&mut mods, 8.75, APPROACH_RATE_STEP));
        assert_eq!(approach_rate(&mods), Some(8.8));
        assert!(change_approach_rate(&mut mods, 8.75, -APPROACH_RATE_STEP));
        assert!(mods.mods().is_empty());
        assert!(change_approach_rate(&mut mods, 8.75, -APPROACH_RATE_STEP));
        assert_eq!(approach_rate(&mods), Some(8.6));
        assert!(change_approach_rate(&mut mods, 8.75, APPROACH_RATE_STEP));
        assert_eq!(approach_rate(&mods), Some(8.7));
        assert!(change_approach_rate(&mut mods, 8.75, APPROACH_RATE_STEP));
        assert!(mods.mods().is_empty());
    }

    #[test]
    fn approach_rate_refuses_incompatible_mods() {
        for m in [Mod::HardRock, Mod::from_acronym("EZ")] {
            let mut mods = set(vec![m.clone()]);
            assert!(!change_approach_rate(&mut mods, 9.0, APPROACH_RATE_STEP));
            assert_eq!(mods.mods(), [m]);
        }
    }
}
