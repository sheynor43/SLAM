//! The rows of the mod settings panel (ADR-0057): built from the selected mods and the
//! settings table of `slam-osu`, in fixed arrays so that a rebuild never allocates, and the
//! table that resolves a row the panel changed back to a mod and a setting.

use slam_osu::mods::{
    DifficultyAttribute, GameplayMod as _, Mod, ModSet, SettingKind, SettingState, SliderBounds,
};
use slam_ui::song_select::{
    MAX_CHOICES, MAX_SETTING_ROWS, SettingRow, SettingRowKind, SettingValue,
};

use super::mod_columns;
use crate::songs::SongFile;

/// The setting a row stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub acronym: &'static str,
    pub key: &'static str,
    pub kind: SettingKind,
}

/// Every Fluent key the panel uses, for the labels the view is given.
pub const TEXT_KEYS: &[&str] = &[
    "song-select-mods-name-ez",
    "song-select-mods-name-ht",
    "song-select-mods-name-dc",
    "song-select-mods-name-dt",
    "song-select-mods-name-nc",
    "song-select-mods-name-hd",
    "song-select-mods-name-cl",
    "song-select-mods-name-da",
    "song-select-mods-name-mr",
    "song-select-mods-name-sd",
    "song-select-mods-name-pf",
    "song-select-mods-setting-retries",
    "song-select-mods-setting-speed-decrease",
    "song-select-mods-setting-speed-increase",
    "song-select-mods-setting-adjust-pitch",
    "song-select-mods-setting-only-fade-approach-circles",
    "song-select-mods-setting-no-slider-head-accuracy",
    "song-select-mods-setting-classic-note-lock",
    "song-select-mods-setting-always-play-tail-sample",
    "song-select-mods-setting-fade-hit-circle-early",
    "song-select-mods-setting-classic-health",
    "song-select-mods-setting-circle-size",
    "song-select-mods-setting-drain-rate",
    "song-select-mods-setting-overall-difficulty",
    "song-select-mods-setting-approach-rate",
    "song-select-mods-setting-extended-limits",
    "song-select-mods-setting-reflection",
    "song-select-mods-setting-fail-on-slider-tail",
    "song-select-mods-setting-restart",
    "song-select-mods-mirror-horizontal",
    "song-select-mods-mirror-vertical",
    "song-select-mods-mirror-both",
];

/// The key of a mod's name.
fn name_key(acronym: &str) -> Option<&'static str> {
    Some(match acronym {
        "EZ" => "song-select-mods-name-ez",
        "HT" => "song-select-mods-name-ht",
        "DC" => "song-select-mods-name-dc",
        "DT" => "song-select-mods-name-dt",
        "NC" => "song-select-mods-name-nc",
        "HD" => "song-select-mods-name-hd",
        "CL" => "song-select-mods-name-cl",
        "DA" => "song-select-mods-name-da",
        "MR" => "song-select-mods-name-mr",
        "SD" => "song-select-mods-name-sd",
        "PF" => "song-select-mods-name-pf",
        _ => return None,
    })
}

/// The key of a setting's label; lazer's texts are the English ones.
fn label_key(acronym: &str, key: &str) -> Option<&'static str> {
    Some(match (acronym, key) {
        (_, "retries") => "song-select-mods-setting-retries",
        ("HT" | "DC", "speed_change") => "song-select-mods-setting-speed-decrease",
        ("DT" | "NC", "speed_change") => "song-select-mods-setting-speed-increase",
        (_, "adjust_pitch") => "song-select-mods-setting-adjust-pitch",
        (_, "only_fade_approach_circles") => "song-select-mods-setting-only-fade-approach-circles",
        (_, "no_slider_head_accuracy") => "song-select-mods-setting-no-slider-head-accuracy",
        (_, "classic_note_lock") => "song-select-mods-setting-classic-note-lock",
        (_, "always_play_tail_sample") => "song-select-mods-setting-always-play-tail-sample",
        (_, "fade_hit_circle_early") => "song-select-mods-setting-fade-hit-circle-early",
        (_, "classic_health") => "song-select-mods-setting-classic-health",
        (_, "circle_size") => "song-select-mods-setting-circle-size",
        (_, "drain_rate") => "song-select-mods-setting-drain-rate",
        (_, "overall_difficulty") => "song-select-mods-setting-overall-difficulty",
        (_, "approach_rate") => "song-select-mods-setting-approach-rate",
        (_, "extended_limits") => "song-select-mods-setting-extended-limits",
        (_, "reflection") => "song-select-mods-setting-reflection",
        (_, "fail_on_slider_tail") => "song-select-mods-setting-fail-on-slider-tail",
        (_, "restart") => "song-select-mods-setting-restart",
        _ => return None,
    })
}

/// The keys of a choice's value names, by raw value.
fn choice_keys(key: &str) -> [&'static str; MAX_CHOICES] {
    match key {
        "reflection" => [
            "song-select-mods-mirror-horizontal",
            "song-select-mods-mirror-vertical",
            "song-select-mods-mirror-both",
        ],
        _ => [""; MAX_CHOICES],
    }
}

/// The acronym of a panel tile equal to `acronym`, as a `'static` string.
fn static_acronym(acronym: &str) -> Option<&'static str> {
    mod_columns()
        .iter()
        .flat_map(|c| c.iter())
        .find(|a| **a == acronym)
        .copied()
}

/// The panel's rows and what each stands for.
#[derive(Clone, Debug)]
pub struct SettingRows {
    rows: [SettingRow; MAX_SETTING_ROWS],
    targets: [Option<Target>; MAX_SETTING_ROWS],
    len: usize,
}

impl Default for SettingRows {
    fn default() -> SettingRows {
        SettingRows {
            rows: [SettingRow::EMPTY; MAX_SETTING_ROWS],
            targets: [None; MAX_SETTING_ROWS],
            len: 0,
        }
    }
}

impl SettingRows {
    pub fn rows(&self) -> &[SettingRow] {
        &self.rows[..self.len]
    }

    pub fn target(&self, row: usize) -> Option<Target> {
        self.targets.get(row).copied().flatten()
    }

    fn push(&mut self, row: SettingRow, target: Option<Target>) -> bool {
        if self.len == MAX_SETTING_ROWS {
            return false;
        }
        self.rows[self.len] = row;
        self.targets[self.len] = target;
        self.len += 1;
        true
    }

    /// Rebuilds the rows for `mods`: a section per mod with settings, in the selection's
    /// order. An unset Difficulty Adjust number shows `file`'s value. Does not allocate.
    pub fn build(&mut self, mods: &ModSet, bounds: &SliderBounds, file: Option<&SongFile>) {
        self.len = 0;
        for m in mods.mods() {
            let infos = m.setting_infos();
            let Some(acronym) = static_acronym(m.acronym()) else {
                continue;
            };
            let Some(name) = name_key(acronym) else {
                continue;
            };
            if infos.is_empty() || !self.push(header(name), None) {
                continue;
            }
            for info in infos {
                let (Some(label), Some(state)) =
                    (label_key(acronym, info.key), m.setting_state(info.key))
                else {
                    continue;
                };
                let Some(kind) = row_kind(m, info.key, info.kind, state, bounds, file) else {
                    continue;
                };
                let suffix = if info.key == "speed_change" { "x" } else { "" };
                let target = Target {
                    acronym,
                    key: info.key,
                    kind: info.kind,
                };
                let row = SettingRow {
                    label,
                    suffix,
                    kind,
                };
                if !self.push(row, Some(target)) {
                    break;
                }
            }
        }
    }
}

fn header(name: &'static str) -> SettingRow {
    SettingRow {
        label: name,
        suffix: "",
        kind: SettingRowKind::Header,
    }
}

fn row_kind(
    m: &Mod,
    key: &'static str,
    kind: SettingKind,
    state: SettingState,
    bounds: &SliderBounds,
    file: Option<&SongFile>,
) -> Option<SettingRowKind> {
    Some(match (kind, state) {
        (
            SettingKind::Number {
                min,
                max,
                decimals,
                beatmap,
                ..
            },
            SettingState::Number(value),
        ) => {
            let (low, high) = bounds.get(m, key).unwrap_or((min, max));
            let from_beatmap = beatmap.zip(file).map(|(attribute, f)| {
                f64::from(match attribute {
                    DifficultyAttribute::CircleSize => f.circle_size,
                    DifficultyAttribute::ApproachRate => f.approach_rate,
                    DifficultyAttribute::DrainRate => f.drain_rate,
                    DifficultyAttribute::OverallDifficulty => f.overall_difficulty,
                })
            });
            // An unset value shows the beatmap's, within the bounds and to the precision.
            let scale = 10f64.powi(decimals as i32);
            let beatmap_value = from_beatmap.map(|v| (v.clamp(low, high) * scale).round() / scale);
            SettingRowKind::Number {
                value: value.or(beatmap_value).unwrap_or(low),
                min: low,
                max: high,
                step: kind.step()?,
                decimals: decimals as u8,
                unset: value.is_none(),
            }
        }
        (SettingKind::Toggle { .. }, SettingState::Toggle(value)) => {
            SettingRowKind::Toggle { value }
        }
        (SettingKind::Choice { count, .. }, SettingState::Choice(index)) => {
            SettingRowKind::Choice {
                index,
                count: count.min(MAX_CHOICES),
                values: choice_keys(key),
            }
        }
        _ => return None,
    })
}

/// The state a value from the panel stands for.
pub fn state_of(value: SettingValue) -> SettingState {
    match value {
        SettingValue::Number(n) => SettingState::Number(Some(n)),
        SettingValue::Toggle(b) => SettingState::Toggle(b),
        SettingValue::Choice(i) => SettingState::Choice(Some(i)),
    }
}
