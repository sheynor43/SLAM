//! The settings model and its defaults.

use std::path::PathBuf;

use serde::{Serialize, Serializer};
use slam_audio::StreamConfig;
use slam_engine::EngineConfig;
use slam_engine::limiter::{InvalidRate, LimiterMode, MAX_HZ, MIN_HZ};
use slam_input::{
    DEFAULT_POSITION, DEFAULT_WINDOWED_SIZE, MAX_DISPLAY, MAX_SIDE, MIN_WINDOWED_SIZE, MouseButton,
    POSITION_RANGE, VideoDriver, WindowMode, WindowState,
};
use slam_osu::replay::Actions;
use slam_play::input::{Binding, Bindings, Input, MAX_BINDINGS};
use slam_play::{BackgroundSettings, FpsCorner};
use slam_ui::cursor::CursorStyle;
use slam_ui::screen::TransitionStyle;
use slam_ui::theme::ColourScheme;

use super::keys;
use crate::backend::Renderer;

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

/// Interface settings (ADR-0088).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct General {
    /// Tag of a built-in interface language; `None` is English. Applied at start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Whether the startup intro plays (ADR-0114). Read at start.
    pub intro: bool,
    /// Whether the interface effects (glow, blur, motion) are on (ADR-0123). Applies live.
    pub effects: bool,
    /// How screens change (ADR-0126). Applies live.
    #[serde(serialize_with = "serialize_transitions")]
    pub transitions: TransitionStyle,
    /// The interface colour scheme (ADR-0117).
    #[serde(serialize_with = "serialize_colour_scheme")]
    pub colour_scheme: ColourScheme,
    /// Scale of the menus, in `slam_ui::theme::UI_SCALE_RANGE` in steps of 0.05 (ADR-0118).
    #[serde(serialize_with = "serialize_scale")]
    pub menu_scale: f32,
    /// Scale of the HUD, in `slam_ui::theme::UI_SCALE_RANGE` in steps of
    /// 0.05 (ADR-0118).
    #[serde(serialize_with = "serialize_scale")]
    pub game_scale: f32,
    /// Skew of the buttons and rows in degrees, `0..=slam_ui::theme::MAX_SKEW_DEGREES` in
    /// whole degrees (ADR-0130). Applies live.
    #[serde(serialize_with = "serialize_skew")]
    pub skew: f32,
    /// What the main menu shows behind its content (ADR-0119).
    pub menu_background: MenuBackground,
    /// The cursor of the menus, the pause menu and the results (ADR-0124). Applies live.
    #[serde(serialize_with = "serialize_cursor_style")]
    pub menu_cursor: CursorStyle,
}

impl Default for General {
    fn default() -> Self {
        General {
            language: None,
            intro: true,
            effects: true,
            transitions: TransitionStyle::default(),
            colour_scheme: ColourScheme::default(),
            menu_scale: 1.0,
            game_scale: 1.0,
            skew: DEFAULT_SKEW_DEGREES,
            menu_background: MenuBackground::default(),
            menu_cursor: CursorStyle::Slam,
        }
    }
}

fn serialize_transitions<S: Serializer>(
    style: &TransitionStyle,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(style.name())
}

fn serialize_cursor_style<S: Serializer>(
    style: &CursorStyle,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(style.name())
}

fn serialize_colour_scheme<S: Serializer>(
    scheme: &ColourScheme,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(scheme.name())
}

/// Writes the skew as a whole number of degrees.
fn serialize_skew<S: Serializer>(skew: &f32, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64(f64::from(*skew).round())
}

/// The skew the buttons and rows have by default, in degrees (ADR-0130).
pub const DEFAULT_SKEW_DEGREES: f32 = 10.0;

/// Writes a scale as a two-decimal number, so that the file holds `1.05` and not the
/// `f32`'s widened `1.0499999523162842`.
fn serialize_scale<S: Serializer>(scale: &f32, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64((f64::from(*scale) * 100.0).round() / 100.0)
}

/// What the main menu shows behind its content (ADR-0119).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuBackground {
    /// The background of the beatmap whose music plays.
    #[default]
    Beatmap,
    /// The skin's menu background.
    Skin,
    /// A plain background.
    None,
}

impl MenuBackground {
    pub const ALL: [MenuBackground; 3] = [
        MenuBackground::Beatmap,
        MenuBackground::Skin,
        MenuBackground::None,
    ];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            MenuBackground::Beatmap => "beatmap",
            MenuBackground::Skin => "skin",
            MenuBackground::None => "none",
        }
    }

    /// The value called `name`.
    pub fn from_name(name: &str) -> Option<MenuBackground> {
        MenuBackground::ALL.into_iter().find(|b| b.name() == name)
    }
}

/// Where the menu sounds come from (ADR-0120).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuSounds {
    /// SLAM's own synthesized sounds.
    #[default]
    Slam,
    /// The skin's sounds.
    Skin,
    /// No menu sounds.
    Off,
}

impl MenuSounds {
    pub const ALL: [MenuSounds; 3] = [MenuSounds::Slam, MenuSounds::Skin, MenuSounds::Off];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            MenuSounds::Slam => "slam",
            MenuSounds::Skin => "skin",
            MenuSounds::Off => "off",
        }
    }

    /// The value called `name`.
    pub fn from_name(name: &str) -> Option<MenuSounds> {
        MenuSounds::ALL.into_iter().find(|s| s.name() == name)
    }
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
    /// Whether music plays on the main menu and the song select (ADR-0090).
    pub menu_music: bool,
    /// Volume of that music, 0.0..=1.0, on top of the master and music volumes.
    pub menu_music_volume: f64,
    /// Start delay of the gameplay hitsounds (ADR-0077).
    pub hitsound_delay: HitsoundDelay,
    /// How far hitsounds pan with the object's horizontal position, 0.0..=1.0 in steps of
    /// 0.01; 0 turns it off (lazer's `OsuSetting.PositionalHitsoundsLevel`, ADR-0087).
    pub positional_hitsounds_level: f32,
    /// Where the menu sounds come from (ADR-0120).
    pub menu_sounds: MenuSounds,
}

/// Extra delay of a stable hitsound over a SLAM one, click to sound, milliseconds
/// (ADR-0077). Estimated from published measurements, not measured (ADR-0079).
pub const STABLE_HITSOUND_DELAY_MS: f64 = 20.0;

/// When gameplay hitsounds start after the judgement (ADR-0077).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HitsoundDelay {
    /// From the next audio buffer.
    #[default]
    None,
    /// [`STABLE_HITSOUND_DELAY_MS`] later, as stable players are used to.
    Stable,
}

impl HitsoundDelay {
    pub const ALL: [HitsoundDelay; 2] = [HitsoundDelay::None, HitsoundDelay::Stable];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            HitsoundDelay::None => "none",
            HitsoundDelay::Stable => "stable",
        }
    }

    /// The value called `name`.
    pub fn from_name(name: &str) -> Option<HitsoundDelay> {
        HitsoundDelay::ALL.into_iter().find(|d| d.name() == name)
    }

    /// The delay for the mixer's hitsound group.
    pub fn ms(self) -> f64 {
        match self {
            HitsoundDelay::None => 0.0,
            HitsoundDelay::Stable => STABLE_HITSOUND_DELAY_MS,
        }
    }
}

/// How the key overlay's keys animate (ADR-0113).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyOverlayAnimation {
    /// As in stable.
    #[default]
    Stable,
    /// As in lazer.
    Lazer,
}

impl KeyOverlayAnimation {
    pub const ALL: [KeyOverlayAnimation; 2] =
        [KeyOverlayAnimation::Stable, KeyOverlayAnimation::Lazer];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            KeyOverlayAnimation::Stable => "stable",
            KeyOverlayAnimation::Lazer => "lazer",
        }
    }

    /// The value called `name`.
    pub fn from_name(name: &str) -> Option<KeyOverlayAnimation> {
        KeyOverlayAnimation::ALL
            .into_iter()
            .find(|a| a.name() == name)
    }
}

impl From<KeyOverlayAnimation> for slam_play::key_overlay::KeyAnimation {
    fn from(animation: KeyOverlayAnimation) -> Self {
        match animation {
            KeyOverlayAnimation::Stable => slam_play::key_overlay::KeyAnimation::Stable,
            KeyOverlayAnimation::Lazer => slam_play::key_overlay::KeyAnimation::Lazer,
        }
    }
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
            menu_music: true,
            menu_music_volume: 1.0,
            hitsound_delay: HitsoundDelay::default(),
            positional_hitsounds_level: slam_play::gameplay_sounds::DEFAULT_POSITIONAL_LEVEL,
            menu_sounds: MenuSounds::default(),
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

/// The window's mode, display, sizes (ADR-0070), windowed position and focus behaviour
/// (ADR-0073), as [`WindowState`] and the rest of [`slam_input::WindowSettings`].
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
    /// Place of the windowed mode: fractions of the display's free space, each in
    /// [`Window::POSITION_RANGE`], as lazer's `WindowedPositionX/Y`.
    #[serde(serialize_with = "serialize_position")]
    pub windowed_position: [f32; 2],
    /// Minimise on focus loss in fullscreen, as lazer's `MinimiseOnFocusLossInFullscreen`.
    pub minimise_on_focus_loss: bool,
    /// The video driver SDL uses on Linux; read once at startup, so a change applies after
    /// a restart (ADR-0097).
    #[serde(serialize_with = "serialize_video_driver")]
    pub video_driver: VideoDriver,
    /// The renderer on Windows; read when the window is made, so a change applies after a
    /// restart. Ignored elsewhere, where OpenGL runs (ADR-0104).
    #[serde(serialize_with = "serialize_renderer")]
    pub renderer: RendererChoice,
}

impl Window {
    /// Smallest fullscreen size, as lazer's `SizeFullscreen` minimum.
    pub const MIN_FULLSCREEN_SIZE: [u16; 2] = [320, 240];
    pub const MIN_WINDOWED_SIZE: [u16; 2] = MIN_WINDOWED_SIZE;
    /// Largest side of either size the window can be asked for.
    pub const MAX_SIDE: u16 = MAX_SIDE;
    pub const MAX_DISPLAY: u8 = MAX_DISPLAY;
    pub const POSITION_RANGE: std::ops::RangeInclusive<f32> = POSITION_RANGE;

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
            windowed_position: DEFAULT_POSITION,
            minimise_on_focus_loss: true,
            video_driver: VideoDriver::Auto,
            renderer: RendererChoice::OpenGl,
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

/// The renderer the settings offer (ADR-0104); there is no "auto", it would be OpenGL.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RendererChoice {
    #[default]
    OpenGl,
    D3d11,
}

impl RendererChoice {
    pub const ALL: [RendererChoice; 2] = [RendererChoice::OpenGl, RendererChoice::D3d11];

    /// The name in the settings file.
    pub fn as_str(self) -> &'static str {
        match self {
            RendererChoice::OpenGl => "opengl",
            RendererChoice::D3d11 => "d3d11",
        }
    }

    /// The value called `name`.
    pub fn from_name(name: &str) -> Option<RendererChoice> {
        RendererChoice::ALL.into_iter().find(|r| r.as_str() == name)
    }

    /// The backend to ask for: each falls back to the other when it cannot start
    /// (ADR-0104, ADR-0105).
    pub fn to_backend(self) -> Renderer {
        match self {
            RendererChoice::OpenGl => Renderer::Auto,
            RendererChoice::D3d11 => Renderer::PreferD3d11,
        }
    }
}

fn serialize_renderer<S: Serializer>(
    renderer: &RendererChoice,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(renderer.as_str())
}

fn serialize_video_driver<S: Serializer>(
    driver: &VideoDriver,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(driver.as_str())
}

fn serialize_window_mode<S: Serializer>(
    mode: &WindowMode,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(window_mode_name(*mode))
}

/// Each fraction as its shortest `f32` text, so the file reads `0.3`, not
/// `0.30000001192092896`.
fn serialize_position<S: Serializer>(
    position: &[f32; 2],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let decimal = |v: f32| v.to_string().parse::<f64>().unwrap_or(f64::from(v));
    position.map(decimal).serialize(serializer)
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

/// HUD options (ADR-0109).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HudSettings {
    /// Id of the active HUD layout: `builtin:*` or a user layout's folder name.
    pub layout: String,
}

impl Default for HudSettings {
    fn default() -> HudSettings {
        HudSettings {
            layout: "builtin:stable".to_owned(),
        }
    }
}

/// Song select options.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SongSelect {
    /// Dim of the song select's background, 0 ..= 1 (ADR-0098).
    pub background_dim: f64,
    /// Height of the beatmap thumbnails in the song list (ADR-0127).
    pub thumbnails: Thumbnails,
}

impl Default for SongSelect {
    fn default() -> SongSelect {
        SongSelect {
            background_dim: 0.7,
            thumbnails: Thumbnails::default(),
        }
    }
}

/// The height of the song list's thumbnails in pixels, or none (ADR-0127).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Thumbnails {
    Off,
    H64,
    #[default]
    H128,
    H256,
}

impl Serialize for Thumbnails {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl Thumbnails {
    pub const ALL: [Thumbnails; 4] = [
        Thumbnails::Off,
        Thumbnails::H64,
        Thumbnails::H128,
        Thumbnails::H256,
    ];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            Thumbnails::Off => "off",
            Thumbnails::H64 => "64",
            Thumbnails::H128 => "128",
            Thumbnails::H256 => "256",
        }
    }

    /// The value called `name`.
    pub fn from_name(name: &str) -> Option<Thumbnails> {
        Thumbnails::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The thumbnails' height in pixels; `None` when off.
    pub fn height(self) -> Option<u32> {
        match self {
            Thumbnails::Off => None,
            Thumbnails::H64 => Some(64),
            Thumbnails::H128 => Some(128),
            Thumbnails::H256 => Some(256),
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
    /// Play the combo break sound on the first break of a play even when the break happens while
    /// the combo is already low (lazer's `OsuSetting.AlwaysPlayFirstComboBreak`, on by default).
    pub always_play_first_combo_break: bool,
    /// Block the Windows key while playing (lazer's `OsuSetting.GameplayDisableWinKey`, on by
    /// default; ADR-0094); on Linux, every shortcut of the desktop. Other platforms ignore it.
    pub disable_win_key: bool,
    pub mouse_buttons: bool,
    /// Sliders grow towards their end as they appear (lazer's `OsuRulesetSetting.SnakingInSliders`,
    /// on by default there; off in SLAM, ADR-0085).
    pub snaking_in: bool,
    /// Sliders shrink behind the ball (lazer's `OsuRulesetSetting.SnakingOutSliders`, on by
    /// default there; off in SLAM, ADR-0085).
    pub snaking_out: bool,
    /// Hit lighting on hit circles and slider ends (lazer's `OsuSetting.HitLighting`).
    pub hit_lighting: bool,
    /// Shows the key overlay during play (lazer's `OsuSetting.KeyOverlay`, ADR-0113).
    pub key_overlay: bool,
    /// How the key overlay's keys light up (ADR-0113).
    pub key_overlay_animation: KeyOverlayAnimation,
    /// Animates the HUD counters and the health bar. When off, the score and accuracy counters
    /// show new values at once instead of rolling, the combo counter shows its value without
    /// pops, and the health bar shows the health at once without easing, bulge and flash. The
    /// key overlay keeps its own animation setting.
    pub hud_animations: bool,
    /// The cursor leaves a trail (lazer's `OsuRulesetSetting.ShowCursorTrail`).
    pub cursor_trail: bool,
    /// Size of the gameplay cursor and its trail, 0.1 ..= 2.0 in steps of 0.01 (lazer's
    /// `OsuSetting.GameplayCursorSize`, ADR-0086).
    pub cursor_size: f64,
    /// Adjusts the cursor's size to the beatmap's circle size (lazer's
    /// `OsuSetting.AutoCursorSize`).
    pub auto_cursor_size: bool,
    /// The gameplay cursor (ADR-0124).
    #[serde(serialize_with = "serialize_cursor_style")]
    pub cursor_style: CursorStyle,
    /// The style of the pause and fail menus (ADR-0122).
    pub pause_menu_style: PauseMenuStyle,
    /// Scale of the pause and fail menus, in `slam_ui::theme::UI_SCALE_RANGE` in steps of 0.05
    /// (ADR-0122).
    #[serde(serialize_with = "serialize_scale")]
    pub pause_menu_scale: f32,
}

impl Default for Gameplay {
    fn default() -> Gameplay {
        Gameplay {
            dim: BackgroundSettings::default().dim_level,
            beatmap_skins: true,
            beatmap_colours: true,
            beatmap_hitsounds: true,
            always_play_first_combo_break: true,
            disable_win_key: true,
            mouse_buttons: true,
            snaking_in: false,
            snaking_out: false,
            hit_lighting: true,
            key_overlay: false,
            key_overlay_animation: KeyOverlayAnimation::default(),
            hud_animations: true,
            cursor_trail: true,
            cursor_size: 1.0,
            auto_cursor_size: false,
            cursor_style: CursorStyle::Skin,
            pause_menu_style: PauseMenuStyle::default(),
            pause_menu_scale: 1.0,
        }
    }
}

/// The style of the pause and fail menus (ADR-0122).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseMenuStyle {
    /// SLAM's own look.
    #[default]
    Slam,
    /// The menu of osu!lazer.
    Lazer,
    /// The images of the skin, laid out as in stable.
    Stable,
}

impl PauseMenuStyle {
    pub const ALL: [PauseMenuStyle; 3] = [
        PauseMenuStyle::Slam,
        PauseMenuStyle::Lazer,
        PauseMenuStyle::Stable,
    ];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            PauseMenuStyle::Slam => "slam",
            PauseMenuStyle::Lazer => "lazer",
            PauseMenuStyle::Stable => "stable",
        }
    }

    /// The value called `name`.
    pub fn from_name(name: &str) -> Option<PauseMenuStyle> {
        PauseMenuStyle::ALL.into_iter().find(|s| s.name() == name)
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

/// Modifier keys held with a hotkey (ADR-0095); left and right are not told apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const CTRL: Mods = Mods(1);
    pub const SHIFT: Mods = Mods(2);
    pub const ALT: Mods = Mods(4);
    /// Every modifier, in the order they are written: Ctrl, Shift, Alt.
    pub const ALL: [Mods; 3] = [Mods::CTRL, Mods::SHIFT, Mods::ALT];

    pub const fn from_bits(bits: u8) -> Mods {
        Mods(bits & 7)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn with(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }
}

/// A hotkey: an input with the modifiers held for it (ADR-0095).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo {
    pub mods: Mods,
    pub input: Input,
}

impl Combo {
    /// `input` without modifiers.
    pub const fn plain(input: Input) -> Combo {
        Combo {
            mods: Mods::NONE,
            input,
        }
    }
}

/// O, lazer's key for `ToggleSettings` with Ctrl.
const KEY_O: u16 = 18;

/// Keys of the whole window, outside the gameplay bindings (ADR-0060, ADR-0095).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Hotkeys {
    /// Shows or hides the FPS counter (lazer's `ToggleFPSDisplay`); unbound by default, as
    /// lazer's.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_combo"
    )]
    pub toggle_fps: Option<Combo>,
    /// Opens or closes the settings panel (lazer's `ToggleSettings`, Ctrl+O). Always
    /// written, an empty name when unbound: left out it would come back as Ctrl+O.
    #[serde(serialize_with = "serialize_combo")]
    pub toggle_settings: Option<Combo>,
}

impl Default for Hotkeys {
    fn default() -> Hotkeys {
        Hotkeys {
            toggle_fps: None,
            toggle_settings: Some(Combo {
                mods: Mods::CTRL,
                input: Input::Key(KEY_O),
            }),
        }
    }
}

fn serialize_combo<S: Serializer>(combo: &Option<Combo>, serializer: S) -> Result<S::Ok, S::Error> {
    match combo {
        Some(combo) => serializer.serialize_str(&keys::combo_name(*combo)),
        None => serializer.serialize_str(""),
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
    ToggleSettings,
}

impl KeySlot {
    /// The hotkey slots.
    pub const HOTKEYS: [KeySlot; 2] = [KeySlot::ToggleFps, KeySlot::ToggleSettings];

    /// Whether it is a hotkey, which takes modifiers.
    pub fn is_hotkey(self) -> bool {
        !matches!(self, KeySlot::Play(..))
    }
}

/// All user settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Settings {
    pub general: General,
    pub paths: Paths,
    pub skin: SkinSettings,
    pub audio: Audio,
    pub timing: Timing,
    pub frame: Frame,
    pub window: Window,
    pub gameplay: Gameplay,
    pub hud: HudSettings,
    pub song_select: SongSelect,
    pub input: InputSettings,
    pub hotkeys: Hotkeys,
}

impl Settings {
    /// The gameplay key bindings.
    pub fn bindings(&self) -> Bindings {
        self.input.bindings
    }

    /// The input in `slot`, `None` if it is empty. Gameplay slots have no modifiers.
    pub fn key(&self, slot: KeySlot) -> Option<Combo> {
        match slot {
            KeySlot::Play(action, n) => self.input.slot(action, n).map(Combo::plain),
            KeySlot::ToggleFps => self.hotkeys.toggle_fps,
            KeySlot::ToggleSettings => self.hotkeys.toggle_settings,
        }
    }

    fn hotkey_mut(&mut self, slot: KeySlot) -> Option<&mut Option<Combo>> {
        match slot {
            KeySlot::Play(..) => None,
            KeySlot::ToggleFps => Some(&mut self.hotkeys.toggle_fps),
            KeySlot::ToggleSettings => Some(&mut self.hotkeys.toggle_settings),
        }
    }

    /// The hotkey `combo` triggers, if any.
    pub fn hotkey(&self, combo: Combo) -> Option<KeySlot> {
        KeySlot::HOTKEYS
            .into_iter()
            .find(|slot| self.key(*slot) == Some(combo))
    }

    /// Where else `combo` is bound when it goes into `slot`: one input does one thing,
    /// gameplay or hotkey. A gameplay slot past [`BINDING_SLOTS`] is still a conflict. A
    /// gameplay key only meets a hotkey without modifiers.
    pub fn key_conflict(&self, slot: KeySlot, combo: Combo) -> Option<KeySlot> {
        if let KeySlot::Play(action, n) = slot
            && let Some((other, other_slot)) = self.input.conflict(action, n, combo.input)
        {
            return Some(KeySlot::Play(other, other_slot));
        }
        let hotkey = KeySlot::HOTKEYS
            .into_iter()
            .find(|other| *other != slot && self.key(*other) == Some(combo));
        if hotkey.is_some() || !slot.is_hotkey() || !combo.mods.is_empty() {
            return hotkey;
        }
        let list = self.input.bindings.as_slice();
        let i = list.iter().position(|b| b.input == combo.input)?;
        let action = list[i].action;
        let n = list[..i].iter().filter(|b| b.action == action).count();
        Some(KeySlot::Play(action, n))
    }

    /// Whether `combo` may go into `slot`. Gameplay slots take no modifiers. Not for a
    /// hotkey: the main mouse buttons (lazer's `AllowMainMouseButtons = false` for global
    /// bindings), and, without modifiers, the menus' keys and the gameplay screen's own keys
    /// (ADR-0060, ADR-0095). F11 belongs to the window.
    pub fn key_allowed(slot: KeySlot, combo: Combo) -> bool {
        let input = combo.input;
        if input == Input::Key(KEY_F11) {
            return false;
        }
        if !slot.is_hotkey() {
            return combo.mods.is_empty();
        }
        if matches!(input, Input::Mouse(MouseButton::Left | MouseButton::Right)) {
            return false;
        }
        !combo.mods.is_empty()
            || !(matches!(
                input,
                Input::Key(MENU_KEYS_FIRST..=MENU_KEYS_LAST)
                    | Input::Key(KEY_TAB | KEY_BACKSPACE | KEY_DELETE)
            ) || slam_play::hotkeys::is_gameplay_hotkey(input))
    }

    /// Puts `combo` into `slot` and takes it out of every other slot. Returns `false` and
    /// changes nothing when the gameplay bindings are full.
    pub fn bind_key(&mut self, slot: KeySlot, combo: Combo) -> bool {
        match slot {
            KeySlot::Play(action, n) => {
                if !self.input.bind(action, n, combo.input) {
                    return false;
                }
            }
            _ => {
                while let Some(KeySlot::Play(action, n)) = self.key_conflict(slot, combo) {
                    self.input.clear(action, n);
                }
            }
        }
        for other in KeySlot::HOTKEYS {
            if other != slot
                && self.key(other) == Some(combo)
                && let Some(hotkey) = self.hotkey_mut(other)
            {
                *hotkey = None;
            }
        }
        if let Some(hotkey) = self.hotkey_mut(slot) {
            *hotkey = Some(combo);
        }
        true
    }

    /// Empties `slot`.
    pub fn clear_key(&mut self, slot: KeySlot) {
        match slot {
            KeySlot::Play(action, n) => self.input.clear(action, n),
            _ => {
                if let Some(hotkey) = self.hotkey_mut(slot) {
                    *hotkey = None;
                }
            }
        }
    }

    /// The default gameplay bindings and hotkeys (lazer's).
    pub fn reset_keys(&mut self) {
        self.input.reset();
        self.hotkeys = Hotkeys::default();
    }
}
