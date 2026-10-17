//! The settings screen's rows: what each edits in [`Settings`], and their texts. The
//! screen itself (`slam-ui`) knows only indices and values.

use slam_audio::Backend;
use slam_engine::RATE_PRESETS;
use slam_osu::replay::Actions;
use slam_ui::i18n::{FluentArgs, Localizer};
use slam_ui::settings_screen::{RowKind, RowLabels, RowSpec, RowValue, SettingsLabels};

use super::keybinds;
use crate::settings::{DrawRate, Settings, Timing, skins};

/// Sections, in order; the keys of their names are in [`SECTION_KEYS`].
const SECTION_KEYS: [&str; 5] = [
    "settings-section-audio",
    "settings-section-offset",
    "settings-section-keys",
    "settings-section-frame",
    "settings-section-gameplay",
];
/// The number of sections.
pub const SECTIONS: usize = SECTION_KEYS.len();

/// The range of the hit error meter size slider (ADR-0050).
const HIT_ERROR_SCALE_RANGE: (f64, f64) = (0.5, 2.0);

const BUFFER_PRESETS: [u32; 6] = [64, 128, 256, 512, 1024, 2048];
const DRAW_PRESETS: [f64; 9] = [
    60.0, 120.0, 144.0, 240.0, 360.0, 1000.0, 2000.0, 4000.0, 8000.0,
];

/// A row of the screen; the screen's row index is the position in [`Row::ALL`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    MasterVolume,
    MusicVolume,
    HitsoundVolume,
    EffectsVolume,
    Backend,
    BufferFrames,
    GlobalOffset,
    MouseButtons,
    LeftKey1,
    LeftKey2,
    RightKey1,
    RightKey2,
    SmokeKey1,
    SmokeKey2,
    ResetKeys,
    UpdateHz,
    DrawHz,
    MenuDrawHz,
    ShowFps,
    Dim,
    BeatmapColours,
    BeatmapHitsounds,
    HitErrorScale,
    Skin,
}

impl Row {
    pub const ALL: [Row; 24] = [
        Row::MasterVolume,
        Row::MusicVolume,
        Row::HitsoundVolume,
        Row::EffectsVolume,
        Row::Backend,
        Row::BufferFrames,
        Row::GlobalOffset,
        Row::MouseButtons,
        Row::LeftKey1,
        Row::LeftKey2,
        Row::RightKey1,
        Row::RightKey2,
        Row::SmokeKey1,
        Row::SmokeKey2,
        Row::ResetKeys,
        Row::UpdateHz,
        Row::DrawHz,
        Row::MenuDrawHz,
        Row::ShowFps,
        Row::Dim,
        Row::BeatmapColours,
        Row::BeatmapHitsounds,
        Row::HitErrorScale,
        Row::Skin,
    ];

    /// The action and binding slot a key row edits.
    pub fn key_slot(self) -> Option<(Actions, usize)> {
        Some(match self {
            Row::LeftKey1 => (Actions::LEFT, 0),
            Row::LeftKey2 => (Actions::LEFT, 1),
            Row::RightKey1 => (Actions::RIGHT, 0),
            Row::RightKey2 => (Actions::RIGHT, 1),
            Row::SmokeKey1 => (Actions::SMOKE, 0),
            Row::SmokeKey2 => (Actions::SMOKE, 1),
            _ => return None,
        })
    }

    /// The screen's row index of the key row for `action`'s `slot`.
    pub fn key_row(action: Actions, slot: usize) -> Option<usize> {
        Row::ALL
            .iter()
            .position(|row| row.key_slot() == Some((action, slot)))
    }

    fn section(self) -> usize {
        match self {
            Row::MasterVolume
            | Row::MusicVolume
            | Row::HitsoundVolume
            | Row::EffectsVolume
            | Row::Backend
            | Row::BufferFrames => 0,
            Row::GlobalOffset => 1,
            Row::MouseButtons
            | Row::LeftKey1
            | Row::LeftKey2
            | Row::RightKey1
            | Row::RightKey2
            | Row::SmokeKey1
            | Row::SmokeKey2
            | Row::ResetKeys => 2,
            Row::UpdateHz | Row::DrawHz | Row::MenuDrawHz | Row::ShowFps => 3,
            Row::Dim
            | Row::BeatmapColours
            | Row::BeatmapHitsounds
            | Row::HitErrorScale
            | Row::Skin => 4,
        }
    }

    fn label_key(self) -> &'static str {
        match self {
            Row::MasterVolume => "settings-audio-master-volume",
            Row::MusicVolume => "settings-audio-music-volume",
            Row::HitsoundVolume => "settings-audio-hitsound-volume",
            Row::EffectsVolume => "settings-audio-effects-volume",
            Row::Backend => "settings-audio-backend",
            Row::BufferFrames => "settings-audio-buffer",
            Row::GlobalOffset => "settings-offset-global",
            Row::MouseButtons => "settings-keys-mouse-buttons",
            Row::LeftKey1 => "settings-keys-left-1",
            Row::LeftKey2 => "settings-keys-left-2",
            Row::RightKey1 => "settings-keys-right-1",
            Row::RightKey2 => "settings-keys-right-2",
            Row::SmokeKey1 => "settings-keys-smoke-1",
            Row::SmokeKey2 => "settings-keys-smoke-2",
            Row::ResetKeys => "settings-keys-reset",
            Row::UpdateHz => "settings-frame-update-rate",
            Row::DrawHz => "settings-frame-draw-rate",
            Row::MenuDrawHz => "settings-frame-menu-draw-rate",
            Row::ShowFps => "settings-frame-show-fps",
            Row::Dim => "settings-gameplay-dim",
            Row::BeatmapColours => "settings-gameplay-beatmap-colours",
            Row::BeatmapHitsounds => "settings-gameplay-beatmap-hitsounds",
            Row::HitErrorScale => "settings-gameplay-hit-error-scale",
            Row::Skin => "settings-gameplay-skin",
        }
    }

    /// Whether a change only takes effect after a restart: the output must be reopened
    /// for these. The others apply when the screen closes, the offset, mouse buttons and
    /// background from the next play (ADR-0048).
    pub fn restart_required(self) -> bool {
        matches!(self, Row::Backend | Row::BufferFrames)
    }
}

/// The options of the dropdown rows. Built once, from the settings the window starts with:
/// the options stay the same for the whole run, so that the draw side's texts match.
#[derive(Clone, Debug)]
pub struct Rows {
    /// `None` follows the first available backend.
    backends: Vec<Option<String>>,
    buffers: Vec<u32>,
    update_hz: Vec<f64>,
    draw: Vec<DrawRate>,
    /// The folder names in `Skins`; option `i + 1` of the skin row is `skins[i]`, option 0
    /// the default skin.
    skins: Vec<String>,
}

/// `presets` with `current` in its place if it is not one of them.
fn with_current<T: PartialOrd + Copy>(presets: &[T], current: T) -> Vec<T> {
    let mut list = presets.to_vec();
    if !list.contains(&current) {
        let at = list.iter().position(|p| *p > current).unwrap_or(list.len());
        list.insert(at, current);
    }
    list
}

impl Rows {
    pub fn new(settings: &Settings) -> Rows {
        let mut backends: Vec<Option<String>> = std::iter::once(None)
            .chain(
                Backend::available()
                    .iter()
                    .map(|b| Some(b.name().to_owned())),
            )
            .collect();
        if !backends.contains(&settings.audio.backend) {
            backends.push(settings.audio.backend.clone());
        }
        let mut draw = vec![DrawRate::Unlimited];
        let mut hz = DRAW_PRESETS.to_vec();
        for rate in [settings.frame.draw, settings.frame.menu_draw] {
            if let DrawRate::Hz(value) = rate
                && !hz.contains(&value)
            {
                hz = with_current(&hz, value);
            }
        }
        draw.extend(hz.into_iter().map(DrawRate::Hz));
        Rows {
            backends,
            buffers: with_current(&BUFFER_PRESETS, settings.audio.buffer_frames),
            update_hz: with_current(&RATE_PRESETS, settings.frame.update_hz),
            draw,
            skins: settings
                .paths
                .songs
                .as_deref()
                .map(skins::skin_dirs)
                .unwrap_or_default()
                .into_iter()
                .map(|(name, _)| name)
                .collect(),
        }
    }

    /// The rows for the screen, showing `settings`.
    pub fn specs(&self, settings: &Settings) -> Vec<RowSpec> {
        Row::ALL
            .iter()
            .map(|row| RowSpec {
                section: row.section(),
                kind: self.kind(*row, settings),
                restart_required: row.restart_required(),
            })
            .collect()
    }

    fn kind(&self, row: Row, s: &Settings) -> RowKind {
        let percent = |value: f64| RowKind::Slider {
            min: 0.0,
            max: 100.0,
            step: 1.0,
            value: (value * 100.0).round(),
        };
        let choice = |len: usize, at: Option<usize>| RowKind::Dropdown {
            options: len,
            selected: at.unwrap_or(0),
        };
        if let Some((action, slot)) = row.key_slot() {
            return RowKind::Key {
                value: keybinds::key_value(s.input.slot(action, slot)),
            };
        }
        match row {
            Row::ResetKeys => RowKind::Button,
            Row::MasterVolume => percent(s.audio.master_volume),
            Row::MusicVolume => percent(s.audio.music_volume),
            Row::HitsoundVolume => percent(s.audio.hitsound_volume),
            Row::EffectsVolume => percent(s.audio.effects_volume),
            Row::Dim => percent(s.gameplay.dim),
            Row::HitErrorScale => RowKind::Slider {
                min: HIT_ERROR_SCALE_RANGE.0,
                max: HIT_ERROR_SCALE_RANGE.1,
                step: 0.01,
                value: round_to(s.gameplay.hit_error_scale, 2),
            },
            Row::Backend => choice(
                self.backends.len(),
                self.backends.iter().position(|b| *b == s.audio.backend),
            ),
            Row::BufferFrames => choice(
                self.buffers.len(),
                self.buffers
                    .iter()
                    .position(|b| *b == s.audio.buffer_frames),
            ),
            Row::GlobalOffset => RowKind::Slider {
                min: *Timing::OFFSET_RANGE.start(),
                max: *Timing::OFFSET_RANGE.end(),
                step: 1.0,
                value: s.timing.global_offset_ms,
            },
            Row::MouseButtons => RowKind::Toggle {
                value: s.gameplay.mouse_buttons,
            },
            Row::UpdateHz => choice(
                self.update_hz.len(),
                self.update_hz.iter().position(|h| *h == s.frame.update_hz),
            ),
            Row::DrawHz => choice(
                self.draw.len(),
                self.draw.iter().position(|d| *d == s.frame.draw),
            ),
            Row::MenuDrawHz => choice(
                self.draw.len(),
                self.draw.iter().position(|d| *d == s.frame.menu_draw),
            ),
            Row::ShowFps => RowKind::Toggle {
                value: s.frame.show_fps,
            },
            Row::BeatmapColours => RowKind::Toggle {
                value: s.gameplay.beatmap_colours,
            },
            Row::BeatmapHitsounds => RowKind::Toggle {
                value: s.gameplay.beatmap_hitsounds,
            },
            // A saved skin that is no longer there shows as the default one, and stays
            // saved until the player picks another.
            Row::Skin => choice(
                self.skins.len() + 1,
                s.skin
                    .name
                    .as_ref()
                    .and_then(|name| self.skins.iter().position(|n| n == name))
                    .map(|i| i + 1)
                    .or(Some(0)),
            ),
            // Handled above.
            Row::LeftKey1
            | Row::LeftKey2
            | Row::RightKey1
            | Row::RightKey2
            | Row::SmokeKey1
            | Row::SmokeKey2 => RowKind::Button,
        }
    }

    /// Writes the new `value` of screen row `row` into `settings`. A row or value of the
    /// wrong kind changes nothing.
    pub fn apply(&self, settings: &mut Settings, row: usize, value: RowValue) {
        let Some(row) = Row::ALL.get(row) else {
            return;
        };
        let percent = |v: f64| round_to(v / 100.0, 2);
        match (row, value) {
            (Row::MasterVolume, RowValue::Slider(v)) => settings.audio.master_volume = percent(v),
            (Row::MusicVolume, RowValue::Slider(v)) => settings.audio.music_volume = percent(v),
            (Row::HitsoundVolume, RowValue::Slider(v)) => {
                settings.audio.hitsound_volume = percent(v);
            }
            (Row::EffectsVolume, RowValue::Slider(v)) => {
                settings.audio.effects_volume = percent(v);
            }
            (Row::Dim, RowValue::Slider(v)) => settings.gameplay.dim = percent(v),
            (Row::HitErrorScale, RowValue::Slider(v)) => {
                settings.gameplay.hit_error_scale =
                    round_to(v, 2).clamp(HIT_ERROR_SCALE_RANGE.0, HIT_ERROR_SCALE_RANGE.1);
            }
            (Row::GlobalOffset, RowValue::Slider(v)) => {
                settings.timing.global_offset_ms = v.round();
            }
            (Row::MouseButtons, RowValue::Toggle(v)) => settings.gameplay.mouse_buttons = v,
            (Row::ShowFps, RowValue::Toggle(v)) => settings.frame.show_fps = v,
            (Row::BeatmapColours, RowValue::Toggle(v)) => settings.gameplay.beatmap_colours = v,
            (Row::BeatmapHitsounds, RowValue::Toggle(v)) => settings.gameplay.beatmap_hitsounds = v,
            (Row::Skin, RowValue::Choice(i)) => match i {
                0 => settings.skin.name = None,
                _ => {
                    if let Some(name) = self.skins.get(i - 1) {
                        settings.skin.name = Some(name.clone());
                    }
                }
            },
            (Row::Backend, RowValue::Choice(i)) => {
                if let Some(backend) = self.backends.get(i) {
                    settings.audio.backend = backend.clone();
                }
            }
            (Row::BufferFrames, RowValue::Choice(i)) => {
                if let Some(frames) = self.buffers.get(i) {
                    settings.audio.buffer_frames = *frames;
                }
            }
            (Row::UpdateHz, RowValue::Choice(i)) => {
                if let Some(hz) = self.update_hz.get(i) {
                    settings.frame.update_hz = *hz;
                }
            }
            (Row::DrawHz, RowValue::Choice(i)) => {
                if let Some(rate) = self.draw.get(i) {
                    settings.frame.draw = *rate;
                }
            }
            (Row::MenuDrawHz, RowValue::Choice(i)) => {
                if let Some(rate) = self.draw.get(i) {
                    settings.frame.menu_draw = *rate;
                }
            }
            _ => {}
        }
    }

    /// The screen's texts in the language of `localizer`.
    pub fn labels(&self, localizer: &Localizer) -> SettingsLabels {
        let text = |key: &str| localizer.format(key, None).into_owned();
        let number = |key: &str, value: String| {
            let mut args = FluentArgs::new();
            args.set("value", value);
            localizer.format(key, Some(&args)).into_owned()
        };
        let rate = |rate: &DrawRate| match rate {
            DrawRate::Unlimited => text("settings-option-unlimited"),
            DrawRate::Hz(hz) => number("settings-option-hz", hz.to_string()),
        };
        let rows = Row::ALL
            .iter()
            .map(|row| {
                let options = match row {
                    Row::Backend => self
                        .backends
                        .iter()
                        .map(|b| match b {
                            Some(name) => name.clone(),
                            None => text("settings-option-default"),
                        })
                        .collect(),
                    Row::BufferFrames => self
                        .buffers
                        .iter()
                        .map(|n| number("settings-option-frames", n.to_string()))
                        .collect(),
                    Row::UpdateHz => self
                        .update_hz
                        .iter()
                        .map(|hz| number("settings-option-hz", hz.to_string()))
                        .collect(),
                    Row::DrawHz | Row::MenuDrawHz => self.draw.iter().map(rate).collect(),
                    Row::Skin => std::iter::once(text("settings-option-default-skin"))
                        .chain(self.skins.iter().cloned())
                        .collect(),
                    _ => Vec::new(),
                };
                let unit = match row {
                    Row::GlobalOffset => text("settings-unit-ms"),
                    Row::MasterVolume
                    | Row::MusicVolume
                    | Row::HitsoundVolume
                    | Row::EffectsVolume
                    | Row::Dim => text("settings-unit-percent"),
                    Row::HitErrorScale => text("settings-unit-times"),
                    _ => String::new(),
                };
                let button = match row {
                    Row::ResetKeys => text("settings-keys-reset-button"),
                    _ => String::new(),
                };
                RowLabels {
                    label: text(row.label_key()),
                    unit,
                    options,
                    button,
                }
            })
            .collect();
        SettingsLabels {
            title: text("settings-title"),
            sections: SECTION_KEYS.iter().map(|key| text(key)).collect(),
            rows,
            restart: text("settings-restart-marker"),
            keys: keybinds::key_labels(localizer),
        }
    }
}

fn round_to(value: f64, decimals: i32) -> f64 {
    let factor = 10_f64.powi(decimals);
    (value * factor).round() / factor
}

/// Rounds the values the sliders edit to their steps, so that the file holds `0.3` and not
/// `0.30000000000000004`: volumes and the dim to two decimals, the offset to whole
/// milliseconds.
pub fn round_for_save(settings: &mut Settings) {
    let audio = &mut settings.audio;
    for volume in [
        &mut audio.master_volume,
        &mut audio.music_volume,
        &mut audio.hitsound_volume,
        &mut audio.effects_volume,
        &mut settings.gameplay.dim,
    ] {
        *volume = round_to(*volume, 2);
    }
    settings.gameplay.hit_error_scale = round_to(settings.gameplay.hit_error_scale, 2);
    settings.timing.global_offset_ms = settings.timing.global_offset_ms.round();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> (Rows, Settings) {
        let settings = Settings::default();
        (Rows::new(&settings), settings)
    }

    fn index(row: Row) -> usize {
        Row::ALL.iter().position(|r| *r == row).unwrap()
    }

    #[test]
    fn specs_show_the_settings() {
        let mut settings = Settings::default();
        settings.audio.music_volume = 0.35;
        settings.timing.global_offset_ms = -12.0;
        settings.gameplay.mouse_buttons = false;
        let rows = Rows::new(&settings);
        let specs = rows.specs(&settings);
        assert_eq!(specs.len(), Row::ALL.len());
        assert!(matches!(
            specs[index(Row::MusicVolume)].kind,
            RowKind::Slider { value, max, .. } if value == 35.0 && max == 100.0
        ));
        assert!(matches!(
            specs[index(Row::GlobalOffset)].kind,
            RowKind::Slider { value, min, max, .. } if value == -12.0 && min == -500.0 && max == 500.0
        ));
        assert_eq!(
            specs[index(Row::MouseButtons)].kind,
            RowKind::Toggle { value: false }
        );
        assert!(specs.iter().all(|s| s.section < SECTIONS));
    }

    #[test]
    fn only_the_output_rows_wait_for_a_restart() {
        for row in Row::ALL {
            let output = matches!(row, Row::Backend | Row::BufferFrames);
            assert_eq!(row.restart_required(), output, "{row:?}");
        }
    }

    #[test]
    fn apply_maps_values_back_to_the_settings() {
        let (rows, mut s) = rows();
        rows.apply(&mut s, index(Row::MasterVolume), RowValue::Slider(30.0));
        rows.apply(&mut s, index(Row::Dim), RowValue::Slider(7.0));
        rows.apply(&mut s, index(Row::GlobalOffset), RowValue::Slider(-3.0));
        rows.apply(&mut s, index(Row::BeatmapColours), RowValue::Toggle(false));
        rows.apply(
            &mut s,
            index(Row::BeatmapHitsounds),
            RowValue::Toggle(false),
        );
        rows.apply(
            &mut s,
            index(Row::HitErrorScale),
            RowValue::Slider(1.2300000000000002),
        );
        assert_eq!(s.gameplay.hit_error_scale, 1.23);
        rows.apply(&mut s, index(Row::HitErrorScale), RowValue::Slider(9.0));
        assert_eq!(s.gameplay.hit_error_scale, 2.0);
        // 0.3 and 0.07 exactly as typed, not 0.30000000000000004.
        assert_eq!(s.audio.master_volume, 0.3);
        assert_eq!(s.gameplay.dim, 0.07);
        assert_eq!(s.timing.global_offset_ms, -3.0);
        assert!(!s.gameplay.beatmap_colours);
        assert!(!s.gameplay.beatmap_hitsounds);
        rows.apply(&mut s, index(Row::ShowFps), RowValue::Toggle(true));
        assert!(s.frame.show_fps);
        // The first draw option is Unlimited, the second 60 Hz.
        rows.apply(&mut s, index(Row::MenuDrawHz), RowValue::Choice(0));
        rows.apply(&mut s, index(Row::DrawHz), RowValue::Choice(1));
        assert_eq!(s.frame.menu_draw, DrawRate::Unlimited);
        assert_eq!(s.frame.draw, DrawRate::Hz(60.0));
        rows.apply(&mut s, index(Row::BufferFrames), RowValue::Choice(0));
        assert_eq!(s.audio.buffer_frames, 64);
        rows.apply(&mut s, index(Row::UpdateHz), RowValue::Choice(1));
        assert_eq!(s.frame.update_hz, 2000.0);
        rows.apply(&mut s, index(Row::Backend), RowValue::Choice(0));
        assert_eq!(s.audio.backend, None);
    }

    #[test]
    fn apply_ignores_wrong_kinds_and_out_of_range_indices() {
        let (rows, mut s) = rows();
        let before = s.clone();
        rows.apply(&mut s, index(Row::MasterVolume), RowValue::Toggle(true));
        rows.apply(&mut s, index(Row::DrawHz), RowValue::Choice(999));
        rows.apply(&mut s, 999, RowValue::Slider(1.0));
        assert_eq!(s, before);
    }

    #[test]
    fn a_selection_round_trips_through_the_options() {
        let (rows, s) = rows();
        for row in [
            Row::Backend,
            Row::BufferFrames,
            Row::UpdateHz,
            Row::DrawHz,
            Row::MenuDrawHz,
        ] {
            let RowKind::Dropdown { options, selected } = rows.specs(&s)[index(row)].kind else {
                panic!("{row:?} is a dropdown");
            };
            let mut applied = Settings::default();
            // Another value first, so that applying the selected one is a change back.
            applied.audio.buffer_frames = 1;
            applied.frame.update_hz = 1.0;
            applied.frame.draw = DrawRate::Hz(1.0);
            applied.frame.menu_draw = DrawRate::Hz(1.0);
            applied.audio.backend = Some("other".into());
            rows.apply(&mut applied, index(row), RowValue::Choice(selected));
            assert!(selected < options);
            let again = rows.specs(&applied)[index(row)].kind;
            assert_eq!(again, RowKind::Dropdown { options, selected }, "{row:?}");
        }
    }

    #[test]
    fn a_value_that_is_not_a_preset_is_offered_in_order() {
        let mut s = Settings::default();
        s.audio.buffer_frames = 300;
        s.frame.update_hz = 1500.0;
        s.frame.draw = DrawRate::Hz(100.0);
        s.audio.backend = Some("jack".into());
        let rows = Rows::new(&s);
        assert_eq!(rows.buffers, [64, 128, 256, 300, 512, 1024, 2048]);
        assert_eq!(rows.update_hz, [1000.0, 1500.0, 2000.0, 4000.0, 8000.0]);
        assert_eq!(rows.draw[0], DrawRate::Unlimited);
        assert_eq!(rows.draw[2], DrawRate::Hz(100.0));
        assert_eq!(rows.backends.last(), Some(&Some("jack".to_owned())));
        let specs = rows.specs(&s);
        assert!(matches!(
            specs[index(Row::BufferFrames)].kind,
            RowKind::Dropdown { selected: 3, .. }
        ));
    }

    fn with_skins(names: &[&str], saved: Option<&str>) -> (Rows, Settings) {
        let mut s = Settings::default();
        s.skin.name = saved.map(str::to_owned);
        let mut rows = Rows::new(&s);
        rows.skins = names.iter().map(|n| (*n).to_owned()).collect();
        (rows, s)
    }

    fn skin_selected(rows: &Rows, s: &Settings) -> (usize, usize) {
        match rows.specs(s)[index(Row::Skin)].kind {
            RowKind::Dropdown { options, selected } => (options, selected),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_skin_row_offers_the_default_and_the_folders() {
        let (rows, mut s) = with_skins(&["Alpha", "beta"], None);
        assert_eq!(skin_selected(&rows, &s), (3, 0));
        assert_eq!(Row::Skin.section(), 4);
        assert!(!Row::Skin.restart_required());
        rows.apply(&mut s, index(Row::Skin), RowValue::Choice(2));
        assert_eq!(s.skin.name.as_deref(), Some("beta"));
        assert_eq!(skin_selected(&rows, &s), (3, 2));
        rows.apply(&mut s, index(Row::Skin), RowValue::Choice(99));
        assert_eq!(s.skin.name.as_deref(), Some("beta"));
        rows.apply(&mut s, index(Row::Skin), RowValue::Choice(0));
        assert_eq!(s.skin.name, None);
        // Case matters: the folder name is matched as saved.
        s.skin.name = Some("ALPHA".into());
        assert_eq!(skin_selected(&rows, &s), (3, 0));
    }

    #[test]
    fn a_missing_skin_shows_as_default_and_stays_saved() {
        let (rows, s) = with_skins(&["Alpha"], Some("Gone"));
        assert_eq!(skin_selected(&rows, &s), (2, 0));
        assert_eq!(s.skin.name.as_deref(), Some("Gone"));
        // No songs folder: only the default skin.
        let (rows, s) = with_skins(&[], Some("Gone"));
        assert_eq!(skin_selected(&rows, &s), (1, 0));
    }

    #[test]
    fn the_skin_options_are_labelled() {
        let (rows, _) = with_skins(&["Alpha"], None);
        let mut localizer = Localizer::builtin().unwrap();
        localizer.set_language("en").unwrap();
        let labels = rows.labels(&localizer);
        assert_eq!(labels.rows[index(Row::Skin)].label, "Skin");
        assert_eq!(labels.rows[index(Row::Skin)].options, ["Default", "Alpha"]);
        localizer.set_language("ru").unwrap();
        let labels = rows.labels(&localizer);
        assert_eq!(labels.rows[index(Row::Skin)].label, "Скин");
        assert_eq!(labels.rows[index(Row::Skin)].options[0], "Стандартный");
    }

    #[test]
    fn saving_rounds_to_the_slider_steps() {
        let mut s = Settings::default();
        s.audio.master_volume = 0.1 + 0.2;
        s.audio.music_volume = 0.555;
        s.gameplay.dim = 0.30000000000000004;
        s.gameplay.hit_error_scale = 1.12 + 0.005;
        s.timing.global_offset_ms = 12.4;
        round_for_save(&mut s);
        assert_eq!(s.audio.master_volume, 0.3);
        assert_eq!(s.audio.music_volume, 0.56);
        assert_eq!(s.gameplay.dim, 0.3);
        assert_eq!(s.gameplay.hit_error_scale, 1.13);
        assert_eq!(s.timing.global_offset_ms, 12.0);
    }

    #[test]
    fn labels_cover_every_row_and_option_in_both_languages() {
        let (rows, s) = rows();
        let specs = rows.specs(&s);
        let mut localizer = Localizer::builtin().unwrap();
        for language in ["en", "ru"] {
            localizer.set_language(language).unwrap();
            let labels = rows.labels(&localizer);
            assert_eq!(labels.sections.len(), SECTIONS);
            assert_eq!(labels.rows.len(), specs.len());
            let texts = labels
                .sections
                .iter()
                .chain([&labels.title, &labels.restart])
                .chain(
                    labels
                        .rows
                        .iter()
                        .flat_map(|r| [&r.label].into_iter().chain(&r.options)),
                );
            for text in texts {
                // An unknown key formats as itself.
                assert!(!text.starts_with("settings-"), "{language}: {text}");
                assert!(!text.is_empty());
            }
            for (spec, row) in specs.iter().zip(&labels.rows) {
                if let RowKind::Dropdown { options, .. } = spec.kind {
                    assert_eq!(row.options.len(), options);
                }
            }
        }
        localizer.set_language("en").unwrap();
        let labels = rows.labels(&localizer);
        let draw = &labels.rows[index(Row::DrawHz)].options;
        assert_eq!(draw[0], "Unlimited");
        assert!(draw.contains(&"240 Hz".to_owned()), "{draw:?}");
        assert_eq!(labels.rows[index(Row::GlobalOffset)].unit, " ms");
        assert_eq!(labels.rows[index(Row::HitErrorScale)].unit, "×");
        localizer.set_language("ru").unwrap();
        let labels = rows.labels(&localizer);
        let draw = &labels.rows[index(Row::DrawHz)].options;
        assert_eq!(draw[0], "Без ограничения");
        assert!(draw.contains(&"240 Гц".to_owned()), "{draw:?}");
    }
}
