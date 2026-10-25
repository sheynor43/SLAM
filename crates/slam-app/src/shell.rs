//! The window with the screens (ADR-0044): the main menu, the song select and the play,
//! prepared on worker threads (ADR-0045). Opened by `slam-app` without a command. Console
//! output is not localised; UI text is.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use slam_audio::{
    AudioPosition, Backend, MixerConfig, MixerHandle, MusicConfig, MusicEvent, MusicPlayer,
    MusicPosition, StreamConfig, TrackSource, VoiceGroup,
};
use slam_engine::clock::now_ns;
use slam_engine::gl::SdlGlSurface;
use slam_engine::{
    Draw, DrawRateControl, Engine, EngineConfig, EngineShared, FrameInfo, TickInfo, Update,
    UpdateRateControl,
};
use slam_input::{
    CursorVisibility, InputEvent, InputKind, InputWindow, MouseButton, MouseSettings, TextInput,
    TickMapper, WindowMode, WindowScale, WindowSettings, WindowState, event_ring, sdl_ticks_ns,
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
use slam_render::gl::{GlDebug, GlDevice, GlTexture};
use slam_render::sprite::{DEFAULT_SLOTS, Projection, SpriteBatch, SpriteRenderer};
use slam_render::textures::Textures;
use slam_render::upload::{TextureLoaderConfig, TextureState, TextureUploads, texture_loader};
use slam_render::{Device, Extent, TextureFormat, shaders};
use slam_text::{FontRegistry, GlyphAtlasConfig, GlyphCache, glyph_atlas};
use slam_ui::i18n::Localizer;
use slam_ui::menu::{
    MainMenu, MenuAction, MenuInput, MenuLabels, MenuLayout, MenuSnapshot, MenuView,
};
use slam_ui::results::{
    Results, ResultsAction, ResultsData, ResultsInput, ResultsLabels, ResultsLayout, ResultsMods,
    ResultsSnapshot, ResultsView, StatKind,
};
use slam_ui::screen::{MAX_LAYERS, Nav, Navigated, Pose, ScreenStack};
use slam_ui::settings_screen::{
    KeyValue, SettingsAction, SettingsInput, SettingsLayout, SettingsScreen, SettingsSnapshot,
    SettingsView,
};
use slam_ui::song_select::{
    BeatmapDetails, DetailLabels, DetailStats, ModColumns, ModSelect, SettingValue, SongFilter,
    SongList, SongSelect, SongSelectAction, SongSelectInput, SongSelectLabels, SongSelectLayout,
    SongSelectSnapshot, SongSelectStatus, SongSelectView, TileDetail,
};
use slam_ui::theme::Theme;
use slam_ui::widgets::DropdownView;

use crate::backgrounds::{BackgroundFade, BackgroundPicker, BackgroundTarget, MAX_BACKGROUNDS};
use crate::details::DetailsPicker;
use crate::loader::{LoaderConfig, LoaderLink, PlayOptions, SceneTrash, Trash};
use crate::play::fps::{FpsDisplay, FpsOverlay, FpsSource, MAX_FPS_GLYPHS};
use crate::play::scene::{CHANNELS, PlayScene, PlaySettings, Prepared, SAMPLE_RATE};
use crate::play::{device_limits, pack_size};
use crate::replays::{ReplayBeatmap, ReplayJob};
use crate::settings::saver::SaveLink;
use crate::settings::{Dirs, InputSettings, Settings};
use crate::songs::{FolderChange, SkinViews, SongFiles, SongsLink};

mod draw_rate;
mod live;

use live::{AppliedUpdateRate, AppliedVolumes, Volumes};
mod keybinds;
mod mod_settings;
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
const MAX_TEXTURES: usize = 16 + MAX_BACKGROUNDS as usize;
/// Rectangles of one frame: the buttons or rows of up to two layers, the FPS counter's
/// box and two backgrounds of each layer.
const MAX_SHAPES: usize = 256 + 1 + 2 * MAX_LAYERS;
/// Glyph sprites of one frame: the texts of up to two layers and the FPS counter.
const MAX_GLYPH_SPRITES: usize = 8192 + MAX_FPS_GLYPHS;
/// How long the draw thread waits for the glyphs before the first frame.
const GLYPH_WARM_TIMEOUT: Duration = Duration::from_secs(1);

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
const KEY_RIGHT_CTRL: u16 = 228;

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
        DIFFICULTY_INCREASE,
        mods_for(ModType::Conversion),
    ]
}

/// `mods_for(DifficultyIncrease)` without Traceable: the play draws it as no mod yet (#207).
const DIFFICULTY_INCREASE: &[&str] = &["HR", "SD", "PF", "DT", "NC", "HD"];

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
    /// The settings; the values being edited live in [`ShellState`].
    Settings(Box<SettingsScreen>),
}

/// What the draw thread needs to draw a screen. The settings' snapshot is large (about 24
/// row widgets, roughly 1 KB per `ScreenView`, times `MAX_LAYERS`, copied through the
/// triple buffer every tick), but it is written every tick: a box would allocate on the
/// update thread.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ScreenView {
    Menu(MenuSnapshot),
    SongSelect(SongSelectSnapshot),
    /// The play of this id, drawn from [`ShellSnapshot::play`]. Only while it is on top:
    /// a play going out leaves at once instead of sliding with a stale snapshot.
    Play(u64),
    Results(ResultsSnapshot),
    Settings(SettingsSnapshot),
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
    /// The Control keys held: bit 0 the left one, bit 1 the right one.
    ctrl: u8,
    /// Last cursor position in pixels.
    cursor: [f32; 2],
    /// Input arrives in window points, layout is in pixels.
    pixels_per_point: f32,
    /// Asks for the song select's background; without a menu texture loader `None`.
    backgrounds: Option<BackgroundPicker>,
    /// Asks for the selected difficulty's statistics; without a library thread `None`.
    details: Option<DetailsPicker>,
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
            settings_layout: SettingsLayout::new(width, height, &theme),
            song_select,
            songs: None,
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
            ctrl: 0,
            theme,
            cursor: [0.0, 0.0],
            pixels_per_point,
            backgrounds: None,
            details: None,
        }
    }

    /// A shell whose song select shows the selected difficulty's length, BPM and objects
    /// (ADR-0067); the difficulty values show without it.
    pub fn with_details(mut self, details: DetailsPicker) -> ShellState {
        self.details = Some(details);
        self
    }

    /// The selected difficulty's details with the selected mods. Neither blocks nor
    /// allocates.
    fn beatmap_details(&mut self, now_ns: u64) -> Option<BeatmapDetails> {
        let selected = self.song_select.selected();
        let statistics = self
            .details
            .as_mut()
            .and_then(|details| details.tick(now_ns, &self.files, selected));
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
        })
    }

    /// A shell whose song select shows the selected beatmap's background (ADR-0066).
    pub fn with_backgrounds(mut self, backgrounds: BackgroundPicker) -> ShellState {
        self.backgrounds = Some(backgrounds);
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

    /// A shell for a window started with `settings`.
    pub fn with_settings(mut self, settings: Settings) -> ShellState {
        self.rows = Rows::new(&settings);
        self.settings = settings;
        self
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
        commit_settings(
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
            self.stack.navigate(after_play(results), now_ns);
            if let AppScreen::Results(results) = self.stack.top() {
                self.held_results.push(results.share());
            }
        }
    }

    /// Follows the pointer and the Control keys while the play has the input, so that the
    /// screen under it knows where they are when the play goes.
    pub fn follow_pointer(&mut self, events: &[InputEvent]) {
        for event in events {
            self.track_ctrl(event.kind);
            if let InputKind::MouseMove { x, y } = event.kind {
                self.cursor = [x * self.pixels_per_point, y * self.pixels_per_point];
            }
        }
    }

    /// Watches for the hotkeys of the whole window while the play has the input: they
    /// reach the play too, which has no binding on them (ADR-0060).
    pub fn watch_hotkeys(&mut self, events: &[InputEvent]) {
        for event in events {
            if self.is_toggle_fps(event.kind) {
                self.toggle_fps();
            }
        }
    }

    /// The window switched itself to `mode` (Alt+Enter, F11): keeps it in the settings
    /// and saves them, as lazer's `CycleMode` changes its `WindowMode` setting. Allocates
    /// for the save: a rare action of the player, as [`ShellState::toggle_fps`].
    /// An open settings screen's copy follows on its own, or closing the screen would
    /// switch back.
    pub fn window_mode_switched(&mut self, mode: WindowMode) {
        if let Some(editing) = &mut self.editing {
            editing.window.mode = mode;
        }
        if self.settings.window.mode == mode {
            return;
        }
        self.settings.window.mode = mode;
        self.save_request = Some(Box::new(self.settings.clone()));
    }

    /// Input points now map to `pixels_per_point` layout pixels.
    pub fn set_pixels_per_point(&mut self, pixels_per_point: f32) {
        self.pixels_per_point = pixels_per_point;
    }

    /// Whether `kind` presses the FPS counter's hotkey.
    fn is_toggle_fps(&self, kind: InputKind) -> bool {
        let pressed = match kind {
            InputKind::Key {
                scancode,
                down: true,
            } => Input::Key(scancode),
            InputKind::MouseButton { button, down: true } => Input::Mouse(button),
            _ => return false,
        };
        self.settings.hotkeys.toggle_fps == Some(pressed)
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
        if let (AppScreen::Settings(screen), Some(row)) = (
            self.stack.top_mut(),
            Row::ALL.iter().position(|r| *r == Row::ShowFps),
        ) {
            screen.set_toggle(row, show);
        }
        self.save_request = Some(Box::new(self.settings.clone()));
    }

    /// Notes a Control key going down or up.
    fn track_ctrl(&mut self, kind: InputKind) {
        if let InputKind::Key { scancode, down } = kind {
            let bit = match scancode {
                KEY_LEFT_CTRL => 1,
                KEY_RIGHT_CTRL => 2,
                _ => return,
            };
            if down {
                self.ctrl |= bit;
            } else {
                self.ctrl &= !bit;
            }
        }
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

    /// Lays the screens out for a window of `width` × `height` pixels.
    pub fn set_size(&mut self, width: f32, height: f32) {
        self.layout = MenuLayout::new(width, height, &self.theme);
        self.song_layout = SongSelectLayout::new(width, height, &self.theme);
        self.results_layout = ResultsLayout::new(width, height, &self.theme);
        self.settings_layout = SettingsLayout::new(width, height, &self.theme);
    }

    /// The input of the screens, as the song select (the larger set) understands it, and
    /// the key only the results use.
    fn screen_input(&mut self, kind: InputKind) -> Option<ScreenInput> {
        self.track_ctrl(kind);
        // A key row that listens takes every key and mouse button, whatever the menus make
        // of them; Escape still goes through, as it cancels.
        if matches!(self.stack.top(), AppScreen::Settings(s) if s.is_listening()) {
            match kind {
                InputKind::Key {
                    scancode: KEY_DELETE,
                    down: true,
                } => return Some(ScreenInput::Clear),
                InputKind::Key {
                    scancode,
                    down: true,
                } if scancode != KEY_ESCAPE => {
                    return Some(ScreenInput::Capture(KeyValue::Key(scancode)));
                }
                InputKind::MouseButton { button, down: true } => {
                    return Some(ScreenInput::Capture(keybinds::key_value(Some(
                        Input::Mouse(button),
                    ))));
                }
                _ => {}
            }
        }
        if self.is_toggle_fps(kind) {
            return Some(ScreenInput::ToggleFps);
        }
        // The letters are the mod hotkeys on the song select, and Retry on the results.
        let song_select = matches!(self.stack.top(), AppScreen::SongSelect);
        if let InputKind::Key {
            scancode,
            down: true,
        } = kind
            && song_select
            && self.ctrl != 0
        {
            match scancode {
                KEY_UP => return Some(ScreenInput::ModSpeed(SPEED_STEP)),
                KEY_DOWN => return Some(ScreenInput::ModSpeed(-SPEED_STEP)),
                KEY_F3 => return Some(ScreenInput::ApproachRate(-APPROACH_RATE_STEP)),
                KEY_F4 => return Some(ScreenInput::ApproachRate(APPROACH_RATE_STEP)),
                _ => {}
            }
        }
        Some(ScreenInput::Common(match kind {
            // Holding Backspace keeps erasing the search, and only that (ADR-0065).
            InputKind::KeyRepeat {
                scancode: KEY_BACKSPACE,
            } if song_select => SongSelectInput::Erase,
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

    /// Takes the newest list the library thread published, if any.
    fn receive_songs(&mut self) {
        let Some(published) = self.songs.as_mut().and_then(SongsLink::latest) else {
            return;
        };
        let old = self.song_select.set_list(published.list, published.filter);
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
            if let AppScreen::Settings(screen) = self.stack.top_mut() {
                screen.set_value_text(Arc::clone(&self.songs_folder));
            }
        }
    }

    /// Takes the songs folder and its skins the player chose (ADR-0064): the next play finds
    /// the skin there, and the settings screen lists them. The old values go back into
    /// `change`, to be freed by the library thread.
    fn change_folder(&mut self, change: &mut FolderChange) {
        std::mem::swap(&mut self.settings.paths.songs, &mut change.songs);
        self.rows.swap_skins(&mut change.skins);
        if let AppScreen::Settings(screen) = self.stack.top_mut() {
            let settings = self.editing.as_ref().unwrap_or(&self.settings);
            let (options, selected) = self.rows.skin_choice(settings);
            screen.set_dropdown(rows::skin_row(), options, selected);
        }
    }

    /// Types `text` into the song select's search (ADR-0065); other screens take no text.
    fn type_text(&mut self, text: &str) {
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
    pub fn handle(&mut self, events: &[InputEvent], now_ns: u64) -> Navigated {
        for event in events {
            // The shell takes window modes from the window itself (`Shell::sync_window`).
            if let InputKind::WindowMode(_) = event.kind {
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
            let common = input.common();
            let nav = match self.stack.top_mut() {
                AppScreen::Menu(menu) => {
                    match common
                        .and_then(menu_input)
                        .and_then(|input| menu.input(input, &self.layout))
                    {
                        Some(MenuAction::Play) => Some(Nav::Push(AppScreen::SongSelect)),
                        Some(MenuAction::Settings) => Some(Nav::Push(open_settings(
                            &self.rows,
                            &self.settings,
                            &mut self.editing,
                            &self.songs_folder,
                        ))),
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
                AppScreen::Settings(screen) => {
                    let action = settings_input(input)
                        .and_then(|input| screen.input(input, &self.settings_layout));
                    match action {
                        Some(SettingsAction::Changed { row, value }) => {
                            if let Some(editing) = &mut self.editing {
                                self.rows.apply(editing, row, value);
                                Rows::refresh(screen, editing);
                            }
                            None
                        }
                        Some(SettingsAction::Pressed { row })
                            if Row::ALL.get(row) == Some(&Row::SongsFolder) =>
                        {
                            self.pick_folder();
                            None
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
                            None
                        }
                        Some(SettingsAction::Close) => {
                            commit_settings(
                                &mut self.settings,
                                &mut self.editing,
                                &mut self.save_request,
                            );
                            Some(Nav::Pop)
                        }
                        None => None,
                    }
                }
                // The play takes its input from the window's play host.
                AppScreen::Play(_) => None,
            };
            if let Some(nav) = nav
                && self.stack.navigate(nav, now_ns) == Navigated::Exit
            {
                return Navigated::Exit;
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
        self.receive_songs();
        let navigated = self.handle(events, now_ns);
        self.song_select.tick(now_ns, &self.song_layout);
        if self.setting_rows_for != Some(self.rows_key()) {
            self.rebuild_setting_rows();
        }
        snapshot.fps = FpsDisplay::of(&self.editing.as_ref().unwrap_or(&self.settings).frame);
        let details = self.beatmap_details(now_ns);
        self.song_select.set_details(details);
        if let Some(backgrounds) = &mut self.backgrounds {
            snapshot.background =
                backgrounds.tick(now_ns, &self.files, self.song_select.selected());
        }
        let top: *const AppScreen = self.stack.top();
        for (slot, layer) in snapshot.layers.iter_mut().zip(self.stack.layers(now_ns)) {
            *slot = layer.and_then(|(screen, pose)| {
                let view = match screen {
                    AppScreen::Menu(menu) => ScreenView::Menu(menu.snapshot()),
                    AppScreen::SongSelect => ScreenView::SongSelect(self.song_select.snapshot()),
                    AppScreen::Results(results) => ScreenView::Results(results.snapshot()),
                    AppScreen::Settings(settings) => ScreenView::Settings(settings.snapshot()),
                    AppScreen::Play(id) if std::ptr::eq(screen, top) => ScreenView::Play(*id),
                    AppScreen::Play(_) => return None,
                };
                Some(LayerSnapshot { view, pose })
            });
        }
        navigated
    }
}

/// The input of a screen: the song select's set, plus the key of the results and the two
/// only the settings use.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ScreenInput {
    Common(SongSelectInput),
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
}

impl ScreenInput {
    fn common(self) -> Option<SongSelectInput> {
        match self {
            ScreenInput::Common(input) => Some(input),
            ScreenInput::Release => Some(SongSelectInput::PointerRelease),
            _ => None,
        }
    }
}

/// The settings screen with the values in `editing` for the player to change; a snapshot
/// of `settings` when it opens. Navigation.
fn open_settings(
    rows: &Rows,
    settings: &Settings,
    editing: &mut Option<Settings>,
    songs_folder: &Arc<str>,
) -> AppScreen {
    *editing = Some(settings.clone());
    let mut screen = SettingsScreen::new(&rows.specs(settings), SECTIONS);
    Rows::refresh(&mut screen, settings);
    screen.set_value_text(Arc::clone(songs_folder));
    AppScreen::Settings(Box::new(screen))
}

/// Makes the edited settings the ones in force, and asks for them to be saved, if they
/// differ. The sliders' values are rounded to their steps first.
fn commit_settings(
    settings: &mut Settings,
    editing: &mut Option<Settings>,
    save_request: &mut Option<Box<Settings>>,
) {
    let Some(mut edited) = editing.take() else {
        return;
    };
    // The songs folder is the library's: it may have changed while the screen was open.
    edited.paths.songs.clone_from(&settings.paths.songs);
    round_for_save(&mut edited);
    if edited != *settings {
        *settings = edited.clone();
        *save_request = Some(Box::new(edited));
    }
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
        | ScreenInput::ToggleFps => {
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
            _ => return None,
        },
    })
}

/// Whether the play screen goes: the player quit, or the results show (a second after
/// passing, as lazer's `progressToResults`).
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
        | ScreenInput::ToggleFps => {
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
        ScreenInput::Common(_) => return None,
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

/// The window's audio: one output for the whole run (ADR-0045).
struct Audio {
    player: MusicPlayer,
    mixer: MixerHandle,
    output: Arc<AudioPosition>,
    music: Arc<MusicPosition>,
    /// The gains `mixer` and `player` were last given.
    volumes: AppliedVolumes,
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
    /// What the play's replay names its beatmap by.
    beatmap: ReplayBeatmap,
}

/// A quit play whose voices the mixer silences. The session keeps its samples until the
/// mixer has handed all of theirs back, so that the update thread never drops the last
/// reference to one.
struct Retiring {
    session: Box<PlaySession>,
    /// Since when the mixer has had no voice.
    idle_since: Option<u64>,
    /// A passed play's replay to save.
    replay: Option<ReplaySource>,
}

/// What a passed play's replay is made of besides the session.
struct ReplaySource {
    beatmap: ReplayBeatmap,
    /// The beatmap file's name, for the replay's.
    name: String,
    ended: SystemTime,
}

impl Retiring {
    /// What the loader takes to free the session, saving its replay first.
    fn into_trash(self) -> Trash {
        match self.replay {
            Some(source) => Trash::Replay(Box::new(ReplayJob {
                session: self.session,
                beatmap: source.beatmap,
                name: source.name,
                ended: source.ended,
            })),
            None => Trash::Other(self.session),
        }
    }
}

/// The update side of plays: loads them, runs the one on top, frees them via the loader.
struct PlayHost {
    audio: Audio,
    loader: LoaderLink,
    /// Scenes for the draw thread, one per play, named by the play's id.
    scenes: rtrb::Producer<(u64, PlayScene)>,
    /// Hides the OS cursor while the play draws its own.
    os_cursor: CursorVisibility,
    pending: Pending,
    game: Option<Game>,
    /// Apart from `game`: a mixer that stops taking commands (lost device) keeps this
    /// session to the end, not the next play from starting.
    retiring: Option<Retiring>,
    /// The track still has to be given its plain sound after a play.
    track_reset: bool,
    next_request: u64,
    /// The window in points, as gameplay input takes it.
    points: [f32; 2],
    input: PlayInput,
    /// Beatmap offsets the hotkeys set, by beatmap file, until the app quits (stored
    /// with the library in #175).
    beatmap_offsets: HashMap<PathBuf, f64>,
}

/// What a play takes from the settings on the update thread.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PlayInput {
    bindings: Bindings,
    /// Mouse buttons press in gameplay (lazer's `MouseDisableButtons` off).
    mouse_buttons: bool,
    offsets: Offsets,
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
        }
    }
}

impl PlayHost {
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
        self.os_cursor.set_hidden(snapshot.state.hides_os_cursor());
        if play_over(snapshot.state) {
            game.ending = Ending::Sounds;
        }
    }

    /// The window is `width` × `height` points.
    fn set_size(&mut self, width: f32, height: f32) {
        self.points = [width, height];
        if let Some(game) = &mut self.game {
            game.session.set_layout(PlayfieldLayout::new(width, height));
        }
    }

    /// Hands changed settings to the audio thread and to the next play (ADR-0048). Does
    /// not allocate or block.
    fn apply_settings(&mut self, settings: &Settings) {
        let audio = &mut self.audio;
        audio.volumes.sync(
            Volumes::of(&settings.audio),
            &mut audio.mixer,
            &mut audio.player,
        );
        // A play copies these when it starts (ADR-0049 for the bindings).
        self.input.bindings = settings.input.bindings;
        self.input.offsets.global_ms = settings.timing.global_offset_ms;
        self.input.mouse_buttons = settings.gameplay.mouse_buttons;
    }

    /// Takes requests, prepared plays and track events; ends a quit play. Allocates only
    /// when a play is asked for, starts or ends; logs and frees on the loader thread.
    fn advance(&mut self, now_ns: u64, state: &mut ShellState) {
        self.end_game(now_ns, state);
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
                Some(map) if self.game.is_none() => {
                    let options = PlayOptions {
                        mods: state.selected_mods().clone(),
                        ..live::play_options(state.settings())
                    };
                    self.loader.request(self.next_request, map, options)
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
        if let Some(loaded) = self.loader.poll() {
            let current = matches!(self.pending, Pending::Preparing(id) if id == loaded.id);
            match loaded.result {
                Ok(mut prepared) if current && state.is_loading() => {
                    let audio = std::mem::take(&mut prepared.audio);
                    match self.audio.player.load(TrackSource::File(audio)) {
                        Ok(generation) => {
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
                        // The draw thread has not taken the last scene: unreachable with
                        // one play at a time, but a play without a scene must not start.
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
            self.os_cursor.set_hidden(false);
            // The results come from the session before it retires.
            let results = game.session.results().map(|r| {
                results::results_data(game.id, std::mem::take(&mut game.title), game.mods, &r)
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
            // A passed play's replay is saved by the loader when it frees the session.
            let replay = game.session.replay_frames().map(|_| ReplaySource {
                beatmap: std::mem::take(&mut game.beatmap),
                name: game.map.as_ref().map_or_else(String::new, |map| {
                    map.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                }),
                ended: SystemTime::now(),
            });
            if let Some(map) = game.map.take() {
                self.beatmap_offsets
                    .insert(map, game.session.beatmap_offset());
            }
            state.leave_play(now_ns, results);
            // Two quits in a row on a stuck mixer: the older session goes anyway.
            if let Some(old) = self.retiring.replace(Retiring {
                session: game.session,
                idle_since: None,
                replay,
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
        let replay_beatmap = ReplayBeatmap {
            hash: beatmap_hash,
            format_version: beatmap.format_version,
        };
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
    /// Changes the draw thread's rate (ADR-0047).
    draw_rate: DrawRateControl,
    rate_switch: DrawRateSwitch,
    /// Changes this thread's own rate (ADR-0048).
    update_rate: AppliedUpdateRate,
    /// Writes the settings file on its own thread; without a settings folder, `None`.
    saver: Option<SaveLink>,
    /// Typed text is on in the menus and off during plays (ADR-0065).
    text_input: TextInput,
    /// Relative mouse mode and sensitivity, for the input thread (ADR-0069).
    mouse: MouseSettings,
    /// Mode, display and sizes of the window, for the input thread (ADR-0070).
    window: WindowSettings,
    /// The window state last handed to `window`, or switched to by the window itself.
    window_sent: WindowState,
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

    /// Keeps the window and the settings in step (ADR-0070). The window's wish word is
    /// the truth: a mode the window switched to by itself (Alt+Enter, F11) is taken into
    /// the settings even if its [`InputKind::WindowMode`] report was lost to a full ring,
    /// and a settings change racing a switch loses to it rather than overwriting it.
    /// One atomic load per tick; a lock and an allocation only on changes.
    fn sync_window(&mut self) {
        let actual = self.window.get();
        if actual != self.window_sent {
            self.window_sent = actual;
            self.state.window_mode_switched(actual.mode);
        }
        let wanted = self.state.settings().window.state();
        if wanted != self.window_sent {
            match self.window.compare_and_set(self.window_sent, wanted) {
                Ok(stored) => self.window_sent = stored,
                Err(found) => {
                    self.window_sent = found;
                    self.state.window_mode_switched(found.mode);
                }
            }
        }
    }

    /// Asks for the draw rate the visible screens call for, hands changed settings to the
    /// threads that use them (ADR-0048) and to the saver. Neither blocks, allocates or
    /// touches the disk.
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
        self.sync_window();
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
        self.follow_surface();
        // Between the exit and the window closing, input no longer navigates.
        if self.exited.load(Ordering::Relaxed) {
            return;
        }
        let now = info.now_ns;
        let playing = self.state.playing();
        // On from the menus, so the first letter typed on the song select is not lost while
        // the window applies the wish after its next event.
        self.text_input.set_enabled(!playing);
        let navigated = match &mut self.host {
            Some(host) if playing => {
                host.tick_game(now, events, &mut snapshot.play);
                self.state.follow_pointer(events);
                self.state.watch_hotkeys(events);
                self.state.tick(now, &[], snapshot)
            }
            _ => self.state.tick(now, events, snapshot),
        };
        if navigated == Navigated::Exit {
            self.exited.store(true, Ordering::Relaxed);
            return;
        }
        match &mut self.host {
            Some(host) => host.advance(now, &mut self.state),
            // Nothing plays without an audio output.
            None => {
                if self.state.take_play_request().is_some() {
                    self.state.finish_loading(now, None);
                }
            }
        }
        self.apply_settings(now);
    }
}

struct Resources {
    device: GlDevice<SdlGlSurface>,
    textures: Textures<GlTexture>,
    renderer: SpriteRenderer<GlDevice<SdlGlSurface>>,
    /// Draws MSDF glyphs.
    text_renderer: SpriteRenderer<GlDevice<SdlGlSurface>>,
    white: AtlasRegion,
}

/// The draw thread's side. The device and its resources exist between `start` and `stop`.
struct View {
    surface: Option<SdlGlSurface>,
    resources: Option<Resources>,
    theme: Theme,
    glyphs: GlyphCache,
    menu: MenuView,
    song_select: SongSelectView,
    results: ResultsView,
    settings: SettingsView,
    /// Skin dropdowns for a new songs folder (ADR-0064).
    skin_views: Option<SkinViews>,
    shapes: SpriteBatch,
    text: SpriteBatch,
    /// Scenes of plays the update thread starts, and the one shown.
    scenes: Option<rtrb::Consumer<(u64, PlayScene)>>,
    scene: Option<(u64, PlayScene)>,
    /// `scene` was on a layer: a snapshot without it now means it went (the scene arrives
    /// before the snapshot that shows it).
    scene_shown: bool,
    /// Takes scenes no longer shown, to free them off this thread.
    scene_trash: Option<SceneTrash>,
    /// Read by the update thread for the layout.
    surface_size: Arc<AtomicU64>,
    /// The FPS counter over every screen (ADR-0060).
    fps: FpsOverlay,
    /// The menu's texture loader and the song select's background (ADR-0066).
    backgrounds: Option<(TextureUploads, BackgroundFade)>,
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
fn retire(
    scene: &mut Option<(u64, PlayScene)>,
    trash: &mut Option<SceneTrash>,
    device: &mut GlDevice<SdlGlSurface>,
) {
    if let Some((_, mut old)) = scene.take() {
        old.stop(device);
        if let Some(trash) = trash {
            trash.free(old);
        }
    }
}

impl Draw<ShellSnapshot> for View {
    fn start(&mut self) {
        // The window thread made the context and released it (ADR-0018).
        let surface = self.surface.take().expect("surface is set before start");
        // The same context made a device in `device_limits` already; failing now is a
        // driver fault this developer shell does not recover from.
        let mut device = GlDevice::new(surface, GlDebug::for_build()).expect("GL device");
        let renderer = SpriteRenderer::new(&mut device, DEFAULT_SLOTS).expect("renderer");
        let text_renderer = SpriteRenderer::with_shader(&mut device, DEFAULT_SLOTS, &shaders::MSDF)
            .expect("text renderer");
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
        self.resources = Some(Resources {
            device,
            textures,
            renderer,
            text_renderer,
            white,
        });
    }

    fn frame(&mut self, info: &FrameInfo, snapshot: &ShellSnapshot) {
        let r = self.resources.as_mut().expect("started");
        let size = r.device.begin_frame(self.theme.background);
        if let Some(scenes) = &mut self.scenes
            && let Ok((id, mut scene)) = scenes.pop()
        {
            retire(&mut self.scene, &mut self.scene_trash, &mut r.device);
            // The glyphs come in over the first frames; the lead-in runs meanwhile.
            scene.start(&mut r.device, Duration::ZERO, None);
            self.scene = Some((id, scene));
            self.scene_shown = false;
        }
        if let Some((id, _)) = &self.scene {
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
        self.glyphs.process(&mut r.device, &mut r.textures);
        // On every screen: releases queued during a play would otherwise fill the queue.
        if let Some((uploads, fade)) = &mut self.backgrounds {
            uploads.process(&mut r.device, &mut r.textures);
            fade.advance(snapshot.background, info.wait.woke_ns, |h| uploads.get(h));
        }
        let (width, height) = (size.width as f32, size.height as f32);
        let gameplay = snapshot
            .layers
            .iter()
            .flatten()
            .last()
            .is_some_and(|layer| matches!(layer.view, ScreenView::Play(_)));
        self.fps
            .frame(info.wait.woke_ns, width, height, snapshot.fps, gameplay);
        self.shapes.clear();
        self.text.clear();
        // Where each layer's sprites end: layers are drawn one after another, shapes then
        // text, so a screen coming in covers the text of the one leaving.
        let mut ends = [(0_u32, 0_u32, false); MAX_LAYERS];
        let mut layers = 0;
        for layer in snapshot.layers.iter().flatten() {
            match &layer.view {
                ScreenView::Menu(menu) => self.menu.push(
                    &mut self.shapes,
                    &mut self.text,
                    &mut self.glyphs,
                    width,
                    height,
                    menu,
                    layer.pose,
                    r.white,
                ),
                ScreenView::SongSelect(song_select) => {
                    // Under the screen and without its slide, as lazer's background stack.
                    if let Some((uploads, fade)) = &self.backgrounds {
                        fade.push(
                            &mut self.shapes,
                            width,
                            height,
                            info.wait.woke_ns,
                            layer.pose.opacity,
                            |h| match uploads.get(h) {
                                TextureState::Ready(region) => Some(region),
                                _ => None,
                            },
                        );
                    }
                    self.song_select.push(
                        &mut self.shapes,
                        &mut self.text,
                        &mut self.glyphs,
                        width,
                        height,
                        song_select,
                        layer.pose,
                        r.white,
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
                ScreenView::Settings(settings) => self.settings.push(
                    &mut self.shapes,
                    &mut self.text,
                    &mut self.glyphs,
                    width,
                    height,
                    settings,
                    layer.pose,
                    r.white,
                ),
                // Drawn whole, without the transition's slide.
                ScreenView::Play(_) => {}
            }
            let play = matches!(layer.view, ScreenView::Play(_));
            ends[layers] = (self.shapes.len() as u32, self.text.len() as u32, play);
            layers += 1;
        }
        // Last, over every screen and the play (ADR-0060).
        let overlay_from = (self.shapes.len() as u32, self.text.len() as u32);
        self.fps
            .push(&mut self.shapes, &mut self.text, &mut self.glyphs, r.white);
        r.renderer.begin_frame();
        r.text_renderer.begin_frame();
        let gpu_textures = &r.textures;
        let projection = Projection::pixels(size);
        let texture = |id| gpu_textures.get(id);
        // A failed draw or present loses one frame; the next one tries again.
        let shapes = r
            .renderer
            .upload(&mut r.device, &self.shapes, &projection, texture);
        let text = r
            .text_renderer
            .upload(&mut r.device, &self.text, &projection, texture);
        let mut from = (0, 0);
        for &(shapes_end, text_end, play) in &ends[..layers] {
            if play {
                // Only the scene of the play on the layer: another one's snapshot would
                // mislead its views.
                if let Some((id, scene)) = &mut self.scene
                    && scene_on_layers(snapshot, *id)
                {
                    scene.draw(&mut r.device, size, info.wait.woke_ns, &snapshot.play, None);
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
        let _ = r.device.present();
    }

    fn stop(&mut self) {
        let Resources {
            mut device,
            mut textures,
            renderer,
            text_renderer,
            ..
        } = self.resources.take().expect("started");
        retire(&mut self.scene, &mut self.scene_trash, &mut device);
        renderer.destroy(&mut device);
        text_renderer.destroy(&mut device);
        for texture in textures.drain() {
            device.destroy_texture(texture);
        }
        self.surface = Some(device.into_surface().expect("release GL context"));
    }
}

impl View {
    fn request_glyphs(&mut self) -> usize {
        self.menu.request_glyphs(&mut self.glyphs)
            + self.song_select.request_glyphs(&mut self.glyphs)
            + self.results.request_glyphs(&mut self.glyphs)
            + self.settings.request_glyphs(&mut self.glyphs)
            + self.fps.request_glyphs(&mut self.glyphs)
    }
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

/// Opens the output the settings name with the mixer and music player.
fn open_audio(
    settings: &crate::settings::Audio,
) -> Result<(Audio, slam_audio::OutputStream), String> {
    let backend = pick_backend(settings.backend.as_deref(), Backend::available())
        .ok_or("no audio backend in this build")?;
    let (mut mixer, mut mixer_handle) = slam_audio::mixer(&MixerConfig {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        ..MixerConfig::default()
    })
    .map_err(|e| e.to_string())?;
    let (music, mut player) = slam_audio::music(&MusicConfig {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        ..MusicConfig::default()
    })
    .map_err(|e| e.to_string())?;
    mixer.set_music(music).map_err(|e| e.to_string())?;
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
    let stream = slam_audio::open_output(
        backend,
        StreamConfig {
            device: settings.device.clone(),
            sample_rate: SAMPLE_RATE,
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
        mixer: mixer_handle,
        volumes: AppliedVolumes::new(volumes),
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
        Some(dirs) => crate::settings::load_or_create(&dirs.settings_file()).settings,
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
    let theme = Theme::default();
    let mut window =
        InputWindow::new_opengl_in("SLAM", settings.window.state()).map_err(|e| e.to_string())?;
    let surface = SdlGlSurface(window.create_gl_context().map_err(|e| e.to_string())?);
    let (pixels_w, pixels_h) = surface.0.size_in_pixels();
    let scale = window.window_scale();
    let pixels_per_point = scale.pixels_per_point();
    let initial_size = pack_size(pixels_w, pixels_h);
    let surface_size = Arc::new(AtomicU64::new(initial_size));
    let (surface, limits) = device_limits(surface)?;
    let registry = Arc::new(FontRegistry::with_builtin());
    let glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits))
        .map_err(|e| e.to_string())?;
    // English until the language setting exists (#98).
    let localizer = Arc::new(Localizer::builtin().map_err(|e| e.to_string())?);
    let text = |key: &str| localizer.format(key, None).into_owned();
    let labels = MenuLabels {
        title: text("menu-title"),
        buttons: [
            text("menu-play-button"),
            text("menu-settings-button"),
            text("menu-exit-button"),
        ],
    };
    let menu = MenuView::new(Arc::clone(&registry), theme, &labels);
    let song_labels = SongSelectLabels {
        no_folder: text("song-select-no-folder"),
        failed: text("song-select-failed"),
        loading: text("song-select-loading"),
        empty: text("song-select-empty"),
        choose_folder: text("song-select-choose-folder"),
        hint: text("song-select-hint"),
        search_placeholder: text("song-select-search-placeholder"),
        search_no_matches: text("song-select-search-no-matches"),
        mods_title: text("song-select-mods-title"),
        mods_hint: text("song-select-mods-hint"),
        mods_customise: text("song-select-mods-customise"),
        setting_texts: mod_settings::TEXT_KEYS
            .iter()
            .map(|key| (*key, text(key)))
            .collect(),
        mod_column_titles: [
            text("song-select-mods-reduction"),
            text("song-select-mods-increase"),
            text("song-select-mods-conversion"),
        ],
        details: DetailLabels {
            length: text("song-select-details-length"),
            bpm: text("song-select-details-bpm"),
            circles: text("song-select-details-circles"),
            sliders: text("song-select-details-sliders"),
            spinners: text("song-select-details-spinners"),
            circle_size: text("song-select-details-circle-size"),
            approach_rate: text("song-select-details-approach-rate"),
            overall_difficulty: text("song-select-details-overall-difficulty"),
            drain_rate: text("song-select-details-drain-rate"),
        },
        mod_columns: mod_columns(),
    };
    let song_select = SongSelectView::new(Arc::clone(&registry), theme, &song_labels);
    let results_labels = ResultsLabels {
        score: text("results-score-title"),
        accuracy: text("results-accuracy-title"),
        max_combo: text("results-max-combo-title"),
        mods: text("results-mods-title"),
        no_mods: text("results-no-mods"),
        stats: StatKind::ALL.map(|kind| text(results_label_key(kind))),
        retry: text("results-retry-button"),
        back: text("results-back-button"),
        hint: text("results-hint"),
    };
    let results = ResultsView::new(Arc::clone(&registry), theme, &results_labels);

    let mut state = ShellState::new(theme, pixels_w as f32, pixels_h as f32, pixels_per_point)
        .with_settings(settings.clone());
    let settings_view = SettingsView::new(
        Arc::clone(&registry),
        theme,
        &state.rows().specs(&settings),
        &state.rows().labels(&localizer),
    );
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
        };
        // Without the menu's texture loader the song select shows the theme's background.
        let (loader, fade) = match texture_loader(&TextureLoaderConfig {
            max_textures: MAX_BACKGROUNDS,
            request_capacity: MAX_BACKGROUNDS as usize,
            job_capacity: 4,
            ..TextureLoaderConfig::new(limits)
        }) {
            Ok((requests, uploads)) => {
                let (picker, loader, fade) = crate::backgrounds::link(requests);
                state = state.with_backgrounds(picker);
                (Some(loader), Some((uploads, fade)))
            }
            Err(e) => {
                tracing::warn!("backgrounds: cannot start the texture loader: {e}");
                (None, None)
            }
        };
        let (picker, reader) = crate::details::link();
        state = state.with_details(picker);
        let (link, skin_views, thread) = crate::songs::spawn(
            settings.paths.songs.clone(),
            folders,
            scanning,
            loader,
            Some(reader),
        )
        .map_err(|e| e.to_string())?;
        state = state.with_songs(link);
        (Some(skin_views), thread, fade)
    };
    // The play: a loader thread, scenes for the draw thread, the audio output.
    let hud_labels = crate::play::scene::hud_labels(&localizer);
    let (loader, scene_trash, _loader) = crate::loader::spawn(LoaderConfig {
        // The skin travels with each request; this is what the first one starts from.
        settings: {
            let options = live::play_options(&settings);
            PlaySettings {
                beatmap_skins: options.beatmap_skins,
                beatmap_colours: options.beatmap_colours,
                beatmap_hitsounds: options.beatmap_hitsounds,
                dim: options.dim,
                hit_error_scale: options.hit_error_scale,
                skin: options.skin,
                ..PlaySettings::default()
            }
        },
        limits,
        registry: Arc::clone(&registry),
        hud_labels: hud_labels.clone(),
        replays: Dirs::detect().map(|dirs| dirs.replays()),
    })
    .map_err(|e| e.to_string())?;
    let (scene_tx, scene_rx) = rtrb::RingBuffer::<(u64, PlayScene)>::new(2);
    // Without an output the window still opens; the song select does not play.
    let (audio, stream) = match open_audio(&settings.audio) {
        Ok((audio, stream)) => (Some(audio), Some(stream)),
        Err(e) => {
            tracing::warn!("audio: {e}");
            (None, None)
        }
    };
    let host = audio.map(|audio| PlayHost {
        audio,
        loader,
        scenes: scene_tx,
        os_cursor: window.cursor_visibility(),
        pending: Pending::Idle,
        game: None,
        retiring: None,
        track_reset: false,
        next_request: 0,
        points: [
            pixels_w as f32 / pixels_per_point,
            pixels_h as f32 / pixels_per_point,
        ],
        input: PlayInput::new(&settings),
        beatmap_offsets: HashMap::new(),
    });

    let exited = Arc::new(AtomicBool::new(false));
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
        window: window.window_settings(),
        window_sent: settings.window.state(),
        draw_rate: draw_rate.clone(),
        rate_switch: DrawRateSwitch::new(menu_rate),
        update_rate: AppliedUpdateRate::new(update_rate.clone(), settings.frame.update_hz),
        saver,
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
        shapes: SpriteBatch::with_capacity(MAX_SHAPES),
        text: SpriteBatch::with_capacity(MAX_GLYPH_SPRITES),
        scenes: Some(scene_rx),
        scene: None,
        scene_shown: false,
        scene_trash: Some(scene_trash),
        surface_size,
        fps: FpsOverlay::new(
            FpsSource::new(
                Arc::clone(&counters),
                update_rate.clone(),
                draw_rate.clone(),
            ),
            Arc::clone(&registry),
            &hud_labels.fps,
        ),
        backgrounds,
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
    // The GL context goes before the window.
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
    drop(stream);
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
    drop(shell);
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
