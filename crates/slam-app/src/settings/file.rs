//! Reading and writing `slam.toml`.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use slam_engine::MAX_UPDATE_HZ;
use slam_engine::limiter::MIN_HZ;
use slam_input::VideoDriver;
use slam_play::input::{Bindings, MAX_BINDINGS};
use slam_ui::i18n::BUILTIN_LANGUAGES;
use slam_ui::theme::{ColourScheme, UI_SCALE_RANGE};
use toml::{Table, Value};

use super::keys;
use super::model::{
    BindingEntry, Combo, DrawRate, FpsScreens, HitsoundDelay, InputSettings, KeyOverlayAnimation,
    KeySlot, MenuBackground, MenuSounds, PauseMenuStyle, RendererChoice, Settings, Timing, Window,
    corner_from_name, parse_size, window_mode_from_name,
};

/// The schema version this build writes.
pub const CURRENT_VERSION: i64 = 1;

/// How loading went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadStatus {
    /// No file; defaults.
    Missing,
    /// Parsed without a complaint.
    Ok,
    /// Parsed, but some fields were invalid or unknown and fell back to defaults.
    Recovered,
    /// Not TOML at all; defaults. The file must not be overwritten.
    Broken,
}

/// The result of [`load`].
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub settings: Settings,
    /// One line per problem, naming the field.
    pub warnings: Vec<String>,
    pub status: LoadStatus,
    /// The retired `[gameplay] hit_error_scale`, for the one-time migration into a HUD
    /// layout. Never part of [`Settings`] and never written back.
    pub legacy_hit_error_scale: Option<f64>,
}

/// Upgrades a table from schema version `from_version` to [`CURRENT_VERSION`]. Version 1 is
/// the first, so there is nothing to do yet.
pub fn migrate(_table: &mut Table, _from_version: i64) {}

/// Reads settings from `path`; never fails.
pub fn load(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Loaded {
                settings: Settings::default(),
                warnings: Vec::new(),
                status: LoadStatus::Missing,
                legacy_hit_error_scale: None,
            };
        }
        Err(e) => return broken(format!("cannot read {}: {e}", path.display())),
    };
    match text.parse::<Table>() {
        Ok(table) => from_table(table),
        Err(e) => broken(format!("{} is not valid TOML: {e}", path.display())),
    }
}

fn broken(warning: String) -> Loaded {
    tracing::warn!("{warning}");
    Loaded {
        settings: Settings::default(),
        warnings: vec![warning],
        status: LoadStatus::Broken,
        legacy_hit_error_scale: None,
    }
}

/// Parses settings from TOML text; see [`load`] for the rules.
pub fn from_toml_str(text: &str) -> Loaded {
    match text.parse::<Table>() {
        Ok(table) => from_table(table),
        Err(e) => broken(format!("not valid TOML: {e}")),
    }
}

struct Ctx {
    warnings: Vec<String>,
}

impl Ctx {
    fn warn(&mut self, message: String) {
        tracing::warn!("settings: {message}");
        self.warnings.push(message);
    }

    /// Takes a sub-table; reports unknown keys left in it.
    fn section(&mut self, table: &mut Table, name: &str) -> Table {
        match table.remove(name) {
            None => Table::new(),
            Some(Value::Table(t)) => t,
            Some(other) => {
                self.warn(format!(
                    "{name}: expected a table, got {}",
                    other.type_str()
                ));
                Table::new()
            }
        }
    }

    fn unknown(&mut self, table: &Table, section: &str) {
        for key in table.keys() {
            let name = if section.is_empty() {
                key.clone()
            } else {
                format!("{section}.{key}")
            };
            self.warn(format!("{name}: unknown key, ignored"));
        }
    }

    /// Reads field `key` into `slot`; keeps `slot` and warns when invalid.
    fn field<T: DeserializeOwned>(
        &mut self,
        table: &mut Table,
        section: &str,
        key: &str,
        slot: &mut T,
        check: impl Fn(&T) -> Result<(), String>,
    ) {
        let Some(value) = table.remove(key) else {
            return;
        };
        let name = format!("{section}.{key}");
        match value.try_into::<T>() {
            Ok(v) => match check(&v) {
                Ok(()) => *slot = v,
                Err(reason) => self.warn(format!("{name}: {reason}, using the default")),
            },
            Err(e) => self.warn(format!("{name}: {}, using the default", e.message())),
        }
    }

    /// Like [`Ctx::field`] for an optional string or path.
    fn opt<T: DeserializeOwned>(
        &mut self,
        table: &mut Table,
        section: &str,
        key: &str,
        slot: &mut Option<T>,
    ) {
        let Some(value) = table.remove(key) else {
            return;
        };
        match value.try_into::<T>() {
            Ok(v) => *slot = Some(v),
            Err(e) => self.warn(format!(
                "{section}.{key}: {}, using the default",
                e.message()
            )),
        }
    }

    /// Reads a float field that may be written as an integer.
    fn float(
        &mut self,
        table: &mut Table,
        section: &str,
        key: &str,
        slot: &mut f64,
        range: std::ops::RangeInclusive<f64>,
    ) {
        if let Some(Value::Integer(i)) = table.get(key) {
            let f = Value::Float(*i as f64);
            table.insert(key.to_string(), f);
        }
        self.field(table, section, key, slot, |v: &f64| {
            if range.contains(v) {
                Ok(())
            } else {
                Err(format!(
                    "{v} is outside [{}, {}]",
                    range.start(),
                    range.end()
                ))
            }
        });
    }
}

fn from_table(mut table: Table) -> Loaded {
    let mut cx = Ctx {
        warnings: Vec::new(),
    };
    let mut settings = Settings::default();

    let mut version = 1;
    if let Some(value) = table.remove("version") {
        match value {
            Value::Integer(v) if v >= 1 => version = v,
            other => cx.warn(format!(
                "version: expected a positive integer, got {other}, assuming 1"
            )),
        }
    }
    if version > CURRENT_VERSION {
        cx.warn(format!(
            "version: {version} is newer than {CURRENT_VERSION}; reading what is understood"
        ));
    } else {
        migrate(&mut table, version);
    }

    let mut general = cx.section(&mut table, "general");
    let mut language: Option<String> = None;
    cx.opt(&mut general, "general", "language", &mut language);
    if let Some(tag) = language {
        if BUILTIN_LANGUAGES.iter().any(|l| l.langid == tag) {
            settings.general.language = Some(tag);
        } else {
            cx.warn(format!(
                "general.language: unknown language \"{tag}\", using English"
            ));
        }
    }
    cx.field(
        &mut general,
        "general",
        "intro",
        &mut settings.general.intro,
        |_| Ok(()),
    );
    cx.field(
        &mut general,
        "general",
        "effects",
        &mut settings.general.effects,
        |_| Ok(()),
    );
    let mut scheme: Option<String> = None;
    cx.opt(&mut general, "general", "colour_scheme", &mut scheme);
    if let Some(name) = scheme {
        match ColourScheme::from_name(&name) {
            Some(scheme) => settings.general.colour_scheme = scheme,
            None => cx.warn(format!(
                "general.colour_scheme: unknown value \"{name}\", using the default"
            )),
        }
    }
    for (key, slot) in [
        ("menu_scale", &mut settings.general.menu_scale),
        ("game_scale", &mut settings.general.game_scale),
    ] {
        let mut scale = f64::from(*slot);
        cx.float(
            &mut general,
            "general",
            key,
            &mut scale,
            f64::from(UI_SCALE_RANGE.0)..=f64::from(UI_SCALE_RANGE.1),
        );
        *slot = ((scale * 20.0).round() / 20.0) as f32;
    }
    let mut background: Option<String> = None;
    cx.opt(&mut general, "general", "menu_background", &mut background);
    if let Some(name) = background {
        match MenuBackground::from_name(&name) {
            Some(background) => settings.general.menu_background = background,
            None => cx.warn(format!(
                "general.menu_background: unknown value \"{name}\", using the default"
            )),
        }
    }
    cx.unknown(&general, "general");

    let mut paths = cx.section(&mut table, "paths");
    cx.opt(&mut paths, "paths", "songs", &mut settings.paths.songs);
    cx.unknown(&paths, "paths");

    let mut skin = cx.section(&mut table, "skin");
    cx.opt(&mut skin, "skin", "name", &mut settings.skin.name);
    // An empty name is the default skin.
    if settings.skin.name.as_deref() == Some("") {
        settings.skin.name = None;
    }
    cx.unknown(&skin, "skin");

    let mut audio = cx.section(&mut table, "audio");
    cx.opt(&mut audio, "audio", "backend", &mut settings.audio.backend);
    cx.opt(&mut audio, "audio", "device", &mut settings.audio.device);
    cx.field(
        &mut audio,
        "audio",
        "buffer_frames",
        &mut settings.audio.buffer_frames,
        |v: &u32| {
            if *v >= 1 {
                Ok(())
            } else {
                Err("must be at least 1".to_string())
            }
        },
    );
    cx.float(
        &mut audio,
        "audio",
        "master_volume",
        &mut settings.audio.master_volume,
        0.0..=1.0,
    );
    cx.float(
        &mut audio,
        "audio",
        "music_volume",
        &mut settings.audio.music_volume,
        0.0..=1.0,
    );
    cx.float(
        &mut audio,
        "audio",
        "hitsound_volume",
        &mut settings.audio.hitsound_volume,
        0.0..=1.0,
    );
    cx.float(
        &mut audio,
        "audio",
        "effects_volume",
        &mut settings.audio.effects_volume,
        0.0..=1.0,
    );
    cx.field(
        &mut audio,
        "audio",
        "menu_music",
        &mut settings.audio.menu_music,
        |_| Ok(()),
    );
    cx.float(
        &mut audio,
        "audio",
        "menu_music_volume",
        &mut settings.audio.menu_music_volume,
        0.0..=1.0,
    );
    let mut delay: Option<String> = None;
    cx.opt(&mut audio, "audio", "hitsound_delay", &mut delay);
    if let Some(name) = delay {
        match HitsoundDelay::from_name(&name) {
            Some(delay) => settings.audio.hitsound_delay = delay,
            None => cx.warn(format!(
                "audio.hitsound_delay: unknown value \"{name}\", using the default"
            )),
        }
    }
    let mut level = f64::from(settings.audio.positional_hitsounds_level);
    cx.float(
        &mut audio,
        "audio",
        "positional_hitsounds_level",
        &mut level,
        0.0..=1.0,
    );
    settings.audio.positional_hitsounds_level = ((level * 100.0).round() / 100.0) as f32;
    let mut sounds: Option<String> = None;
    cx.opt(&mut audio, "audio", "menu_sounds", &mut sounds);
    if let Some(name) = sounds {
        match MenuSounds::from_name(&name) {
            Some(sounds) => settings.audio.menu_sounds = sounds,
            None => cx.warn(format!(
                "audio.menu_sounds: unknown value \"{name}\", using the default"
            )),
        }
    }
    cx.unknown(&audio, "audio");

    let mut timing = cx.section(&mut table, "timing");
    cx.float(
        &mut timing,
        "timing",
        "global_offset_ms",
        &mut settings.timing.global_offset_ms,
        Timing::OFFSET_RANGE,
    );
    cx.unknown(&timing, "timing");

    let mut frame = cx.section(&mut table, "frame");
    cx.float(
        &mut frame,
        "frame",
        "update_hz",
        &mut settings.frame.update_hz,
        MIN_HZ..=MAX_UPDATE_HZ,
    );
    if let Some(value) = frame.remove("draw") {
        match DrawRate::from_value(&value) {
            Ok(rate) => settings.frame.draw = rate,
            Err(e) => cx.warn(format!("frame.draw: {e}, using the default")),
        }
    }
    if let Some(value) = frame.remove("menu_draw") {
        match DrawRate::from_value(&value) {
            Ok(rate) => settings.frame.menu_draw = rate,
            Err(e) => cx.warn(format!("frame.menu_draw: {e}, using the default")),
        }
    }
    cx.field(
        &mut frame,
        "frame",
        "show_fps",
        &mut settings.frame.show_fps,
        |_| Ok(()),
    );
    let mut corner: Option<String> = None;
    cx.opt(&mut frame, "frame", "fps_corner", &mut corner);
    if let Some(name) = corner {
        match corner_from_name(&name) {
            Some(corner) => settings.frame.fps_corner = corner,
            None => cx.warn(format!(
                "frame.fps_corner: unknown corner \"{name}\", using the default"
            )),
        }
    }
    let mut screens: Option<String> = None;
    cx.opt(&mut frame, "frame", "fps_screens", &mut screens);
    if let Some(name) = screens {
        match FpsScreens::from_name(&name) {
            Some(screens) => settings.frame.fps_screens = screens,
            None => cx.warn(format!(
                "frame.fps_screens: unknown value \"{name}\", using the default"
            )),
        }
    }
    cx.unknown(&frame, "frame");

    let mut gameplay = cx.section(&mut table, "gameplay");
    cx.float(
        &mut gameplay,
        "gameplay",
        "dim",
        &mut settings.gameplay.dim,
        0.0..=1.0,
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "beatmap_skins",
        &mut settings.gameplay.beatmap_skins,
        |_| Ok(()),
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "beatmap_colours",
        &mut settings.gameplay.beatmap_colours,
        |_| Ok(()),
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "beatmap_hitsounds",
        &mut settings.gameplay.beatmap_hitsounds,
        |_| Ok(()),
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "always_play_first_combo_break",
        &mut settings.gameplay.always_play_first_combo_break,
        |_| Ok(()),
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "disable_win_key",
        &mut settings.gameplay.disable_win_key,
        |_| Ok(()),
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "mouse_buttons",
        &mut settings.gameplay.mouse_buttons,
        |_| Ok(()),
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "snaking_in",
        &mut settings.gameplay.snaking_in,
        |_| Ok(()),
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "snaking_out",
        &mut settings.gameplay.snaking_out,
        |_| Ok(()),
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "hit_lighting",
        &mut settings.gameplay.hit_lighting,
        |_| Ok(()),
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "key_overlay",
        &mut settings.gameplay.key_overlay,
        |_| Ok(()),
    );
    let mut animation: Option<String> = None;
    cx.opt(
        &mut gameplay,
        "gameplay",
        "key_overlay_animation",
        &mut animation,
    );
    if let Some(name) = animation {
        match KeyOverlayAnimation::from_name(&name) {
            Some(animation) => settings.gameplay.key_overlay_animation = animation,
            None => cx.warn(format!(
                "gameplay.key_overlay_animation: unknown value \"{name}\", using the default"
            )),
        }
    }
    cx.field(
        &mut gameplay,
        "gameplay",
        "hud_animations",
        &mut settings.gameplay.hud_animations,
        |_| Ok(()),
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "cursor_trail",
        &mut settings.gameplay.cursor_trail,
        |_| Ok(()),
    );
    cx.float(
        &mut gameplay,
        "gameplay",
        "cursor_size",
        &mut settings.gameplay.cursor_size,
        0.1..=2.0,
    );
    settings.gameplay.cursor_size = (settings.gameplay.cursor_size * 100.0).round() / 100.0;
    cx.field(
        &mut gameplay,
        "gameplay",
        "auto_cursor_size",
        &mut settings.gameplay.auto_cursor_size,
        |_| Ok(()),
    );
    let mut style: Option<String> = None;
    cx.opt(&mut gameplay, "gameplay", "pause_menu_style", &mut style);
    if let Some(name) = style {
        match PauseMenuStyle::from_name(&name) {
            Some(style) => settings.gameplay.pause_menu_style = style,
            None => cx.warn(format!(
                "gameplay.pause_menu_style: unknown value \"{name}\", using the default"
            )),
        }
    }
    let mut scale = f64::from(settings.gameplay.pause_menu_scale);
    cx.float(
        &mut gameplay,
        "gameplay",
        "pause_menu_scale",
        &mut scale,
        f64::from(UI_SCALE_RANGE.0)..=f64::from(UI_SCALE_RANGE.1),
    );
    settings.gameplay.pause_menu_scale = ((scale * 20.0).round() / 20.0) as f32;
    // Retired in favour of the HUD layout (ADR-0109); read only to migrate it.
    let mut legacy_scale = f64::NAN;
    cx.float(
        &mut gameplay,
        "gameplay",
        "hit_error_scale",
        &mut legacy_scale,
        0.5..=2.0,
    );
    let legacy_hit_error_scale =
        (!legacy_scale.is_nan()).then(|| (legacy_scale * 100.0).round() / 100.0);
    cx.unknown(&gameplay, "gameplay");

    let mut hud = cx.section(&mut table, "hud");
    cx.field(
        &mut hud,
        "hud",
        "layout",
        &mut settings.hud.layout,
        |id: &String| match crate::hud_layouts::LayoutId::parse(id) {
            Some(_) => Ok(()),
            None => Err(format!("`{id}` is not a layout id")),
        },
    );
    cx.unknown(&hud, "hud");

    let mut song_select = cx.section(&mut table, "song_select");
    cx.float(
        &mut song_select,
        "song_select",
        "background_dim",
        &mut settings.song_select.background_dim,
        0.0..=1.0,
    );
    cx.unknown(&song_select, "song_select");

    read_window(&mut cx, &mut table, &mut settings.window);

    let mut input = cx.section(&mut table, "input");
    cx.field(
        &mut input,
        "input",
        "high_precision_mouse",
        &mut settings.input.high_precision_mouse,
        |_| Ok(()),
    );
    cx.float(
        &mut input,
        "input",
        "sensitivity",
        &mut settings.input.sensitivity,
        f64::NEG_INFINITY..=f64::INFINITY,
    );
    let sensitivity = settings.input.sensitivity;
    if !InputSettings::SENSITIVITY_RANGE.contains(&sensitivity) {
        cx.warn(format!(
            "input.sensitivity: {sensitivity} is outside [{}, {}], clamped",
            InputSettings::SENSITIVITY_RANGE.start(),
            InputSettings::SENSITIVITY_RANGE.end()
        ));
    }
    settings.input.sensitivity = InputSettings::clean_sensitivity(sensitivity);
    if let Some(value) = input.remove("bindings") {
        match value {
            Value::Array(items) => {
                let mut entries = Vec::new();
                let mut skipped = 0;
                for (i, item) in items.into_iter().enumerate() {
                    let name = format!("input.bindings[{i}]");
                    match item.try_into::<RawBinding>() {
                        Ok(raw) => {
                            let entry = BindingEntry {
                                input: raw.input,
                                action: raw.action,
                            };
                            if let Some(binding) = entry.to_binding() {
                                if Settings::key_allowed(
                                    KeySlot::Play(binding.action, 0),
                                    Combo::plain(binding.input),
                                ) {
                                    entries.push(binding);
                                } else {
                                    skipped += 1;
                                    cx.warn(format!(
                                        "{name}: {} is reserved for the window, entry skipped",
                                        entry.input
                                    ));
                                }
                            } else {
                                skipped += 1;
                                cx.warn(format!(
                                    "{name}: unknown input \"{}\" or action \"{}\", entry skipped",
                                    entry.input, entry.action
                                ));
                            }
                        }
                        Err(e) => {
                            skipped += 1;
                            cx.warn(format!("{name}: {}, entry skipped", e.message()));
                        }
                    }
                }
                if entries.is_empty() && skipped > 0 {
                    cx.warn("input.bindings: no valid entry, using the defaults".to_string());
                } else {
                    if entries.len() > MAX_BINDINGS {
                        cx.warn(format!(
                            "input.bindings: {} entries, only the first {MAX_BINDINGS} are used",
                            entries.len()
                        ));
                    }
                    settings.input.bindings = Bindings::new(&entries);
                }
            }
            other => cx.warn(format!(
                "input.bindings: expected an array, got {}, using the default",
                other.type_str()
            )),
        }
    }
    cx.unknown(&input, "input");

    let mut hotkeys = cx.section(&mut table, "hotkeys");
    for (name, slot) in [
        ("toggle_fps", KeySlot::ToggleFps),
        ("toggle_settings", KeySlot::ToggleSettings),
    ] {
        let mut value: Option<String> = None;
        cx.opt(&mut hotkeys, "hotkeys", name, &mut value);
        let Some(value) = value else { continue };
        let combo = if value.trim().is_empty() {
            None
        } else {
            match keys::combo_from_name(&value) {
                Some(combo) => Some(combo),
                None => {
                    cx.warn(format!(
                        "hotkeys.{name}: unknown input \"{value}\", left unbound"
                    ));
                    None
                }
            }
        };
        match slot {
            KeySlot::ToggleFps => settings.hotkeys.toggle_fps = combo,
            _ => settings.hotkeys.toggle_settings = combo,
        }
    }
    cx.unknown(&hotkeys, "hotkeys");
    // A hotkey on a gameplay or menu key, or on another hotkey's, would do both: the other
    // wins, as the settings screen cannot write it so.
    for (name, slot) in [
        ("toggle_fps", KeySlot::ToggleFps),
        ("toggle_settings", KeySlot::ToggleSettings),
    ] {
        let Some(combo) = settings.key(slot) else {
            continue;
        };
        if settings.key_conflict(slot, combo).is_some() || !Settings::key_allowed(slot, combo) {
            cx.warn(format!(
                "hotkeys.{name}: {} is a gameplay, menu or other hotkey's key, left unbound",
                keys::combo_name(combo)
            ));
            settings.clear_key(slot);
        }
    }

    cx.unknown(&table, "");

    let status = if cx.warnings.is_empty() {
        LoadStatus::Ok
    } else {
        LoadStatus::Recovered
    };
    Loaded {
        settings,
        warnings: cx.warnings,
        status,
        legacy_hit_error_scale,
    }
}

/// The `[window]` section (ADR-0070): each invalid value falls back to its default.
fn read_window(cx: &mut Ctx, table: &mut Table, window: &mut Window) {
    let mut section = cx.section(table, "window");
    let mut mode: Option<String> = None;
    cx.opt(&mut section, "window", "mode", &mut mode);
    if let Some(name) = mode {
        match window_mode_from_name(&name) {
            Some(mode) => window.mode = mode,
            None => cx.warn(format!(
                "window.mode: unknown mode \"{name}\", using the default"
            )),
        }
    }
    let mut size: Option<String> = None;
    cx.opt(&mut section, "window", "fullscreen_size", &mut size);
    if let Some(text) = size {
        match parse_size(&text) {
            _ if text == "native" => window.fullscreen_size = None,
            Some(size) if Window::size_fits(size, Window::MIN_FULLSCREEN_SIZE) => {
                window.fullscreen_size = Some(size);
            }
            _ => cx.warn(format!(
                "window.fullscreen_size: \"{text}\" is not \"native\" or a size from {} to {}, using the default",
                size_text(Window::MIN_FULLSCREEN_SIZE),
                size_text([Window::MAX_SIDE; 2])
            )),
        }
    }
    let mut size: Option<String> = None;
    cx.opt(&mut section, "window", "windowed_size", &mut size);
    if let Some(text) = size {
        match parse_size(&text) {
            Some(size) if Window::size_fits(size, Window::MIN_WINDOWED_SIZE) => {
                window.windowed_size = size;
            }
            _ => cx.warn(format!(
                "window.windowed_size: \"{text}\" is not a size from {} to {}, using the default",
                size_text(Window::MIN_WINDOWED_SIZE),
                size_text([Window::MAX_SIDE; 2])
            )),
        }
    }
    cx.field(
        &mut section,
        "window",
        "display",
        &mut window.display,
        |&d| {
            if d <= Window::MAX_DISPLAY {
                Ok(())
            } else {
                Err(format!("{d} is above {}", Window::MAX_DISPLAY))
            }
        },
    );
    read_position(cx, &mut section, &mut window.windowed_position);
    cx.field(
        &mut section,
        "window",
        "minimise_on_focus_loss",
        &mut window.minimise_on_focus_loss,
        |_| Ok(()),
    );
    let mut driver: Option<String> = None;
    cx.opt(&mut section, "window", "video_driver", &mut driver);
    if let Some(name) = driver {
        match VideoDriver::parse(&name) {
            Some(driver) => window.video_driver = driver,
            None => cx.warn(format!(
                "window.video_driver: unknown driver \"{name}\", using the default"
            )),
        }
    }
    let mut renderer: Option<String> = None;
    cx.opt(&mut section, "window", "renderer", &mut renderer);
    if let Some(name) = renderer {
        match RendererChoice::from_name(&name) {
            Some(renderer) => window.renderer = renderer,
            None => cx.warn(format!(
                "window.renderer: unknown renderer \"{name}\", using the default"
            )),
        }
    }
    cx.unknown(&section, "window");
}

/// `windowed_position = [x, y]`, numbers in [`Window::POSITION_RANGE`] (ADR-0073).
fn read_position(cx: &mut Ctx, section: &mut Table, slot: &mut [f32; 2]) {
    let Some(value) = section.remove("windowed_position") else {
        return;
    };
    let number = |v: &Value| match v {
        Value::Float(f) => Some(*f as f32),
        Value::Integer(i) => Some(*i as f32),
        _ => None,
    };
    let position = match &value {
        Value::Array(items) if items.len() == 2 => number(&items[0]).zip(number(&items[1])),
        _ => None,
    };
    match position {
        Some((x, y))
            if Window::POSITION_RANGE.contains(&x) && Window::POSITION_RANGE.contains(&y) =>
        {
            *slot = [x, y];
        }
        _ => cx.warn(format!(
            "window.windowed_position: {value} is not two numbers from {} to {}, using the default",
            Window::POSITION_RANGE.start(),
            Window::POSITION_RANGE.end()
        )),
    }
}

fn size_text([w, h]: [u16; 2]) -> String {
    format!("{w}x{h}")
}

#[derive(serde::Deserialize)]
struct RawBinding {
    input: String,
    action: String,
}

/// Serializes settings with the `version` key first.
pub fn to_toml_string(settings: &Settings) -> Result<String, toml::ser::Error> {
    #[derive(serde::Serialize)]
    struct File<'a> {
        version: i64,
        #[serde(flatten)]
        settings: &'a Settings,
    }
    toml::to_string(&File {
        version: CURRENT_VERSION,
        settings,
    })
}

/// Writes settings atomically: a temporary file in the same directory, flushed to disk,
/// then renamed over `path`.
pub fn save(path: &Path, settings: &Settings) -> io::Result<()> {
    let text = to_toml_string(settings).map_err(io::Error::other)?;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp: PathBuf = path.to_path_buf();
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    tmp.set_file_name(name);
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Loads settings; when the file is missing, writes the defaults. A broken or recovered
/// file is never overwritten.
pub fn load_or_create(path: &Path) -> Loaded {
    let loaded = load(path);
    if loaded.status == LoadStatus::Missing
        && let Err(e) = save(path, &loaded.settings)
    {
        tracing::warn!("settings: cannot write {}: {e}", path.display());
    }
    loaded
}
