//! Mods: lazer's model (acronym + settings), the built-in osu!standard mods and the
//! [`GameplayMod`] trait that built-in and scripted mods share.
//!
//! A [`ModSet`] is built from a lazer score block ([`ModSet::from_api`]), a stable bitmask
//! ([`ModSet::from_legacy`]) or a whole replay ([`ModSet::from_replay`]). Settings are read
//! the way lazer's `APIMod.ToMod` reads them: a value lazer cannot convert keeps the default,
//! numbers are clamped and rounded to the setting's precision.
//!
//! Mods that change the beatmap (HR, EZ, DA, Mirror) do so through the beatmap hooks of
//! [`GameplayMod`]; [`ModSet::playable_beatmap`] runs them in lazer's order.

mod multiplier;
mod setting;

use slam_formats::osr::{ApiMod, OsrError, Replay, ScoreInfo, SettingValue};
use slam_formats::osu as osu_file;

use crate::beatmap::{Beatmap, BeatmapError, Difficulty};
use crate::dotnet;
use crate::objects::OsuHitObject;
use setting::DifficultyRange;

pub use multiplier::{MultiplierVersion, TOTAL_SCORE_VERSION_MULTIPLIER_REBALANCE};

/// Lazer's `LegacyScoreEncoder.FIRST_LAZER_VERSION`: replays of older versions come from
/// stable and are played with Classic.
pub const FIRST_LAZER_VERSION: i32 = 30_000_000;

/// Why a set of mods cannot be built.
#[derive(Debug, thiserror::Error)]
pub enum ModError {
    /// The same acronym appears twice, in any letter case (lazer cannot score such a set
    /// either).
    #[error("mod {0} appears more than once")]
    Duplicate(String),
    /// The replay's score-info block cannot be read.
    #[error(transparent)]
    Replay(#[from] OsrError),
}

/// How a mod changes the playback speed.
///
/// The track runs `rate` times faster. `frequency` of that is applied as a frequency change
/// (pitch moves with it); the rest, `rate / frequency`, as a tempo change (pitch kept).
/// Samples always get a frequency change by the full `rate` (lazer's
/// `ModRateAdjust.ApplyToSample`), whatever the pitch setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateChange {
    /// Overall speed factor.
    pub rate: f64,
    /// Part of the speed applied as a frequency change.
    pub frequency: f64,
}

/// Context of a score multiplier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MultiplierContext<'a> {
    /// Which table of multipliers applies.
    pub version: MultiplierVersion,
    /// The beatmap's difficulty before mods (Difficulty Adjust compares against it).
    pub difficulty_without_mods: &'a Difficulty,
}

/// The interface of every mod, built-in or scripted (ARCHITECTURE §14), in the scope M1
/// needs: the beatmap and difficulty hooks, the speed, failing and the score multiplier.
/// Hooks that need the drawing and input types (`on_object_spawn`, `on_frame`,
/// `transform_input`, `on_judgement`) are added together with those types.
///
/// Lazer applies the beatmap hooks in this order (`WorkingBeatmap.GetPlayableBeatmap`):
/// [`apply_to_difficulty`](Self::apply_to_difficulty), the objects' defaults,
/// [`apply_to_hit_object`](Self::apply_to_hit_object) (all objects for one mod, then the next
/// mod), stacking, [`apply_to_beatmap`](Self::apply_to_beatmap). Together the last two are
/// `on_beatmap_load` of ARCHITECTURE §14.
pub trait GameplayMod {
    /// The mod's acronym, e.g. `DT`.
    fn acronym(&self) -> &str;

    /// Changes the difficulty settings before the objects get their defaults
    /// (lazer's `IApplicableToDifficulty`).
    fn apply_to_difficulty(&self, _difficulty: &mut Difficulty) {}

    /// Changes one object after the defaults are applied and before stacking
    /// (lazer's `IApplicableToHitObject`).
    fn apply_to_hit_object(&self, _object: &mut OsuHitObject) {}

    /// Changes the whole beatmap after stacking (lazer's `IApplicableToBeatmap`).
    fn apply_to_beatmap(&mut self, _beatmap: &mut Beatmap) {}

    /// The speed change, if the mod has one (lazer's `IApplicableToRate`).
    fn rate(&self) -> Option<RateChange> {
        None
    }

    /// Called when the player would fail; `false` prevents the fail (lazer's
    /// `IApplicableFailOverride.PerformFail`). May change the mod's state, e.g. use up a
    /// life.
    fn perform_fail(&mut self) -> bool {
        true
    }

    /// The score multiplier when this mod is the only one of its combination.
    fn score_multiplier(&self, _context: &MultiplierContext<'_>) -> f64 {
        1.0
    }
}

/// Speed and pitch settings of Double Time and Half Time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateAdjust {
    /// `speed_change`: the playback speed.
    pub speed_change: f64,
    /// `adjust_pitch`: whether the pitch changes with the speed.
    pub adjust_pitch: bool,
}

/// Settings of Classic. All default to `true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassicSettings {
    /// `no_slider_head_accuracy`: sliders are scored by the number of ticks hit.
    pub no_slider_head_accuracy: bool,
    /// `classic_note_lock`: note lock applies to the full hit window.
    pub classic_note_lock: bool,
    /// `always_play_tail_sample`: a slider's tail sample plays even when missed.
    pub always_play_tail_sample: bool,
    /// `fade_hit_circle_early`: hit circles fade out into a miss, rather than after it.
    pub fade_hit_circle_early: bool,
    /// `classic_health`: stable's HP drain.
    pub classic_health: bool,
}

impl Default for ClassicSettings {
    fn default() -> Self {
        ClassicSettings {
            no_slider_head_accuracy: true,
            classic_note_lock: true,
            always_play_tail_sample: true,
            fade_hit_circle_early: true,
            classic_health: true,
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModDoubleTime.cs
/// `SpeedChange` of Double Time and Nightcore: default, minimum, maximum.
const SPEED_UP: (f64, f64, f64) = (1.5, 1.01, 2.0);
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModHalfTime.cs
/// `SpeedChange` of Half Time and Daycore: default, minimum, maximum.
const SPEED_DOWN: (f64, f64, f64) = (0.75, 0.5, 0.99);
/// `Precision = 0.01` of every `SpeedChange`, as decimal places.
const SPEED_DECIMALS: u32 = 2;

// Ported from osu-framework 2026.921.1: osu.Framework/Bindables/BindableNumber.cs (IsDefault)
/// `IsDefault` of a speed setting: within half the precision of the default.
fn is_default_speed(value: f64, default: f64) -> bool {
    (value - default).abs() <= 0.01 / 2.0
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModEasyWithExtraLives.cs
/// `Retries` of Easy: default, minimum, maximum.
const EASY_RETRIES: (i32, i32, i32) = (2, 0, 10);

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModHardRock.cs
/// `ADJUST_RATIO` of Hard Rock (Circle Size uses 1.3).
const HARD_ROCK_RATIO: f32 = 1.4;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModEasy.cs
/// `ADJUST_RATIO` of Easy.
const EASY_RATIO: f32 = 0.5;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModDifficultyAdjust.cs
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModDifficultyAdjust.cs
/// Bounds of Circle Size, Drain Rate and Overall Difficulty in Difficulty Adjust.
const DA_RANGE: DifficultyRange = DifficultyRange {
    min: 0.0,
    max: 10.0,
    extended_min: None,
    extended_max: Some(11.0),
};
/// Bounds of Approach Rate in Difficulty Adjust.
const DA_AR_RANGE: DifficultyRange = DifficultyRange {
    min: 0.0,
    max: 10.0,
    extended_min: Some(-10.0),
    extended_max: Some(11.0),
};

/// Settings of Difficulty Adjust. A set value replaces the beatmap's; `None` keeps it.
///
/// Values are clamped to 0..=10 (Approach Rate -10..=11, the others 0..=11 with the extended
/// limits) when read, whatever `extended_limits` says, as lazer's `DifficultyBindable` does.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DifficultyAdjustSettings {
    /// `circle_size`.
    pub circle_size: Option<f32>,
    /// `approach_rate`.
    pub approach_rate: Option<f32>,
    /// `drain_rate`: HP drain.
    pub drain_rate: Option<f32>,
    /// `overall_difficulty`: accuracy.
    pub overall_difficulty: Option<f32>,
    /// `extended_limits`: whether the settings UI offers values beyond 10.
    pub extended_limits: bool,
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModMirror.cs (MirrorType)
/// The axes Mirror flips the objects along (lazer's `MirrorType`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MirrorType {
    /// Flip left and right.
    #[default]
    Horizontal,
    /// Flip up and down.
    Vertical,
    /// Flip both.
    Both,
    /// A value without a name, which a score block can carry; it flips nothing.
    Undefined(i32),
}

/// The members of `MirrorType` as `Enum.Parse` sees them.
const MIRROR_TYPE_NAMES: [(&str, i32); 3] = [("Horizontal", 0), ("Vertical", 1), ("Both", 2)];

impl MirrorType {
    /// The enum of a raw value.
    pub fn from_raw(value: i32) -> MirrorType {
        match value {
            0 => MirrorType::Horizontal,
            1 => MirrorType::Vertical,
            2 => MirrorType::Both,
            v => MirrorType::Undefined(v),
        }
    }

    /// The raw value, as lazer writes it into a score block.
    pub fn raw(self) -> i32 {
        match self {
            MirrorType::Horizontal => 0,
            MirrorType::Vertical => 1,
            MirrorType::Both => 2,
            MirrorType::Undefined(v) => v,
        }
    }
}

/// A mod with its settings.
#[derive(Debug, Clone, PartialEq)]
pub enum Mod {
    /// `NF`.
    NoFail,
    /// `EZ`, with `retries` extra lives (0..=10).
    Easy {
        /// `retries`: extra lives.
        retries: i32,
    },
    /// `HT`, speed 0.5..=0.99.
    HalfTime(RateAdjust),
    /// `DC`, speed 0.5..=0.99; the pitch drops as at the default speed.
    Daycore {
        /// `speed_change`: the playback speed.
        speed_change: f64,
    },
    /// `HR`.
    HardRock,
    /// `DT`, speed 1.01..=2.
    DoubleTime(RateAdjust),
    /// `NC`, speed 1.01..=2; the pitch rises as at the default speed.
    Nightcore {
        /// `speed_change`: the playback speed.
        speed_change: f64,
    },
    /// `HD`.
    Hidden {
        /// `only_fade_approach_circles`: the object bodies do not fade.
        only_fade_approach_circles: bool,
    },
    /// `TC`.
    Traceable,
    /// `CL`.
    Classic(ClassicSettings),
    /// `DA`.
    DifficultyAdjust(DifficultyAdjustSettings),
    /// `MR`.
    Mirror {
        /// `reflection`: the flipped axes.
        reflection: MirrorType,
    },
    /// Any other acronym: kept with its raw settings, has no effect. A set with such a mod
    /// is not [`supported`](ModSet::is_supported).
    Other {
        /// The acronym as read.
        acronym: String,
        /// The settings as read.
        settings: Vec<(String, SettingValue)>,
    },
}

impl Mod {
    /// The built-in mod with default settings, or [`Mod::Other`] without settings.
    ///
    /// Acronyms are matched ignoring the letter case, as lazer's `CreateModFromAcronym` does;
    /// [`Mod::Other`] keeps the acronym as given.
    pub fn from_acronym(acronym: &str) -> Mod {
        match acronym.to_ascii_uppercase().as_str() {
            "NF" => Mod::NoFail,
            "EZ" => Mod::Easy {
                retries: EASY_RETRIES.0,
            },
            "HT" => Mod::HalfTime(RateAdjust {
                speed_change: SPEED_DOWN.0,
                adjust_pitch: false,
            }),
            "DC" => Mod::Daycore {
                speed_change: SPEED_DOWN.0,
            },
            "HR" => Mod::HardRock,
            "DT" => Mod::DoubleTime(RateAdjust {
                speed_change: SPEED_UP.0,
                adjust_pitch: false,
            }),
            "NC" => Mod::Nightcore {
                speed_change: SPEED_UP.0,
            },
            "HD" => Mod::Hidden {
                only_fade_approach_circles: false,
            },
            "TC" => Mod::Traceable,
            "CL" => Mod::Classic(ClassicSettings::default()),
            "DA" => Mod::DifficultyAdjust(DifficultyAdjustSettings::default()),
            "MR" => Mod::Mirror {
                reflection: MirrorType::default(),
            },
            _ => Mod::Other {
                acronym: acronym.to_owned(),
                settings: Vec::new(),
            },
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Online/API/APIMod.cs (ToMod)
    /// The mod of a lazer `APIMod`. Settings are looked up by their snake_case names; a value
    /// lazer cannot convert keeps the default. Settings are applied in order, so of duplicate
    /// keys the last convertible value wins (a JSON block has no duplicates: its reader keeps
    /// the last one).
    pub fn from_api(api: &ApiMod) -> Mod {
        let mut m = Mod::from_acronym(&api.acronym);
        if let Mod::Other { settings, .. } = &mut m {
            settings.clone_from(&api.settings);
            return m;
        }
        for (key, value) in &api.settings {
            m.set_setting(key, value);
        }
        m
    }

    /// Sets one setting the way lazer's bindable would: unknown keys and values lazer cannot
    /// convert are ignored, numbers are clamped and rounded to the setting's precision.
    /// Returns whether the value was accepted.
    ///
    /// Each setting is meant to be set once, as from a score block: a NaN Difficulty Adjust
    /// value is kept, but lazer would stop clamping later values of that setting (see
    /// `set_difficulty_bindable`).
    // Ported from osu-framework 2026.921.1: osu.Framework/Bindables/Bindable.cs (Parse)
    // Ported from osu-framework 2026.921.1: osu.Framework/Bindables/BindableBool.cs (Parse)
    pub fn set_setting(&mut self, key: &str, value: &SettingValue) -> bool {
        use setting::{
            set_bindable_double, set_bindable_int, set_difficulty_bindable, to_bool, to_enum,
            to_f64, to_i32, to_nullable_f32,
        };

        let speed = |range: (f64, f64, f64)| {
            to_f64(value).and_then(|v| set_bindable_double(v, range.1, range.2, SPEED_DECIMALS))
        };
        // `BindableBool` accepts "1" and "0"; Classic's last two settings are plain `Bindable<bool>`.
        let bindable_bool = || to_bool(value, true);
        let plain_bool = || to_bool(value, false);
        let difficulty = |range| to_nullable_f32(value).map(|v| set_difficulty_bindable(v, range));

        fn store<T>(slot: &mut T, new: Option<T>) -> bool {
            match new {
                Some(v) => {
                    *slot = v;
                    true
                }
                None => false,
            }
        }

        match (self, key) {
            (Mod::Easy { retries }, "retries") => store(
                retries,
                to_i32(value).map(|v| set_bindable_int(v, EASY_RETRIES.1, EASY_RETRIES.2)),
            ),
            (Mod::HalfTime(r), "speed_change") => store(&mut r.speed_change, speed(SPEED_DOWN)),
            (Mod::DoubleTime(r), "speed_change") => store(&mut r.speed_change, speed(SPEED_UP)),
            (Mod::HalfTime(r) | Mod::DoubleTime(r), "adjust_pitch") => {
                store(&mut r.adjust_pitch, bindable_bool())
            }
            (Mod::Daycore { speed_change }, "speed_change") => {
                store(speed_change, speed(SPEED_DOWN))
            }
            (Mod::Nightcore { speed_change }, "speed_change") => {
                store(speed_change, speed(SPEED_UP))
            }
            (
                Mod::Hidden {
                    only_fade_approach_circles,
                },
                "only_fade_approach_circles",
            ) => store(only_fade_approach_circles, bindable_bool()),
            (Mod::Classic(c), "no_slider_head_accuracy") => {
                store(&mut c.no_slider_head_accuracy, bindable_bool())
            }
            (Mod::Classic(c), "classic_note_lock") => {
                store(&mut c.classic_note_lock, bindable_bool())
            }
            (Mod::Classic(c), "always_play_tail_sample") => {
                store(&mut c.always_play_tail_sample, bindable_bool())
            }
            (Mod::Classic(c), "fade_hit_circle_early") => {
                store(&mut c.fade_hit_circle_early, plain_bool())
            }
            (Mod::Classic(c), "classic_health") => store(&mut c.classic_health, plain_bool()),
            (Mod::DifficultyAdjust(d), "circle_size") => {
                store(&mut d.circle_size, difficulty(DA_RANGE))
            }
            (Mod::DifficultyAdjust(d), "approach_rate") => {
                store(&mut d.approach_rate, difficulty(DA_AR_RANGE))
            }
            (Mod::DifficultyAdjust(d), "drain_rate") => {
                store(&mut d.drain_rate, difficulty(DA_RANGE))
            }
            (Mod::DifficultyAdjust(d), "overall_difficulty") => {
                store(&mut d.overall_difficulty, difficulty(DA_RANGE))
            }
            (Mod::DifficultyAdjust(d), "extended_limits") => {
                store(&mut d.extended_limits, bindable_bool())
            }
            (Mod::Mirror { reflection }, "reflection") => store(
                reflection,
                to_enum(value, &MIRROR_TYPE_NAMES).map(MirrorType::from_raw),
            ),
            _ => false,
        }
    }

    /// The settings that differ from their defaults, in lazer's order and with lazer's names
    /// (what lazer's `APIMod(Mod)` writes).
    pub fn settings(&self) -> Vec<(String, SettingValue)> {
        let mut out = Vec::new();
        let mut push = |name: &str, v: SettingValue| out.push((name.to_owned(), v));
        match self {
            Mod::Easy { retries } => {
                if *retries != EASY_RETRIES.0 {
                    push("retries", SettingValue::Int(i64::from(*retries)));
                }
            }
            Mod::HalfTime(r) | Mod::DoubleTime(r) => {
                let default = if matches!(self, Mod::HalfTime(_)) {
                    SPEED_DOWN.0
                } else {
                    SPEED_UP.0
                };
                if !is_default_speed(r.speed_change, default) {
                    push("speed_change", SettingValue::Float(r.speed_change));
                }
                if r.adjust_pitch {
                    push("adjust_pitch", SettingValue::Bool(true));
                }
            }
            Mod::Daycore { speed_change } if !is_default_speed(*speed_change, SPEED_DOWN.0) => {
                push("speed_change", SettingValue::Float(*speed_change));
            }
            Mod::Nightcore { speed_change } if !is_default_speed(*speed_change, SPEED_UP.0) => {
                push("speed_change", SettingValue::Float(*speed_change));
            }
            Mod::Hidden {
                only_fade_approach_circles: true,
            } => {
                push("only_fade_approach_circles", SettingValue::Bool(true));
            }
            Mod::Classic(c) => {
                let fields = [
                    ("no_slider_head_accuracy", c.no_slider_head_accuracy),
                    ("classic_note_lock", c.classic_note_lock),
                    ("always_play_tail_sample", c.always_play_tail_sample),
                    ("fade_hit_circle_early", c.fade_hit_circle_early),
                    ("classic_health", c.classic_health),
                ];
                for (name, value) in fields {
                    if !value {
                        push(name, SettingValue::Bool(false));
                    }
                }
            }
            Mod::DifficultyAdjust(d) => {
                // Lazer's reflection order: the osu! class's settings, then the base class's.
                let fields = [
                    ("circle_size", d.circle_size),
                    ("approach_rate", d.approach_rate),
                    ("drain_rate", d.drain_rate),
                    ("overall_difficulty", d.overall_difficulty),
                ];
                for (name, value) in fields {
                    if let Some(v) = value {
                        push(name, SettingValue::Float(float_setting(v)));
                    }
                }
                if d.extended_limits {
                    push("extended_limits", SettingValue::Bool(true));
                }
            }
            Mod::Mirror { reflection } if *reflection != MirrorType::default() => {
                push("reflection", SettingValue::Int(i64::from(reflection.raw())));
            }
            Mod::Other { settings, .. } => out.clone_from(settings),
            _ => {}
        }
        out
    }

    /// The lazer `APIMod` of this mod.
    pub fn to_api(&self) -> ApiMod {
        ApiMod {
            acronym: self.acronym().to_owned(),
            settings: self.settings(),
            extra: Vec::new(),
        }
    }

    /// Whether every setting has its default value (lazer's `UsesDefaultConfiguration`).
    pub fn uses_default_configuration(&self) -> bool {
        self.settings().is_empty()
    }

    /// Whether SLAM implements this mod.
    pub fn is_supported(&self) -> bool {
        !matches!(self, Mod::Other { .. })
    }
}

/// A `float` setting as the `double` that Newtonsoft's text of it (the shortest round-trip
/// form) reads back as, so a written block shows `8.3`, not `8.300000190734863`.
fn float_setting(v: f32) -> f64 {
    if v.is_finite() {
        v.to_string().parse().unwrap_or(f64::from(v))
    } else {
        f64::from(v)
    }
}

impl GameplayMod for Mod {
    fn acronym(&self) -> &str {
        match self {
            Mod::NoFail => "NF",
            Mod::Easy { .. } => "EZ",
            Mod::HalfTime(_) => "HT",
            Mod::Daycore { .. } => "DC",
            Mod::HardRock => "HR",
            Mod::DoubleTime(_) => "DT",
            Mod::Nightcore { .. } => "NC",
            Mod::Hidden { .. } => "HD",
            Mod::Traceable => "TC",
            Mod::Classic(_) => "CL",
            Mod::DifficultyAdjust(_) => "DA",
            Mod::Mirror { .. } => "MR",
            Mod::Other { acronym, .. } => acronym,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModHardRock.cs (ApplyToDifficulty)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModHardRock.cs (ApplyToDifficulty)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModEasy.cs (ApplyToDifficulty)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModEasy.cs (ApplyToDifficulty)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModDifficultyAdjust.cs (ApplySettings)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModDifficultyAdjust.cs (ApplySettings)
    fn apply_to_difficulty(&self, difficulty: &mut Difficulty) {
        match self {
            Mod::HardRock => {
                difficulty.drain_rate =
                    dotnet::min_f32(difficulty.drain_rate * HARD_ROCK_RATIO, 10.0);
                difficulty.overall_difficulty =
                    dotnet::min_f32(difficulty.overall_difficulty * HARD_ROCK_RATIO, 10.0);
                // CS uses a custom 1.3 ratio.
                difficulty.circle_size = dotnet::min_f32(difficulty.circle_size * 1.3, 10.0);
                difficulty.approach_rate =
                    dotnet::min_f32(difficulty.approach_rate * HARD_ROCK_RATIO, 10.0);
            }
            Mod::Easy { .. } => {
                difficulty.circle_size *= EASY_RATIO;
                difficulty.approach_rate *= EASY_RATIO;
                difficulty.drain_rate *= EASY_RATIO;
                difficulty.overall_difficulty *= EASY_RATIO;
            }
            Mod::DifficultyAdjust(d) => {
                if let Some(v) = d.drain_rate {
                    difficulty.drain_rate = v;
                }
                if let Some(v) = d.overall_difficulty {
                    difficulty.overall_difficulty = v;
                }
                if let Some(v) = d.circle_size {
                    difficulty.circle_size = v;
                }
                if let Some(v) = d.approach_rate {
                    difficulty.approach_rate = v;
                }
            }
            _ => {}
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModHardRock.cs (ApplyToHitObject)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModMirror.cs (ApplyToHitObject)
    fn apply_to_hit_object(&self, object: &mut OsuHitObject) {
        match self {
            Mod::HardRock => object.reflect_vertically(),
            Mod::Mirror { reflection } => match reflection {
                MirrorType::Horizontal => object.reflect_horizontally(),
                MirrorType::Vertical => object.reflect_vertically(),
                MirrorType::Both => {
                    object.reflect_horizontally();
                    object.reflect_vertically();
                }
                MirrorType::Undefined(_) => {}
            },
            _ => {}
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModRateAdjust.cs (ApplyToRate)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/RateAdjustModHelper.cs
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModNightcore.cs
    fn rate(&self) -> Option<RateChange> {
        match *self {
            Mod::HalfTime(r) | Mod::DoubleTime(r) => Some(RateChange {
                rate: r.speed_change,
                frequency: if r.adjust_pitch { r.speed_change } else { 1.0 },
            }),
            // The frequency is fixed at the default speed, the tempo makes up the rest.
            Mod::Daycore { speed_change } => Some(RateChange {
                rate: speed_change,
                frequency: SPEED_DOWN.0,
            }),
            Mod::Nightcore { speed_change } => Some(RateChange {
                rate: speed_change,
                frequency: SPEED_UP.0,
            }),
            _ => None,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModNoFail.cs (PerformFail)
    // Easy's extra lives (`ModEasyWithExtraLives.PerformFail`) refill the health and belong to
    // the health processor (issue #52); until then Easy lets the player fail.
    fn perform_fail(&mut self) -> bool {
        !matches!(self, Mod::NoFail)
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuScoreMultiplierCalculatorV1.cs
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuScoreMultiplierCalculatorV2.cs
    fn score_multiplier(&self, context: &MultiplierContext<'_>) -> f64 {
        use multiplier::*;
        match context.version {
            MultiplierVersion::V1 => match self {
                Mod::Easy { .. } | Mod::NoFail => 0.5,
                Mod::HalfTime(RateAdjust { speed_change, .. })
                | Mod::DoubleTime(RateAdjust { speed_change, .. })
                | Mod::Daycore { speed_change }
                | Mod::Nightcore { speed_change } => rate_adjust_v1(*speed_change),
                // Hard Rock has no settings, so it always uses the default configuration.
                Mod::HardRock => 1.06,
                Mod::Hidden { .. } if self.uses_default_configuration() => 1.06,
                Mod::Classic(_) => 0.96,
                Mod::DifficultyAdjust(_) => 0.5,
                _ => 1.0,
            },
            MultiplierVersion::V2 => match self {
                Mod::Easy { retries } => easy_v2(*retries, EASY_RETRIES.0),
                Mod::NoFail => 0.5,
                Mod::HalfTime(RateAdjust { speed_change, .. }) | Mod::Daycore { speed_change } => {
                    half_time_v2(*speed_change)
                }
                Mod::HardRock => 1.09,
                Mod::DoubleTime(RateAdjust { speed_change, .. })
                | Mod::Nightcore { speed_change } => double_time_v2(*speed_change),
                // No supported mod forms a combination with Hidden.
                Mod::Hidden {
                    only_fade_approach_circles,
                } => hidden_v2(*only_fade_approach_circles, false),
                Mod::Traceable => 1.02,
                Mod::DifficultyAdjust(d) => {
                    difficulty_adjust_v2(d, context.difficulty_without_mods)
                }
                Mod::Classic(c) => {
                    if c.classic_note_lock {
                        0.985
                    } else {
                        0.96
                    }
                }
                _ => 1.0,
            },
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Legacy/LegacyMods.cs
/// Bits of stable's mods bitmask that lazer converts for osu!standard.
mod legacy {
    pub const NO_FAIL: u32 = 1;
    pub const EASY: u32 = 1 << 1;
    pub const TOUCH_DEVICE: u32 = 1 << 2;
    pub const HIDDEN: u32 = 1 << 3;
    pub const HARD_ROCK: u32 = 1 << 4;
    pub const SUDDEN_DEATH: u32 = 1 << 5;
    pub const DOUBLE_TIME: u32 = 1 << 6;
    pub const RELAX: u32 = 1 << 7;
    pub const HALF_TIME: u32 = 1 << 8;
    pub const NIGHTCORE: u32 = 1 << 9;
    pub const FLASHLIGHT: u32 = 1 << 10;
    pub const AUTOPLAY: u32 = 1 << 11;
    pub const SPUN_OUT: u32 = 1 << 12;
    pub const AUTOPILOT: u32 = 1 << 13;
    pub const PERFECT: u32 = 1 << 14;
    pub const CINEMA: u32 = 1 << 22;
    pub const TARGET: u32 = 1 << 23;
    pub const SCORE_V2: u32 = 1 << 29;
}

/// The mods of a play, in order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModSet {
    mods: Vec<Mod>,
}

impl ModSet {
    /// A set of the given mods; each acronym may appear once (in any letter case).
    pub fn new(mods: Vec<Mod>) -> Result<ModSet, ModError> {
        for (i, m) in mods.iter().enumerate() {
            let same = |other: &Mod| other.acronym().eq_ignore_ascii_case(m.acronym());
            if mods[..i].iter().any(same) {
                return Err(ModError::Duplicate(m.acronym().to_owned()));
            }
        }
        Ok(ModSet { mods })
    }

    /// The mods of a lazer score block, in order.
    pub fn from_api(mods: &[ApiMod]) -> Result<ModSet, ModError> {
        ModSet::new(mods.iter().map(Mod::from_api).collect())
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/OsuRuleset.cs (ConvertFromLegacyMods)
    /// The mods of stable's bitmask, in lazer's order. Nightcore implies Double Time and
    /// Perfect implies Sudden Death, as in stable; bits lazer does not convert for
    /// osu!standard (keys, Fade In, Random, Mirror, ...) are ignored.
    pub fn from_legacy(mask: u32) -> ModSet {
        use legacy::*;
        let has = |bit: u32| mask & bit != 0;
        let mut mods = Vec::new();
        let mut add = |acronym: &str| mods.push(Mod::from_acronym(acronym));

        if has(NIGHTCORE) {
            add("NC");
        } else if has(DOUBLE_TIME) {
            add("DT");
        }
        if has(PERFECT) {
            add("PF");
        } else if has(SUDDEN_DEATH) {
            add("SD");
        }
        if has(AUTOPILOT) {
            add("AP");
        }
        if has(CINEMA) {
            add("CN");
        } else if has(AUTOPLAY) {
            add("AT");
        }
        let rest = [
            (EASY, "EZ"),
            (FLASHLIGHT, "FL"),
            (HALF_TIME, "HT"),
            (HARD_ROCK, "HR"),
            (HIDDEN, "HD"),
            (NO_FAIL, "NF"),
            (RELAX, "RX"),
            (SPUN_OUT, "SO"),
            (TARGET, "TP"),
            (TOUCH_DEVICE, "TD"),
            (SCORE_V2, "SV2"),
        ];
        for (bit, acronym) in rest {
            if has(bit) {
                add(acronym);
            }
        }
        ModSet { mods }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreDecoder.cs (Parse)
    /// The mods of a replay from its parts: the score block's mods when it has a non-empty
    /// block, otherwise the bitmask, plus Classic for versions before lazer. The version is
    /// not checked against the block: the `.osr` decoder reads a block only from versions that
    /// have one.
    pub fn from_replay_parts(
        version: i32,
        legacy_mask: u32,
        score_info: Option<&ScoreInfo>,
    ) -> Result<ModSet, ModError> {
        if let Some(info) = score_info {
            return ModSet::from_api(&info.mods);
        }
        let mut set = ModSet::from_legacy(legacy_mask);
        if version < FIRST_LAZER_VERSION {
            set.mods.push(Mod::Classic(ClassicSettings::default()));
        }
        Ok(set)
    }

    /// The mods of a decoded replay, see [`ModSet::from_replay_parts`].
    pub fn from_replay(replay: &Replay) -> Result<ModSet, ModError> {
        let info = replay.score_info()?;
        ModSet::from_replay_parts(replay.version, replay.mods, info.as_ref())
    }

    /// The playable beatmap of a decoded `.osu` file with these mods applied, see
    /// [`Beatmap::from_file_with_mods`]. Mods may keep state from their beatmap hooks and
    /// [`perform_fail`](Self::perform_fail), so each play (and each retry) takes a fresh clone
    /// of the selected set, as lazer deep-clones the mods for every `Player`.
    pub fn playable_beatmap(&mut self, file: osu_file::Beatmap) -> Result<Beatmap, BeatmapError> {
        Beatmap::from_file_with_mods(file, &mut self.mods)
    }

    /// The mods in order.
    pub fn mods(&self) -> &[Mod] {
        &self.mods
    }

    /// The mod with the given acronym.
    pub fn get(&self, acronym: &str) -> Option<&Mod> {
        self.mods.iter().find(|m| m.acronym() == acronym)
    }

    /// Whether SLAM implements every mod of the set.
    pub fn is_supported(&self) -> bool {
        self.mods.iter().all(Mod::is_supported)
    }

    /// The lazer `APIMod`s of the set.
    pub fn to_api(&self) -> Vec<ApiMod> {
        self.mods.iter().map(Mod::to_api).collect()
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/ModUtils.cs (CalculateRateWithMods)
    /// The combined speed change: rates multiplied in mod order, starting from 1.
    pub fn rate(&self) -> RateChange {
        let mut rate = RateChange {
            rate: 1.0,
            frequency: 1.0,
        };
        for change in self.mods.iter().filter_map(GameplayMod::rate) {
            rate.rate *= change.rate;
            rate.frequency *= change.frequency;
        }
        rate
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (CheckModsAllowFailure)
    /// Asks the mods in order whether the player fails; stops at the first that prevents it,
    /// so later mods keep their state (lazer's `All(m => m.PerformFail())`).
    pub fn perform_fail(&mut self) -> bool {
        self.mods.iter_mut().all(GameplayMod::perform_fail)
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreMultiplierCalculator.cs (CalculateFor)
    /// The score multiplier of the set: the mods' multipliers multiplied in mod order.
    /// Lazer's combination multipliers (Hidden with Blinds, Wiggle, ...) involve only mods SLAM
    /// does not implement. Those count as 1, so the result is lazer's only for a
    /// [`supported`](Self::is_supported) set.
    pub fn score_multiplier(&self, context: &MultiplierContext<'_>) -> f64 {
        let mut result = 1.0;
        for m in &self.mods {
            result *= m.score_multiplier(context);
        }
        result
    }
}

#[cfg(test)]
mod tests;
