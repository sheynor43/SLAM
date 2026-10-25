//! The settings model and its defaults.

use std::path::PathBuf;

use serde::{Serialize, Serializer};
use slam_audio::StreamConfig;
use slam_engine::EngineConfig;
use slam_engine::limiter::{InvalidRate, LimiterMode, MAX_HZ, MIN_HZ};
use slam_input::{
    DEFAULT_WINDOWED_SIZE, MAX_DISPLAY, MAX_SIDE, MIN_WINDOWED_SIZE, MouseButton, WindowMode,
    WindowState,
};
use slam_osu::replay::Actions;
use slam_play::input::{Binding, Bindings, Input, MAX_BINDINGS};
use slam_play::{BackgroundSettings, FpsCorner};

use super::keys;

/// The draw frame rate: unlimited or a fixed rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrawRate {
    Unlimited,
    Hz(f64),
}

impl DrawRate {
    /// The limiter mode for this rate.
    pub fn to_limiter(self) -> LimiterMode {
        match self {
            DrawRate::Unlimited => LimiterMode::Unlimited,
            DrawRate::Hz(hz) => LimiterMode::Hz(hz),
        }
    }

    /// Checks a fixed rate against the limiter's range.
    pub fn validate(self) -> Result<DrawRate, InvalidRate> {
        match self {
            DrawRate::Hz(hz) if !(MIN_HZ..=MAX_HZ).contains(&hz) => Err(InvalidRate),
            other => Ok(other),
        }
    }
}

impl From<LimiterMode> for DrawRate {
    fn from(mode: LimiterMode) -> DrawRate {
        match mode {
            LimiterMode::Unlimited => DrawRate::Unlimited,
            LimiterMode::Hz(hz) => DrawRate::Hz(hz),
        }
    }
}

impl Serialize for DrawRate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            DrawRate::Unlimited => serializer.serialize_str("unlimited"),
            DrawRate::Hz(hz) => serializer.serialize_f64(*hz),
        }
    }
}

impl DrawRate {
    /// Reads a TOML value: the string `unlimited` (any case) or a number.
    pub fn from_value(value: &toml::Value) -> Result<DrawRate, String> {
        let rate = match value {
            toml::Value::String(s) if s.eq_ignore_ascii_case("unlimited") => DrawRate::Unlimited,
            toml::Value::String(s) => {
                return Err(format!("expected \"unlimited\" or a number, got \"{s}\""));
            }
            toml::Value::Float(f) => DrawRate::Hz(*f),
            toml::Value::Integer(i) => DrawRate::Hz(*i as f64),
            other => {
                return Err(format!(
                    "expected \"unlimited\" or a number, got {}",
                    other.type_str()
                ));
            }
        };
        rate.validate().map_err(|e| format!("{e}"))
    }
}

/// Library locations.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Paths {
    /// The `Songs` directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub songs: Option<PathBuf>,
}

/// Skin choice (ADR-0051).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SkinSettings {
    /// Folder name inside `Skins`, next to the songs folder; `None` plays on the default
    /// skin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Audio output and volumes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Audio {
    /// Backend name; validated when applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Output device id; `None` follows the system default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub buffer_frames: u32,
    pub master_volume: f64,
    pub music_volume: f64,
    /// Gameplay hitsound volume, 0.0..=1.0, on top of the master volume.
    pub hitsound_volume: f64,
    /// Volume of the other sound effects, 0.0..=1.0, on top of the master volume.
    pub effects_volume: f64,
}

impl Default for Audio {
    fn default() -> Audio {
        Audio {
            backend: None,
            device: None,
            buffer_frames: StreamConfig::default().buffer_frames,
            master_volume: 1.0,
            music_volume: 1.0,
            hitsound_volume: 1.0,
            effects_volume: 1.0,
        }
    }
}

/// Timing offsets.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Timing {
    /// Global audio offset in milliseconds, in [`Timing::OFFSET_RANGE`].
    pub global_offset_ms: f64,
}

impl Timing {
    /// Allowed global offset, milliseconds.
    pub const OFFSET_RANGE: std::ops::RangeInclusive<f64> = -500.0..=500.0;
}

/// The window's mode, display and sizes (ADR-0070), as [`WindowState`] for the window.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Window {
    #[serde(serialize_with = "serialize_window_mode")]
    pub mode: WindowMode,
    /// Video mode size of the fullscreen mode in pixels; `None` is the display's own
    /// size ("native" in the file, lazer's 9999×9999).
    #[serde(serialize_with = "serialize_fullscreen_size")]
    pub fullscreen_size: Option<[u16; 2]>,
    /// Size of the windowed mode in points.
    #[serde(serialize_with = "serialize_size")]
    pub windowed_size: [u16; 2],
    /// Index in the OS display list; a missing display means the primary one.
    pub display: u8,
}

impl Window {
    /// Smallest fullscreen size, as lazer's `SizeFullscreen` minimum.
    pub const MIN_FULLSCREEN_SIZE: [u16; 2] = [320, 240];
    pub const MIN_WINDOWED_SIZE: [u16; 2] = MIN_WINDOWED_SIZE;
    /// Largest side of either size the window can be asked for.
    pub const MAX_SIDE: u16 = MAX_SIDE;
    pub const MAX_DISPLAY: u8 = MAX_DISPLAY;

    /// What the window applies.
    pub fn state(&self) -> WindowState {
        WindowState {
            mode: self.mode,
            display: self.display,
            fullscreen_size: self.fullscreen_size,
            windowed_size: self.windowed_size,
        }
    }

    /// Whether `size` fits between `min` and [`Window::MAX_SIDE`].
    pub fn size_fits(size: [u16; 2], min: [u16; 2]) -> bool {
        size[0] >= min[0] && size[1] >= min[1] && size.iter().all(|&side| side <= MAX_SIDE)
    }
}

impl Default for Window {
    fn default() -> Window {
        Window {
            mode: WindowMode::DEFAULT,
            fullscreen_size: None,
            windowed_size: DEFAULT_WINDOWED_SIZE,
            display: 0,
        }
    }
}

/// The name of `mode` in the settings file.
pub fn window_mode_name(mode: WindowMode) -> &'static str {
    match mode {
        WindowMode::Windowed => "windowed",
        WindowMode::Borderless => "borderless",
        WindowMode::Fullscreen => "fullscreen",
    }
}

pub fn window_mode_from_name(name: &str) -> Option<WindowMode> {
    [
        WindowMode::Windowed,
        WindowMode::Borderless,
        WindowMode::Fullscreen,
    ]
    .into_iter()
    .find(|&mode| window_mode_name(mode) == name)
}

/// Reads `"WIDTHxHEIGHT"`, as `"1920x1080"`.
pub fn parse_size(text: &str) -> Option<[u16; 2]> {
    let (w, h) = text.split_once('x')?;
    Some([w.parse().ok()?, h.parse().ok()?])
}

fn serialize_window_mode<S: Serializer>(
    mode: &WindowMode,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(window_mode_name(*mode))
}

fn serialize_size<S: Serializer>(size: &[u16; 2], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(&format_args!("{}x{}", size[0], size[1]))
}

fn serialize_fullscreen_size<S: Serializer>(
    size: &Option<[u16; 2]>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match size {
        Some(size) => serialize_size(size, serializer),
        None => serializer.serialize_str("native"),
    }
}

/// Update and draw rates.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Frame {
    pub update_hz: f64,
    /// Draw rate during gameplay.
    pub draw: DrawRate,
    /// Draw rate on every other screen (ADR-0047).
    pub menu_draw: DrawRate,
    /// Show the FPS counter (lazer's `ShowFpsDisplay`; ADR-0053).
    pub show_fps: bool,
    /// The window corner of the FPS counter (ADR-0060).
    #[serde(serialize_with = "serialize_corner")]
    pub fps_corner: FpsCorner,
    /// The screens the FPS counter shows on (ADR-0060).
    pub fps_screens: FpsScreens,
}

/// Where the FPS counter shows (ADR-0060); lazer shows it everywhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FpsScreens {
    #[default]
    Everywhere,
    Gameplay,
    Menus,
}

impl FpsScreens {
    pub const ALL: [FpsScreens; 3] = [
        FpsScreens::Everywhere,
        FpsScreens::Gameplay,
        FpsScreens::Menus,
    ];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            FpsScreens::Everywhere => "everywhere",
            FpsScreens::Gameplay => "gameplay",
            FpsScreens::Menus => "menus",
        }
    }

    /// The value called `name`.
    pub fn from_name(name: &str) -> Option<FpsScreens> {
        FpsScreens::ALL.into_iter().find(|s| s.name() == name)
    }

    /// Whether the counter shows while the play is (`true`) or is not on top.
    pub fn shows(self, gameplay: bool) -> bool {
        match self {
            FpsScreens::Everywhere => true,
            FpsScreens::Gameplay => gameplay,
            FpsScreens::Menus => !gameplay,
        }
    }
}

/// The name of a corner in the settings file.
pub fn corner_name(corner: FpsCorner) -> &'static str {
    match corner {
        FpsCorner::TopLeft => "top_left",
        FpsCorner::TopRight => "top_right",
        FpsCorner::BottomLeft => "bottom_left",
        FpsCorner::BottomRight => "bottom_right",
    }
}

/// The corner called `name`.
pub fn corner_from_name(name: &str) -> Option<FpsCorner> {
    FpsCorner::ALL.into_iter().find(|c| corner_name(*c) == name)
}

fn serialize_corner<S: Serializer>(corner: &FpsCorner, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(corner_name(*corner))
}

impl Default for Frame {
    fn default() -> Frame {
        let engine = EngineConfig::default();
        Frame {
            update_hz: engine.update_hz,
            draw: engine.draw.into(),
            menu_draw: engine.draw.into(),
            show_fps: false,
            fps_corner: FpsCorner::default(),
            fps_screens: FpsScreens::default(),
        }
    }
}

/// Gameplay options.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Gameplay {
    /// Background dim, 0 ..= 1.
    pub dim: f64,
    /// Beatmap skin textures and configuration (lazer's `BeatmapSkins`): skin elements from the
    /// beatmap's folder. Colours stay under `beatmap_colours`, samples under `beatmap_hitsounds`.
    pub beatmap_skins: bool,
    pub beatmap_colours: bool,
    /// Play the beatmap's own hitsound samples (lazer's `BeatmapHitsounds`).
    pub beatmap_hitsounds: bool,
    pub mouse_buttons: bool,
    /// Size multiplier of the hit error meter, 0.5 ..= 2.0 in steps of 0.1. Temporary,
    /// until the HUD layout editor takes it over (ADR-0050).
    pub hit_error_scale: f64,
}

impl Default for Gameplay {
    fn default() -> Gameplay {
        Gameplay {
            dim: BackgroundSettings::default().dim_level,
            beatmap_skins: true,
            beatmap_colours: true,
            beatmap_hitsounds: true,
            mouse_buttons: true,
            hit_error_scale: 1.0,
        }
    }
}

/// One key binding as written in the file.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BindingEntry {
    /// A key or mouse button name, see [`keys`].
    pub input: String,
    /// `left`, `right` or `smoke`.
    pub action: String,
}

impl BindingEntry {
    /// The entry for a binding.
    pub fn from_binding(binding: &Binding) -> Option<BindingEntry> {
        Some(BindingEntry {
            input: keys::input_name(binding.input),
            action: keys::action_name(binding.action)?.to_string(),
        })
    }

    /// The binding, `None` if the input or action name is unknown.
    pub fn to_binding(&self) -> Option<Binding> {
        Some(Binding {
            input: keys::input_from_name(&self.input)?,
            action: keys::action_from_name(&self.action)?,
        })
    }
}

/// Bindings each action shows on the settings screen; bindings past them stay but are hidden.
pub const BINDING_SLOTS: usize = 2;

/// Input settings. The bindings have a fixed capacity, so editing them allocates nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputSettings {
    pub bindings: Bindings,
    /// Relative ("high precision") mouse mode (ADR-0069). On by default on Windows only,
    /// as in lazer.
    pub high_precision_mouse: bool,
    /// The multiplier of relative mouse motion, [`InputSettings::SENSITIVITY_RANGE`] in
    /// steps of [`InputSettings::SENSITIVITY_STEP`].
    pub sensitivity: f64,
}

impl Default for InputSettings {
    fn default() -> InputSettings {
        InputSettings {
            bindings: Bindings::default(),
            high_precision_mouse: cfg!(target_os = "windows"),
            sensitivity: 1.0,
        }
    }
}

impl InputSettings {
    /// The sensitivities lazer's slider offers.
    pub const SENSITIVITY_RANGE: std::ops::RangeInclusive<f64> = 0.1..=10.0;
    /// Its precision (`BindableDouble.Precision`).
    pub const SENSITIVITY_STEP: f64 = 0.01;

    /// `value` clamped to [`InputSettings::SENSITIVITY_RANGE`] and rounded to its step.
    /// NaN gives the default, 1.
    pub fn clean_sensitivity(value: f64) -> f64 {
        if value.is_nan() {
            return 1.0;
        }
        let clamped = value.clamp(
            *Self::SENSITIVITY_RANGE.start(),
            *Self::SENSITIVITY_RANGE.end(),
        );
        ((clamped / Self::SENSITIVITY_STEP).round() * Self::SENSITIVITY_STEP).clamp(
            *Self::SENSITIVITY_RANGE.start(),
            *Self::SENSITIVITY_RANGE.end(),
        )
    }

    /// The index in the list of `action`'s binding number `slot`.
    fn slot_index(&self, action: Actions, slot: usize) -> Option<usize> {
        self.bindings
            .as_slice()
            .iter()
            .enumerate()
            .filter(|(_, b)| b.action == action)
            .nth(slot)
            .map(|(i, _)| i)
    }

    /// The input in `action`'s binding number `slot`, `None` if the slot is empty.
    pub fn slot(&self, action: Actions, slot: usize) -> Option<Input> {
        self.slot_index(action, slot)
            .map(|i| self.bindings.as_slice()[i].input)
    }

    /// Where else `input` is bound when it goes into `action`'s binding number `slot`: the
    /// action and its slot number of the first other binding of the same input.
    pub fn conflict(&self, action: Actions, slot: usize, input: Input) -> Option<(Actions, usize)> {
        let own = self.slot_index(action, slot);
        let list = self.bindings.as_slice();
        let (i, other) = list
            .iter()
            .enumerate()
            .find(|&(i, b)| b.input == input && Some(i) != own)?;
        let other_slot = list[..i]
            .iter()
            .filter(|b| b.action == other.action)
            .count();
        Some((other.action, other_slot))
    }

    /// Puts `input` into `action`'s binding number `slot` and removes every other binding of
    /// the same input. An empty slot takes the next free place of the action. Returns `false`
    /// and changes nothing when the list is full.
    pub fn bind(&mut self, action: Actions, slot: usize, input: Input) -> bool {
        let mut list = [Binding { input, action }; MAX_BINDINGS];
        let current = self.bindings.as_slice();
        list[..current.len()].copy_from_slice(current);
        let mut len = current.len();
        let target = match self.slot_index(action, slot) {
            Some(i) => i,
            None if len < MAX_BINDINGS => {
                len += 1;
                len - 1
            }
            None => return false,
        };
        list[target] = Binding { input, action };
        let mut kept = 0;
        for i in 0..len {
            if i == target || list[i].input != input {
                list[kept] = list[i];
                kept += 1;
            }
        }
        self.bindings = Bindings::new(&list[..kept]);
        true
    }

    /// Empties `action`'s binding number `slot`; later bindings of the action move up.
    pub fn clear(&mut self, action: Actions, slot: usize) {
        let Some(target) = self.slot_index(action, slot) else {
            return;
        };
        let current = self.bindings.as_slice();
        let mut list = [current[0]; MAX_BINDINGS];
        let mut kept = 0;
        for (i, binding) in current.iter().enumerate() {
            if i != target {
                list[kept] = *binding;
                kept += 1;
            }
        }
        self.bindings = Bindings::new(&list[..kept]);
    }

    /// Lazer's default bindings.
    pub fn reset(&mut self) {
        self.bindings = Bindings::default();
    }
}

impl Serialize for InputSettings {
    /// Writes `bindings` as a list of named entries, see [`BindingEntry`].
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        struct Entries<'a>(&'a [Binding]);
        impl Serialize for Entries<'_> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_seq(self.0.iter().filter_map(BindingEntry::from_binding))
            }
        }

        let mut state = serializer.serialize_struct("InputSettings", 3)?;
        // The array of tables goes last: TOML wants the plain values before it.
        state.serialize_field("high_precision_mouse", &self.high_precision_mouse)?;
        state.serialize_field("sensitivity", &self.sensitivity)?;
        state.serialize_field("bindings", &Entries(self.bindings.as_slice()))?;
        state.end()
    }
}

/// Keys of the whole window, outside the gameplay bindings (ADR-0060). Unbound by default,
/// as lazer's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Hotkeys {
    /// Shows or hides the FPS counter (lazer's `ToggleFPSDisplay`).
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_input"
    )]
    pub toggle_fps: Option<Input>,
}

fn serialize_input<S: Serializer>(input: &Option<Input>, serializer: S) -> Result<S::Ok, S::Error> {
    match input {
        Some(input) => serializer.serialize_str(&keys::input_name(*input)),
        None => serializer.serialize_none(),
    }
}

/// The arrows, which the menus move with (SDL scancodes, Right to Up).
const MENU_KEYS_FIRST: u16 = 79;
const MENU_KEYS_LAST: u16 = 82;
const KEY_BACKSPACE: u16 = 42;
const KEY_TAB: u16 = 43;
const KEY_DELETE: u16 = 76;
/// Cycles the window mode, alone or with Alt+Enter; the window takes it (ADR-0070).
const KEY_F11: u16 = 68;

/// A key the settings screen edits: a gameplay binding slot or a hotkey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySlot {
    /// `action`'s binding number `slot`.
    Play(Actions, usize),
    ToggleFps,
}

/// All user settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Settings {
    pub paths: Paths,
    pub skin: SkinSettings,
    pub audio: Audio,
    pub timing: Timing,
    pub frame: Frame,
    pub window: Window,
    pub gameplay: Gameplay,
    pub input: InputSettings,
    pub hotkeys: Hotkeys,
}

impl Settings {
    /// The gameplay key bindings.
    pub fn bindings(&self) -> Bindings {
        self.input.bindings
    }

    /// The input in `slot`, `None` if it is empty.
    pub fn key(&self, slot: KeySlot) -> Option<Input> {
        match slot {
            KeySlot::Play(action, n) => self.input.slot(action, n),
            KeySlot::ToggleFps => self.hotkeys.toggle_fps,
        }
    }

    /// Where else `input` is bound when it goes into `slot`: one input does one thing,
    /// gameplay or hotkey. A gameplay slot past [`BINDING_SLOTS`] is still a conflict.
    pub fn key_conflict(&self, slot: KeySlot, input: Input) -> Option<KeySlot> {
        match slot {
            KeySlot::Play(action, n) => {
                if let Some((other, other_slot)) = self.input.conflict(action, n, input) {
                    return Some(KeySlot::Play(other, other_slot));
                }
                (self.hotkeys.toggle_fps == Some(input)).then_some(KeySlot::ToggleFps)
            }
            KeySlot::ToggleFps => {
                let list = self.input.bindings.as_slice();
                let i = list.iter().position(|b| b.input == input)?;
                let action = list[i].action;
                let n = list[..i].iter().filter(|b| b.action == action).count();
                Some(KeySlot::Play(action, n))
            }
        }
    }

    /// Whether `input` may go into `slot`. Not for a hotkey: the main mouse buttons (lazer's
    /// `AllowMainMouseButtons = false` for global bindings), the menus' keys and the
    /// gameplay screen's own keys (ADR-0060).
    pub fn key_allowed(slot: KeySlot, input: Input) -> bool {
        if input == Input::Key(KEY_F11) {
            return false;
        }
        match slot {
            KeySlot::Play(..) => true,
            KeySlot::ToggleFps => {
                !matches!(
                    input,
                    Input::Mouse(MouseButton::Left | MouseButton::Right)
                        | Input::Key(MENU_KEYS_FIRST..=MENU_KEYS_LAST)
                        | Input::Key(KEY_TAB | KEY_BACKSPACE | KEY_DELETE)
                ) && !slam_play::hotkeys::is_gameplay_hotkey(input)
            }
        }
    }

    /// Puts `input` into `slot` and takes it out of every other slot. Returns `false` and
    /// changes nothing when the gameplay bindings are full.
    pub fn bind_key(&mut self, slot: KeySlot, input: Input) -> bool {
        match slot {
            KeySlot::Play(action, n) => {
                if !self.input.bind(action, n, input) {
                    return false;
                }
                if self.hotkeys.toggle_fps == Some(input) {
                    self.hotkeys.toggle_fps = None;
                }
            }
            KeySlot::ToggleFps => {
                while let Some(KeySlot::Play(action, n)) = self.key_conflict(slot, input) {
                    self.input.clear(action, n);
                }
                self.hotkeys.toggle_fps = Some(input);
            }
        }
        true
    }

    /// Empties `slot`.
    pub fn clear_key(&mut self, slot: KeySlot) {
        match slot {
            KeySlot::Play(action, n) => self.input.clear(action, n),
            KeySlot::ToggleFps => self.hotkeys.toggle_fps = None,
        }
    }

    /// The default gameplay bindings (lazer's) and no hotkeys.
    pub fn reset_keys(&mut self) {
        self.input.reset();
        self.hotkeys = Hotkeys::default();
    }
}
