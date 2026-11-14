//! The window with the screens (ADR-0044): the main menu, the song select and the play,
//! prepared on worker threads (ADR-0045). Opened by `slam-app` without a command. Console
//! output is not localised; UI text is.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use slam_audio::{
    AudioPosition, Backend, MixerConfig, MixerHandle, MusicConfig, MusicEvent, MusicPlayer,
    MusicPosition, Sample, StreamConfig, TrackSource, VoiceGroup, VoiceId, VoiceParams,
};
use slam_engine::clock::now_ns;
use slam_engine::{
    Draw, DrawRateControl, Engine, EngineConfig, EngineShared, FrameInfo, TickInfo, Update,
    UpdateRateControl,
};
use slam_input::{
    CursorVisibility, InputEvent, InputKind, InputWindow, MouseButton, MouseSettings, TextInput,
    TickMapper, WinKeyBlock, WindowScale, WindowSettings, WindowState, event_ring, sdl_ticks_ns,
};
use slam_osu::Difficulty;
use slam_osu::mods::{
    APPROACH_RATE_STEP, Mod, ModSet, ModType, SPEED_STEP, SliderBounds, SpeedHotkeys,
    change_approach_rate, mods_for,
};
use slam_osu::statistics::{adjusted_display_difficulty, round_bpm};
use slam_play::input::Input;
use slam_play::{
    Bindings, LiveInput, Offsets, PlaySession, PlaySnapshot, PlayState, PlayfieldLayout,
};
use slam_render::atlas::{Atlas, AtlasRegion};
use slam_render::blur::Blur;
use slam_render::shape::{SHAPE_LAYOUT, ShapeBatch};
use slam_render::sprite::{Affine, DEFAULT_SLOTS, Projection, SpriteBatch, SpriteRenderer};
use slam_render::textures::{TextureId, Textures};
use slam_render::upload::{TextureLoaderConfig, TextureState, TextureUploads, texture_loader};
use slam_render::{Color, Device, Extent, TextureFormat, shaders};
use slam_text::{FontRegistry, GlyphAtlasConfig, GlyphCache, glyph_atlas};
use slam_ui::cursor::CursorStyle;
use slam_ui::i18n::Localizer;
use slam_ui::intro::{Intro, IntroSnapshot, IntroView};
use slam_ui::menu::{MainMenu, MenuAction, MenuInput, MenuLayout, MenuSnapshot, MenuView};
use slam_ui::results::{
    Results, ResultsAction, ResultsData, ResultsInput, ResultsLayout, ResultsMods, ResultsSnapshot,
    ResultsView, StatKind,
};
use slam_ui::screen::{MAX_LAYERS, Nav, Navigated, Pose, ScreenStack, TransitionFx};
use slam_ui::settings_screen::panel::{
    PanelMotion, panel_layout, panel_pose, panel_width, push_backdrop,
};
use slam_ui::settings_screen::{
    KeyValue, RowWidget, SettingsAction, SettingsInput, SettingsLayout, SettingsScreen,
    SettingsSnapshot, SettingsView,
};
use slam_ui::song_select::{
    BeatmapDetails, DetailStats, ModColumns, ModSelect, SettingValue, SongFilter, SongList,
    SongSelect, SongSelectAction, SongSelectInput, SongSelectLayout, SongSelectSnapshot,
    SongSelectStatus, SongSelectView, TileDetail,
};
use slam_ui::song_select::{MAX_VISIBLE_ROWS, RowThumb, Thumbs};
use slam_ui::theme::{CursorFx, Theme};
use slam_ui::widgets::DropdownView;

use crate::backend::{self as render, Opened, WindowKind, WithBackend};
use crate::backgrounds::{
    BackgroundFade, BackgroundPicker, BackgroundSource, BackgroundTarget, MAX_BACKGROUNDS,
    SELECTION_DEBOUNCE_NS,
};
use crate::details::DetailsPicker;
use crate::displays::{ListsLink, ShownViews};
use crate::loader::{LoaderConfig, LoaderLink, PlayOptions, SceneTrash, Trash};
use crate::play::fps::{FpsDisplay, FpsOverlay, FpsSource, MAX_FPS_GLYPHS};
use crate::play::pack_size;
use crate::play::scene::{CHANNELS, PlayScene, PlaySettings, Prepared, SceneGpu};
use crate::replays::{ReplayBeatmap, ReplayJob};
use crate::settings::saver::SaveLink;
use crate::settings::{Combo, Dirs, InputSettings, KeySlot, MenuBackground, Mods, Settings};
use crate::songs::{FolderChange, SkinViews, SongFiles, SongsLink};

mod draw_rate;
mod intro_sound;
mod live;

use language::{LanguageSwap, LanguageViews};
use live::{AppliedUpdateRate, AppliedVolumes, Volumes};
use output::{DeviceList, DeviceViews, OutputLink, Request};
mod keybinds;
mod language;
mod menu_music;
mod mod_settings;
mod ui_sounds;
use menu_music::{MenuMusic, Picked, Want};
mod output;
mod results;
mod rows;

pub use draw_rate::DrawRateSwitch;
pub use rows::{Row, Rows, SECTIONS, round_for_save};

pub const USAGE: &str = "usage: slam-app [--songs DIR]\n\
                         slam-app [bench|click|click-analyze|play|i18n] ...\n\
                         without a command, opens the window with the main menu; --songs is \
                         the folder of beatmap sets (as stable's Songs) and overrides \
                         paths.songs of slam.toml";

/// Textures of the draw thread: the white square, the glyph pages and the song select's
/// backgrounds.
const MAX_TEXTURES: usize = 16
    + MAX_BACKGROUNDS as usize
    + crate::thumbnails::MAX_HANDLES as usize
    + crate::menu_cursor::MAX_CURSOR_TEXTURES as usize;
/// Rectangles of one frame: the buttons or rows of up to two layers (each a few layers deep,
/// ADR-0136), the FPS counter's
/// box, two backgrounds of each layer, the language curtain (ADR-0091) and the intro's
/// black (ADR-0114).
const MAX_SHAPES: usize = 1024
    + slam_ui::screen::MAX_FX_SHAPES
    + 1
    + 2 * MAX_LAYERS
    + slam_ui::curtain::MAX_SPRITES
    + 1
    + crate::menu_cursor::MAX_MENU_CURSOR_SHAPES;
/// Outlined glyphs of the startup intro's title (ADR-0114).
const MAX_INTRO_OUTLINES: usize = 64;
/// Glyph sprites of one frame: the texts of up to two layers and the FPS counter.
const MAX_GLYPH_SPRITES: usize = 8192 + MAX_FPS_GLYPHS;
/// How long the draw thread waits for the glyphs before the first frame.
const GLYPH_WARM_TIMEOUT: Duration = Duration::from_secs(1);
/// How long the window must stay still before a size, display or position it reported
/// is saved (ADR-0073).
const WINDOW_SAVE_DELAY_NS: u64 = 500_000_000;

// USB HID usage ids, as `SDL_Scancode`.
const KEY_ENTER: u16 = 40;
const KEY_ESCAPE: u16 = 41;
const KEY_R: u16 = 21;
const KEY_BACKSPACE: u16 = 42;
const KEY_F1: u16 = 58;
const KEY_F3: u16 = 60;
const KEY_F4: u16 = 61;
const KEY_TAB: u16 = 43;
const KEY_SPACE: u16 = 44;
const KEY_HOME: u16 = 74;
const KEY_DELETE: u16 = 76;
const KEY_PAGE_UP: u16 = 75;
const KEY_END: u16 = 77;
const KEY_PAGE_DOWN: u16 = 78;
const KEY_RIGHT: u16 = 79;
const KEY_LEFT: u16 = 80;
const KEY_DOWN: u16 = 81;
const KEY_UP: u16 = 82;
const KEY_KEYPAD_ENTER: u16 = 88;
const KEY_LEFT_CTRL: u16 = 224;
const KEY_LEFT_SHIFT: u16 = 225;
const KEY_LEFT_ALT: u16 = 226;
const KEY_RIGHT_CTRL: u16 = 228;
const KEY_RIGHT_SHIFT: u16 = 229;
const KEY_RIGHT_ALT: u16 = 230;

/// The keys of the mod select's hotkeys, by key row, as lazer's sequential hotkey handler maps
/// them: the top letter row is the first column, the home row the second (the third column,
/// conversion, has none). Keys are USB HID scancodes.
const MOD_HOTKEY_ROWS: [&[u16]; 2] = [
    // Q W E R T Y U I O P
    &[20, 26, 8, 21, 23, 28, 24, 12, 18, 19],
    // A S D F G H J K L
    &[4, 22, 7, 9, 10, 11, 13, 14, 15],
];

/// The setting a selected mod shows after its acronym, as lazer's `ExtendedIconInformation`:
/// a speed off the default, or the approach rate when it is the only setting Difficulty
/// Adjust changes. Does not allocate.
fn tile_detail(m: &Mod) -> TileDetail {
    if let Some(speed) = m.custom_speed() {
        return TileDetail::Speed((speed * 100.0).round() as u16);
    }
    match m {
        Mod::DifficultyAdjust(d)
            if d.circle_size.is_none()
                && d.drain_rate.is_none()
                && d.overall_difficulty.is_none() =>
        {
            d.approach_rate.map_or(TileDetail::None, |ar| {
                TileDetail::ApproachRate((ar * 10.0).round() as i16)
            })
        }
        _ => TileDetail::None,
    }
}

/// The song select input of a mod hotkey.
fn mod_hotkey(scancode: u16) -> Option<SongSelectInput> {
    MOD_HOTKEY_ROWS
        .iter()
        .enumerate()
        .find_map(|(column, row)| {
            let index = row.iter().position(|&key| key == scancode)?;
            Some(SongSelectInput::ModHotkey { column, index })
        })
}

/// The columns of the mod select panel: lazer's mod types, with the mods SLAM has.
pub fn mod_columns() -> ModColumns {
    [
        mods_for(ModType::DifficultyReduction),
        mods_for(ModType::DifficultyIncrease),
        mods_for(ModType::Conversion),
    ]
}

/// The mods the player can have selected at the most (every tile of the panel).
const SELECTED_MODS_CAPACITY: usize = 16;

/// The application's screens, as the update thread holds them. The song select's state
/// lives in [`ShellState`]: its list and selection outlast a visit.
#[derive(Clone, Debug, PartialEq)]
pub enum AppScreen {
    Menu(MainMenu),
    SongSelect,
    /// The play of this id; its state lives with the window's audio, outside
    /// [`ShellState`].
    Play(u64),
    /// The score of a passed play.
    Results(Results),
}

/// What the draw thread needs to draw a screen.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ScreenView {
    Menu(MenuSnapshot),
    SongSelect(SongSelectSnapshot),
    /// The play of this id, drawn from [`ShellSnapshot::play`]. Only while it is on top:
    /// a play going out leaves at once instead of sliding with a stale snapshot.
    Play(u64),
    Results(ResultsSnapshot),
}

/// The settings panel as drawn (ADR-0095). Its snapshot is large (roughly 1 KB), but it is
/// written every tick the panel shows: a box would allocate on the update thread.
#[derive(Clone, Debug)]
pub struct PanelView {
    pub settings: SettingsSnapshot,
    /// How far the panel is in: 0 out of sight, 1 fully in.
    pub openness: f32,
}

#[derive(Clone, Debug)]
pub struct LayerSnapshot {
    pub view: ScreenView,
    pub pose: Pose,
}

/// The visible screens, bottom first.
#[derive(Clone, Debug, Default)]
pub struct ShellSnapshot {
    pub layers: [Option<LayerSnapshot>; MAX_LAYERS],
    /// The play, while its screen is shown; too large for a layer (ADR-0044).
    pub play: PlaySnapshot,
    /// The FPS counter's settings, in force or being edited (ADR-0060).
    pub fps: FpsDisplay,
    /// The song select's background (ADR-0066).
    pub background: BackgroundTarget,
    /// The thumbnails of the song list's rows (ADR-0127).
    pub thumbnails: crate::thumbnails::ThumbSnapshot,
    /// Dim of the background, 0 ..= 1: the song select's (ADR-0098), or the theme's behind
    /// the main menu (ADR-0119).
    pub background_dim: f32,
    /// The main menu shows the background (ADR-0119).
    pub menu_background: bool,
    /// Strength of the menus' parallax, 0 ..= 2: 0 with it off (ADR-0132, ADR-0136).
    pub parallax: f32,
    /// The display lists the settings screen shows (ADR-0072).
    pub display_generation: u64,
    /// How many language switches asked for views so far: a new one closes the curtain
    /// (ADR-0091).
    pub language_switch: u64,
    /// The shapes over the layers during a fancy screen transition (ADR-0126, ADR-0129).
    pub fx: Option<TransitionFx>,
    /// The settings panel over the screens, while it shows.
    pub panel: Option<PanelView>,
    /// The startup intro over everything, while it shows (ADR-0114).
    pub intro: Option<IntroSnapshot>,
    /// The menu cursor drawn over everything (ADR-0124).
    pub menu_cursor: crate::menu_cursor::MenuCursorFrame,
}

/// The update logic without the window: input to navigation to snapshot.
pub struct ShellState {
    stack: ScreenStack<AppScreen>,
    theme: Theme,
    /// Layouts in pixels, the unit the draw thread lays out in.
    layout: MenuLayout,
    song_layout: SongSelectLayout,
    results_layout: ResultsLayout,
    settings_layout: SettingsLayout,
    song_select: SongSelect,
    /// The library's end, without a songs folder `None`.
    songs: Option<SongsLink>,
    /// New display lists for the settings screen (ADR-0072); `None` in tests.
    displays: Option<ListsLink>,
    /// The `.osu` files of the song select's list, by set and difficulty.
    files: Arc<SongFiles>,
    /// The lists before the first publication, kept so that replacing them frees nothing
    /// on the update thread.
    _empty: (Arc<SongList>, Arc<SongFiles>, Arc<str>),
    /// The songs folder as the settings row shows it; empty without one. Replaced only when
    /// the text changes.
    songs_folder: Arc<str>,
    /// What the library thread said last; a load in progress shows over it.
    library_status: SongSelectStatus,
    /// A difficulty is being prepared to play.
    loading: bool,
    /// A difficulty to prepare, not yet taken by [`ShellState::take_play_request`].
    play_request: Option<(usize, usize)>,
    /// The `.osu` file of the last play asked for from the song select: what Retry plays.
    last_file: Option<PathBuf>,
    /// Every results screen's data stays here until a navigation finds this the last
    /// reference: the update tick and the snapshot buffers only change counts, never free.
    held_results: Vec<Arc<ResultsData>>,
    /// The beatmap line of `last_file` for the results.
    last_title: String,
    /// The results ask to play `last_file` again, not yet taken by
    /// [`ShellState::take_retry_request`].
    retry_request: bool,
    /// The results went back to the song select: the track they played on stops, not yet
    /// taken by [`ShellState::take_track_stop`].
    track_stop: bool,
    /// The settings screen opened: its device list is to be made again (ADR-0089), not yet
    /// taken by [`ShellState::take_device_list_request`].
    device_list_request: bool,
    /// The settings in force: what the file said, then what the settings screen committed.
    settings: Settings,
    /// The options of the settings screen's dropdowns, fixed for the run.
    rows: Rows,
    /// The settings being edited while the screen is open; committed when it closes.
    editing: Option<Settings>,
    /// Settings to write to the file, not yet taken by [`ShellState::take_save_request`].
    save_request: Option<Box<Settings>>,
    /// The mods the next play takes: they outlast a play and the results, and are not saved.
    selected_mods: ModSet,
    /// The panel's mods with default settings, by column and tile: a toggle clones one
    /// (`Mod::from_acronym` allocates).
    mod_templates: Vec<Vec<Mod>>,
    /// The mod select's selection being built on a toggle; kept for its capacity.
    mod_buffer: Vec<Mod>,
    /// What the mod speed hotkeys remember of the selection.
    speed_hotkeys: SpeedHotkeys,
    /// The bounds of the settings panel's sliders, kept as lazer's bindables keep them.
    slider_bounds: SliderBounds,
    /// The settings panel's rows and the settings they stand for.
    setting_rows: mod_settings::SettingRows,
    /// The difficulty (and library state) the rows' beatmap values were built for.
    setting_rows_for: Option<(usize, usize, usize)>,
    /// The modifier keys held, by [`modifier_bit`]: left and right Control, Shift and Alt.
    modifiers: u8,
    /// The settings panel, open or sliding out (ADR-0095).
    panel: Option<Panel>,
    /// Where the settings list was when the panel went: its scroll and the sidebar's pick,
    /// put back when it opens again; not saved (ADR-0136).
    settings_position: (f32, Option<usize>),
    /// The play shows its pause or fail menu: the settings panel may open over it.
    play_menu: bool,
    /// The settings panel closed with changes not yet looked at by the play (ADR-0096).
    settings_changed: bool,
    /// Formats the search texts of the settings panel as it opens; `None` in tests.
    localizer: Option<Arc<Localizer>>,
    /// The key a key row took, until it is released: its OS repeats are not navigation
    /// (ADR-0092). 0 when none.
    captured_key: u16,
    /// Last cursor position in pixels.
    cursor: [f32; 2],
    /// Input arrives in window points, layout is in pixels.
    pixels_per_point: f32,
    /// The window in pixels the screens are laid out for.
    size: [f32; 2],
    /// The beatmap whose music plays in the menus (ADR-0119).
    menu_track: Option<(usize, usize)>,
    /// The chosen skin's folder, for its menu background, and the songs folder and skin
    /// name it was made from (ADR-0119).
    menu_skin: Option<Arc<Path>>,
    menu_skin_of: (Option<PathBuf>, Option<String>),
    /// Interface sounds noted this tick (ADR-0120).
    ui_sounds: ui_sounds::UiSoundQueue,
    /// What the sounds compare with: stack depth, menu item, song select selection,
    /// settings panel open and its focus, as of the last tick.
    ui_seen: (
        usize,
        slam_ui::menu::MenuItem,
        Option<(u64, u64)>,
        bool,
        usize,
    ),
    /// Asks for the song select's background; without a menu texture loader `None`.
    backgrounds: Option<BackgroundPicker>,
    /// Asks for the song list's thumbnails; without a menu texture loader `None` (ADR-0127).
    thumbnails: Option<crate::thumbnails::Thumbnails>,
    /// The update tick's number, for the thumbnails' release.
    thumb_seq: u32,
    /// Loads the skin's cursor for the menus (ADR-0124); without it the skin's cursor is
    /// SLAM's.
    skin_cursor: Option<crate::menu_cursor::SkinCursorLink>,
    /// Asks for the selected difficulty's statistics; without a library thread `None`.
    details: Option<DetailsPicker>,
    /// The startup intro until it is over; `None` when off (ADR-0114).
    intro: Option<Intro>,
    /// Set by the draw thread once it has shown a frame: the intro's clock starts then,
    /// not while the draw thread still prepares its device and glyphs.
    intro_gate: Arc<AtomicBool>,
    /// The intro started or was skipped since the play host last asked: it plays or fades
    /// out the intro's sound.
    intro_events: IntroEvents,
}

/// What happened to the intro since the play host last asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IntroEvents {
    pub started: bool,
    pub skipped: bool,
}

impl ShellState {
    pub fn new(theme: Theme, width: f32, height: f32, pixels_per_point: f32) -> ShellState {
        let empty: (Arc<SongList>, Arc<SongFiles>, Arc<str>) =
            (Arc::default(), Arc::from([]), Arc::from(""));
        let mut song_select = SongSelect::new();
        song_select.set_list(Arc::clone(&empty.0), SongFilter::default());
        song_select.set_scanning_text(Arc::clone(&empty.2));
        *song_select.mods_mut() = ModSelect::new(mod_columns());
        ShellState {
            stack: ScreenStack::new(AppScreen::Menu(MainMenu::new())),
            layout: MenuLayout::new(width, height, &theme),
            song_layout: SongSelectLayout::new(width, height, &theme),
            results_layout: ResultsLayout::new(width, height, &theme),
            settings_layout: panel_layout(width, height, &theme),
            song_select,
            songs: None,
            displays: None,
            files: Arc::clone(&empty.1),
            songs_folder: Arc::clone(&empty.2),
            _empty: empty,
            library_status: SongSelectStatus::NoFolder,
            loading: false,
            play_request: None,
            last_file: None,
            last_title: String::new(),
            held_results: Vec::new(),
            retry_request: false,
            track_stop: false,
            device_list_request: false,
            settings: Settings::default(),
            rows: Rows::new(&Settings::default()),
            editing: None,
            save_request: None,
            // An empty set is always compatible; the capacity keeps a toggle from allocating.
            selected_mods: ModSet::new(Vec::with_capacity(SELECTED_MODS_CAPACITY))
                .unwrap_or_default(),
            mod_templates: mod_columns()
                .iter()
                .map(|c| c.iter().map(|a| Mod::from_acronym(a)).collect())
                .collect(),
            mod_buffer: Vec::with_capacity(SELECTED_MODS_CAPACITY),
            speed_hotkeys: SpeedHotkeys::default(),
            slider_bounds: SliderBounds::default(),
            setting_rows: mod_settings::SettingRows::default(),
            setting_rows_for: None,
            modifiers: 0,
            panel: None,
            settings_position: (0.0, None),
            play_menu: false,
            settings_changed: false,
            localizer: None,
            captured_key: 0,
            theme,
            cursor: [0.0, 0.0],
            pixels_per_point,
            size: [width, height],
            menu_track: None,
            menu_skin: None,
            skin_cursor: None,
            menu_skin_of: (None, None),
            ui_sounds: ui_sounds::UiSoundQueue::default(),
            ui_seen: (1, slam_ui::menu::MenuItem::ALL[0], None, false, 0),
            backgrounds: None,
            thumbnails: None,
            thumb_seq: 0,
            details: None,
            intro: None,
            intro_gate: Arc::new(AtomicBool::new(false)),
            intro_events: IntroEvents::default(),
        }
    }

    /// A shell that opens with the startup intro over the menu (ADR-0114); its clock
    /// starts on the first tick after `gate` is set, by the draw thread once it has shown
    /// a frame.
    pub fn with_intro(mut self, gate: Arc<AtomicBool>) -> ShellState {
        self.intro = Some(Intro::new());
        self.intro_gate = gate;
        self
    }

    /// The intro started or was skipped since the last call.
    pub fn take_intro_events(&mut self) -> IntroEvents {
        std::mem::take(&mut self.intro_events)
    }

    /// Whether the intro holds the menu's music back: until its impact (ADR-0114).
    pub fn intro_holds_music(&self, now_ns: u64) -> bool {
        self.intro.is_some_and(|intro| !intro.music_due(now_ns))
    }

    /// Whether the intro takes `kind`: while it plays, a press skips it and nothing else
    /// reaches the screens. Once skipped, the menu shows and takes input again.
    fn intro_input(&mut self, kind: InputKind, now_ns: u64) -> bool {
        let Some(intro) = &mut self.intro else {
            return false;
        };
        // The rest of the skipping tick's events go too: the text of the key that
        // skipped, say.
        if intro.skipped_at().is_some_and(|at| at != now_ns) {
            return false;
        }
        if let InputKind::Key { down: true, .. } | InputKind::MouseButton { down: true, .. } = kind
        {
            intro.skip(now_ns);
            self.intro_events.skipped |= intro.is_skipped();
        }
        // The modifiers and the pointer stay known for the menu after the intro.
        self.track_modifiers(kind);
        if let InputKind::MouseMove { x, y } = kind {
            self.cursor = [x * self.pixels_per_point, y * self.pixels_per_point];
        }
        true
    }

    /// A shell whose song select shows the selected difficulty's length, BPM and objects
    /// (ADR-0067); the difficulty values show without it.
    pub fn with_details(mut self, details: DetailsPicker) -> ShellState {
        self.details = Some(details);
        self
    }

    /// Asks for the thumbnails of the song list's rows in view and fills `out` with the
    /// loaded ones (ADR-0127). Neither blocks nor allocates.
    fn tick_thumbnails(&mut self, out: &mut crate::thumbnails::ThumbSnapshot) {
        use crate::thumbnails::{MARGIN, WANTED};
        let Some(thumbnails) = &mut self.thumbnails else {
            *out = crate::thumbnails::ThumbSnapshot::default();
            return;
        };
        self.thumb_seq = self.thumb_seq.wrapping_add(1);
        let height = self
            .editing
            .as_ref()
            .unwrap_or(&self.settings)
            .song_select
            .thumbnails
            .height();
        let first = self.song_select.first_row();
        let mut wanted = [None; WANTED];
        for (i, slot) in wanted.iter_mut().enumerate() {
            let flat = (first + i).checked_sub(MARGIN);
            *slot = match flat.and_then(|flat| self.song_select.row_at_index(flat)) {
                Some(slam_ui::song_select::Row::Set(set)) => Some(set),
                _ => None,
            };
        }
        let handles = thumbnails.tick(self.thumb_seq, height, &self.files, &wanted);
        out.seq = self.thumb_seq;
        out.enabled = height.is_some();
        out.handles.copy_from_slice(&handles[MARGIN..]);
    }

    /// The selected difficulty's details with the selected mods. Neither blocks nor
    /// allocates.
    fn beatmap_details(&mut self, now_ns: u64) -> Option<BeatmapDetails> {
        let selected = self.song_select.selected();
        let read = self
            .details
            .as_mut()
            .map(|details| details.tick(now_ns, &self.files, selected))
            .unwrap_or_default();
        let (statistics, scores) = (read.statistics, read.scores);
        let (set, difficulty) = selected?;
        let file = self.files.get(set)?.get(difficulty)?;
        let base = Difficulty {
            drain_rate: file.drain_rate.clamp(0.0, 10.0),
            circle_size: file.circle_size.clamp(0.0, 10.0),
            overall_difficulty: file.overall_difficulty.clamp(0.0, 10.0),
            approach_rate: file.approach_rate.clamp(0.0, 10.0),
            // Not shown, and no mod changes the others by them.
            slider_multiplier: 1.0,
            slider_tick_rate: 1.0,
        };
        let shown = adjusted_display_difficulty(base, &self.selected_mods);
        let rate = self.selected_mods.rate().rate;
        Some(BeatmapDetails {
            circle_size: shown.circle_size,
            approach_rate: shown.approach_rate,
            overall_difficulty: shown.overall_difficulty,
            drain_rate: shown.drain_rate,
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/BeatmapTitleWedge.cs (updateLengthAndBpmStatistics)
            stats: statistics.map(|s| DetailStats {
                length_ms: (s.length / rate).round_ties_even().max(0.0) as u64,
                bpm_minimum: round_bpm(s.bpm_minimum, rate),
                bpm_maximum: round_bpm(s.bpm_maximum, rate),
                bpm_most_common: round_bpm(s.bpm_most_common, rate),
                circles: s.circles,
                sliders: s.sliders,
                spinners: s.spinners,
            }),
            scores,
        })
    }

    /// A shell whose song select shows the selected beatmap's background (ADR-0066).
    pub fn with_skin_cursor(mut self, link: crate::menu_cursor::SkinCursorLink) -> ShellState {
        self.skin_cursor = Some(link);
        self
    }

    pub fn with_backgrounds(mut self, backgrounds: BackgroundPicker) -> ShellState {
        self.backgrounds = Some(backgrounds);
        self
    }

    /// A shell whose song list shows thumbnails (ADR-0127).
    pub fn with_thumbnails(mut self, thumbnails: crate::thumbnails::Thumbnails) -> ShellState {
        self.thumbnails = Some(thumbnails);
        self
    }

    /// A shell whose song select shows what the library thread publishes; without a
    /// link, it says that no songs folder was given.
    pub fn with_songs(mut self, songs: SongsLink) -> ShellState {
        self.library_status = SongSelectStatus::Scanning { sets: 0 };
        self.song_select.set_status(self.library_status);
        self.songs = Some(songs);
        self
    }

    /// A shell whose settings screen lists the displays the window reports (ADR-0072).
    pub fn with_displays(mut self, displays: ListsLink) -> ShellState {
        self.displays = Some(displays);
        self
    }

    /// A shell for a window started with `settings`.
    pub fn with_settings(mut self, settings: Settings) -> ShellState {
        self.rows = Rows::new(&settings);
        self.settings = settings;
        self
    }

    /// A shell whose settings screen offers the output devices the backend lists (#286).
    pub fn with_devices(mut self, devices: Vec<slam_audio::DeviceInfo>) -> ShellState {
        self.rows
            .set_devices(devices, self.settings.audio.device.as_deref());
        self
    }

    /// The audio settings to list the output devices for, once after the settings screen
    /// opened (ADR-0089).
    /// Lists the output devices again if the settings screen is open (ADR-0091).
    pub fn relist_devices(&mut self) {
        if self.editing.is_some() {
            self.device_list_request = true;
        }
    }

    pub fn take_device_list_request(&mut self) -> Option<&crate::settings::Audio> {
        std::mem::take(&mut self.device_list_request).then_some(&self.settings.audio)
    }

    /// Offers the devices of `list` on the device row and on an open settings screen; the
    /// draw thread gets the dropdown made for the same list (ADR-0089).
    pub fn set_devices(&mut self, list: &mut DeviceList) {
        self.rows
            .set_devices(std::mem::take(&mut list.devices), list.saved.as_deref());
        if let Some(panel) = &mut self.panel {
            let settings = self.editing.as_ref().unwrap_or(&self.settings);
            let (options, selected) = self.rows.device_choice(settings);
            panel
                .screen
                .set_dropdown(rows::device_row(), options, selected);
        }
    }

    /// The settings in force.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The input settings the input thread follows: the open settings screen's edits, else
    /// the ones in force.
    pub fn live_input(&self) -> &InputSettings {
        &self.editing.as_ref().unwrap_or(&self.settings).input
    }

    /// The settings in force with the settings screen's edits: the language switches as it
    /// is picked (ADR-0091).
    pub fn live_settings(&self) -> &Settings {
        self.editing.as_ref().unwrap_or(&self.settings)
    }

    /// The window settings in force: the settings screen's edits count at once, as lazer
    /// applies its layout settings (ADR-0072).
    pub fn live_window(&self) -> &crate::settings::model::Window {
        &self.editing.as_ref().unwrap_or(&self.settings).window
    }

    /// The options of the settings screen's rows.
    pub fn rows(&self) -> &Rows {
        &self.rows
    }

    /// The settings the player changed, to write to the file, once. Taking them is up to
    /// the update thread, which hands them to the saver thread.
    pub fn take_save_request(&mut self) -> Option<Box<Settings>> {
        self.save_request.take()
    }

    /// Puts back a request the saver could not take yet.
    pub fn requeue_save_request(&mut self, settings: Box<Settings>) {
        // A newer request wins.
        self.save_request.get_or_insert(settings);
    }

    /// Commits the edits of a settings screen still open, as closing it does.
    pub fn commit_editing(&mut self) {
        let _ = commit_settings(
            &mut self.settings,
            &mut self.editing,
            &mut self.save_request,
        );
    }

    /// Whether a play is on a visible layer, in a transition too (ADR-0047).
    pub fn gameplay_visible(&self, now_ns: u64) -> bool {
        self.stack
            .layers(now_ns)
            .iter()
            .flatten()
            .any(|(screen, _)| matches!(screen, AppScreen::Play(_)))
    }

    /// Whether a difficulty is being prepared to play.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// The difficulty the player asked to play, once; it is then loading.
    pub fn take_play_request(&mut self) -> Option<(usize, usize)> {
        self.play_request.take()
    }

    /// The results asked to play the last beatmap again, once; it is then loading. The
    /// file is [`ShellState::last_file`].
    pub fn take_retry_request(&mut self) -> bool {
        std::mem::take(&mut self.retry_request)
    }

    /// Whether the track playing under the results should stop, once.
    pub fn take_track_stop(&mut self) -> bool {
        std::mem::take(&mut self.track_stop)
    }

    /// The beatmap line of the last play asked for.
    pub fn last_title(&self) -> &str {
        &self.last_title
    }

    /// "artist - title [difficulty]" of a difficulty of the song select's list.
    fn beatmap_title(&self, set: usize, difficulty: usize) -> String {
        let row = self.song_select.list().sets.get(set);
        match (row, row.and_then(|r| r.difficulties.get(difficulty))) {
            (Some(row), Some(d)) => results::title(&row.artist, &row.title, &d.version),
            _ => String::new(),
        }
    }

    /// The mods the mod select has selected.
    pub fn selected_mods(&self) -> &ModSet {
        &self.selected_mods
    }

    /// A toggle of the mod select panel: the active tiles in column order, as lazer's overlay
    /// has them, make the new selection (which drops what the added mod conflicts with); then
    /// every tile shows what is selected. Does not allocate.
    fn toggle_mod(&mut self, column: usize, index: usize) {
        self.mod_buffer.clear();
        for (c, i, acronym, active) in self.song_select.mods().iter() {
            if active != (c == column && i == index) {
                // The selected instance stays, with its settings.
                let selected = self.selected_mods.get(acronym).cloned();
                self.mod_buffer
                    .extend(selected.or_else(|| self.mod_templates.get(c)?.get(i).cloned()));
            }
        }
        self.selected_mods.apply_selection(&self.mod_buffer);
        self.sync_mod_flags();
    }

    /// Deselects every mod.
    fn deselect_mods(&mut self) {
        self.selected_mods.apply_selection(&[]);
        self.sync_mod_flags();
    }

    /// Ctrl+Up/Down: the speed of the selection by `delta` (ADR-0056). A play being
    /// prepared has taken the mods already, so nothing changes while it loads.
    fn change_mod_speed(&mut self, delta: f64) {
        if self.loading {
            return;
        }
        self.speed_hotkeys
            .change_speed(&mut self.selected_mods, delta);
        self.sync_mod_flags();
    }

    /// Ctrl+F3/F4: the approach rate by `delta` from the selected difficulty's (ADR-0056);
    /// not while a play loads.
    fn change_approach_rate(&mut self, delta: f64) {
        if self.loading {
            return;
        }
        let Some((set, difficulty)) = self.song_select.selected() else {
            return;
        };
        let Some(file) = self.files.get(set).and_then(|s| s.get(difficulty)) else {
            return;
        };
        change_approach_rate(&mut self.selected_mods, file.approach_rate, delta);
        self.sync_mod_flags();
    }

    /// Makes the panel's tiles show the selection and its settings; the speed hotkeys and
    /// the slider bounds observe it, and the settings rows follow it. As in lazer, a change
    /// of which mods are selected collapses the settings panel, and selecting a mod that
    /// needs configuring expands it.
    fn sync_mod_flags(&mut self) {
        self.speed_hotkeys.observe(&self.selected_mods);
        self.slider_bounds.observe(&self.selected_mods);
        let before = self.song_select.mods().active_flags();
        let columns = *self.song_select.mods().columns();
        for (c, acronyms) in columns.iter().enumerate() {
            for (i, acronym) in acronyms.iter().enumerate() {
                let selected = self.selected_mods.get(acronym);
                let mods = self.song_select.mods_mut();
                mods.set_active(c, i, selected.is_some());
                mods.set_detail(c, i, selected.map_or(TileDetail::None, tile_detail));
            }
        }
        let after = self.song_select.mods().active_flags();
        if before != after {
            self.song_select.mods_mut().collapse_customisation();
        }
        self.rebuild_setting_rows();
        if before == after {
            return;
        }
        let configure = columns.iter().enumerate().any(|(c, acronyms)| {
            acronyms.iter().enumerate().any(|(i, acronym)| {
                let new = (after[c] & !before[c]) >> i & 1 != 0;
                new && self
                    .selected_mods
                    .get(acronym)
                    .is_some_and(Mod::requires_configuration)
            })
        });
        if configure {
            self.song_select.mods_mut().expand_customisation_by_mod();
        }
    }

    /// Gives the settings panel the rows of the selected mods, with the beatmap values of
    /// the selected difficulty.
    fn rebuild_setting_rows(&mut self) {
        self.setting_rows_for = Some(self.rows_key());
        let file = self
            .song_select
            .selected()
            .and_then(|(set, difficulty)| self.files.get(set)?.get(difficulty));
        self.setting_rows
            .build(&self.selected_mods, &self.slider_bounds, file);
        self.song_select
            .mods_mut()
            .set_setting_rows(self.setting_rows.rows());
    }

    /// What the rows' beatmap values depend on: the selected difficulty and the library's
    /// files.
    fn rows_key(&self) -> (usize, usize, usize) {
        let (set, difficulty) = self.song_select.selected().unwrap_or((usize::MAX, 0));
        (
            set,
            difficulty,
            Arc::as_ptr(&self.files).cast::<()>() as usize,
        )
    }

    /// The settings panel changed a row (`Some`) or reset it (`None`): writes it to the mod.
    fn apply_setting(&mut self, row: usize, value: Option<SettingValue>) {
        if self.loading {
            return;
        }
        let Some(target) = self.setting_rows.target(row) else {
            return;
        };
        let state = value.map_or_else(|| target.kind.default_state(), mod_settings::state_of);
        self.selected_mods.set_setting_state(
            target.acronym,
            target.key,
            state,
            &mut self.slider_bounds,
        );
        self.sync_mod_flags();
    }

    /// The `.osu` file of the last play asked for.
    pub fn last_file(&self) -> Option<&PathBuf> {
        self.last_file.as_ref()
    }

    /// Ends a load: with the id of a started play, its screen comes in (over the results
    /// of the play it retries, in their place), else the screen that asked takes input
    /// again. Navigation.
    /// Whether the song select says that nothing plays without an audio output (#317).
    pub fn set_no_audio(&mut self, no_audio: bool) {
        self.song_select.set_no_audio(no_audio);
    }

    pub fn finish_loading(&mut self, now_ns: u64, started: Option<u64>) {
        self.set_loading(false);
        if let Some(id) = started {
            let play = AppScreen::Play(id);
            let nav = if matches!(self.stack.top(), AppScreen::Results(_)) {
                Nav::Replace(play)
            } else {
                Nav::Push(play)
            };
            // A full stack cannot happen from the song select or the results: the play
            // would be unreachable then, so the shell's depth leaves room for it.
            self.stack.set_origin(Some(self.cursor));
            self.stack.navigate(nav, now_ns);
        }
    }

    /// Whether a play is the top screen.
    pub fn playing(&self) -> bool {
        matches!(self.stack.top(), AppScreen::Play(_))
    }

    /// The play screen goes: the results of a passed play take its place, else the song
    /// select is back. Navigation.
    pub fn leave_play(&mut self, now_ns: u64, results: Option<ResultsData>) {
        if self.playing() {
            // Navigation: the data nobody else holds goes now.
            self.held_results.retain(|data| Arc::strong_count(data) > 1);
            self.stack.set_origin(Some(self.cursor));
            self.stack.navigate(after_play(results), now_ns);
            if let AppScreen::Results(results) = self.stack.top() {
                self.held_results.push(results.share());
            }
        }
    }

    /// Follows the pointer and the modifier keys while the play has the input, so that the
    /// screen under it knows where they are when the play goes.
    pub fn follow_pointer(&mut self, events: &[InputEvent]) {
        for event in events {
            self.track_modifiers(event.kind);
            if let InputKind::MouseMove { x, y } = event.kind {
                self.cursor = [x * self.pixels_per_point, y * self.pixels_per_point];
            }
        }
    }

    /// Watches for the hotkeys of the whole window while the play has the input: they
    /// reach the play too, which has no binding on them (ADR-0060).
    pub fn watch_hotkeys(&mut self, events: &[InputEvent]) {
        for event in events {
            if self.hotkey(event.kind) == Some(KeySlot::ToggleFps) {
                self.toggle_fps();
            }
        }
    }

    /// Sets whether the play shows its pause or fail menu, over which the settings panel
    /// may open (ADR-0095).
    pub fn set_play_menu(&mut self, shown: bool) {
        self.play_menu = shown;
    }

    /// Gives the settings panel the localizer for its search texts.
    pub fn set_localizer(&mut self, localizer: Arc<Localizer>) {
        self.localizer = Some(localizer);
    }

    /// Whether the settings panel is open or opening, and so takes the input.
    pub fn settings_open(&self) -> bool {
        self.panel.as_ref().is_some_and(|p| p.motion.is_open())
    }

    /// Whether `events` press the settings panel's hotkey.
    pub fn toggles_settings(&mut self, events: &[InputEvent]) -> bool {
        let mut modifiers = self.modifiers;
        events.iter().any(|event| {
            track(&mut modifiers, event.kind);
            hotkey_of(&self.settings, modifiers, event.kind) == Some(KeySlot::ToggleSettings)
        })
    }

    /// Whether the settings panel may open over the screen shown: not over the play
    /// itself, only its pause or fail menu (ADR-0095).
    pub fn settings_allowed(&self) -> bool {
        match self.stack.top() {
            AppScreen::Play(_) => self.play_menu,
            _ => true,
        }
    }

    /// Opens the settings panel over the screen shown, or, open, starts closing it.
    /// Allocates on opening, as a navigation.
    fn toggle_settings(&mut self, now_ns: u64) {
        if self.settings_open() {
            self.close_settings(now_ns);
        } else if self.settings_allowed() {
            self.open_settings(now_ns);
        }
    }

    /// Opens the settings panel with the settings in force to edit, or turns a closing one
    /// back. Allocates.
    fn open_settings(&mut self, now_ns: u64) {
        if let Some(panel) = &mut self.panel {
            panel.motion.reopen(now_ns);
            return;
        }
        self.editing = Some(self.settings.clone());
        let mut screen = SettingsScreen::new(&self.rows.specs(&self.settings), SECTIONS);
        self.rows.refresh(&mut screen, &self.settings);
        screen.set_value_text(Arc::clone(&self.songs_folder));
        if let Some(localizer) = &self.localizer {
            screen.set_search_texts(Rows::search_texts(localizer));
        }
        let (scroll, pinned) = self.settings_position;
        screen.restore_position(scroll, pinned);
        self.panel = Some(Panel {
            screen: Box::new(screen),
            motion: PanelMotion::opening(now_ns, &self.theme),
            pressed_outside: false,
        });
        self.device_list_request = true;
    }

    /// Commits the edits and starts sliding the panel out; it goes when out of sight.
    fn close_settings(&mut self, now_ns: u64) {
        let Some(panel) = &mut self.panel else {
            return;
        };
        panel.motion.close(now_ns);
        if commit_settings(
            &mut self.settings,
            &mut self.editing,
            &mut self.save_request,
        ) {
            self.settings_changed = true;
        }
    }

    /// Whether the settings panel closed with changes since last asked: a play running takes
    /// the ones it was prepared with anew (ADR-0096).
    pub fn take_settings_changed(&mut self) -> bool {
        std::mem::take(&mut self.settings_changed)
    }

    /// The open settings panel's screen.
    pub fn settings_screen(&self) -> Option<&SettingsScreen> {
        self.panel.as_ref().map(|p| &*p.screen)
    }

    /// The window changed itself from `before` to `after` (ADR-0071, ADR-0073), each a
    /// state and a windowed position: keeps the fields it changed in the settings, and
    /// only those, so an edit of another field in the same tick stands. A new mode
    /// (Alt+Enter, F11) is saved at once, as lazer's `CycleMode` changes its `WindowMode`
    /// setting; returns whether anything else changed, which the caller saves once the
    /// window settles. Allocates for a save only: a rare action of the player, as
    /// [`ShellState::toggle_fps`]. An open settings screen's copy follows on its own, or
    /// closing the screen would undo the change.
    pub fn window_reported(
        &mut self,
        before: (WindowState, [f32; 2]),
        after: (WindowState, [f32; 2]),
    ) -> bool {
        let ((was, was_at), (state, position)) = (before, after);
        let take = |window: &mut crate::settings::model::Window| {
            if state.mode != was.mode {
                window.mode = state.mode;
            }
            if state.display != was.display {
                window.display = state.display;
            }
            if state.windowed_size != was.windowed_size {
                window.windowed_size = state.windowed_size;
            }
            if position != was_at {
                window.windowed_position = position;
            }
        };
        if let Some(editing) = &mut self.editing {
            take(&mut editing.window);
        }
        let before = self.settings.window;
        take(&mut self.settings.window);
        // A mode the window switched to by itself, new to the settings in force. Compared
        // with `was` too: while the settings screen's edit is applied, the settings in
        // force may hold another mode that no report should save again.
        if state.mode != was.mode && before.mode != state.mode {
            self.save_request = Some(Box::new(self.settings.clone()));
            return false;
        }
        before != self.settings.window
    }

    /// Keeps the window's title bar height for the windowed resolution row (ADR-0073).
    pub fn set_title_bar(&mut self, top: i32) {
        self.rows.set_title_bar(top);
    }

    /// Queues the settings in force for the file. Allocates.
    pub fn save_settings(&mut self) {
        self.save_request = Some(Box::new(self.settings.clone()));
    }

    /// Input points now map to `pixels_per_point` layout pixels.
    pub fn set_pixels_per_point(&mut self, pixels_per_point: f32) {
        self.pixels_per_point = pixels_per_point;
    }

    /// The hotkey `kind` presses with the modifiers held, if any (ADR-0095).
    fn hotkey(&self, kind: InputKind) -> Option<KeySlot> {
        hotkey_of(&self.settings, self.modifiers, kind)
    }

    /// The modifiers held.
    fn mods(&self) -> Mods {
        mods_of(self.modifiers)
    }

    /// Shows or hides the FPS counter and saves the settings, as lazer's
    /// `ToggleFPSDisplay`. An open settings screen shows the change. Allocates for the save:
    /// a rare action of the player, as navigation (ADR-0060).
    pub fn toggle_fps(&mut self) {
        let show = !self.settings.frame.show_fps;
        self.settings.frame.show_fps = show;
        if let Some(editing) = &mut self.editing {
            editing.frame.show_fps = show;
        }
        if let (Some(panel), Some(row)) = (
            &mut self.panel,
            Row::ALL.iter().position(|r| *r == Row::ShowFps),
        ) {
            panel.screen.set_toggle(row, show);
        }
        self.save_request = Some(Box::new(self.settings.clone()));
    }

    /// Notes a modifier key going down or up.
    fn track_modifiers(&mut self, kind: InputKind) {
        track(&mut self.modifiers, kind);
    }

    fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
        self.song_select.set_status(if loading {
            SongSelectStatus::Loading
        } else {
            self.library_status
        });
    }

    pub fn stack(&self) -> &ScreenStack<AppScreen> {
        &self.stack
    }

    pub fn song_select(&self) -> &SongSelect {
        &self.song_select
    }

    /// The `.osu` file of a difficulty of the song select's list.
    pub fn file(&self, set: usize, difficulty: usize) -> Option<&PathBuf> {
        self.files.get(set)?.get(difficulty).map(|f| &f.path)
    }

    /// The song select's selected difficulty, if any.
    pub fn selected(&self) -> Option<(usize, usize)> {
        self.song_select.selected()
    }

    /// The library's files, by set and difficulty.
    pub fn files(&self) -> &SongFiles {
        &self.files
    }

    /// Lays the screens out for a window of `width` × `height` pixels.
    pub fn set_size(&mut self, width: f32, height: f32) {
        self.size = [width, height];
        self.layout = MenuLayout::new(width, height, &self.theme);
        self.song_layout = SongSelectLayout::new(width, height, &self.theme);
        self.results_layout = ResultsLayout::new(width, height, &self.theme);
        self.settings_layout = panel_layout(width, height, &self.theme);
    }

    /// The beatmap whose music plays in the menus, as set and difficulty (ADR-0119).
    pub fn set_menu_track(&mut self, at: Option<(usize, usize)>) {
        self.menu_track = at;
    }

    /// The skin folder whose menu background the menu shows; made again only when the
    /// songs folder or the skin changes (allocates then).
    pub(crate) fn menu_skin(&mut self) -> Option<&Arc<Path>> {
        let settings = self.editing.as_ref().unwrap_or(&self.settings);
        let songs = settings.paths.songs.as_deref();
        let name = settings.skin.name.as_deref();
        if (
            self.menu_skin_of.0.as_deref(),
            self.menu_skin_of.1.as_deref(),
        ) != (songs, name)
        {
            self.menu_skin_of = (songs.map(Path::to_path_buf), name.map(str::to_owned));
            self.menu_skin = crate::settings::skins::skin_path(songs, name).map(Arc::from);
        }
        self.menu_skin.as_ref()
    }

    /// The interface sounds noted since the last call (ADR-0120).
    pub fn take_ui_sounds(&mut self) -> [bool; crate::menu_sounds::MenuSound::ALL.len()] {
        self.ui_sounds.take()
    }

    /// Notes the sounds of what changed since the last tick: screens in and out, the
    /// hovered menu item, the selected beatmap, the settings panel and its focus.
    fn note_ui_sounds(&mut self, events: &[InputEvent]) {
        use crate::menu_sounds::MenuSound;
        let depth = self.stack.depth();
        let item = match self.stack.top() {
            AppScreen::Menu(menu) => menu.selected(),
            _ => self.ui_seen.1,
        };
        // By key: a list reordered under the selection is not the player's move.
        let selected = self.song_select.selected_keys();
        let open = self.settings_open();
        let focus = self.settings_screen().map_or(0, |screen| screen.focus());
        let (old_depth, old_item, old_selected, old_open, old_focus) = self.ui_seen;
        let playing = matches!(self.stack.top(), AppScreen::Play(_));
        if !playing {
            if depth > old_depth || (open && !old_open) {
                self.ui_sounds.push(MenuSound::Enter);
            } else if depth < old_depth || (!open && old_open) {
                self.ui_sounds.push(MenuSound::Back);
            }
            // A selection appearing with the library is not the player's move.
            let moved = old_selected.is_some() && selected != old_selected;
            if item != old_item || moved || (open && focus != old_focus) {
                self.ui_sounds.push(MenuSound::Hover);
            }
            let clicked = events.iter().any(|e| {
                matches!(
                    e.kind,
                    InputKind::MouseButton {
                        button: MouseButton::Left,
                        down: true
                    }
                )
            });
            if clicked {
                self.ui_sounds.push(MenuSound::Click);
            }
        }
        self.ui_seen = (depth, item, selected, open, focus);
    }

    /// The theme the screens are laid out with.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Lays the screens out with `theme` (another colour scheme or interface scale,
    /// ADR-0117, ADR-0118) when it differs from the one in force. Allocates nothing.
    pub fn set_theme(&mut self, theme: Theme) {
        if theme != self.theme {
            self.theme = theme;
            let [width, height] = self.size;
            self.set_size(width, height);
        }
    }

    /// The input of the screens, as the song select (the larger set) understands it, and
    /// the key only the results use.
    fn screen_input(&mut self, kind: InputKind) -> Option<ScreenInput> {
        self.track_modifiers(kind);
        // Lazer's key row in focus drops repeats (`KeyBindingRow.OnKeyDown`): a key held
        // past its capture neither walks the rows nor answers the conflict.
        match kind {
            InputKind::KeyRepeat { scancode } if scancode == self.captured_key => return None,
            InputKind::Key {
                scancode,
                down: false,
            } if scancode == self.captured_key => self.captured_key = 0,
            _ => {}
        }
        // A key row that listens takes every key and mouse button, whatever the menus make
        // of them; Escape still goes through, as it cancels.
        let listening = self
            .panel
            .as_ref()
            .filter(|p| p.motion.is_open())
            .and_then(|p| p.screen.listening_row());
        if let Some(row) = listening {
            // A hotkey takes the modifiers held with its key, which alone do not end the
            // capture (ADR-0095).
            let hotkey = Row::ALL
                .get(row)
                .and_then(|r| r.key_slot())
                .is_some_and(KeySlot::is_hotkey);
            match kind {
                InputKind::Key {
                    scancode: KEY_DELETE,
                    down: true,
                } => return Some(ScreenInput::Clear),
                InputKind::Key {
                    scancode,
                    down: true,
                } if hotkey && modifier_bit(scancode).is_some() => return None,
                InputKind::Key {
                    scancode,
                    down: true,
                } if scancode != KEY_ESCAPE => {
                    self.captured_key = scancode;
                    let mods = if hotkey { self.mods() } else { Mods::NONE };
                    return Some(ScreenInput::Capture(keybinds::key_value(Some(Combo {
                        mods,
                        input: Input::Key(scancode),
                    }))));
                }
                InputKind::MouseButton { button, down: true } => {
                    return Some(ScreenInput::Capture(keybinds::key_value(Some(
                        Combo::plain(Input::Mouse(button)),
                    ))));
                }
                _ => {}
            }
        }
        match self.hotkey(kind) {
            Some(KeySlot::ToggleFps) => return Some(ScreenInput::ToggleFps),
            Some(KeySlot::ToggleSettings) => return Some(ScreenInput::ToggleSettings),
            _ => {}
        }
        // The letters are the mod hotkeys on the song select, and Retry on the results;
        // the open settings panel has them instead.
        let song_select =
            matches!(self.stack.top(), AppScreen::SongSelect) && !self.settings_open();
        if let InputKind::Key {
            scancode,
            down: true,
        } = kind
            && song_select
            && self.mods().contains(Mods::CTRL)
        {
            match scancode {
                KEY_UP => return Some(ScreenInput::ModSpeed(SPEED_STEP)),
                KEY_DOWN => return Some(ScreenInput::ModSpeed(-SPEED_STEP)),
                KEY_F3 => return Some(ScreenInput::ApproachRate(-APPROACH_RATE_STEP)),
                KEY_F4 => return Some(ScreenInput::ApproachRate(APPROACH_RATE_STEP)),
                _ => {}
            }
        }
        // Lazer repeats the last pressed global action on OS key repeats. Here only the
        // steps and the walks repeat: a held Enter, Escape or letter acts once (ADR-0092).
        if let InputKind::KeyRepeat { scancode } = kind {
            if song_select && self.mods().contains(Mods::CTRL) {
                match scancode {
                    KEY_UP => return Some(ScreenInput::ModSpeed(SPEED_STEP)),
                    KEY_DOWN => return Some(ScreenInput::ModSpeed(-SPEED_STEP)),
                    KEY_F3 => return Some(ScreenInput::ApproachRate(-APPROACH_RATE_STEP)),
                    KEY_F4 => return Some(ScreenInput::ApproachRate(APPROACH_RATE_STEP)),
                    _ => {}
                }
            }
            return Some(ScreenInput::Repeat(match scancode {
                // Holding Backspace keeps erasing the search (ADR-0065).
                KEY_BACKSPACE if song_select || self.settings_open() => SongSelectInput::Erase,
                KEY_UP => SongSelectInput::Up,
                KEY_DOWN => SongSelectInput::Down,
                KEY_LEFT => SongSelectInput::Left,
                KEY_RIGHT => SongSelectInput::Right,
                KEY_PAGE_UP => SongSelectInput::PageUp,
                KEY_PAGE_DOWN => SongSelectInput::PageDown,
                _ => return None,
            }));
        }
        Some(ScreenInput::Common(match kind {
            InputKind::Key {
                scancode,
                down: true,
            } => match scancode {
                KEY_R if !song_select => return Some(ScreenInput::Retry),
                KEY_F1 => SongSelectInput::ToggleMods,
                KEY_BACKSPACE => SongSelectInput::DeselectMods,
                KEY_TAB if song_select => SongSelectInput::ToggleCustomisation,
                KEY_DELETE if song_select => SongSelectInput::ResetSetting,
                KEY_SPACE if song_select => SongSelectInput::Space,
                KEY_TAB => return Some(ScreenInput::Tab),
                KEY_UP => SongSelectInput::Up,
                KEY_DOWN => SongSelectInput::Down,
                KEY_LEFT => SongSelectInput::Left,
                KEY_RIGHT => SongSelectInput::Right,
                KEY_PAGE_UP => SongSelectInput::PageUp,
                KEY_PAGE_DOWN => SongSelectInput::PageDown,
                KEY_HOME => SongSelectInput::Home,
                KEY_END => SongSelectInput::End,
                KEY_ENTER | KEY_KEYPAD_ENTER => SongSelectInput::Activate,
                KEY_ESCAPE => SongSelectInput::Back,
                code => {
                    return mod_hotkey(code)
                        .filter(|_| song_select)
                        .map(ScreenInput::Common);
                }
            },
            InputKind::MouseButton {
                button: MouseButton::X1,
                down: true,
            } => SongSelectInput::Back,
            InputKind::MouseButton {
                button: MouseButton::Left,
                down: false,
            } => return Some(ScreenInput::Release),
            InputKind::MouseButton {
                button: MouseButton::Left,
                down: true,
            } => SongSelectInput::PointerPress {
                x: self.cursor[0],
                y: self.cursor[1],
            },
            InputKind::MouseMove { x, y } => {
                self.cursor = [x * self.pixels_per_point, y * self.pixels_per_point];
                SongSelectInput::PointerMove {
                    x: self.cursor[0],
                    y: self.cursor[1],
                }
            }
            // The wheel's positive notches go away from the user, towards the list's start.
            InputKind::MouseWheel { y } => SongSelectInput::Scroll { lines: -y },
            _ => return None,
        }))
    }

    /// Takes the newest display lists, if any, and shows them on the open settings panel.
    /// The old lists go back to the `slam-displays` thread.
    fn receive_displays(&mut self) {
        let Some(link) = &mut self.displays else {
            return;
        };
        let Some(mut lists) = link.take() else {
            return;
        };
        self.rows.swap_displays(&mut lists);
        link.give_back(lists);
        if let Some(panel) = &mut self.panel {
            self.rows.refresh(
                &mut panel.screen,
                self.editing.as_ref().unwrap_or(&self.settings),
            );
        }
    }

    /// Takes the newest list the library thread published, if any.
    fn receive_songs(&mut self) {
        let Some(published) = self.songs.as_mut().and_then(SongsLink::latest) else {
            return;
        };
        let old = if published.refresh {
            self.song_select
                .refresh_list(published.list, published.filter)
        } else {
            self.song_select.set_list(published.list, published.filter)
        };
        if let Some(songs) = &mut self.songs {
            songs.retire_filter(old);
        }
        self.library_status = published.status;
        if !self.loading {
            self.song_select.set_status(published.status);
        }
        self.song_select.set_scanning_text(published.scanning_text);
        self.files = published.files;
        if let Some(mut change) = published.change {
            self.change_folder(&mut change);
            if let Some(songs) = &mut self.songs {
                songs.retire(change);
            }
        }
        // Every publication brings a new `Arc`; the text rarely changes.
        if *published.folder != *self.songs_folder {
            self.songs_folder = published.folder;
            if let Some(panel) = &mut self.panel {
                panel.screen.set_value_text(Arc::clone(&self.songs_folder));
            }
        }
    }

    /// Takes the songs folder and its skins the player chose (ADR-0064): the next play finds
    /// the skin there, and the settings screen lists them. The old values go back into
    /// `change`, to be freed by the library thread.
    fn change_folder(&mut self, change: &mut FolderChange) {
        std::mem::swap(&mut self.settings.paths.songs, &mut change.songs);
        self.rows.swap_skins(&mut change.skins);
        if let Some(panel) = &mut self.panel {
            let settings = self.editing.as_ref().unwrap_or(&self.settings);
            let (options, selected) = self.rows.skin_choice(settings);
            panel
                .screen
                .set_dropdown(rows::skin_row(), options, selected);
        }
    }

    /// Types `text` into the open settings panel's search (ADR-0095), else the song
    /// select's (ADR-0065); other screens take no text.
    fn type_text(&mut self, text: &str) {
        if self.settings_open() {
            if let Some(panel) = &mut self.panel {
                for c in text.chars() {
                    panel
                        .screen
                        .input(SettingsInput::Type(c), &self.settings_layout);
                }
            }
            return;
        }
        if !matches!(self.stack.top(), AppScreen::SongSelect) {
            return;
        }
        self.song_select.type_text(text, &self.song_layout);
    }

    /// Asks the library thread to show the folder picker (ADR-0064). A full command ring
    /// drops the click; the player clicks again.
    fn pick_folder(&mut self) {
        if let Some(songs) = &mut self.songs {
            songs.pick_folder();
        }
    }

    /// Gives `events` to the top screen and applies what it asks for at `now_ns`. Stops at
    /// the first exit.
    /// Gives `input` to the open settings panel: its screen, laid out where the panel is
    /// now, or, outside the panel, a press and release that close it (ADR-0095).
    fn panel_input(&mut self, input: ScreenInput, now_ns: u64) {
        let Some(panel) = &mut self.panel else {
            return;
        };
        let width = self.settings_layout.width();
        let shift = panel.motion.shift(now_ns, width);
        let outside = |x: f32| x >= width + shift;
        match input {
            ScreenInput::Common(SongSelectInput::PointerPress { x, .. }) => {
                panel.pressed_outside = outside(x);
                if panel.pressed_outside {
                    return;
                }
            }
            ScreenInput::Release if panel.pressed_outside => {
                panel.pressed_outside = false;
                if outside(self.cursor[0]) {
                    self.close_settings(now_ns);
                }
                return;
            }
            // Each language choice is a switch behind the curtain: a held arrow on the
            // language row would close it again and again (ADR-0092).
            ScreenInput::Repeat(SongSelectInput::Left | SongSelectInput::Right)
                if Row::ALL.get(panel.screen.focus()) == Some(&Row::Language) =>
            {
                return;
            }
            _ => {}
        }
        let layout = self.settings_layout.shifted(shift);
        let action = settings_input(input).and_then(|input| panel.screen.input(input, &layout));
        let screen = &mut panel.screen;
        match action {
            Some(SettingsAction::Changed { row, value }) => {
                use crate::menu_sounds::MenuSound;
                self.ui_sounds.push(match value {
                    slam_ui::settings_screen::RowValue::Toggle(true) => MenuSound::ToggleOn,
                    slam_ui::settings_screen::RowValue::Toggle(false) => MenuSound::ToggleOff,
                    slam_ui::settings_screen::RowValue::Slider(_) => MenuSound::SliderStep,
                    slam_ui::settings_screen::RowValue::Choice(_) => MenuSound::Click,
                });
                if let Some(editing) = &mut self.editing {
                    self.rows.apply(editing, row, value);
                    self.rows.refresh(screen, editing);
                }
            }
            Some(SettingsAction::Pressed { row })
                if Row::ALL.get(row) == Some(&Row::SongsFolder) =>
            {
                self.pick_folder();
            }
            Some(
                action @ (SettingsAction::KeyCaptured { .. }
                | SettingsAction::KeyCleared { .. }
                | SettingsAction::Pressed { .. }
                | SettingsAction::ConflictResolved { .. }),
            ) => {
                if let Some(editing) = &mut self.editing {
                    keybinds::apply(screen, editing, action);
                }
            }
            Some(SettingsAction::Close) => self.close_settings(now_ns),
            None => {}
        }
    }

    pub fn handle(&mut self, events: &[InputEvent], now_ns: u64) -> Navigated {
        for event in events {
            // The shell takes window modes from the window itself (`Shell::sync_window`).
            if let InputKind::WindowMode(_) = event.kind {
                continue;
            }
            if self.intro_input(event.kind, now_ns) {
                continue;
            }
            if let InputKind::Text(text) = event.kind {
                self.type_text(text.as_str());
                continue;
            }
            let Some(input) = self.screen_input(event.kind) else {
                continue;
            };
            if input == ScreenInput::ToggleFps {
                self.toggle_fps();
                continue;
            }
            if input == ScreenInput::ToggleSettings {
                self.toggle_settings(now_ns);
                continue;
            }
            if self.settings_open() {
                self.panel_input(input, now_ns);
                continue;
            }
            let common = input.common();
            let mut open_panel = false;
            let nav = match self.stack.top_mut() {
                AppScreen::Menu(menu) => {
                    match common
                        .and_then(menu_input)
                        .and_then(|input| menu.input(input, &self.layout))
                    {
                        Some(MenuAction::Play) => Some(Nav::Push(AppScreen::SongSelect)),
                        Some(MenuAction::Settings) => {
                            open_panel = true;
                            None
                        }
                        Some(MenuAction::Exit) => Some(Nav::Exit),
                        None => None,
                    }
                }
                AppScreen::SongSelect if let ScreenInput::ModSpeed(delta) = input => {
                    self.change_mod_speed(delta);
                    None
                }
                AppScreen::SongSelect if let ScreenInput::ApproachRate(delta) = input => {
                    self.change_approach_rate(delta);
                    None
                }
                AppScreen::SongSelect => {
                    match common.and_then(|input| self.song_select.input(input, &self.song_layout))
                    {
                        // Back while loading cancels the load and stays.
                        Some(SongSelectAction::Back) if self.loading => {
                            self.play_request = None;
                            self.set_loading(false);
                            None
                        }
                        Some(SongSelectAction::Back) => Some(Nav::Pop),
                        Some(SongSelectAction::ToggleMod { column, index }) => {
                            self.toggle_mod(column, index);
                            None
                        }
                        Some(SongSelectAction::DeselectMods) => {
                            self.deselect_mods();
                            None
                        }
                        Some(SongSelectAction::SetModSetting { row, value }) => {
                            self.apply_setting(row, Some(value));
                            None
                        }
                        Some(SongSelectAction::ResetModSetting { row }) => {
                            self.apply_setting(row, None);
                            None
                        }
                        Some(SongSelectAction::ChooseFolder) => {
                            self.pick_folder();
                            None
                        }
                        Some(SongSelectAction::Play { set, difficulty }) => {
                            if let Some(file) = self.file(set, difficulty) {
                                self.last_file = Some(file.clone());
                                self.last_title = self.beatmap_title(set, difficulty);
                                self.play_request = Some((set, difficulty));
                                self.set_loading(true);
                            }
                            None
                        }
                        None => None,
                    }
                }
                AppScreen::Results(results) => {
                    let action = results_input(input)
                        .and_then(|input| results.input(input, &self.results_layout));
                    match action {
                        // Back while loading cancels the load and stays.
                        Some(ResultsAction::Back) if self.loading => {
                            self.retry_request = false;
                            self.set_loading(false);
                            None
                        }
                        Some(ResultsAction::Back) => {
                            self.track_stop = true;
                            Some(Nav::Pop)
                        }
                        Some(ResultsAction::Retry) => {
                            if !self.loading && self.last_file.is_some() {
                                self.retry_request = true;
                                self.set_loading(true);
                            }
                            None
                        }
                        None => None,
                    }
                }
                // The play takes its input from the window's play host.
                AppScreen::Play(_) => None,
            };
            if open_panel {
                self.open_settings(now_ns);
            }
            if let Some(nav) = nav {
                self.stack.set_origin(Some(self.cursor));
                if self.stack.navigate(nav, now_ns) == Navigated::Exit {
                    return Navigated::Exit;
                }
            }
        }
        Navigated::Stay
    }

    /// One tick: ends a finished transition, handles `events`, rewrites every layer of
    /// `snapshot`. Allocates only when `events` navigate.
    pub fn tick(
        &mut self,
        now_ns: u64,
        events: &[InputEvent],
        snapshot: &mut ShellSnapshot,
    ) -> Navigated {
        self.stack.tick(now_ns);
        if let Some(intro) = &mut self.intro
            && !intro.is_started()
            && self.intro_gate.load(Ordering::Acquire)
        {
            intro.start(now_ns);
            self.intro_events.started = true;
        }
        self.receive_songs();
        self.receive_displays();
        let navigated = self.handle(events, now_ns);
        self.note_ui_sounds(events);
        if self.intro.is_some_and(|intro| intro.is_done(now_ns)) {
            self.intro = None;
        }
        snapshot.intro = self.intro.and_then(|intro| intro.snapshot(now_ns));
        self.song_select.tick(now_ns, &self.song_layout);
        // The star thread calculates what is in view first (ADR-0134).
        if let (Some(songs), Some((key, span))) = (&self.songs, self.song_select.view_focus()) {
            songs.set_focus(key, span);
        }
        // Not while a play is loading or running: stars wait for the way out (ADR-0135).
        if let Some(songs) = &self.songs {
            songs.set_playing(self.loading || self.playing() || self.gameplay_visible(now_ns));
        }
        if self.setting_rows_for != Some(self.rows_key()) {
            self.rebuild_setting_rows();
        }
        snapshot.display_generation = self.rows.display_generation();
        snapshot.fps = FpsDisplay::of(&self.editing.as_ref().unwrap_or(&self.settings).frame);
        snapshot.background_dim = self
            .editing
            .as_ref()
            .unwrap_or(&self.settings)
            .song_select
            .background_dim as f32;
        let details = self.beatmap_details(now_ns);
        self.song_select.set_details(details);
        // The main menu shows the menu music's beatmap or the skin's background
        // (ADR-0119); the other screens the song select's selection.
        let live_general = &self.editing.as_ref().unwrap_or(&self.settings).general;
        let menu_background = live_general.menu_background;
        // Transitions started from now on; one running keeps its style (ADR-0126).
        self.stack.set_style(live_general.transitions);
        let on_menu = matches!(self.stack.top(), AppScreen::Menu(_));
        snapshot.menu_background = menu_background != MenuBackground::None;
        snapshot.parallax = if live_general.parallax_enabled {
            live_general.parallax as f32
        } else {
            0.0
        };
        if on_menu {
            snapshot.background_dim = self.theme.menu_background_dim;
        }
        let skin = match menu_background {
            MenuBackground::Skin if on_menu => Some(self.menu_skin().cloned()),
            _ => None,
        };
        if let Some(backgrounds) = &mut self.backgrounds {
            let source = background_source(
                on_menu,
                menu_background,
                &self.files,
                self.menu_track,
                self.song_select.selected(),
                skin.as_ref().and_then(Option::as_ref),
            );
            snapshot.background = backgrounds.tick_source(now_ns, source);
        }
        snapshot.fx = self.stack.fx(now_ns);
        self.tick_thumbnails(&mut snapshot.thumbnails);
        // The cursor's style as edited in the settings screen counts at once (ADR-0124).
        let cursor_style = self
            .editing
            .as_ref()
            .unwrap_or(&self.settings)
            .general
            .menu_cursor;
        let cursor_skin = (cursor_style == CursorStyle::Skin).then(|| self.menu_skin().cloned());
        snapshot.menu_cursor.style = cursor_style;
        snapshot.menu_cursor.skin = match &mut self.skin_cursor {
            Some(link) => {
                let skin = link.tick(cursor_skin.as_ref().map(Option::as_ref));
                snapshot.menu_cursor.seq = link.seq();
                skin
            }
            None => Default::default(),
        };
        let top: *const AppScreen = self.stack.top();
        for (slot, layer) in snapshot.layers.iter_mut().zip(self.stack.layers(now_ns)) {
            *slot = layer.and_then(|(screen, pose)| {
                let view = match screen {
                    AppScreen::Menu(menu) => ScreenView::Menu(menu.snapshot()),
                    AppScreen::SongSelect => ScreenView::SongSelect(self.song_select.snapshot()),
                    AppScreen::Results(results) => ScreenView::Results(results.snapshot()),
                    AppScreen::Play(id) if std::ptr::eq(screen, top) => ScreenView::Play(*id),
                    AppScreen::Play(_) => return None,
                };
                Some(LayerSnapshot { view, pose })
            });
        }
        if self
            .panel
            .as_ref()
            .is_some_and(|p| p.motion.is_gone(now_ns))
        {
            // Navigation: the screen goes with the panel, the place in its list stays.
            if let Some(panel) = self.panel.take() {
                self.settings_position = panel.screen.position();
            }
        }
        let layout = self.settings_layout;
        if let Some(panel) = &mut self.panel {
            // The list scrolls smoothly (ADR-0133).
            panel.screen.tick(now_ns, &layout);
        }
        snapshot.panel = self.panel.as_ref().map(|p| PanelView {
            settings: p.screen.snapshot(),
            openness: p.motion.openness(now_ns),
        });
        navigated
    }
}

/// The input of a screen: the song select's set, plus the key of the results and the two
/// only the settings use.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ScreenInput {
    Common(SongSelectInput),
    /// An OS repeat of a held key that steps or walks (ADR-0092).
    Repeat(SongSelectInput),
    Retry,
    /// The Tab key.
    Tab,
    /// The primary button released.
    Release,
    /// Any key or mouse button, while a key row listens.
    Capture(KeyValue),
    /// Delete, while a key row listens.
    Clear,
    /// Ctrl+Up/Down on the song select: the mod speed by this much.
    ModSpeed(f64),
    /// Ctrl+F3/F4 on the song select: the approach rate by this much.
    ApproachRate(f64),
    /// The FPS counter's hotkey, on any screen (ADR-0060).
    ToggleFps,
    /// The settings panel's hotkey (ADR-0095).
    ToggleSettings,
}

impl ScreenInput {
    fn common(self) -> Option<SongSelectInput> {
        match self {
            ScreenInput::Common(input) | ScreenInput::Repeat(input) => Some(input),
            ScreenInput::Release => Some(SongSelectInput::PointerRelease),
            _ => None,
        }
    }
}

/// Makes the edited settings the ones in force, and asks for them to be saved, if they
/// differ. The sliders' values are rounded to their steps first.
fn commit_settings(
    settings: &mut Settings,
    editing: &mut Option<Settings>,
    save_request: &mut Option<Box<Settings>>,
) -> bool {
    let Some(mut edited) = editing.take() else {
        return false;
    };
    // The songs folder is the library's: it may have changed while the screen was open.
    edited.paths.songs.clone_from(&settings.paths.songs);
    round_for_save(&mut edited);
    if edited == *settings {
        return false;
    }
    *settings = edited.clone();
    *save_request = Some(Box::new(edited));
    true
}

/// The input as the settings screen understands it.
fn settings_input(input: ScreenInput) -> Option<SettingsInput> {
    Some(match input {
        ScreenInput::Tab => SettingsInput::NextSection,
        ScreenInput::Release => SettingsInput::PointerRelease,
        ScreenInput::Capture(key) => SettingsInput::Captured(key),
        ScreenInput::Clear => SettingsInput::Clear,
        ScreenInput::Retry
        | ScreenInput::ModSpeed(_)
        | ScreenInput::ApproachRate(_)
        | ScreenInput::ToggleFps
        | ScreenInput::ToggleSettings => {
            return None;
        }
        ScreenInput::Common(input) => match input {
            SongSelectInput::Up => SettingsInput::Up,
            SongSelectInput::Down => SettingsInput::Down,
            SongSelectInput::Left => SettingsInput::Left,
            SongSelectInput::Right => SettingsInput::Right,
            SongSelectInput::PageUp => SettingsInput::PrevSection,
            SongSelectInput::PageDown => SettingsInput::NextSection,
            SongSelectInput::Activate => SettingsInput::Activate,
            SongSelectInput::Back => SettingsInput::Back,
            SongSelectInput::PointerMove { x, y } => SettingsInput::PointerMove { x, y },
            SongSelectInput::PointerPress { x, y } => SettingsInput::PointerPress { x, y },
            SongSelectInput::Scroll { lines } => SettingsInput::Scroll { lines },
            // Backspace, as the song select's search takes it.
            SongSelectInput::DeselectMods => SettingsInput::Erase,
            _ => return None,
        },
        ScreenInput::Repeat(input) => match input {
            SongSelectInput::Up => SettingsInput::Up,
            SongSelectInput::Down => SettingsInput::Down,
            SongSelectInput::Left => SettingsInput::Left,
            SongSelectInput::Right => SettingsInput::Right,
            SongSelectInput::PageUp => SettingsInput::PrevSection,
            SongSelectInput::PageDown => SettingsInput::NextSection,
            SongSelectInput::Erase => SettingsInput::Erase,
            _ => return None,
        },
    })
}

/// Whether the play screen goes: the player quit, or the results show (a second after
/// passing, as lazer's `progressToResults`).
/// The settings panel over the screens (ADR-0095).
struct Panel {
    screen: Box<SettingsScreen>,
    motion: PanelMotion,
    /// The primary button went down outside the panel: its release outside closes it, as
    /// lazer's `OsuFocusedOverlayContainer`.
    pressed_outside: bool,
}

/// The bit of [`ShellState::modifiers`] for a modifier key's scancode.
fn modifier_bit(scancode: u16) -> Option<u8> {
    Some(match scancode {
        KEY_LEFT_CTRL => 1,
        KEY_RIGHT_CTRL => 2,
        KEY_LEFT_SHIFT => 4,
        KEY_RIGHT_SHIFT => 8,
        KEY_LEFT_ALT => 16,
        KEY_RIGHT_ALT => 32,
        _ => return None,
    })
}

/// Notes in `modifiers` a modifier key going down or up.
fn track(modifiers: &mut u8, kind: InputKind) {
    if let InputKind::Key { scancode, down } = kind
        && let Some(bit) = modifier_bit(scancode)
    {
        if down {
            *modifiers |= bit;
        } else {
            *modifiers &= !bit;
        }
    }
}

/// The modifiers held in `modifiers`, left and right as one.
fn mods_of(modifiers: u8) -> Mods {
    let mut mods = Mods::NONE;
    for (bits, m) in [(3, Mods::CTRL), (12, Mods::SHIFT), (48, Mods::ALT)] {
        if modifiers & bits != 0 {
            mods = mods.with(m);
        }
    }
    mods
}

/// The hotkey of `settings` that `kind` presses with `modifiers` held: the modifiers must
/// be exactly the hotkey's (ADR-0095).
fn hotkey_of(settings: &Settings, modifiers: u8, kind: InputKind) -> Option<KeySlot> {
    let input = match kind {
        InputKind::Key {
            scancode,
            down: true,
        } if modifier_bit(scancode).is_none() => Input::Key(scancode),
        InputKind::MouseButton { button, down: true } => Input::Mouse(button),
        _ => return None,
    };
    settings.hotkey(Combo {
        mods: mods_of(modifiers),
        input,
    })
}

fn play_over(state: PlayState) -> bool {
    matches!(state, PlayState::Results | PlayState::Exited)
}

/// Where the player goes when a play ends: the results of a passed play replace it, a
/// failed or quit play is popped back to the song select.
fn after_play(results: Option<ResultsData>) -> Nav<AppScreen> {
    match results {
        Some(data) => Nav::Replace(AppScreen::Results(Results::new(data))),
        None => Nav::Pop,
    }
}

/// The input as the results understand it.
fn results_input(input: ScreenInput) -> Option<ResultsInput> {
    Some(match input {
        ScreenInput::Tab
        | ScreenInput::Release
        | ScreenInput::Capture(_)
        | ScreenInput::Clear
        | ScreenInput::ModSpeed(_)
        | ScreenInput::ApproachRate(_)
        | ScreenInput::ToggleFps
        | ScreenInput::ToggleSettings => {
            return None;
        }
        ScreenInput::Retry => ResultsInput::Retry,
        ScreenInput::Common(SongSelectInput::Back) => ResultsInput::Back,
        ScreenInput::Common(SongSelectInput::PointerMove { x, y }) => {
            ResultsInput::PointerMove { x, y }
        }
        ScreenInput::Common(SongSelectInput::PointerPress { x, y }) => {
            ResultsInput::PointerPress { x, y }
        }
        ScreenInput::Common(_) | ScreenInput::Repeat(_) => return None,
    })
}

/// The song select's input as the main menu understands it.
fn menu_input(input: SongSelectInput) -> Option<MenuInput> {
    Some(match input {
        SongSelectInput::Up => MenuInput::Up,
        SongSelectInput::Down => MenuInput::Down,
        SongSelectInput::Activate => MenuInput::Activate,
        SongSelectInput::Back => MenuInput::Back,
        SongSelectInput::PointerMove { x, y } => MenuInput::PointerMove { x, y },
        SongSelectInput::PointerPress { x, y } => MenuInput::PointerPress { x, y },
        _ => return None,
    })
}

/// The window's audio: one output at a time (ADR-0045, ADR-0089).
struct Audio {
    /// The play's track, playing on under the results.
    player: MusicPlayer,
    /// The menus' two decks (ADR-0090).
    previews: [MusicPlayer; 2],
    mixer: MixerHandle,
    output: Arc<AudioPosition>,
    music: Arc<MusicPosition>,
    /// The gains `mixer` and `player` were last given.
    volumes: AppliedVolumes,
    /// Rate of the mixer, the tracks and the output (ADR-0102).
    sample_rate: u32,
}

/// How long a play's track may take to open.
const TRACK_TIMEOUT_NS: u64 = 10_000_000_000;
/// How long the mixer must have had no voice before a quit play's samples may go: longer
/// than any output buffer, so that it has taken every command queued before the stop.
const VOICES_IDLE_NS: u64 = 100_000_000;

/// Where the play the player asked for is.
enum Pending {
    Idle,
    /// The loader prepares the play of this request.
    Preparing(u64),
    /// Play `id` is prepared; its track of `generation` is opening since `since_ns`.
    Track {
        id: u64,
        prepared: Box<Prepared>,
        generation: u32,
        since_ns: u64,
    },
}

/// How far the play on the stack is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    Playing,
    /// The player quit: its looping sounds are stopping.
    Sounds,
}

/// A play on the stack.
struct Game {
    session: Box<PlaySession>,
    ending: Ending,
    /// The play's id, and what its results show besides the score; made when it starts.
    id: u64,
    title: String,
    mods: ResultsMods,
    /// The beatmap file played: its offset is kept when the play ends.
    map: Option<PathBuf>,
    /// What the replays of the play's passed attempts name its beatmap by.
    beatmap: Arc<ReplayBeatmap>,
    /// What the play was prepared with; `None` when not known.
    options: Option<PlayOptions>,
    /// The display sizes of the play's skin's stable pause buttons, if it has them all
    /// (ADR-0122).
    stable_pause: Option<[[f32; 2]; 3]>,
}

/// Lays the pause and fail menus of `session` out as `(style, scale, scheme)` say
/// (ADR-0122): stable's style without the skin's buttons is SLAM's.
fn style_menu(
    session: &mut PlaySession,
    (style, scale, scheme): (
        crate::settings::PauseMenuStyle,
        f32,
        slam_ui::theme::ColourScheme,
    ),
    stable_pause: Option<[[f32; 2]; 3]>,
) {
    let sizes = stable_pause.unwrap_or_default();
    session.set_menu_style(menu_style(style, stable_pause.is_some()), scheme, sizes);
    session.set_menu_scale(scale);
}

/// The style the pause and fail menus show for the setting `style` (ADR-0122).
fn menu_style(
    style: crate::settings::PauseMenuStyle,
    stable_pause: bool,
) -> slam_play::menu_overlay::MenuStyle {
    use crate::settings::PauseMenuStyle;
    use slam_play::menu_overlay::MenuStyle;
    match style {
        PauseMenuStyle::Slam => MenuStyle::Slam,
        PauseMenuStyle::Lazer => MenuStyle::Lazer,
        PauseMenuStyle::Stable if stable_pause => MenuStyle::Stable,
        PauseMenuStyle::Stable => MenuStyle::Slam,
    }
}

/// The play prepared again with changed settings, to take its scene and samples (ADR-0096).
enum Reskin {
    Idle,
    /// Asked of the loader as this request, with these options, for the play of this id.
    Preparing(u64, PlayOptions, u64),
    /// Ready for the play of this id, waiting for it to show its pause or fail menu.
    Ready(Box<Prepared>, PlayOptions, u64),
}

/// A quit play whose voices the mixer silences. The session keeps its samples until the
/// mixer has handed all of theirs back, so that the update thread never drops the last
/// reference to one.
struct Retiring {
    session: Box<PlaySession>,
    /// Since when the mixer has had no voice.
    idle_since: Option<u64>,
}

impl Retiring {
    /// What the loader takes to free the session.
    fn into_trash(self) -> Trash {
        Trash::Session(self.session)
    }
}

/// The update side of plays: loads them, runs the one on top, frees them via the loader.
struct PlayHost {
    audio: Audio,
    /// The `slam-audio-output` thread, which owns the output (ADR-0089); `None` in tests
    /// and when it did not start.
    output: Option<OutputLink<Audio>>,
    /// The output settings last asked of the output thread.
    requested: crate::settings::Audio,
    /// An output is opening: `audio` is silent, and no play starts until it comes.
    switching: bool,
    /// Neither the asked nor the previous output opened: `audio` is silent, and no play
    /// starts until an output opens.
    no_output: bool,
    /// Old handles the output thread has not taken yet, offered again every tick.
    retired: Option<Box<Audio>>,
    /// The track `audio.player` last loaded, to play on in a new output.
    track: Option<PathBuf>,
    /// The music of the menus and the song select (ADR-0090).
    menu_music: MenuMusic,
    /// The song select's selection and since when, to wait for it to settle.
    selection: (Option<(usize, usize)>, u64),
    /// Picks the menu's random tracks (xorshift).
    rng: u64,
    /// The beatmap whose music plays in the menus, by set and difficulty (ADR-0119).
    music_at: Option<(usize, usize)>,
    /// Plays the menus' sounds (ADR-0120).
    ui_sounds: ui_sounds::UiSoundPlayer,
    loader: LoaderLink,
    /// Scenes for the draw thread, one per play, named by the play's id.
    scenes: rtrb::Producer<(u64, PlayScene)>,
    /// Blocks the Windows key while playing (ADR-0094).
    win_key: WinKeyBlock,
    /// The play showed its pause or fail menu at its last tick (ADR-0095).
    menu_shown: bool,
    pending: Pending,
    game: Option<Game>,
    /// Apart from `game`: a mixer that stops taking commands (lost device) keeps this
    /// session to the end, not the next play from starting.
    retiring: Option<Retiring>,
    /// The track still has to be given its plain sound after a play.
    track_reset: bool,
    next_request: u64,
    /// The options of the play asked of the loader, as request id.
    play_requested: Option<(u64, PlayOptions)>,
    /// The running play prepared again with changed settings (ADR-0096).
    reskin: Reskin,
    /// The window in points, as gameplay input takes it.
    points: [f32; 2],
    input: PlayInput,
    /// Beatmap offsets the hotkeys set, by beatmap file, until the app quits (stored
    /// with the library in #175).
    beatmap_offsets: HashMap<PathBuf, f64>,
    /// Formats the texts of a play's results, which hold numbers.
    localizer: Arc<Localizer>,
    /// The startup intro's sound (ADR-0114); `None` with the intro off. Kept for good, so
    /// that the audio thread never drops the last reference.
    intro_sound: Option<Arc<Sample>>,
    /// The voice playing `intro_sound`, or [`VoiceId::NONE`].
    intro_voice: VoiceId,
    /// The pause and fail menus' style, scale and colour scheme (ADR-0122).
    pause_menu: (
        crate::settings::PauseMenuStyle,
        f32,
        slam_ui::theme::ColourScheme,
    ),
    /// Whether the interface effects are on (ADR-0123).
    effects: bool,
}

/// What a play takes from the settings on the update thread.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PlayInput {
    bindings: Bindings,
    /// Mouse buttons press in gameplay (lazer's `MouseDisableButtons` off).
    mouse_buttons: bool,
    offsets: Offsets,
    /// Block the Windows key while playing (lazer's `GameplayDisableWinKey`, ADR-0094);
    /// applies at once, not from the next play.
    disable_win_key: bool,
}

impl PlayInput {
    fn new(settings: &Settings) -> PlayInput {
        PlayInput {
            bindings: settings.bindings(),
            mouse_buttons: settings.gameplay.mouse_buttons,
            offsets: Offsets {
                global_ms: settings.timing.global_offset_ms,
                // Each play takes its beatmap's ([`PlayHost::beatmap_offsets`]).
                beatmap_ms: 0.0,
            },
            disable_win_key: settings.gameplay.disable_win_key,
        }
    }
}

/// Whether the Windows key is blocked in `state`: while lazer's player is
/// `LocalUserPlayingState.Playing`, so not paused, resuming, in a break, failed or passed.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (updateGameplayState)
fn blocks_win_key(state: PlayState, is_break_time: bool) -> bool {
    matches!(state, PlayState::Waiting | PlayState::Playing) && !is_break_time
}

impl PlayHost {
    /// Whether the play shows its pause or fail menu, over which the settings panel may
    /// open (ADR-0095).
    fn menu_shown(&self) -> bool {
        self.game.is_some() && self.menu_shown
    }

    /// Blocks the Windows key as the play's state asks.
    fn follow_play(&mut self, snapshot: &PlaySnapshot) {
        self.menu_shown = matches!(snapshot.state, PlayState::Paused | PlayState::Failed);
        self.win_key.set_blocked(
            self.input.disable_win_key && blocks_win_key(snapshot.state, snapshot.is_break_time),
        );
    }

    /// Lets the play on top follow `events` the settings panel takes over its menu.
    fn follow_covered(&mut self, events: &[InputEvent]) {
        if let Some(game) = self.game.as_mut()
            && game.ending == Ending::Playing
        {
            game.session.follow_covered(events);
        }
    }

    /// Runs the play on top with `events`.
    fn tick_game(&mut self, now_ns: u64, events: &[InputEvent], snapshot: &mut PlaySnapshot) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        if game.ending != Ending::Playing {
            return;
        }
        let audio = &mut self.audio;
        let (output, music) = (audio.output.read(), audio.music.read());
        game.session.tick(
            now_ns,
            events,
            output,
            music,
            &mut audio.player,
            &mut audio.mixer,
            snapshot,
        );
        // A passed attempt's replay is saved as it passes, as lazer imports the score on
        // completion: quitting before the results or retrying keeps it. A failed one comes
        // when the fail menu's button saves it (ADR-0084).
        if let Some(attempt) = game.session.take_finished_attempt() {
            self.loader.free(Trash::Replay(ReplayJob {
                attempt,
                beatmap: Arc::clone(&game.beatmap),
                ended: SystemTime::now(),
            }));
        }
        if play_over(snapshot.state) {
            game.ending = Ending::Sounds;
        }
        self.follow_play(snapshot);
    }

    /// The window is `width` × `height` points.
    fn set_size(&mut self, width: f32, height: f32) {
        self.points = [width, height];
        if let Some(game) = &mut self.game {
            game.session.set_layout(PlayfieldLayout::new(width, height));
        }
    }

    /// Hands changed settings to the audio thread and to the next play (ADR-0048). Does
    /// not block; allocates only to ask for another output (ADR-0089).
    fn apply_settings(&mut self, settings: &Settings) {
        self.pause_menu = (
            settings.gameplay.pause_menu_style,
            settings.gameplay.pause_menu_scale,
            settings.general.colour_scheme,
        );
        self.effects = settings.general.effects;
        if let Some(game) = &mut self.game {
            style_menu(&mut game.session, self.pause_menu, game.stable_pause);
            game.session.set_menu_effects(self.effects);
        }
        let audio = &mut self.audio;
        audio.volumes.sync(
            Volumes::of(&settings.audio),
            &mut audio.mixer,
            &mut audio.player,
            &mut audio.previews,
        );
        // One output at a time, and none under a play: its clock follows the output.
        if !self.switching
            && self.game.is_none()
            && output::output_changed(&settings.audio, &self.requested)
            && let Some(link) = &mut self.output
        {
            // Allocates, as the player closed the settings screen; a full ring asks again
            // on the next tick.
            if link
                .request(Request::Open(Box::new(settings.audio.clone())))
                .is_ok()
            {
                self.requested.clone_from(&settings.audio);
                self.switching = true;
            }
        }
        // A play copies these when it starts (ADR-0049 for the bindings).
        self.input.bindings = settings.input.bindings;
        self.input.offsets.global_ms = settings.timing.global_offset_ms;
        self.input.mouse_buttons = settings.gameplay.mouse_buttons;
        self.input.disable_win_key = settings.gameplay.disable_win_key;
    }

    /// Takes requests, prepared plays and track events; ends a quit play. Allocates only
    /// when a play is asked for, starts or ends; logs and frees on the loader thread.
    fn advance(&mut self, now_ns: u64, state: &mut ShellState) {
        self.end_game(now_ns, state);
        self.switch_output();
        if !self.no_output {
            state.set_no_audio(false);
        }
        self.intro_sound(state);
        self.menu_music(now_ns, state);
        state.set_menu_track(self.music_at);
        let noted = state.take_ui_sounds();
        let mode = state.live_settings().audio.menu_sounds;
        let skin = state.menu_skin().cloned();
        self.ui_sounds.follow_skin(now_ns, mode, skin.as_ref());
        self.ui_sounds
            .play(now_ns, noted, mode, &mut self.audio.mixer);
        if let Some(link) = &mut self.output {
            if let Some(settings) = state.take_device_list_request() {
                // A full ring drops it: the screen keeps the list it has.
                let _ = link.request(Request::List(Box::new(settings.clone())));
            }
            if let Some(mut list) = link.list() {
                state.set_devices(&mut list);
            }
        }
        // Tried until the queue takes it, while no play owns the track.
        if self.track_reset
            && self.game.is_none()
            && slam_play::fail::reset_track(&mut self.audio.player).is_ok()
        {
            self.track_reset = false;
        }
        if state.take_track_stop() {
            // A failed command leaves the track playing under the song select.
            let _ = self.audio.player.pause();
            // The results go: the track loses the play's mods.
            self.track_reset = true;
        }
        // From the song select, or the results playing their beatmap again.
        let request = match state.take_play_request() {
            Some((set, difficulty)) => Some(state.file(set, difficulty).cloned()),
            None => state
                .take_retry_request()
                .then(|| state.last_file().cloned()),
        };
        if let Some(map) = request {
            self.next_request += 1;
            let asked = match map {
                // A play started on a closed output would never hear its track.
                Some(map) if self.game.is_none() && !self.switching && !self.no_output => {
                    let options = PlayOptions {
                        mods: state.selected_mods().clone(),
                        ..live::play_options(state.settings())
                    };
                    self.play_requested = Some((self.next_request, options.clone()));
                    self.loader
                        .request(self.next_request, map, options, self.audio.sample_rate)
                }
                Some(_) if self.no_output => {
                    tracing::warn!("play: not started, no audio output is open");
                    state.set_no_audio(true);
                    false
                }
                Some(_) if self.switching => {
                    tracing::warn!("play: not started, the audio output is being switched");
                    false
                }
                _ => false,
            };
            if asked {
                self.free_pending();
                self.pending = Pending::Preparing(self.next_request);
            } else {
                state.finish_loading(now_ns, None);
            }
        }
        self.ask_reskin(state);
        let loaded = match self.loader.poll() {
            Some(loaded) if matches!(self.reskin, Reskin::Preparing(id, ..) if id == loaded.id) => {
                self.take_reskin(loaded);
                None
            }
            other => other,
        };
        self.apply_reskin();
        if let Some(loaded) = loaded {
            let current = matches!(self.pending, Pending::Preparing(id) if id == loaded.id);
            match loaded.result {
                Ok(mut prepared) if current && state.is_loading() => {
                    let audio = std::mem::take(&mut prepared.audio);
                    let track = audio.clone();
                    match self.audio.player.load(TrackSource::File(audio)) {
                        Ok(generation) => {
                            self.track = Some(track);
                            self.pending = Pending::Track {
                                id: loaded.id,
                                prepared,
                                generation,
                                since_ns: now_ns,
                            }
                        }
                        Err(error) => {
                            self.loader.free(Trash::Prepared(prepared));
                            self.loader.free(Trash::TrackCommand(error));
                            self.pending = Pending::Idle;
                            state.finish_loading(now_ns, None);
                        }
                    }
                }
                // Cancelled, or an older request's.
                Ok(prepared) => {
                    self.loader.free(Trash::Prepared(prepared));
                    if current {
                        self.pending = Pending::Idle;
                    }
                }
                // The loader logged it.
                Err(message) => {
                    self.loader.free(Trash::Message(message));
                    if current {
                        self.pending = Pending::Idle;
                        if state.is_loading() {
                            state.finish_loading(now_ns, None);
                        }
                    }
                }
            }
        }
        if let Pending::Track {
            id,
            generation,
            since_ns,
            ..
        } = self.pending
        {
            if !state.is_loading() {
                self.free_pending();
                return;
            }
            match self.audio.player.poll_event() {
                Some(MusicEvent::Loaded { generation: g, .. }) if g == generation => {
                    if self.scenes.slots() == 0 {
                        // The draw thread has not taken the last scene (a reskin's of the
                        // play before, ADR-0096): a play without a scene must not start.
                        self.free_pending();
                        state.finish_loading(now_ns, None);
                    } else if let Pending::Track { prepared, .. } =
                        std::mem::replace(&mut self.pending, Pending::Idle)
                    {
                        let title = state.last_title().to_owned();
                        let map = state.last_file().cloned();
                        self.start(id, *prepared, title, map);
                        state.finish_loading(now_ns, Some(id));
                    }
                }
                Some(event @ MusicEvent::Failed { generation: g, .. }) => {
                    self.loader.free(Trash::Music(event));
                    if g == generation {
                        self.free_pending();
                        state.finish_loading(now_ns, None);
                    }
                }
                Some(MusicEvent::Loaded { .. }) => {}
                None if now_ns.saturating_sub(since_ns) > TRACK_TIMEOUT_NS => {
                    self.free_pending();
                    state.finish_loading(now_ns, None);
                }
                None => {}
            }
        }
    }

    /// Asks the loader for the running play again when the settings panel closed with
    /// changes to what it was prepared with (ADR-0096). Allocates, on that event only.
    fn ask_reskin(&mut self, state: &mut ShellState) {
        if !state.take_settings_changed() {
            return;
        }
        let Some(game) = &self.game else {
            return;
        };
        let (Some(map), Some(loaded_with)) = (&game.map, &game.options) else {
            return;
        };
        if game.ending != Ending::Playing {
            return;
        }
        let options = PlayOptions {
            mods: loaded_with.mods.clone(),
            ..live::play_options(state.settings())
        };
        if options == *loaded_with {
            // Back to the play's own look: what was coming or waiting is not wanted. A
            // result still coming finds no request and is freed.
            if let Reskin::Ready(prepared, ..) = std::mem::replace(&mut self.reskin, Reskin::Idle) {
                self.loader.free(Trash::Prepared(prepared));
            }
            return;
        }
        if let Reskin::Preparing(_, wanted, _) | Reskin::Ready(_, wanted, _) = &self.reskin
            && options == *wanted
        {
            return;
        }
        self.next_request += 1;
        if !self.loader.request(
            self.next_request,
            map.clone(),
            options.clone(),
            self.audio.sample_rate,
        ) {
            // The loader's queue is full: asked again on the next tick.
            state.settings_changed = true;
        } else {
            // A newer look wins over one still coming or waiting.
            let id = game.id;
            if let Reskin::Ready(prepared, ..) = std::mem::replace(
                &mut self.reskin,
                Reskin::Preparing(self.next_request, options, id),
            ) {
                self.loader.free(Trash::Prepared(prepared));
            }
        }
    }

    /// Keeps the play prepared again, for [`PlayHost::apply_reskin`].
    fn take_reskin(&mut self, loaded: crate::loader::LoadResult) {
        let Reskin::Preparing(_, options, id) = std::mem::replace(&mut self.reskin, Reskin::Idle)
        else {
            return;
        };
        match loaded.result {
            // The same beatmap only: a file changed on disk would not match the session's
            // objects.
            Ok(prepared)
                if self
                    .game
                    .as_ref()
                    .is_some_and(|g| g.id == id && g.beatmap.hash == prepared.beatmap_hash) =>
            {
                self.reskin = Reskin::Ready(prepared, options, id);
            }
            Ok(prepared) => self.loader.free(Trash::Prepared(prepared)),
            // The loader logged it; the play keeps its look.
            Err(message) => self.loader.free(Trash::Message(message)),
        }
    }

    /// Gives the running play the scene and samples prepared again, once it shows its
    /// pause or fail menu (ADR-0096); the rest goes back to the loader to free. Allocates
    /// as the scene goes to the draw thread, on that event only.
    fn apply_reskin(&mut self) {
        let stale = matches!(&self.reskin, Reskin::Ready(_, _, id)
            if self.game.as_ref().is_none_or(|g| g.id != *id));
        // The scene and the samples change together: without room for the scene, neither
        // does until the draw thread takes the last one.
        if !matches!(self.reskin, Reskin::Ready(..))
            || (!stale && (!self.menu_shown() || self.scenes.slots() == 0))
        {
            return;
        }
        let Reskin::Ready(prepared, options, id) =
            std::mem::replace(&mut self.reskin, Reskin::Idle)
        else {
            return;
        };
        let Some(game) = self.game.as_mut().filter(|g| g.id == id) else {
            self.loader.free(Trash::Prepared(prepared));
            return;
        };
        let Prepared {
            beatmap,
            mods,
            audio,
            mut sounds,
            scene,
            background_name,
            beatmap_hash,
        } = *prepared;
        // A full mixer queue leaves a looping voice to the pause's own stops.
        let _ = game
            .session
            .replace_samples(&mut sounds, &mut self.audio.mixer);
        game.stable_pause = scene.stable_pause_sizes();
        style_menu(&mut game.session, self.pause_menu, game.stable_pause);
        if let Err(rtrb::PushError::Full((_, scene))) = self.scenes.push((game.id, scene)) {
            // Room was checked above; the update thread is the only producer.
            self.loader.free(Trash::Other(Box::new(scene)));
        }
        game.options = Some(options);
        self.loader.free(Trash::Other(Box::new((
            beatmap,
            mods,
            audio,
            sounds,
            background_name,
            beatmap_hash,
        ))));
    }

    /// Moves a quit play along: its sounds stop and its screen goes; once the mixer has
    /// let go of its samples, the loader frees the session.
    fn end_game(&mut self, now_ns: u64, state: &mut ShellState) {
        let mixer = &mut self.audio.mixer;
        if let Some(game) = &mut self.game
            && game.ending == Ending::Sounds
            // A full queue retries on the next tick.
            && game.session.stop_sounds(mixer)
            && mixer.stop_all().is_ok()
            && let Some(mut game) = self.game.take()
        {
            self.win_key.set_blocked(false);
            // The results come from the session before it retires.
            let results = game.session.results().map(|r| {
                results::results_data(
                    game.id,
                    std::mem::take(&mut game.title),
                    game.mods,
                    &r,
                    &self.localizer,
                )
            });
            // The track plays on under the results, as in lazer; a failed command leaves it
            // playing under the song select.
            if results.is_none() {
                let _ = self.audio.player.pause();
            }
            // A failed play leaves the track stopped and filtered, a play with rate mods
            // sped up; the next play resets it too, but the song select may play it before.
            // Under the results the track keeps the mods until they go, as lazer's
            // `Player.OnExiting` resets it (ADR-0055).
            self.track_reset = results.is_none();
            if let Some(map) = game.map.take() {
                self.beatmap_offsets
                    .insert(map, game.session.beatmap_offset());
            }
            state.leave_play(now_ns, results);
            // Two quits in a row on a stuck mixer: the older session goes anyway.
            if let Some(old) = self.retiring.replace(Retiring {
                session: game.session,
                idle_since: None,
            }) {
                self.loader.free(old.into_trash());
            }
        }
        if let Some(retiring) = &mut self.retiring {
            // The session still holds every sample: these only drop references.
            mixer.collect();
            if mixer.stats().active_voices.load(Ordering::Acquire) == 0 {
                let since = *retiring.idle_since.get_or_insert(now_ns);
                if now_ns.saturating_sub(since) >= VOICES_IDLE_NS {
                    mixer.collect();
                    if let Some(retiring) = self.retiring.take() {
                        self.loader.free(retiring.into_trash());
                    }
                }
            } else {
                retiring.idle_since = None;
            }
        }
    }

    /// Takes the output the output thread opened, once no play, play's preparation or
    /// play's track waits on the old one (ADR-0089; the preparation's samples are at the old
    /// output's rate, ADR-0102): the track plays on from where the old output left it, with its
    /// rate. The old handles go to the output thread to be freed: their player joins its
    /// decoder. Allocates only then.
    fn switch_output(&mut self) {
        let Some(link) = &mut self.output else {
            return;
        };
        if let Some(old) = self.retired.take()
            && let Err(Request::Retire(old)) = link.request(Request::Retire(old))
        {
            self.retired = Some(old);
        }
        if !self.switching
            || self.retired.is_some()
            || self.game.is_some()
            || matches!(self.pending, Pending::Preparing(_) | Pending::Track { .. })
        {
            return;
        }
        let mut new = match link.opened() {
            None => return,
            Some(output::Opened::Lost) => {
                self.switching = false;
                self.no_output = true;
                return;
            }
            Some(output::Opened::Asked(new) | output::Opened::Previous(new)) => new,
        };
        self.switching = false;
        self.no_output = false;
        let music = self
            .audio
            .music
            .read()
            .filter(|m| m.generation == self.audio.player.generation());
        let resume = output::resume_point(self.audio.output.read(), music, new.sample_rate);
        if let (Some(track), Some((frame, playing))) = (&self.track, resume) {
            let player = &mut new.player;
            // A fresh queue takes these; a failure leaves the track stopped.
            let _ = player.set_rate(self.audio.player.rate());
            let _ = player
                .load(TrackSource::File(track.clone()))
                .and_then(|_| player.seek(frame));
            if playing {
                let _ = player.play();
            }
        }
        std::mem::swap(&mut self.audio, &mut new);
        // Voice ids count anew in the new mixer.
        self.intro_voice = VoiceId::NONE;
        // The new decks start from nothing: the menu music starts over.
        self.menu_music = MenuMusic::default();
        if let Err(Request::Retire(old)) = link.request(Request::Retire(new)) {
            self.retired = Some(old);
        }
    }

    /// Plays the startup intro's sound when it starts and fades it out with a skip
    /// (ADR-0114). A full queue loses the sound: the intro goes on silent.
    fn intro_sound(&mut self, state: &mut ShellState) {
        let events = state.take_intro_events();
        if self.no_output {
            return;
        }
        if events.started
            && let Some(sample) = &self.intro_sound
        {
            let params = VoiceParams {
                group: VoiceGroup::Effects,
                ..VoiceParams::default()
            };
            self.intro_voice = self
                .audio
                .mixer
                .play_voice(sample, params)
                .unwrap_or(VoiceId::NONE);
        }
        if events.skipped && self.intro_voice != VoiceId::NONE {
            let ramp_ms = slam_ui::intro::SKIP_NS as f64 / 1e6;
            let _ = self
                .audio
                .mixer
                .set_voice_gain(self.intro_voice, 0.0, ramp_ms);
        }
    }

    /// Plays the menus' music the screens call for (ADR-0090). Allocates only when another
    /// track starts; frees failed tracks' messages on the loader thread.
    fn menu_music(&mut self, now_ns: u64, state: &ShellState) {
        // Without an output nothing takes the decks' commands: they would only fill up.
        if self.no_output {
            return;
        }
        for deck in 0..2 {
            while let Some(event) = self.audio.previews[deck].poll_event() {
                match event {
                    MusicEvent::Loaded { generation, length } => {
                        self.menu_music
                            .loaded(deck, generation, length, &mut self.audio.previews);
                    }
                    MusicEvent::Failed { generation, .. } => {
                        self.menu_music
                            .failed(now_ns, deck, generation, &mut self.audio.previews);
                        self.loader.free(Trash::Music(event));
                    }
                }
            }
        }
        let selected = state.selected();
        if selected != self.selection.0 {
            self.selection = (selected, now_ns);
        }
        let settled = now_ns.saturating_sub(self.selection.1) >= SELECTION_DEBOUNCE_NS;
        let gameplay =
            self.game.is_some() || !matches!(self.pending, Pending::Idle) || state.is_loading();
        let want =
            if !state.settings().audio.menu_music || gameplay || state.intro_holds_music(now_ns) {
                Want::Silence
            } else {
                match state.stack().top() {
                    AppScreen::Play(_) | AppScreen::Results(_) => Want::Silence,
                    AppScreen::SongSelect => selected
                        .filter(|_| settled)
                        .and_then(|(set, difficulty)| state.files().get(set)?.get(difficulty))
                        .and_then(|file| {
                            Some(Want::Preview {
                                path: file.audio.as_deref()?,
                                preview_ms: file.preview_ms,
                            })
                        })
                        .unwrap_or(Want::Keep),
                    AppScreen::Menu(_) => Want::Menu,
                }
            };
        // The background behind the menu follows the music (ADR-0119).
        if matches!(want, Want::Preview { .. }) {
            self.music_at = selected;
        }
        let rng = &mut self.rng;
        let music_at = &mut self.music_at;
        self.menu_music
            .tick(now_ns, want, &mut self.audio.previews, || {
                let picked = random_track(rng, state.files());
                if let Some(picked) = &picked {
                    *music_at = Some(picked.at);
                }
                picked
            });
    }

    /// Frees a prepared play nobody waits for.
    fn free_pending(&mut self) {
        if let Pending::Track { prepared, .. } = std::mem::replace(&mut self.pending, Pending::Idle)
        {
            self.loader.free(Trash::Prepared(prepared));
        }
    }

    /// Starts `prepared` as play `id` of the beatmap file `map` on its opened track, with
    /// the offset the file last had. Navigation.
    fn start(&mut self, id: u64, prepared: Prepared, title: String, map: Option<PathBuf>) {
        let Prepared {
            beatmap,
            mods,
            sounds,
            scene,
            beatmap_hash,
            ..
        } = prepared;
        let replay_beatmap = Arc::new(ReplayBeatmap {
            hash: beatmap_hash,
            format_version: beatmap.format_version,
            name: map.as_ref().map_or_else(String::new, |map| {
                map.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }),
        });
        let shown_mods = results::mods(&mods);
        let mut input = LiveInput::new(
            self.input.bindings,
            PlayfieldLayout::new(self.points[0], self.points[1]),
        );
        input.set_mouse_buttons_disabled(!self.input.mouse_buttons);
        let session = Box::new(PlaySession::new(
            &beatmap,
            &mods,
            &self.audio.player,
            input,
            sounds,
            Offsets {
                beatmap_ms: map
                    .as_ref()
                    .and_then(|map| self.beatmap_offsets.get(map))
                    .copied()
                    .unwrap_or(0.0),
                ..self.input.offsets
            },
        ));
        let mut session = session;
        let stable_pause = scene.stable_pause_sizes();
        style_menu(&mut session, self.pause_menu, stable_pause);
        session.set_menu_effects(self.effects);
        self.loader.free(Trash::Other(Box::new((beatmap, mods))));
        // The caller checked for a free slot.
        let _ = self.scenes.push((id, scene));
        self.game = Some(Game {
            session,
            ending: Ending::Playing,
            id,
            title,
            mods: shown_mods,
            map,
            beatmap: replay_beatmap,
            stable_pause,
            options: self
                .play_requested
                .take()
                .and_then(|(request, options)| (request == id).then_some(options)),
        });
    }
}

/// The update thread's side.
struct Shell {
    state: ShellState,
    /// Without an audio output, the song select does not play.
    host: Option<PlayHost>,
    /// Surface size in pixels the draw thread last drew at, see [`pack_size`].
    surface: Arc<AtomicU64>,
    layout_size: u64,
    /// The window's pixels per point: input arrives in points, the surface is in pixels.
    scale: WindowScale,
    /// The scale the layouts were made for.
    pixels_per_point: f32,
    /// Set when the menu quits; the window closes then.
    exited: Arc<AtomicBool>,
    /// Hides the OS cursor while a cursor of our own is shown (ADR-0124).
    os_cursor: CursorVisibility,
    /// Changes the draw thread's rate (ADR-0047).
    draw_rate: DrawRateControl,
    rate_switch: DrawRateSwitch,
    /// Changes this thread's own rate (ADR-0048).
    update_rate: AppliedUpdateRate,
    /// Writes the settings file on its own thread; without a settings folder, `None`.
    saver: Option<SaveLink>,
    /// The interface language in force (ADR-0091).
    language: language::AppliedLanguage,
    /// Typed text is on in the menus and off during plays (ADR-0065).
    text_input: TextInput,
    /// Relative mouse mode and sensitivity, for the input thread (ADR-0069).
    mouse: MouseSettings,
    /// Mode, display and sizes of the window, for the input thread (ADR-0070).
    window: WindowSettings,
    /// The window state last handed to `window`, or switched to by the window itself.
    window_sent: WindowState,
    /// The windowed position last handed to `window` or reported by it (ADR-0073).
    position_sent: [f32; 2],
    /// When to save a size, display or position the window reported, once it stops
    /// changing; `None` when nothing waits.
    window_save_at: Option<u64>,
}

impl Shell {
    /// Remakes the layouts when the surface size or the window's scale changed.
    fn follow_surface(&mut self) {
        let size = self.surface.load(Ordering::Relaxed);
        let ppp = self.scale.pixels_per_point();
        // A minimised window may report 0×0: keep the last layout.
        if (size != self.layout_size || ppp != self.pixels_per_point)
            && size >> 32 != 0
            && size & 0xffff_ffff != 0
        {
            self.layout_size = size;
            self.pixels_per_point = ppp;
            let (w, h) = ((size >> 32) as f32, (size & 0xffff_ffff) as f32);
            self.state.set_pixels_per_point(ppp);
            self.state.set_size(w, h);
            if let Some(host) = &mut self.host {
                host.set_size(w / ppp, h / ppp);
            }
        }
    }

    /// Keeps the window and the settings in step (ADR-0070, ADR-0073). The window's
    /// handle is the truth: a mode the window switched to by itself (Alt+Enter, F11) is
    /// taken into the settings even if its [`InputKind::WindowMode`] report was lost to a
    /// full ring, and a settings change racing a switch loses to it rather than
    /// overwriting it. Sizes, displays and positions the player gave the window are taken
    /// too and saved [`WINDOW_SAVE_DELAY_NS`] after the last change, as dragging reports
    /// one every frame. A few atomic loads per tick; a lock and an allocation only on
    /// changes.
    fn sync_window(&mut self, now_ns: u64) {
        let actual = self.window.get();
        let position = self.window.position();
        if actual != self.window_sent || position != self.position_sent {
            self.window_reported((actual, position), now_ns);
        }
        self.state.set_title_bar(self.window.title_bar());
        let live = *self.state.live_window();
        self.window
            .set_minimise_on_focus_loss(live.minimise_on_focus_loss);
        // The position goes first: the window reads it after the word (ADR-0073).
        if live.windowed_position != self.position_sent {
            self.window.set_position(live.windowed_position);
            // Not read back: a move of the window meanwhile is reported next tick.
            self.position_sent = slam_input::clamp_position(live.windowed_position);
        }
        let wanted = live.state();
        if wanted != self.window_sent {
            match self.window.compare_and_set(self.window_sent, wanted) {
                Ok(stored) => self.window_sent = stored,
                Err(found) => self.window_reported((found, self.position_sent), now_ns),
            }
        }
        if self.window_save_at.is_some_and(|at| now_ns >= at) {
            self.window_save_at = None;
            self.state.save_settings();
        }
    }

    /// Takes what the window changed by itself since the state and position last sent.
    fn window_reported(&mut self, after: (WindowState, [f32; 2]), now_ns: u64) {
        let before = (self.window_sent, self.position_sent);
        (self.window_sent, self.position_sent) = after;
        if self.state.window_reported(before, after) {
            self.window_save_at = Some(now_ns.saturating_add(WINDOW_SAVE_DELAY_NS));
        }
    }

    /// Asks for the draw rate the visible screens call for, hands changed settings to the
    /// threads that use them (ADR-0048) and to the saver. Neither blocks nor touches the
    /// disk; allocates only to ask for another output (ADR-0089).
    fn apply_settings(&mut self, now_ns: u64) {
        let settings = self.state.settings();
        self.update_rate.sync(settings);
        // The settings screen's edits count at once, not when it closes.
        let input = self.state.live_input();
        self.mouse.set_relative(input.high_precision_mouse);
        self.mouse.set_sensitivity(input.sensitivity);
        if let Some(host) = &mut self.host {
            host.apply_settings(settings);
        }
        let theme = interface_theme(self.state.live_settings(), self.state.settings());
        if self
            .language
            .apply(self.state.live_settings(), self.state.rows(), theme)
        {
            // The open screen's device list in the new language too.
            self.state.relist_devices();
        }
        // Another colour scheme or menu scale: the update side lays out at once, the draw
        // side swaps views laid out with it behind the curtain, as for a language. Without
        // the thread laying them out the draw side never would: the theme stays.
        if self.language.lays_out_views() {
            let theme = interface_theme(self.state.live_settings(), self.state.settings());
            self.state.set_theme(theme);
        }
        self.sync_window(now_ns);
        let gameplay = self.state.gameplay_visible(now_ns);
        if let Some(mode) = self
            .rate_switch
            .next(gameplay, &self.state.settings().frame)
        {
            // Validated when the settings were read.
            let _ = self.draw_rate.set_mode(mode);
        }
        if let Some(request) = self.state.take_save_request()
            && let Some(saver) = &mut self.saver
            && let Err(request) = saver.request(request)
        {
            self.state.requeue_save_request(request);
        }
    }
}

impl Update for Shell {
    type Snapshot = ShellSnapshot;

    fn tick(&mut self, info: &TickInfo, events: &[InputEvent], snapshot: &mut ShellSnapshot) {
        // First: every slot published carries it, the early returns' too. A switch applied
        // later this tick shows on the next.
        snapshot.language_switch = self.language.switches();
        self.follow_surface();
        // Between the exit and the window closing, input no longer navigates.
        if self.exited.load(Ordering::Relaxed) {
            return;
        }
        let now = info.now_ns;
        let playing = self.state.playing();
        // The settings panel may open over the pause and fail menus, not over the play
        // itself (ADR-0095); the play's last tick tells which shows.
        self.state
            .set_play_menu(self.host.as_ref().is_some_and(PlayHost::menu_shown));
        // Over the play, the open panel (or the hotkey opening it) takes the input; the
        // play stands still in its menu meanwhile.
        let panel = playing
            && (self.state.settings_open()
                || (self.state.settings_allowed() && self.state.toggles_settings(events)));
        // On from the menus, so the first letter typed on the song select is not lost while
        // the window applies the wish after its next event.
        self.text_input.set_enabled(!playing || panel);
        let navigated = match &mut self.host {
            Some(host) if playing && !panel => {
                host.tick_game(now, events, &mut snapshot.play);
                self.state.follow_pointer(events);
                self.state.watch_hotkeys(events);
                self.state.tick(now, &[], snapshot)
            }
            Some(host) if playing => {
                // The play still follows the keys held, or one released over the panel
                // stays held after it (#324). First, as a release goes before a hold fires
                // on the play's own ticks.
                host.follow_covered(events);
                host.tick_game(now, &[], &mut snapshot.play);
                self.state.tick(now, events, snapshot)
            }
            _ => self.state.tick(now, events, snapshot),
        };
        if navigated == Navigated::Exit {
            self.exited.store(true, Ordering::Relaxed);
            return;
        }
        match &mut self.host {
            Some(host) => host.advance(now, &mut self.state),
            // Nothing plays without an audio output: the song select says so (#317).
            None => {
                if self.state.take_play_request().is_some() {
                    tracing::warn!("play: not started, the audio output failed to open");
                    self.state.set_no_audio(true);
                    self.state.finish_loading(now, None);
                }
            }
        }
        self.apply_settings(now);
        let play = self
            .host
            .as_ref()
            .and_then(|host| host.game.as_ref())
            .map(|_| snapshot.play.state);
        self.os_cursor
            .set_hidden(crate::menu_cursor::os_cursor_hidden(
                self.state.live_settings(),
                play,
            ));
    }
}

struct Resources<D: Device> {
    device: D,
    textures: Textures<D::Texture>,
    renderer: SpriteRenderer<D>,
    /// Draws MSDF glyphs.
    text_renderer: SpriteRenderer<D>,
    /// Draws outlined MSDF glyphs (ADR-0114).
    outline_renderer: SpriteRenderer<D>,
    white: AtlasRegion,
    /// The menus' backgrounds blurred, up to two while one fades into another (ADR-0123).
    blurs: [BlurSlot<D>; 2],
    /// The last blur that failed, not tried again.
    blur_failed: Option<BlurKey>,
}

/// A background blurred into a render target, and which image at which window size.
struct BlurSlot<D: Device> {
    blur: Blur<D>,
    image: Option<slam_render::upload::TextureHandle>,
    window: Extent,
    /// The blur's width the image was blurred with.
    radius: f32,
}

/// What a blur is of: the image, the window and the blur's width. One that failed is not
/// tried again until one of them changes.
type BlurKey = (slam_render::upload::TextureHandle, Extent, f32);

/// Texture ids the blurred backgrounds are drawn by: past any the registry hands out.
const BLUR_TEXTURES: [TextureId; 2] = [TextureId(u32::MAX - 1), TextureId(u32::MAX - 2)];

/// Blurs the background images `wanted` that are not blurred at the `window`'s size yet
/// (ADR-0123), at a quarter of the window, `blur` window pixels wide. Draws into render
/// targets only when an image or the size changes; then targets the window again.
#[allow(clippy::too_many_arguments)]
fn blur_backgrounds<D: Device>(
    device: &mut D,
    textures: &Textures<D::Texture>,
    slots: &mut [BlurSlot<D>; 2],
    failed: &mut Option<BlurKey>,
    uploads: &TextureUploads,
    wanted: [Option<slam_render::upload::TextureHandle>; 2],
    window: Extent,
    blur: f32,
) {
    let mut drew = false;
    let max_side = device.limits().max_texture_size;
    for image in wanted.into_iter().flatten() {
        if slots
            .iter()
            .any(|s| s.image == Some(image) && s.window == window && s.radius == blur)
            || *failed == Some((image, window, blur))
        {
            continue;
        }
        let TextureState::Ready(region) = uploads.get(image) else {
            continue;
        };
        let Some(texture) = textures.get(region.texture) else {
            continue;
        };
        // Its own slot at another size, or one showing nothing wanted.
        let index = slots
            .iter()
            .position(|s| s.image == Some(image))
            .or_else(|| {
                slots
                    .iter()
                    .position(|s| s.image.is_none_or(|old| !wanted.contains(&Some(old))))
            });
        let Some(slot) = index.and_then(|i| slots.get_mut(i)) else {
            continue;
        };
        let height = (window.height / 4).clamp(1, max_side);
        let scale = height as f32 / region.size.height.max(1) as f32;
        let width = ((region.size.width as f32 * scale).round() as u32).clamp(1, max_side);
        drew = true;
        slot.image = None;
        if slot
            .blur
            .run(
                device,
                texture,
                region.uv,
                Extent::new(width, height),
                blur / 4.0,
            )
            .is_ok()
        {
            slot.image = Some(image);
            slot.window = window;
            slot.radius = blur;
        } else {
            *failed = Some((image, window, blur));
        }
    }
    if drew {
        device.begin_pass(None, None);
    }
}

/// The draw thread's side. The device and its resources exist between `start` and `stop`.
struct View<B: render::Backend> {
    surface: Option<B>,
    resources: Option<Resources<B::Device>>,
    theme: Theme,
    glyphs: GlyphCache,
    menu: MenuView,
    song_select: SongSelectView,
    results: ResultsView,
    settings: SettingsView,
    /// Skin dropdowns for a new songs folder (ADR-0064).
    skin_views: Option<SkinViews>,
    /// Display and resolution dropdowns (ADR-0072).
    display_views: Option<ShownViews>,
    /// Output device dropdowns listed when the settings screen opens (ADR-0089).
    device_views: Option<DeviceViews>,
    shapes: ShapeBatch,
    text: SpriteBatch,
    /// The startup intro's view, with the intro on (ADR-0114). Kept after the intro: it is
    /// small, and freeing it would allocate in a frame.
    intro: Option<IntroView>,
    /// Outlined glyphs, drawn last with the MSDF outline shader (ADR-0114).
    outlines: SpriteBatch,
    /// Set once a frame is shown: the intro starts then (ADR-0114).
    shown: Arc<AtomicBool>,
    /// Scenes of plays the update thread starts, and the one shown with its GPU resources.
    scenes: Option<rtrb::Consumer<(u64, PlayScene)>>,
    scene: Option<(u64, PlayScene, SceneGpu<B::Device>)>,
    /// `scene` was on a layer: a snapshot without it now means it went (the scene arrives
    /// before the snapshot that shows it).
    scene_shown: bool,
    /// Takes scenes no longer shown, to free them off this thread.
    scene_trash: Option<SceneTrash>,
    /// Read by the update thread for the layout.
    surface_size: Arc<AtomicU64>,
    /// The FPS counter over every screen (ADR-0060).
    fps: FpsOverlay,
    /// Views laid out in a language switched to (ADR-0091).
    languages: Option<LanguageViews>,
    /// The curtain over a language switch and the views it waits for.
    swap: LanguageSwap<Box<language::InterfaceViews>>,
    /// The menu's texture loader and the song select's background (ADR-0066).
    backgrounds: Option<(TextureUploads, BackgroundFade)>,
    /// Tells the update thread which snapshot was drawn (ADR-0127).
    thumb_shown: Option<crate::thumbnails::ThumbShown>,
    /// The menu cursor over everything (ADR-0124).
    menu_cursor: crate::menu_cursor::MenuCursorView,
    /// The pointer's smoothed place for the parallax (ADR-0132).
    parallax: crate::parallax::Parallax,
    _quit: crate::bench::QuitOnDrop,
}

/// Whether a layer of `snapshot` shows the play `id`.
fn scene_on_layers(snapshot: &ShellSnapshot, id: u64) -> bool {
    snapshot
        .layers
        .iter()
        .flatten()
        .any(|layer| matches!(layer.view, ScreenView::Play(shown) if shown == id))
}

/// Destroys `scene`'s GPU resources and hands it to the loader. Navigation.
fn retire<D: Device>(
    scene: &mut Option<(u64, PlayScene, SceneGpu<D>)>,
    trash: &mut Option<SceneTrash>,
    device: &mut D,
) {
    if let Some((_, old, gpu)) = scene.take() {
        gpu.destroy(device);
        if let Some(trash) = trash {
            trash.free(old);
        }
    }
}

impl<B: render::Backend> Draw<ShellSnapshot> for View<B> {
    fn start(&mut self) {
        // The window thread made the surface and a device on it (ADR-0018, ADR-0080);
        // failing now is a driver fault the shell does not recover from.
        let surface = self.surface.take().expect("surface is set before start");
        let mut device = surface.create_device().expect("render device");
        let renderer = SpriteRenderer::with_layout(
            &mut device,
            DEFAULT_SLOTS,
            &shaders::UI_SHAPE,
            SHAPE_LAYOUT,
        )
        .expect("renderer");
        let text_renderer = SpriteRenderer::with_shader(&mut device, DEFAULT_SLOTS, &shaders::MSDF)
            .expect("text renderer");
        let outline_renderer =
            SpriteRenderer::with_shader(&mut device, DEFAULT_SLOTS, &shaders::MSDF_OUTLINE)
                .expect("outline renderer");
        let mut textures = Textures::with_capacity(MAX_TEXTURES);
        let mut atlas = Atlas::new(Extent::new(64, 64), TextureFormat::Rgba8);
        let white = atlas
            .add(
                &mut device,
                &mut textures,
                Extent::new(8, 8),
                &[255; 8 * 8 * 4],
            )
            .expect("white square");
        // Glyphs are made before the first frame: a glyph takes several frames to appear.
        let deadline = Instant::now() + GLYPH_WARM_TIMEOUT;
        while self.request_glyphs() > 0 && Instant::now() < deadline {
            self.glyphs.process(&mut device, &mut textures);
            std::thread::sleep(Duration::from_millis(1));
        }
        let blur_slot = |device: &mut B::Device| BlurSlot {
            blur: Blur::new(device).expect("blur"),
            image: None,
            window: Extent::new(0, 0),
            radius: 0.0,
        };
        let blurs = [blur_slot(&mut device), blur_slot(&mut device)];
        self.resources = Some(Resources {
            device,
            textures,
            renderer,
            text_renderer,
            outline_renderer,
            white,
            blurs,
            blur_failed: None,
        });
    }

    fn frame(&mut self, info: &FrameInfo, snapshot: &ShellSnapshot) {
        let r = self.resources.as_mut().expect("started");
        let mut layer_views = snapshot.layers.iter().flatten().map(|layer| &layer.view);
        let gameplay = layer_views
            .clone()
            .next_back()
            .is_some_and(|view| matches!(view, ScreenView::Play(_)));
        // Black with a play on any layer, as lazer's renderer clears: a fully dimmed
        // background is not drawn (`BackgroundView::push`) and shows as this.
        let clear = if layer_views.any(|view| matches!(view, ScreenView::Play(_))) {
            Color::BLACK
        } else {
            self.theme.background
        };
        let size = r.device.begin_frame(clear);
        if let Some(scenes) = &mut self.scenes
            && let Ok((id, mut scene)) = scenes.pop()
        {
            // A play's new look (ADR-0096) is shown as its old scene was: if the play has
            // gone meanwhile, it goes at once instead of waiting for the next play.
            let shown = self.scene_shown && self.scene.as_ref().is_some_and(|(old, ..)| *old == id);
            retire(&mut self.scene, &mut self.scene_trash, &mut r.device);
            // The glyphs come in over the first frames; the lead-in runs meanwhile.
            let gpu = scene.start(&mut r.device, Duration::ZERO, None);
            self.scene = Some((id, scene, gpu));
            self.scene_shown = shown;
        }
        if let Some((id, _, _)) = &self.scene {
            match (scene_on_layers(snapshot, *id), self.scene_shown) {
                (true, _) => self.scene_shown = true,
                (false, true) => {
                    retire(&mut self.scene, &mut self.scene_trash, &mut r.device);
                    self.scene_shown = false;
                }
                (false, false) => {}
            }
        }
        self.surface_size
            .store(pack_size(size.width, size.height), Ordering::Relaxed);
        if let Some(views) = &mut self.skin_views
            && let Some(mut view) = views.take()
        {
            // Its glyphs are requested as it is drawn, like every text's.
            self.settings.swap_dropdown(rows::skin_row(), &mut view);
            views.give_back(view);
        }
        if let Some(views) = &mut self.device_views
            && let Some(mut view) = views.take()
        {
            self.settings.swap_dropdown(rows::device_row(), &mut view);
            views.give_back(view);
        }
        if let Some(views) = &mut self.display_views {
            views.receive();
        }
        let now = info.wait.woke_ns;
        let arrived = self.languages.as_mut().and_then(LanguageViews::take);
        if let Some(older) = self.swap.receive(now, snapshot.language_switch, arrived)
            && let Some(languages) = &mut self.languages
        {
            languages.give_back(older);
        }
        if let Some(mut views) = self.swap.take()
            && let Some(languages) = &mut self.languages
        {
            language::carry_dropdowns(
                &mut self.settings,
                &mut views.settings,
                &[rows::skin_row(), rows::device_row()],
            );
            if let Some(shown) = &mut self.display_views {
                shown.move_screen(&mut self.settings, &mut views.settings);
            }
            std::mem::swap(&mut self.menu, &mut views.menu);
            std::mem::swap(&mut self.song_select, &mut views.song_select);
            std::mem::swap(&mut self.results, &mut views.results);
            std::mem::swap(&mut self.settings, &mut views.settings);
            self.fps.swap_view(&mut views.fps);
            self.theme = views.theme;
            languages.give_back(views);
        }
        self.swap.advance(now, || {
            glyphs_pending(
                &mut self.glyphs,
                &self.menu,
                &self.song_select,
                &self.results,
                &self.settings,
                &self.fps,
            )
        });
        self.glyphs.process(&mut r.device, &mut r.textures);
        self.menu_cursor.process(&mut r.device, &mut r.textures);
        // On every screen: releases queued during a play would otherwise fill the queue.
        if let Some(shown) = &self.thumb_shown {
            shown.set(snapshot.thumbnails.seq);
        }
        if let Some((uploads, fade)) = &mut self.backgrounds {
            uploads.process(&mut r.device, &mut r.textures);
            fade.set_dim(snapshot.background_dim);
            fade.advance(snapshot.background, info.wait.woke_ns, |h| uploads.get(h));
            // Only where a background shows, and not in a minimised window.
            let shows_background = snapshot
                .layers
                .iter()
                .flatten()
                .any(|layer| match layer.view {
                    ScreenView::Menu(_) => snapshot.menu_background,
                    ScreenView::SongSelect(_) => true,
                    _ => false,
                });
            if self.theme.background_blur > 0.0
                && shows_background
                && size.width > 0
                && size.height > 0
            {
                blur_backgrounds(
                    &mut r.device,
                    &r.textures,
                    &mut r.blurs,
                    &mut r.blur_failed,
                    uploads,
                    fade.visible(info.wait.woke_ns),
                    size,
                    self.theme.background_blur * self.theme.scale(size.height as f32),
                );
            }
        }
        // The blurred image of each background shown, if any: drawn in its place.
        let effects = self.theme.effects;
        let blur_on = self.theme.background_blur > 0.0;
        let blurred = |image| {
            let slot = r.blurs.iter().position(|s| s.image == Some(image))?;
            blur_on.then_some(BLUR_TEXTURES[slot])
        };
        let in_place = |region: AtlasRegion, image| match blurred(image) {
            Some(texture) => AtlasRegion {
                texture,
                uv: slam_render::atlas::UvRect::FULL,
                ..region
            },
            None => region,
        };
        // The main menu's background drifts in and back out (ADR-0123).
        let zoom = if effects {
            let t =
                info.wait.woke_ns as f64 / 1e9 / f64::from(self.theme.background_motion_seconds);
            1.0 + self.theme.background_motion
                * 0.5
                * (1.0 - (t * std::f64::consts::TAU).cos()) as f32
        } else {
            1.0
        };
        let (width, height) = (size.width as f32, size.height as f32);
        self.fps
            .frame(info.wait.woke_ns, width, height, snapshot.fps, gameplay);
        self.shapes.clear();
        self.text.clear();
        self.outlines.clear();
        // The menus' parallax: the pointer's smoothed place and the shifts it makes
        // (ADR-0132). Not in a play, a pause or the results.
        let pointer = self.parallax.update(
            info.wait.woke_ns,
            [width, height],
            self.theme.parallax_half_life,
        );
        let strength = snapshot.parallax;
        let background_map = crate::parallax::layer_map(
            pointer,
            [width, height],
            self.theme.parallax_background,
            strength,
            true,
        );
        let menu_map = crate::parallax::layer_map(
            pointer,
            [width, height],
            self.theme.parallax_menu,
            strength,
            false,
        );
        // The sheens sweep and the glows pulse with the frame's time (ADR-0123), wrapped in
        // f64 before it narrows to f32.
        let cycles = (info.wait.woke_ns as f64 / 1e9 * f64::from(self.theme.effect_speed))
            .rem_euclid(1024.0);
        self.shapes.set_drift(cycles as f32);
        self.menu
            .set_title_hidden(snapshot.intro.is_some_and(|i| i.hides_menu_title()));
        // Where each layer's sprites end: layers are drawn one after another, shapes then
        // text, so a screen coming in covers the text of the one leaving.
        // The song list's thumbnails that are uploaded (ADR-0127).
        let mut thumb_rows: [Option<RowThumb>; MAX_VISIBLE_ROWS] = [None; MAX_VISIBLE_ROWS];
        if let Some((uploads, _)) = &self.backgrounds {
            for (slot, handle) in thumb_rows.iter_mut().zip(snapshot.thumbnails.handles) {
                if let Some(handle) = handle
                    && let TextureState::Ready(region) = uploads.get(handle)
                {
                    *slot = Some(RowThumb { handle, region });
                }
            }
        }
        // The screens' layers and the settings panel's.
        let mut ends = [(0_u32, 0_u32, false); MAX_LAYERS + 1];
        let mut layers = 0;
        for layer in snapshot.layers.iter().flatten() {
            // Where the screen's own quads start: its background is not scaled.
            let mut layer_from = (self.shapes.len(), self.text.len());
            match &layer.view {
                ScreenView::Menu(menu) => {
                    // Under the menu and without its slide, as on the song select (ADR-0119).
                    if snapshot.menu_background
                        && let Some((uploads, fade)) = &self.backgrounds
                    {
                        let from = self.shapes.len();
                        fade.push(
                            &mut self.shapes,
                            width,
                            height,
                            info.wait.woke_ns,
                            layer.pose.opacity,
                            zoom,
                            |h| match uploads.get(h) {
                                TextureState::Ready(region) => Some(in_place(region, h)),
                                _ => None,
                            },
                        );
                        self.shapes.transform_from(from, &background_map, [1.0; 4]);
                    }
                    layer_from = (self.shapes.len(), self.text.len());
                    self.menu.push(
                        &mut self.shapes,
                        &mut self.text,
                        &mut self.glyphs,
                        width,
                        height,
                        menu,
                        layer.pose,
                        r.white,
                    );
                    // The buttons shift less than the background; hit-testing stays on
                    // the layout without it.
                    self.shapes
                        .transform_from(layer_from.0, &menu_map, [1.0; 4]);
                    // Glyphs move by whole pixels: a fraction would blur them.
                    self.text.transform_from(
                        layer_from.1,
                        &crate::parallax::snap_to_pixels(&menu_map),
                        [1.0; 4],
                    );
                }
                ScreenView::SongSelect(song_select) => {
                    // Under the screen and without its slide, as lazer's background stack.
                    if let Some((uploads, fade)) = &self.backgrounds {
                        let from = self.shapes.len();
                        fade.push(
                            &mut self.shapes,
                            width,
                            height,
                            info.wait.woke_ns,
                            layer.pose.opacity,
                            1.0,
                            |h| match uploads.get(h) {
                                TextureState::Ready(region) => Some(in_place(region, h)),
                                _ => None,
                            },
                        );
                        self.shapes.transform_from(from, &background_map, [1.0; 4]);
                    }
                    layer_from = (self.shapes.len(), self.text.len());
                    self.song_select.push(
                        &mut self.shapes,
                        &mut self.text,
                        &mut self.glyphs,
                        width,
                        height,
                        song_select,
                        layer.pose,
                        r.white,
                        Thumbs {
                            enabled: snapshot.thumbnails.enabled,
                            rows: &thumb_rows,
                            now_ns: info.wait.woke_ns,
                        },
                    )
                }
                ScreenView::Results(results) => self.results.push(
                    &mut self.shapes,
                    &mut self.text,
                    &mut self.glyphs,
                    width,
                    height,
                    results,
                    layer.pose,
                    r.white,
                ),
                // Drawn whole, without the transition's slide.
                ScreenView::Play(_) => {}
            }
            // The transition's scale, about the window's centre, for all the layer's quads
            // (ADR-0126).
            if layer.pose.scale != 1.0 {
                let centre = [width * 0.5, height * 0.5];
                self.shapes
                    .scale_from(layer_from.0, centre, layer.pose.scale);
                self.text.transform_from(
                    layer_from.1,
                    &Affine::about(centre, layer.pose.scale, 0.0, [0.0, 0.0]),
                    [1.0; 4],
                );
            }
            let play = matches!(layer.view, ScreenView::Play(_));
            ends[layers] = (self.shapes.len() as u32, self.text.len() as u32, play);
            layers += 1;
        }
        // The settings panel over the screens and the play, under the curtain (ADR-0095).
        if let Some(panel) = &snapshot.panel {
            let settings = &panel.settings;
            if let Some(views) = &mut self.display_views
                && let Some(RowWidget::Dropdown(display)) =
                    settings.widgets.get(views.display_row())
            {
                views.show(
                    &mut self.settings,
                    snapshot.display_generation,
                    display.selected(),
                );
            }
            push_backdrop(
                &mut self.shapes,
                &self.theme,
                r.white,
                width,
                height,
                panel.openness,
            );
            self.settings.push(
                &mut self.shapes,
                &mut self.text,
                &mut self.glyphs,
                panel_width(width, height, &self.theme),
                height,
                settings,
                panel_pose(panel.openness),
                r.white,
            );
            ends[layers] = (self.shapes.len() as u32, self.text.len() as u32, false);
            layers += 1;
        }
        // Last, over every screen and the play (ADR-0060).
        let overlay_from = (self.shapes.len() as u32, self.text.len() as u32);
        // A fancy transition's shapes cross over the layers; not without effects.
        if self.theme.effects
            && let Some(fx) = &snapshot.fx
        {
            fx.push(&mut self.shapes, &self.theme, r.white, width, height);
        }
        // Over every screen and the panel, under the curtain and the FPS counter; its
        // outlines are drawn last, over them, while the letters are outlined.
        if let (Some(view), Some(intro)) = (&mut self.intro, &snapshot.intro) {
            view.push(
                &mut self.shapes,
                &mut self.text,
                &mut self.outlines,
                &mut self.glyphs,
                width,
                height,
                intro,
                r.white,
            );
        }
        // Over every screen, under the FPS counter.
        self.swap.curtain().push(
            &mut self.shapes,
            &self.theme,
            width,
            height,
            info.wait.woke_ns,
            r.white,
        );
        self.fps
            .push(&mut self.shapes, &mut self.text, &mut self.glyphs, r.white);
        // The menu cursor is the last of all, over the curtain and the FPS counter, unless
        // gameplay draws its own (ADR-0124).
        let mut cursor = snapshot.menu_cursor;
        cursor.gameplay_cursor = gameplay && snapshot.play.state.hides_os_cursor();
        self.menu_cursor.push(
            &mut self.shapes,
            &self.theme,
            r.white,
            [width, height],
            info.wait.woke_ns,
            &cursor,
        );
        r.renderer.begin_frame();
        r.text_renderer.begin_frame();
        r.outline_renderer.begin_frame();
        let gpu_textures = &r.textures;
        let projection = Projection::pixels(size);
        let blur_results = [
            r.blurs[0].blur.result(&r.device),
            r.blurs[1].blur.result(&r.device),
        ];
        let texture = |id: TextureId| match BLUR_TEXTURES.iter().position(|b| *b == id) {
            Some(slot) => blur_results[slot],
            None => gpu_textures.get(id),
        };
        // A failed draw or present loses one frame; the next one tries again.
        let shapes = r
            .renderer
            .upload(&mut r.device, &self.shapes, &projection, texture);
        let text = r
            .text_renderer
            .upload(&mut r.device, &self.text, &projection, texture);
        let mut from = (0, 0);
        for (i, &(shapes_end, text_end, play)) in ends[..layers].iter().enumerate() {
            if play {
                // Only the scene of the play on the layer: another one's snapshot would
                // mislead its views.
                if let Some((id, scene, gpu)) = &mut self.scene
                    && scene_on_layers(snapshot, *id)
                {
                    // Over the screen it enters from (a 200 ms transition), the background
                    // must hide that screen even when fully dimmed.
                    let over_layer = i > 0;
                    scene.draw(
                        gpu,
                        &mut r.device,
                        size,
                        info.wait.woke_ns,
                        &snapshot.play,
                        None,
                        over_layer,
                    );
                }
                continue;
            }
            if let Ok(uploaded) = &shapes {
                let _ = r.renderer.draw_range(
                    &mut r.device,
                    uploaded,
                    &self.shapes,
                    from.0..shapes_end,
                    texture,
                );
            }
            if let Ok(uploaded) = &text {
                let _ = r.text_renderer.draw_range(
                    &mut r.device,
                    uploaded,
                    &self.text,
                    from.1..text_end,
                    texture,
                );
            }
            from = (shapes_end, text_end);
        }
        if let Ok(uploaded) = &shapes {
            let _ = r.renderer.draw_range(
                &mut r.device,
                uploaded,
                &self.shapes,
                overlay_from.0..self.shapes.len() as u32,
                texture,
            );
        }
        if let Ok(uploaded) = &text {
            let _ = r.text_renderer.draw_range(
                &mut r.device,
                uploaded,
                &self.text,
                overlay_from.1..self.text.len() as u32,
                texture,
            );
        }
        if !self.outlines.is_empty()
            && let Ok(uploaded) =
                r.outline_renderer
                    .upload(&mut r.device, &self.outlines, &projection, texture)
        {
            let _ = r.outline_renderer.draw_range(
                &mut r.device,
                &uploaded,
                &self.outlines,
                0..self.outlines.len() as u32,
                texture,
            );
        }
        let _ = r.device.present();
        self.shown.store(true, Ordering::Release);
    }

    fn stop(&mut self) {
        let Resources {
            mut device,
            mut textures,
            renderer,
            text_renderer,
            outline_renderer,
            blurs,
            ..
        } = self.resources.take().expect("started");
        for slot in blurs {
            slot.blur.destroy(&mut device);
        }
        retire(&mut self.scene, &mut self.scene_trash, &mut device);
        renderer.destroy(&mut device);
        text_renderer.destroy(&mut device);
        outline_renderer.destroy(&mut device);
        for texture in textures.drain() {
            device.destroy_texture(texture);
        }
        self.surface = B::release(device).expect("release the render device");
    }
}

impl<B: render::Backend> View<B> {
    fn request_glyphs(&mut self) -> usize {
        glyphs_pending(
            &mut self.glyphs,
            &self.menu,
            &self.song_select,
            &self.results,
            &self.settings,
            &self.fps,
        )
    }
}

/// Asks `glyphs` for every glyph of the screens' texts; how many are not ready yet.
fn glyphs_pending(
    glyphs: &mut GlyphCache,
    menu: &MenuView,
    song_select: &SongSelectView,
    results: &ResultsView,
    settings: &SettingsView,
    fps: &FpsOverlay,
) -> usize {
    menu.request_glyphs(glyphs)
        + song_select.request_glyphs(glyphs)
        + results.request_glyphs(glyphs)
        + settings.request_glyphs(glyphs)
        + fps.request_glyphs(glyphs)
}

/// The backend `name` names if this build has it, else the first available one.
fn pick_backend(name: Option<&str>, available: &[Backend]) -> Option<Backend> {
    let named = name.and_then(|name| {
        let backend = Backend::from_name(name).filter(|b| available.contains(b));
        if backend.is_none() {
            tracing::warn!("audio: backend {name} is not available, taking the first one");
        }
        backend
    });
    named.or_else(|| available.first().copied())
}

/// The device to open: the saved one while the backend still lists it, else the default.
/// An endpoint can vanish between runs (unplugged, or recreated with a new id when its
/// format changes on Windows); opening it would leave the game without sound. `listed` is
/// `None` when the backend cannot list its devices: the saved one is tried as is.
fn pick_device(saved: Option<&str>, listed: Option<&[slam_audio::DeviceInfo]>) -> Option<String> {
    let saved = saved?;
    match listed {
        Some(devices) if !devices.iter().any(|d| d.id == saved) => {
            tracing::warn!("audio: device {saved} is gone, taking the default one");
            None
        }
        _ => Some(saved.to_owned()),
    }
}

/// The output devices of the backend the settings pick, for the settings screen (#286):
/// at the start, then by the output thread each time the screen opens (ADR-0089). Empty if
/// the backend cannot list them.
fn output_devices(settings: &crate::settings::Audio) -> Vec<slam_audio::DeviceInfo> {
    let Some(backend) = pick_backend(settings.backend.as_deref(), Backend::available()) else {
        return Vec::new();
    };
    slam_audio::devices(backend).unwrap_or_else(|e| {
        tracing::warn!("audio: cannot list the output devices: {e}");
        Vec::new()
    })
}

/// Where the background comes from (ADR-0119): on the main menu, the skin's folder or the
/// beatmap whose music plays, as `setting` says; elsewhere the song select's selection.
fn background_source<'a>(
    on_menu: bool,
    setting: MenuBackground,
    files: &'a Arc<SongFiles>,
    menu_track: Option<(usize, usize)>,
    selected: Option<(usize, usize)>,
    skin: Option<&'a Arc<Path>>,
) -> BackgroundSource<'a> {
    match (on_menu, setting) {
        (true, MenuBackground::Skin) => BackgroundSource::Skin(skin),
        (true, _) => BackgroundSource::Beatmap {
            files,
            selected: menu_track,
        },
        (false, _) => BackgroundSource::Beatmap { files, selected },
    }
}

/// A random difficulty with an audio file in `files`, for the main menu (ADR-0090): a few
/// tries, `None` for a library without one.
fn random_track(rng: &mut u64, files: &SongFiles) -> Option<Picked> {
    for _ in 0..8 {
        // xorshift64; never 0 once seeded non-zero.
        *rng ^= *rng << 13;
        *rng ^= *rng >> 7;
        *rng ^= *rng << 17;
        let index = (*rng % files.len().max(1) as u64) as usize;
        let set = files.get(index)?;
        if let Some((difficulty, file)) = set.iter().enumerate().find(|(_, f)| f.audio.is_some()) {
            return Some(Picked {
                path: file.audio.clone()?,
                preview_ms: file.preview_ms,
                at: (index, difficulty),
            });
        }
    }
    None
}

/// Opens outputs and lists devices on the output thread (ADR-0089).
struct AppOpener {
    registry: Arc<FontRegistry>,
    theme: Theme,
    localizer: Arc<Localizer>,
}

impl output::Opener for AppOpener {
    type Audio = Audio;
    type Stream = slam_audio::OutputStream;

    fn open(
        &mut self,
        settings: &crate::settings::Audio,
    ) -> Result<(Audio, slam_audio::OutputStream), String> {
        open_audio(settings)
    }

    fn devices(&mut self, settings: &crate::settings::Audio) -> Vec<slam_audio::DeviceInfo> {
        output_devices(settings)
    }

    fn view(
        &mut self,
        devices: &[slam_audio::DeviceInfo],
        saved: Option<&str>,
    ) -> Option<DropdownView> {
        Some(rows::device_view(
            &self.registry,
            self.theme,
            &self.localizer,
            devices.to_vec(),
            saved,
        ))
    }
}

/// Opens the output the settings name with the mixer and music player.
fn open_audio(
    settings: &crate::settings::Audio,
) -> Result<(Audio, slam_audio::OutputStream), String> {
    let backend = pick_backend(settings.backend.as_deref(), Backend::available())
        .ok_or("no audio backend in this build")?;
    let listed = settings
        .device
        .as_ref()
        .and_then(|_| slam_audio::devices(backend).ok());
    let device = pick_device(settings.device.as_deref(), listed.as_deref());
    // The device's own rate spares the system a resampler and lets WASAPI LowLatency
    // and Exclusive open (ADR-0102).
    let sample_rate = slam_audio::stream_rate(slam_audio::device_rate(backend, device.as_deref()));
    tracing::info!(sample_rate, "audio: mixer rate");
    let (mut mixer, mut mixer_handle) = slam_audio::mixer(&MixerConfig {
        sample_rate,
        channels: CHANNELS,
        ..MixerConfig::default()
    })
    .map_err(|e| e.to_string())?;
    let (music, mut player) = slam_audio::music(&MusicConfig {
        sample_rate,
        channels: CHANNELS,
        ..MusicConfig::default()
    })
    .map_err(|e| e.to_string())?;
    mixer.set_music(music).map_err(|e| e.to_string())?;
    let mut previews = Vec::with_capacity(2);
    for _ in 0..2 {
        let (music, mut preview) = slam_audio::music(&MusicConfig {
            sample_rate,
            channels: CHANNELS,
            ..MusicConfig::default()
        })
        .map_err(|e| e.to_string())?;
        mixer.add_music(music).map_err(|e| e.to_string())?;
        preview
            .set_volume(Volumes::of(settings).menu_music)
            .map_err(|e| e.to_string())?;
        previews.push(preview);
    }
    let previews: [MusicPlayer; 2] = previews
        .try_into()
        .map_err(|_| "two menu decks".to_owned())?;
    let volumes = Volumes::of(settings);
    mixer_handle
        .set_master_gain(volumes.master)
        .map_err(|e| e.to_string())?;
    player
        .set_volume(volumes.music)
        .map_err(|e| e.to_string())?;
    mixer_handle
        .set_group_gain(VoiceGroup::Hitsounds, volumes.hitsounds)
        .map_err(|e| e.to_string())?;
    mixer_handle
        .set_group_gain(VoiceGroup::Effects, volumes.effects)
        .map_err(|e| e.to_string())?;
    mixer_handle
        .set_group_delay(VoiceGroup::Hitsounds, volumes.hitsound_delay_ms)
        .map_err(|e| e.to_string())?;
    let stream = slam_audio::open_output(
        backend,
        StreamConfig {
            device,
            sample_rate,
            channels: CHANNELS,
            buffer_frames: settings.buffer_frames,
            name: "SLAM".to_owned(),
        },
        Box::new(mixer),
    )
    .map_err(|e| e.to_string())?;
    let audio = Audio {
        output: Arc::clone(stream.position()),
        music: Arc::clone(player.position()),
        player,
        previews,
        mixer: mixer_handle,
        volumes: AppliedVolumes::new(volumes),
        sample_rate,
    };
    Ok((audio, stream))
}

/// The Fluent key of a judgement row's label.
fn results_label_key(kind: StatKind) -> &'static str {
    match kind {
        StatKind::Great => "results-great-label",
        StatKind::Ok => "results-ok-label",
        StatKind::Meh => "results-meh-label",
        StatKind::Miss => "results-miss-label",
        StatKind::SliderTick => "results-slider-tick-label",
        StatKind::SliderEnd | StatKind::SliderTail => "results-slider-end-label",
        StatKind::SpinnerBonus => "results-spinner-bonus-label",
        StatKind::SpinnerSpin => "results-spinner-spin-label",
    }
}

/// Parses the arguments of the window: `--songs DIR`.
pub fn parse_songs<I: IntoIterator<Item = String>>(args: I) -> Result<Option<PathBuf>, String> {
    let mut songs = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--songs" => songs = Some(args.next().ok_or("--songs needs a value")?.into()),
            _ => return Err(format!("unexpected argument {arg}")),
        }
    }
    Ok(songs)
}

/// The settings of `slam.toml` in the settings folder, created with the defaults if missing;
/// `songs` from the command line overrides the file's folder. Without either, an osu!stable
/// install found in the usual places gives the folder, saved to the file (ADR-0064). Without
/// a settings folder or with a broken file, the defaults.
pub fn load_settings(songs: Option<PathBuf>) -> Settings {
    let dirs = Dirs::detect();
    let settings = match &dirs {
        Some(dirs) => {
            let loaded = crate::settings::load_or_create(&dirs.settings_file());
            let mut settings = loaded.settings;
            if let Some(scale) = loaded.legacy_hit_error_scale {
                migrate_hit_error_scale(dirs, &mut settings, scale);
            }
            let store = crate::hud_layouts::LayoutStore::in_data_dir(dirs);
            crate::hud_layouts::migrate::refresh_migrated_copies(&store);
            settings
        }
        None => {
            tracing::warn!("settings: no settings folder, using the defaults");
            Settings::default()
        }
    };
    let mut settings = with_songs(settings, songs);
    if settings.paths.songs.is_none()
        && let Some(found) = crate::settings::stable::detect()
    {
        tracing::info!("songs: found osu!stable songs at {}", found.display());
        if let Some(dirs) = &dirs
            && let Err(e) = crate::settings::saver::write_songs(&dirs.settings_file(), &found)
        {
            tracing::warn!("settings: {e}");
        }
        settings.paths.songs = Some(found);
    }
    settings
}

/// Moves the retired `gameplay.hit_error_scale` into a HUD layout and rewrites the settings
/// file without the key (ADR-0109). Failures are warnings: the play goes on with the layout
/// as it is.
fn migrate_hit_error_scale(dirs: &Dirs, settings: &mut Settings, scale: f64) {
    let localizer = match interface_localizer(settings) {
        Ok(localizer) => localizer,
        Err(e) => {
            tracing::warn!("hud: cannot migrate the hit error meter size: {e}");
            return;
        }
    };
    let store = crate::hud_layouts::LayoutStore::in_data_dir(dirs);
    if crate::hud_layouts::migrate::migrate_on_startup(&store, settings, scale, &localizer)
        && let Err(e) = crate::settings::saver::write(&dirs.settings_file(), settings.clone())
    {
        tracing::warn!("settings: {e}");
    }
}

/// `settings` with the songs folder of the command line, if given.
fn with_songs(mut settings: Settings, songs: Option<PathBuf>) -> Settings {
    if songs.is_some() {
        settings.paths.songs = songs;
    }
    settings
}

/// Opens the window with the main menu and runs until it is closed. With a songs folder, the
/// song select lists the beatmap sets of that folder.
pub fn run(settings: Settings) -> Result<(), String> {
    // Read once, before SDL starts; a change at runtime waits for a restart (ADR-0097).
    slam_input::set_video_driver(settings.window.video_driver);
    let (state, position) = (settings.window.state(), settings.window.windowed_position);
    let window = |kind| {
        match kind {
            WindowKind::OpenGl => InputWindow::new_opengl_in("SLAM", state, position),
            WindowKind::Native => InputWindow::new_native_in("SLAM", state, position),
        }
        .map_err(|e| e.to_string())
    };
    render::run(
        settings.window.renderer.to_backend(),
        slam_input::PriorityRequest::High,
        window,
        App(settings),
    )?
}

struct App(Settings);

impl WithBackend for App {
    type Output = Result<(), String>;

    fn run<B: render::Backend>(self, window: InputWindow, opened: Opened<B>) -> Self::Output {
        run_with(self.0, window, opened)
    }
}

/// The built-in languages with the settings' language active; an unknown one stays English.
fn interface_localizer(settings: &Settings) -> Result<Localizer, String> {
    let localizer = Localizer::builtin().map_err(|e| e.to_string())?;
    if let Some(language) = &settings.general.language
        && let Err(e) = localizer.set_language(language)
    {
        tracing::warn!("settings: general.language: {e}, using English");
    }
    Ok(localizer)
}

/// The interface theme the settings ask for (ADR-0117, ADR-0118): the colour scheme of
/// `live` (with the settings screen's edits), the menu scale of `committed`: the menus'
/// scale changes when the settings are kept, not under the slider dragging it.
pub fn interface_theme(live: &Settings, committed: &Settings) -> Theme {
    let general = &live.general;
    let mut theme = Theme::new(general.colour_scheme, committed.general.menu_scale)
        .with_effects(general.effects)
        .with_skew(general.skew)
        .with_shadow(if general.shadows {
            general.shadow_strength as f32
        } else {
            0.0
        })
        .with_cursor_fx(CursorFx {
            rotate: general.cursor_rotate,
            spin: general.cursor_spin,
            trail: general.cursor_trail,
            ripple: general.cursor_ripple,
        });
    // The background blur is its own switch, whatever the effects say (ADR-0136).
    theme.background_blur = if general.blur {
        theme.background_blur * (general.blur_strength as f32).clamp(0.0, 2.0)
    } else {
        0.0
    };
    theme
}

fn run_with<B: render::Backend>(
    settings: Settings,
    mut window: InputWindow,
    opened: Opened<B>,
) -> Result<(), String> {
    let Opened {
        surface,
        limits,
        size_in_pixels: (pixels_w, pixels_h),
        priority,
    } = opened;
    crate::gpu::log_priority(&priority);
    let theme = interface_theme(&settings, &settings);
    let scale = window.window_scale();
    let pixels_per_point = scale.pixels_per_point();
    let initial_size = pack_size(pixels_w, pixels_h);
    let surface_size = Arc::new(AtomicU64::new(initial_size));
    let registry = Arc::new(FontRegistry::with_builtin());
    let glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits))
        .map_err(|e| e.to_string())?;
    let localizer = Arc::new(interface_localizer(&settings)?);
    let mut state = ShellState::new(theme, pixels_w as f32, pixels_h as f32, pixels_per_point)
        .with_settings(settings.clone())
        .with_devices(output_devices(&settings.audio));
    state.set_localizer(Arc::clone(&localizer));
    // The intro flies into the menu's title, in the language the menu opens in (ADR-0114).
    let intro_title = settings
        .general
        .intro
        .then(|| localizer.format("menu-title", None).into_owned());
    let shown = Arc::new(AtomicBool::new(false));
    if intro_title.is_some() {
        state = state.with_intro(Arc::clone(&shown));
    }
    let intro_view = intro_title
        .as_deref()
        .map(|title| IntroView::new(Arc::clone(&registry), theme, title));
    // Laid out again in another language by the `slam-language` thread (ADR-0091).
    let language::InterfaceViews {
        menu,
        song_select,
        results,
        settings: settings_view,
        fps: fps_view,
        ..
    } = language::interface_views(&registry, theme, &localizer, state.rows(), &settings);
    let (language_link, language_views, _language) = match language::spawn(
        Arc::clone(&registry),
        Arc::clone(&localizer),
        Arc::clone(&surface_size),
    ) {
        Ok((link, views, thread)) => (Some(link), Some(views), Some(thread)),
        Err(e) => {
            tracing::warn!("language: cannot start the thread: {e}");
            (None, None, None)
        }
    };
    // Joined when `run` returns, after the engine: it writes what is still queued.
    let (saver, saver_thread) = match Dirs::detect() {
        Some(dirs) => match crate::settings::saver::spawn(dirs.settings_file()) {
            Ok((link, thread)) => (Some(link), Some(thread)),
            Err(e) => {
                tracing::warn!("settings: cannot start the saver: {e}");
                (None, None)
            }
        },
        None => (None, None),
    };
    // The thumbnail thread is joined when `run` returns, after the engine (ADR-0127).
    let mut thumb_worker = None;
    let mut thumb_shown = None;
    // The library thread is joined when `run` returns, after the engine.
    let (skin_views, _library, backgrounds) = {
        let scanning_localizer = Arc::clone(&localizer);
        let scanning = Box::new(move |count: usize| {
            let mut args = slam_ui::i18n::FluentArgs::new();
            args.set("count", count);
            scanning_localizer
                .format("song-select-scanning", Some(&args))
                .into_owned()
        });
        let dialog = window.folder_dialog();
        let settings_file = Dirs::detect().map(|dirs| dirs.settings_file());
        let folders = crate::songs::Folders {
            open: Box::new(move |start, done| dialog.open(start, done)),
            resolve: |folder| {
                crate::settings::stable::songs_of(folder, &crate::settings::stable::RealFs)
            },
            save: Box::new(move |songs| {
                if let Some(file) = &settings_file
                    && let Err(e) = crate::settings::saver::write_songs(file, songs)
                {
                    tracing::warn!("settings: {e}");
                }
            }),
            skins: {
                let (registry, localizer) = (Arc::clone(&registry), Arc::clone(&localizer));
                Box::new(move |songs| {
                    let names: Vec<String> = crate::settings::skins::skin_dirs(songs)
                        .into_iter()
                        .map(|(name, _)| name)
                        .collect();
                    let options = rows::skin_options(&localizer, &names);
                    let view = DropdownView::new(Arc::clone(&registry), theme, &options);
                    crate::songs::SkinList {
                        names,
                        view: Some(view),
                    }
                })
            },
            relabel: {
                let (registry, localizer) = (Arc::clone(&registry), Arc::clone(&localizer));
                let language = Arc::clone(&localizer);
                crate::songs::SkinRelabel {
                    names: state.rows().skins().to_vec(),
                    language: Box::new(move || language.generation()),
                    view: Box::new(move |names| {
                        let options = rows::skin_options(&localizer, names);
                        Some(DropdownView::new(Arc::clone(&registry), theme, &options))
                    }),
                }
            },
        };
        // Without the menu's texture loader the song select shows the theme's background.
        let loader_config = TextureLoaderConfig {
            max_textures: MAX_BACKGROUNDS + crate::thumbnails::MAX_HANDLES,
            request_capacity: 128,
            job_capacity: 8,
            ..TextureLoaderConfig::new(limits)
        };
        let (loader, fade) = match texture_loader(&loader_config) {
            Ok((requests, uploads)) => {
                let (picker, loader, fade) = crate::backgrounds::link(requests);
                state = state.with_backgrounds(picker);
                let cache = Dirs::detect().map(|dirs| dirs.cache());
                let loader = match crate::thumbnails::link(
                    cache,
                    theme.song_thumbnail_aspect,
                    loader_config.image_limits(),
                ) {
                    Ok((thumbnails, bridge, worker, shown)) => {
                        state = state.with_thumbnails(thumbnails);
                        thumb_worker = Some(worker);
                        thumb_shown = Some(shown);
                        loader.with_thumbnails(bridge)
                    }
                    Err(e) => {
                        tracing::warn!("thumbnails: cannot start the thread: {e}");
                        loader
                    }
                };
                (Some(loader), Some((uploads, fade)))
            }
            Err(e) => {
                tracing::warn!("backgrounds: cannot start the texture loader: {e}");
                (None, None)
            }
        };
        let (picker, reader) = crate::details::link();
        let reader = match Dirs::detect() {
            Some(dirs) => reader.with_scores(dirs.scores()),
            None => reader,
        };
        state = state.with_details(picker);
        let (link, skin_views, thread) = crate::songs::spawn(
            settings.paths.songs.clone(),
            folders,
            scanning,
            loader,
            Some(reader),
            Some(crate::songs::StarsConfig {
                cache: Dirs::detect().map(|dirs| dirs.cache().join("stars.bin")),
                calculator: crate::stars::rosu_calculator(),
                version: slam_osu::stars::CALCULATOR_VERSION,
            }),
        )
        .map_err(|e| e.to_string())?;
        state = state.with_songs(link);
        (Some(skin_views), thread, fade)
    };
    // The skin's cursor for the menus; it ends once the update side drops its link.
    let mut cursor_thread = None;
    let mut cursor_shown = None;
    let cursor_uploads = match crate::menu_cursor::spawn(&TextureLoaderConfig::new(limits)) {
        Ok((link, uploads, shown, thread)) => {
            state = state.with_skin_cursor(link);
            cursor_thread = Some(thread);
            cursor_shown = Some(shown);
            Some(uploads)
        }
        Err(e) => {
            tracing::warn!("menu cursor: cannot start the loader: {e}");
            None
        }
    };
    // Joined when `run` returns, after the engine. Without the thread the settings screen
    // hides the display and resolution rows.
    let (display_views, _displays) = {
        let language = Arc::clone(&localizer);
        let (registry, localizer) = (Arc::clone(&registry), Arc::clone(&localizer));
        let make_views: crate::displays::MakeViews = Box::new(move |displays, lists| {
            Some(rows::display_views(
                &registry, theme, &localizer, displays, lists,
            ))
        });
        match crate::displays::spawn(make_views, Box::new(move || language.generation())) {
            Ok((sender, lists, views, thread)) => {
                state = state.with_displays(lists);
                window.set_displays_callback(Box::new(move |displays| {
                    // The thread outlives the window's events; a send fails only after it
                    // stopped, when nobody needs the list.
                    let _ = sender.send(displays);
                }));
                (
                    Some(ShownViews::new(views, rows::display_rows())),
                    Some(thread),
                )
            }
            Err(e) => {
                tracing::warn!("displays: cannot start the thread: {e}");
                (None, None)
            }
        }
    };
    // The play: a loader thread, scenes for the draw thread, the audio output.
    let (loader, scene_trash, _loader) = crate::loader::spawn(LoaderConfig {
        // The skin travels with each request; this is what the first one starts from.
        settings: {
            let options = live::play_options(&settings);
            PlaySettings {
                beatmap_skins: options.beatmap_skins,
                beatmap_colours: options.beatmap_colours,
                beatmap_hitsounds: options.beatmap_hitsounds,
                always_play_first_combo_break: options.always_play_first_combo_break,
                dim: options.dim,
                snaking: options.snaking(),
                hit_lighting: options.hit_lighting,
                key_overlay: options.key_overlay,
                hud_animations: options.hud_animations,
                cursor_trail: options.cursor_trail,
                cursor_size: options.cursor_size as f32,
                auto_cursor_size: options.auto_cursor_size,
                cursor_style: options.cursor_style,
                cursor_scheme: options.cursor_scheme,
                positional_hitsounds_level: options.positional_hitsounds_level,
                skin: options.skin,
                ..PlaySettings::default()
            }
        },
        limits,
        registry: Arc::clone(&registry),
        localizer: Arc::clone(&localizer),
        replays: Dirs::detect().map(|dirs| dirs.replays()),
        scores: Dirs::detect().map(|dirs| dirs.scores()),
        hud_layouts: Dirs::detect().map(|dirs| crate::hud_layouts::LayoutStore::in_data_dir(&dirs)),
    })
    .map_err(|e| e.to_string())?;
    let (scene_tx, scene_rx) = rtrb::RingBuffer::<(u64, PlayScene)>::new(2);
    // Without an output the window still opens; the song select does not play.
    // The output thread owns the output from here on and closes it when it is dropped,
    // after the engine (ADR-0089). Without the thread there is no output.
    let (audio, output, device_views, output_thread) = match open_audio(&settings.audio) {
        Ok((audio, stream)) => {
            let opener = AppOpener {
                registry: Arc::clone(&registry),
                theme,
                localizer: Arc::clone(&localizer),
            };
            match output::spawn(opener, stream, settings.audio.clone()) {
                Ok((link, views, thread)) => (Some(audio), Some(link), Some(views), Some(thread)),
                Err(e) => {
                    tracing::warn!("audio: cannot start the output thread: {e}");
                    (None, None, None, None)
                }
            }
        }
        Err(e) => {
            tracing::warn!("audio: {e}");
            (None, None, None, None)
        }
    };
    let host = audio.map(|audio| {
        let audio_rate = audio.sample_rate;
        PlayHost {
            audio,
            output,
            requested: settings.audio.clone(),
            switching: false,
            no_output: false,
            retired: None,
            track: None,
            menu_music: MenuMusic::default(),
            selection: (None, 0),
            // Any non-zero seed; the clock makes each run's first track differ.
            rng: now_ns() | 1,
            music_at: None,
            ui_sounds: ui_sounds::UiSoundPlayer::new(audio_rate),
            loader,
            scenes: scene_tx,
            win_key: window.win_key_block(),
            menu_shown: false,
            pending: Pending::Idle,
            game: None,
            retiring: None,
            track_reset: false,
            next_request: 0,
            play_requested: None,
            reskin: Reskin::Idle,
            points: [
                pixels_w as f32 / pixels_per_point,
                pixels_h as f32 / pixels_per_point,
            ],
            input: PlayInput::new(&settings),
            beatmap_offsets: HashMap::new(),
            localizer: Arc::clone(&localizer),
            intro_sound: intro_view.as_ref().and_then(|view| {
                let data = intro_sound::render(view.letters(), audio_rate);
                Sample::from_interleaved(data, audio_rate, CHANNELS)
                    .inspect_err(|e| tracing::warn!("intro: no sound: {e}"))
                    .ok()
                    .map(Arc::new)
            }),
            intro_voice: VoiceId::NONE,
            pause_menu: (
                settings.gameplay.pause_menu_style,
                settings.gameplay.pause_menu_scale,
                settings.general.colour_scheme,
            ),
            effects: settings.general.effects,
        }
    });

    let exited = Arc::new(AtomicBool::new(false));
    let scale_for_cursor = scale.clone();
    let scale_for_parallax = scale.clone();
    // The window opens on the menu: it starts at the menu's rate.
    let menu_rate = settings.frame.menu_draw.to_limiter();
    let draw_rate = DrawRateControl::new();
    let update_rate = UpdateRateControl::new();
    let counters = Arc::<slam_engine::EngineCounters>::default();
    let shell = Shell {
        state,
        host,
        surface: Arc::clone(&surface_size),
        layout_size: initial_size,
        scale,
        pixels_per_point,
        exited: Arc::clone(&exited),
        text_input: window.text_input(),
        mouse: {
            let mouse = window.mouse_settings();
            mouse.set_relative(settings.input.high_precision_mouse);
            mouse.set_sensitivity(settings.input.sensitivity);
            mouse
        },
        window: {
            let handle = window.window_settings();
            handle.set_minimise_on_focus_loss(settings.window.minimise_on_focus_loss);
            handle
        },
        window_sent: settings.window.state(),
        position_sent: settings.window.windowed_position,
        window_save_at: None,
        os_cursor: window.cursor_visibility(),
        draw_rate: draw_rate.clone(),
        rate_switch: DrawRateSwitch::new(menu_rate),
        update_rate: AppliedUpdateRate::new(update_rate.clone(), settings.frame.update_hz),
        saver,
        language: language::AppliedLanguage::new(
            Arc::clone(&localizer),
            settings.general.language.clone(),
            language_link,
            theme,
        ),
    };
    let draw = View {
        surface: Some(surface),
        resources: None,
        theme,
        glyphs,
        menu,
        song_select,
        results,
        settings: settings_view,
        skin_views,
        display_views,
        device_views,
        shapes: ShapeBatch::with_capacity(MAX_SHAPES),
        text: SpriteBatch::with_capacity(MAX_GLYPH_SPRITES),
        intro: intro_view,
        outlines: SpriteBatch::with_capacity(MAX_INTRO_OUTLINES),
        shown,
        scenes: Some(scene_rx),
        scene: None,
        scene_shown: false,
        scene_trash: Some(scene_trash),
        surface_size,
        fps: FpsOverlay::with_view(
            FpsSource::new(
                Arc::clone(&counters),
                update_rate.clone(),
                draw_rate.clone(),
            ),
            fps_view,
        ),
        languages: language_views,
        swap: LanguageSwap::default(),
        backgrounds,
        thumb_shown,
        menu_cursor: crate::menu_cursor::MenuCursorView::new(
            window.pointer(),
            scale_for_cursor,
            cursor_uploads,
        )
        .with_shown_opt(cursor_shown),
        parallax: crate::parallax::Parallax::new(window.pointer(), scale_for_parallax),
        _quit: crate::bench::QuitOnDrop(window.quit_handle()),
    };
    let (mut sink, source) = event_ring(1024);
    let engine = Engine::spawn_shared(
        EngineConfig {
            update_hz: settings.frame.update_hz,
            draw: menu_rate,
            ..EngineConfig::default()
        },
        shell,
        draw,
        ShellSnapshot::default(),
        source,
        EngineShared {
            counters,
            update_rate,
            draw_rate,
        },
    )
    .map_err(|e| e.to_string())?;
    sink.set_notify(engine.waker().as_notify());

    // The update thread must not take the window's lock: a watcher closes the window
    // once the menu quits.
    let closed = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (quit, exited, closed) = (
            window.quit_handle(),
            Arc::clone(&exited),
            Arc::clone(&closed),
        );
        std::thread::spawn(move || {
            while !closed.load(Ordering::Relaxed) {
                // A full event queue refuses the request: try again.
                if exited.load(Ordering::Relaxed) && quit.request() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    let mut mapper = TickMapper::new(now_ns, sdl_ticks_ns);
    let ran = window.run(&mut sink, &mut mapper);
    closed.store(true, Ordering::Relaxed);
    let _ = watcher.join();

    let slam_engine::Stopped {
        update: mut shell,
        draw,
        ..
    } = engine.shutdown();
    // The surface goes before the window.
    drop(draw);
    // A play still on: its sounds stop before the output closes.
    if let Some(host) = &mut shell.host
        && let Some(game) = &mut host.game
    {
        let deadline = Instant::now() + Duration::from_secs(1);
        while !game.session.stop_sounds(&mut host.audio.mixer) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    drop(output_thread);
    // After the engine: the update thread no longer asks it for anything.
    drop(thumb_worker);
    // A place the window was given just before closing waits no longer.
    if shell.window_save_at.take().is_some() {
        shell.state.save_settings();
    }
    // Closing the window saves, as leaving the settings screen does.
    shell.state.commit_editing();
    if let Some(mut link) = shell.saver.take() {
        let deadline = Instant::now() + Duration::from_secs(1);
        while let Some(request) = shell.state.take_save_request() {
            if let Err(request) = link.request(request) {
                shell.state.requeue_save_request(request);
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    // Dropping the update state dropped the cursor link, which ends the loader thread.
    drop(shell);
    if let Some(thread) = cursor_thread {
        let _ = thread.join();
    }
    if let Some(thread) = saver_thread {
        thread.finish();
    }
    drop(window);
    ran.map_err(|e| e.to_string())
}

#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod tests;
