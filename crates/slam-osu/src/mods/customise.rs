//! What the mod settings panel shows of each built-in mod (ADR-0057): its settings in lazer's
//! `SettingSource` order, with their kinds, bounds and precision, and their values as the
//! panel reads and writes them. Nothing here allocates.

use slam_formats::osr::SettingValue;

use super::setting::{DifficultyRange, set_bindable_double, set_bindable_int};
use super::{
    DA_AR_RANGE, DA_RANGE, EASY_RETRIES, GameplayMod, MIRROR_TYPE_NAMES, MirrorType, Mod, ModSet,
    SPEED_DECIMALS, SPEED_DOWN, SPEED_UP,
};

/// `Precision = 0.1` of Difficulty Adjust's settings, as decimal places.
pub(super) const DA_DECIMALS: u32 = 1;

/// The step of a number by its decimal places.
const STEPS: [f64; 3] = [1.0, 0.1, 0.01];

/// The beatmap attribute an unset Difficulty Adjust setting stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DifficultyAttribute {
    /// Circle Size.
    CircleSize,
    /// Approach Rate.
    ApproachRate,
    /// HP Drain.
    DrainRate,
    /// Overall Difficulty (Accuracy).
    OverallDifficulty,
}

/// The kind of a setting and its bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingKind {
    /// A number on a slider (lazer's `BindableNumber`), `decimals` places of precision.
    Number {
        /// The value of a fresh mod; `None` for a Difficulty Adjust setting, which is unset.
        default: Option<f64>,
        /// `MinValue`.
        min: f64,
        /// `MaxValue`.
        max: f64,
        /// The bounds with Difficulty Adjust's extended limits on, where they differ.
        extended_min: Option<f64>,
        /// See `extended_min`.
        extended_max: Option<f64>,
        /// `Precision` as decimal places.
        decimals: u32,
        /// What an unset value shows: the beatmap's attribute.
        beatmap: Option<DifficultyAttribute>,
    },
    /// A checkbox.
    Toggle {
        /// The value of a fresh mod.
        default: bool,
    },
    /// A dropdown of `count` named values.
    Choice {
        /// The number of values.
        count: usize,
        /// The index of the value of a fresh mod.
        default: usize,
    },
}

/// One setting of a mod, as the panel lists it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettingInfo {
    /// Lazer's setting name, as in a score block.
    pub key: &'static str,
    /// The kind and bounds.
    pub kind: SettingKind,
}

/// The value of a setting, as the panel shows and changes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingState {
    /// `None`: an unset Difficulty Adjust setting.
    Number(Option<f64>),
    /// A checkbox.
    Toggle(bool),
    /// `None`: a value without a name (a Mirror type read from a score block).
    Choice(Option<usize>),
}

const fn number(default: f64, min: f64, max: f64, decimals: u32) -> SettingKind {
    SettingKind::Number {
        default: Some(default),
        min,
        max,
        extended_min: None,
        extended_max: None,
        decimals,
        beatmap: None,
    }
}

const fn difficulty(range: DifficultyRange, beatmap: DifficultyAttribute) -> SettingKind {
    SettingKind::Number {
        default: None,
        min: range.min as f64,
        max: range.max as f64,
        extended_min: match range.extended_min {
            Some(v) => Some(v as f64),
            None => None,
        },
        extended_max: match range.extended_max {
            Some(v) => Some(v as f64),
            None => None,
        },
        decimals: DA_DECIMALS,
        beatmap: Some(beatmap),
    }
}

const fn info(key: &'static str, kind: SettingKind) -> SettingInfo {
    SettingInfo { key, kind }
}

const fn toggle(key: &'static str, default: bool) -> SettingInfo {
    info(key, SettingKind::Toggle { default })
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModEasyWithExtraLives.cs
const EASY: [SettingInfo; 1] = [info(
    "retries",
    number(
        EASY_RETRIES.0 as f64,
        EASY_RETRIES.1 as f64,
        EASY_RETRIES.2 as f64,
        0,
    ),
)];
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModHalfTime.cs
const HALF_TIME: [SettingInfo; 2] = [
    info(
        "speed_change",
        number(SPEED_DOWN.0, SPEED_DOWN.1, SPEED_DOWN.2, SPEED_DECIMALS),
    ),
    toggle("adjust_pitch", false),
];
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModDaycore.cs
const DAYCORE: [SettingInfo; 1] = [info(
    "speed_change",
    number(SPEED_DOWN.0, SPEED_DOWN.1, SPEED_DOWN.2, SPEED_DECIMALS),
)];
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModDoubleTime.cs
const DOUBLE_TIME: [SettingInfo; 2] = [
    info(
        "speed_change",
        number(SPEED_UP.0, SPEED_UP.1, SPEED_UP.2, SPEED_DECIMALS),
    ),
    toggle("adjust_pitch", false),
];
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModNightcore.cs
const NIGHTCORE: [SettingInfo; 1] = [info(
    "speed_change",
    number(SPEED_UP.0, SPEED_UP.1, SPEED_UP.2, SPEED_DECIMALS),
)];
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModHidden.cs
const HIDDEN: [SettingInfo; 1] = [toggle("only_fade_approach_circles", false)];
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModClassic.cs
const CLASSIC: [SettingInfo; 5] = [
    toggle("no_slider_head_accuracy", true),
    toggle("classic_note_lock", true),
    toggle("always_play_tail_sample", true),
    toggle("fade_hit_circle_early", true),
    toggle("classic_health", true),
];
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModDifficultyAdjust.cs
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModDifficultyAdjust.cs
/// By `OrderPosition`: Circle Size, HP Drain, Accuracy, Approach Rate, then Extended Limits,
/// which has none.
const DIFFICULTY_ADJUST: [SettingInfo; 5] = [
    info(
        "circle_size",
        difficulty(DA_RANGE, DifficultyAttribute::CircleSize),
    ),
    info(
        "drain_rate",
        difficulty(DA_RANGE, DifficultyAttribute::DrainRate),
    ),
    info(
        "overall_difficulty",
        difficulty(DA_RANGE, DifficultyAttribute::OverallDifficulty),
    ),
    info(
        "approach_rate",
        difficulty(DA_AR_RANGE, DifficultyAttribute::ApproachRate),
    ),
    toggle("extended_limits", false),
];
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModMirror.cs
const MIRROR: [SettingInfo; 1] = [info(
    "reflection",
    SettingKind::Choice {
        count: MIRROR_TYPE_NAMES.len(),
        default: 0,
    },
)];
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModSuddenDeath.cs
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModFailCondition.cs
/// The osu! class's setting, then the base class's, as reflection lists them.
const SUDDEN_DEATH: [SettingInfo; 2] = [
    toggle("fail_on_slider_tail", false),
    toggle("restart", false),
];
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModPerfect.cs
const PERFECT: [SettingInfo; 1] = [toggle("restart", true)];

impl SettingKind {
    /// The bounds of a number: the normal ones, or the extended ones with `extended` on
    /// (`DifficultyBindable.updateExtents`).
    fn plain_bounds(&self, extended: bool) -> Option<(f64, f64)> {
        let SettingKind::Number {
            min,
            max,
            extended_min,
            extended_max,
            ..
        } = *self
        else {
            return None;
        };
        Some(if extended {
            (extended_min.unwrap_or(min), extended_max.unwrap_or(max))
        } else {
            (min, max)
        })
    }

    /// The step of one press: the precision of a number, nothing for the other kinds.
    pub fn step(&self) -> Option<f64> {
        match *self {
            SettingKind::Number { decimals, .. } => STEPS.get(decimals as usize).copied(),
            _ => None,
        }
    }

    /// The value of a fresh mod.
    pub fn default_state(&self) -> SettingState {
        match *self {
            SettingKind::Number { default, .. } => SettingState::Number(default),
            SettingKind::Toggle { default } => SettingState::Toggle(default),
            SettingKind::Choice { default, .. } => SettingState::Choice(Some(default)),
        }
    }
}

impl Mod {
    /// The settings the panel lists for this mod, in lazer's order; none for a mod without
    /// settings and for [`Mod::Other`].
    pub fn setting_infos(&self) -> &'static [SettingInfo] {
        match self {
            Mod::Easy { .. } => &EASY,
            Mod::HalfTime(_) => &HALF_TIME,
            Mod::Daycore { .. } => &DAYCORE,
            Mod::DoubleTime(_) => &DOUBLE_TIME,
            Mod::Nightcore { .. } => &NIGHTCORE,
            Mod::Hidden { .. } => &HIDDEN,
            Mod::Classic(_) => &CLASSIC,
            Mod::DifficultyAdjust(_) => &DIFFICULTY_ADJUST,
            Mod::Mirror { .. } => &MIRROR,
            Mod::SuddenDeath { .. } => &SUDDEN_DEATH,
            Mod::Perfect { .. } => &PERFECT,
            _ => &[],
        }
    }

    /// Whether lazer opens the settings panel when this mod is selected
    /// (`RequiresConfiguration`): only Difficulty Adjust.
    pub fn requires_configuration(&self) -> bool {
        matches!(self, Mod::DifficultyAdjust(_))
    }

    /// Whether Difficulty Adjust's extended limits are on; `false` for any other mod.
    pub fn extended_limits(&self) -> bool {
        matches!(self, Mod::DifficultyAdjust(d) if d.extended_limits)
    }

    /// The value of the setting `key`, if this mod lists it.
    pub fn setting_state(&self, key: &str) -> Option<SettingState> {
        use SettingState::{Choice, Number, Toggle};
        let number = |v: Option<f32>| Number(v.map(f64::from));
        Some(match (self, key) {
            (Mod::Easy { retries, .. }, "retries") => Number(Some(f64::from(*retries))),
            (Mod::HalfTime(r) | Mod::DoubleTime(r), "speed_change") => Number(Some(r.speed_change)),
            (Mod::HalfTime(r) | Mod::DoubleTime(r), "adjust_pitch") => Toggle(r.adjust_pitch),
            (Mod::Daycore { speed_change } | Mod::Nightcore { speed_change }, "speed_change") => {
                Number(Some(*speed_change))
            }
            (
                Mod::Hidden {
                    only_fade_approach_circles,
                },
                "only_fade_approach_circles",
            ) => Toggle(*only_fade_approach_circles),
            (Mod::Classic(c), "no_slider_head_accuracy") => Toggle(c.no_slider_head_accuracy),
            (Mod::Classic(c), "classic_note_lock") => Toggle(c.classic_note_lock),
            (Mod::Classic(c), "always_play_tail_sample") => Toggle(c.always_play_tail_sample),
            (Mod::Classic(c), "fade_hit_circle_early") => Toggle(c.fade_hit_circle_early),
            (Mod::Classic(c), "classic_health") => Toggle(c.classic_health),
            (Mod::DifficultyAdjust(d), "circle_size") => number(d.circle_size),
            (Mod::DifficultyAdjust(d), "drain_rate") => number(d.drain_rate),
            (Mod::DifficultyAdjust(d), "overall_difficulty") => number(d.overall_difficulty),
            (Mod::DifficultyAdjust(d), "approach_rate") => number(d.approach_rate),
            (Mod::DifficultyAdjust(d), "extended_limits") => Toggle(d.extended_limits),
            (Mod::Mirror { reflection }, "reflection") => Choice(match reflection {
                MirrorType::Undefined(_) => None,
                r => Some(r.raw() as usize),
            }),
            (Mod::SuddenDeath { restart, .. } | Mod::Perfect { restart }, "restart") => {
                Toggle(*restart)
            }
            (
                Mod::SuddenDeath {
                    fail_on_slider_tail,
                    ..
                },
                "fail_on_slider_tail",
            ) => Toggle(*fail_on_slider_tail),
            _ => return None,
        })
    }

    /// Sets the setting `key` from the panel; `bounds` are the slider's for a number. A
    /// number is clamped to them, goes to the setting's precision and is clamped again, as
    /// lazer's slider `BindableNumber` does (an integer is truncated, as `int.CreateTruncating`
    /// in `BindableNumber<int>.Set`). Difficulty Adjust's values round in `f64` and are then
    /// stored as `f32`, where lazer converts the `float` to a 7-digit `decimal`: values on the
    /// precision's grid agree, a value or bound off it (only a score block brings one) can
    /// round to the other neighbour. `Number(None)` unsets a Difficulty Adjust setting.
    /// Turning Difficulty Adjust's extended limits off clamps its values into the normal
    /// bounds: in lazer that is the settings control clamping its slider's `CurrentNumber`
    /// and writing it back, so it happens on this path only, not in [`Mod::set_setting`]. Lazer
    /// clamps the shown, rounded value, so an override off the grid just past a normal bound
    /// (CS 10.03) stays there, and an unset value whose beatmap attribute is past the bound
    /// becomes an override; here the first is clamped and the second stays unset.
    fn set_setting_state(&mut self, key: &str, state: SettingState, bounds: (f64, f64)) -> bool {
        let Some(info) = self.setting_infos().iter().find(|i| i.key == key) else {
            return false;
        };
        let (low, high) = bounds;
        let value = match (info.kind, state) {
            (_, SettingState::Number(None)) if !matches!(self, Mod::DifficultyAdjust(_)) => {
                return false;
            }
            (SettingKind::Number { .. }, SettingState::Number(None)) => SettingValue::Null,
            (SettingKind::Number { decimals: 0, .. }, SettingState::Number(Some(v))) => {
                // `int.CreateTruncating` saturates; NaN gives 0.
                let v = if v.is_nan() { 0 } else { v.trunc() as i32 };
                SettingValue::Int(i64::from(set_bindable_int(v, low as i32, high as i32)))
            }
            (SettingKind::Number { decimals, .. }, SettingState::Number(Some(v))) => {
                match set_bindable_double(v, low, high, decimals) {
                    Some(v) => SettingValue::Float(v),
                    None => return false,
                }
            }
            (SettingKind::Toggle { .. }, SettingState::Toggle(b)) => SettingValue::Bool(b),
            (SettingKind::Choice { count, .. }, SettingState::Choice(Some(i))) if i < count => {
                SettingValue::Int(i as i64)
            }
            _ => return false,
        };
        let was_extended = self.extended_limits();
        if !self.set_setting(key, &value) {
            return false;
        }
        if let Mod::DifficultyAdjust(d) = self
            && was_extended
            && !d.extended_limits
        {
            let clamp = |v: &mut Option<f32>, range: DifficultyRange| {
                if let Some(v) = v {
                    *v = crate::dotnet::clamp_f32(*v, range.min, range.max);
                }
            };
            clamp(&mut d.circle_size, DA_RANGE);
            clamp(&mut d.drain_rate, DA_RANGE);
            clamp(&mut d.overall_difficulty, DA_RANGE);
            clamp(&mut d.approach_rate, DA_AR_RANGE);
        }
        true
    }
}

/// The settings of Difficulty Adjust with a slider, in [`DIFFICULTY_ADJUST`]'s order.
const DA_NUMBERS: usize = 4;

/// The bounds the sliders of Difficulty Adjust offer, kept between changes as lazer's
/// `DifficultyBindable.CurrentNumber` keeps them: a value set from outside the panel (a
/// score block, the approach rate hotkeys) widens them up to the extended bounds, and they
/// go back to the normal or extended bounds only when the extended limits flip. Other
/// numbers have fixed bounds. Keep one next to the selection and [`observe`](Self::observe)
/// it after every change.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SliderBounds {
    /// The extended limits seen last and the bounds of each slider; `None` without
    /// Difficulty Adjust.
    difficulty: Option<(bool, [(f64, f64); DA_NUMBERS])>,
}

impl SliderBounds {
    /// Follows the selection: a new Difficulty Adjust (or flipped extended limits) gets the
    /// normal or extended bounds, then every set value widens its slider's
    /// (`DifficultyBindable.Value`). Does not allocate.
    pub fn observe(&mut self, mods: &ModSet) {
        let Some(Mod::DifficultyAdjust(d)) = mods
            .mods
            .iter()
            .find(|m| matches!(m, Mod::DifficultyAdjust(_)))
        else {
            self.difficulty = None;
            return;
        };
        let extended = d.extended_limits;
        let kinds = DIFFICULTY_ADJUST.map(|i| i.kind);
        let (mut seen, mut bounds) = self
            .difficulty
            .unwrap_or((!extended, [(0.0, 0.0); DA_NUMBERS]));
        if seen != extended {
            for (b, kind) in bounds.iter_mut().zip(kinds) {
                *b = kind.plain_bounds(extended).unwrap_or(*b);
            }
            seen = extended;
        }
        let values = [
            d.circle_size,
            d.drain_rate,
            d.overall_difficulty,
            d.approach_rate,
        ];
        for ((b, kind), value) in bounds.iter_mut().zip(kinds).zip(values) {
            let (Some(v), Some((min, max)), Some((lowest, highest))) =
                (value, kind.plain_bounds(false), kind.plain_bounds(true))
            else {
                continue;
            };
            let v = f64::from(v);
            // `Math.Clamp(MathF.Min(CurrentNumber.MinValue, value), ExtendedMinValue ?? MinValue,
            // MinValue)`; NaN, which only a score block can bring, leaves the bounds alone.
            if !v.is_nan() {
                b.0 = b.0.min(v).clamp(lowest, min);
                b.1 = b.1.max(v).clamp(max, highest);
            }
        }
        self.difficulty = Some((seen, bounds));
    }

    /// Forgets the bounds, as for a fresh Difficulty Adjust. Call when the selection is
    /// replaced as a whole (a preset, the mods of a score), where a new Difficulty Adjust may
    /// take the place of the old one without a selection in between.
    pub fn reset(&mut self) {
        self.difficulty = None;
    }

    /// The bounds of the slider of `key` of `m`; `None` for a setting that is not a number.
    pub fn get(&self, m: &Mod, key: &str) -> Option<(f64, f64)> {
        let index = m.setting_infos().iter().position(|i| i.key == key)?;
        let kind = m.setting_infos()[index].kind;
        match (m, self.difficulty) {
            (Mod::DifficultyAdjust(_), Some((_, bounds))) if index < DA_NUMBERS => {
                Some(bounds[index])
            }
            _ => kind.plain_bounds(m.extended_limits()),
        }
    }
}

impl ModSet {
    /// Sets the setting `key` of the mod `acronym` from the panel and follows the change in
    /// `bounds`. A number is clamped to its slider's bounds and goes to the setting's
    /// precision (see the private `Mod::set_setting_state`). Returns `false`, changing
    /// nothing, when the set has no such mod, the mod does not list `key` or the value does
    /// not fit its kind. Settings never change which mods are compatible. Does not allocate.
    pub fn set_setting_state(
        &mut self,
        acronym: &str,
        key: &str,
        state: SettingState,
        bounds: &mut SliderBounds,
    ) -> bool {
        let Some(m) = self.mods.iter_mut().find(|m| m.acronym() == acronym) else {
            return false;
        };
        let slider = bounds.get(m, key).unwrap_or((0.0, 0.0));
        let changed = m.set_setting_state(key, state, slider);
        if changed {
            bounds.observe(self);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::{ClassicSettings, DifficultyAdjustSettings, RateAdjust};

    fn da(settings: DifficultyAdjustSettings) -> Mod {
        Mod::DifficultyAdjust(settings)
    }

    fn set(mods: Vec<Mod>) -> (ModSet, SliderBounds) {
        let set = ModSet::new(mods).unwrap();
        let mut bounds = SliderBounds::default();
        bounds.observe(&set);
        (set, bounds)
    }

    fn change(
        (set, bounds): &mut (ModSet, SliderBounds),
        acronym: &str,
        key: &str,
        state: SettingState,
    ) -> bool {
        set.set_setting_state(acronym, key, state, bounds)
    }

    fn state((set, _): &(ModSet, SliderBounds), acronym: &str, key: &str) -> SettingState {
        set.get(acronym).unwrap().setting_state(key).unwrap()
    }

    fn slider((set, bounds): &(ModSet, SliderBounds), key: &str) -> (f64, f64) {
        bounds.get(set.get("DA").unwrap(), key).unwrap()
    }

    fn number(v: f64) -> SettingState {
        SettingState::Number(Some(v))
    }

    #[test]
    fn settings_are_listed_in_lazers_order() {
        let keys = |m: Mod| m.setting_infos().iter().map(|i| i.key).collect::<Vec<_>>();
        assert_eq!(
            keys(da(Default::default())),
            [
                "circle_size",
                "drain_rate",
                "overall_difficulty",
                "approach_rate",
                "extended_limits"
            ]
        );
        assert_eq!(
            keys(Mod::from_acronym("DT")),
            ["speed_change", "adjust_pitch"]
        );
        assert_eq!(keys(Mod::from_acronym("NC")), ["speed_change"]);
        assert_eq!(keys(Mod::from_acronym("CL")).len(), 5);
        assert_eq!(keys(Mod::from_acronym("MR")), ["reflection"]);
        assert_eq!(
            keys(Mod::from_acronym("SD")),
            ["fail_on_slider_tail", "restart"]
        );
        assert!(keys(Mod::from_acronym("HR")).is_empty());
        assert!(keys(Mod::from_acronym("XX")).is_empty());
    }

    #[test]
    fn defaults_match_a_fresh_mod() {
        for acronym in [
            "EZ", "HT", "DC", "DT", "NC", "HD", "CL", "DA", "MR", "SD", "PF",
        ] {
            let m = Mod::from_acronym(acronym);
            assert!(!m.setting_infos().is_empty(), "{acronym}");
            for info in m.setting_infos() {
                assert_eq!(
                    m.setting_state(info.key),
                    Some(info.kind.default_state()),
                    "{acronym} {}",
                    info.key
                );
            }
        }
    }

    #[test]
    fn fixed_bounds_and_steps() {
        let s = set(vec![
            Mod::from_acronym("EZ"),
            Mod::from_acronym("DT"),
            Mod::from_acronym("DA"),
        ]);
        let bounds = |acronym, key| s.1.get(s.0.get(acronym).unwrap(), key);
        assert_eq!(bounds("DT", "speed_change"), Some((1.01, 2.0)));
        assert_eq!(bounds("EZ", "retries"), Some((0.0, 10.0)));
        assert_eq!(bounds("DT", "adjust_pitch"), None);
        assert_eq!(slider(&s, "circle_size"), (0.0, 10.0));
        assert_eq!(slider(&s, "approach_rate"), (0.0, 10.0));
        let step = |m: Mod, key| {
            m.setting_infos()
                .iter()
                .find(|i| i.key == key)
                .unwrap()
                .kind
                .step()
        };
        assert_eq!(step(Mod::from_acronym("HT"), "speed_change"), Some(0.01));
        assert_eq!(step(Mod::from_acronym("DA"), "approach_rate"), Some(0.1));
        assert_eq!(step(Mod::from_acronym("EZ"), "retries"), Some(1.0));
    }

    // From lazer's `TestSceneModDifficultyAdjustSettings.TestExtendedLimits`.
    #[test]
    fn extended_limits_widen_and_narrow_the_sliders() {
        let mut s = set(vec![Mod::from_acronym("DA")]);
        assert!(change(&mut s, "DA", "circle_size", number(99.0)));
        assert_eq!(
            state(&s, "DA", "circle_size"),
            SettingState::Number(Some(10.0))
        );

        assert!(change(
            &mut s,
            "DA",
            "extended_limits",
            SettingState::Toggle(true)
        ));
        assert_eq!(slider(&s, "circle_size"), (0.0, 11.0));
        assert_eq!(slider(&s, "approach_rate"), (-10.0, 11.0));
        assert!(change(&mut s, "DA", "circle_size", number(99.0)));
        assert_eq!(
            state(&s, "DA", "circle_size"),
            SettingState::Number(Some(11.0))
        );
        assert!(change(&mut s, "DA", "approach_rate", number(-5.0)));
        assert_eq!(
            state(&s, "DA", "approach_rate"),
            SettingState::Number(Some(-5.0))
        );

        assert!(change(
            &mut s,
            "DA",
            "extended_limits",
            SettingState::Toggle(false)
        ));
        assert_eq!(
            state(&s, "DA", "circle_size"),
            SettingState::Number(Some(10.0))
        );
        assert_eq!(
            state(&s, "DA", "approach_rate"),
            SettingState::Number(Some(0.0))
        );
        assert_eq!(slider(&s, "circle_size"), (0.0, 10.0));
        assert_eq!(slider(&s, "approach_rate"), (0.0, 10.0));
    }

    // From lazer's `TestValueAboveRangeStillApplied` and `TestValueBelowRangeStillApplied`.
    #[test]
    fn values_set_from_outside_widen_the_sliders_until_the_limits_flip() {
        let mut s = set(vec![da(DifficultyAdjustSettings {
            circle_size: Some(11.0),
            approach_rate: Some(-5.0),
            ..Default::default()
        })]);
        assert_eq!(slider(&s, "circle_size"), (0.0, 11.0));
        assert_eq!(slider(&s, "approach_rate"), (-5.0, 10.0));

        // Dragging down keeps the widened bounds, so the value can come back.
        assert!(change(&mut s, "DA", "circle_size", number(5.0)));
        assert_eq!(slider(&s, "circle_size"), (0.0, 11.0));
        assert!(change(&mut s, "DA", "circle_size", number(10.96)));
        assert_eq!(
            state(&s, "DA", "circle_size"),
            SettingState::Number(Some(11.0))
        );
        assert!(change(
            &mut s,
            "DA",
            "approach_rate",
            SettingState::Number(None)
        ));
        assert_eq!(slider(&s, "approach_rate"), (-5.0, 10.0));

        // Turning the extended limits on and off resets them.
        assert!(change(
            &mut s,
            "DA",
            "extended_limits",
            SettingState::Toggle(true)
        ));
        assert!(change(
            &mut s,
            "DA",
            "extended_limits",
            SettingState::Toggle(false)
        ));
        assert_eq!(slider(&s, "circle_size"), (0.0, 10.0));
        assert_eq!(slider(&s, "approach_rate"), (0.0, 10.0));
        assert_eq!(
            state(&s, "DA", "circle_size"),
            SettingState::Number(Some(10.0))
        );
    }

    #[test]
    fn a_new_difficulty_adjust_gets_fresh_sliders() {
        let mut s = set(vec![da(DifficultyAdjustSettings {
            circle_size: Some(11.0),
            ..Default::default()
        })]);
        assert_eq!(slider(&s, "circle_size"), (0.0, 11.0));
        s.0 = ModSet::new(vec![]).unwrap();
        s.1.observe(&s.0);
        s.0 = ModSet::new(vec![Mod::from_acronym("DA")]).unwrap();
        s.1.observe(&s.0);
        assert_eq!(slider(&s, "circle_size"), (0.0, 10.0));

        s.0 = ModSet::new(vec![da(DifficultyAdjustSettings {
            circle_size: Some(11.0),
            ..Default::default()
        })])
        .unwrap();
        s.1.observe(&s.0);
        s.0 = ModSet::new(vec![Mod::from_acronym("DA")]).unwrap();
        s.1.reset();
        s.1.observe(&s.0);
        assert_eq!(slider(&s, "circle_size"), (0.0, 10.0));
    }

    #[test]
    fn numbers_go_to_the_precision_within_the_slider() {
        let mut s = set(vec![
            Mod::from_acronym("DT"),
            Mod::from_acronym("EZ"),
            Mod::from_acronym("DA"),
        ]);
        assert!(change(&mut s, "DT", "speed_change", number(1.234)));
        assert_eq!(state(&s, "DT", "speed_change"), number(1.23));
        assert!(change(&mut s, "DT", "speed_change", number(3.0)));
        assert_eq!(state(&s, "DT", "speed_change"), number(2.0));
        assert!(!change(
            &mut s,
            "DT",
            "speed_change",
            SettingState::Number(None)
        ));

        assert!(change(&mut s, "DA", "approach_rate", number(9.44)));
        assert_eq!(state(&s, "DA", "approach_rate"), number(f64::from(9.4f32)));
        assert!(change(&mut s, "DA", "approach_rate", number(-20.0)));
        assert_eq!(state(&s, "DA", "approach_rate"), number(0.0));
        assert!(change(
            &mut s,
            "DA",
            "approach_rate",
            SettingState::Number(None)
        ));
        assert_eq!(state(&s, "DA", "approach_rate"), SettingState::Number(None));

        // An integer slider truncates.
        assert!(change(&mut s, "EZ", "retries", number(4.9)));
        assert_eq!(state(&s, "EZ", "retries"), number(4.0));
        assert!(change(&mut s, "EZ", "retries", number(12.0)));
        assert_eq!(state(&s, "EZ", "retries"), number(10.0));
    }

    #[test]
    fn toggles_and_choices() {
        let mut s = set(vec![
            Mod::from_acronym("HT"),
            Mod::from_acronym("CL"),
            Mod::from_acronym("MR"),
            Mod::from_acronym("SD"),
        ]);
        assert!(change(
            &mut s,
            "HT",
            "adjust_pitch",
            SettingState::Toggle(true)
        ));
        assert!(change(&mut s, "HT", "speed_change", number(0.6)));
        assert_eq!(
            s.0.get("HT"),
            Some(&Mod::HalfTime(RateAdjust {
                speed_change: 0.6,
                adjust_pitch: true
            }))
        );
        assert!(!change(&mut s, "HT", "adjust_pitch", number(1.0)));

        assert!(change(
            &mut s,
            "CL",
            "classic_health",
            SettingState::Toggle(false)
        ));
        assert_eq!(
            s.0.get("CL"),
            Some(&Mod::Classic(ClassicSettings {
                classic_health: false,
                ..Default::default()
            }))
        );

        assert!(change(
            &mut s,
            "MR",
            "reflection",
            SettingState::Choice(Some(2))
        ));
        assert_eq!(
            s.0.get("MR"),
            Some(&Mod::Mirror {
                reflection: MirrorType::Both
            })
        );
        assert!(!change(
            &mut s,
            "MR",
            "reflection",
            SettingState::Choice(Some(3))
        ));
        assert!(!change(
            &mut s,
            "MR",
            "reflection",
            SettingState::Choice(None)
        ));

        assert!(change(
            &mut s,
            "SD",
            "fail_on_slider_tail",
            SettingState::Toggle(true)
        ));
        assert!(change(&mut s, "SD", "restart", SettingState::Toggle(true)));
        assert_eq!(
            s.0.get("SD"),
            Some(&Mod::SuddenDeath {
                restart: true,
                fail_on_slider_tail: true
            })
        );
        assert!(!change(&mut s, "PF", "restart", SettingState::Toggle(true)));
        assert!(!change(&mut s, "SD", "speed_change", number(1.0)));

        let m = Mod::Mirror {
            reflection: MirrorType::Undefined(7),
        };
        assert_eq!(
            m.setting_state("reflection"),
            Some(SettingState::Choice(None))
        );
    }

    #[test]
    fn only_difficulty_adjust_requires_configuration() {
        assert!(Mod::from_acronym("DA").requires_configuration());
        assert!(!Mod::from_acronym("DT").requires_configuration());
    }
}
