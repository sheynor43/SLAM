//! The window with the screens (ADR-0044): the main menu, the song select and the play,
//! prepared on worker threads (ADR-0045). Opened by `slam-app` without a command. Console
//! output is not localised; UI text is.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use slam_audio::{
    AudioPosition, Backend, MixerConfig, MixerHandle, MusicConfig, MusicEvent, MusicPlayer,
    MusicPosition, StreamConfig, TrackSource, VoiceGroup,
};
use slam_engine::clock::now_ns;
use slam_engine::gl::SdlGlSurface;
use slam_engine::{Draw, Engine, EngineConfig, FrameInfo, TickInfo, Update};
use slam_input::{
    CursorVisibility, InputEvent, InputKind, InputWindow, MouseButton, TickMapper, event_ring,
    sdl_ticks_ns,
};
use slam_play::{
    Bindings, LiveInput, Offsets, PlaySession, PlaySnapshot, PlayState, PlayfieldLayout,
};
use slam_render::atlas::{Atlas, AtlasRegion};
use slam_render::gl::{GlDebug, GlDevice, GlTexture};
use slam_render::sprite::{DEFAULT_SLOTS, Projection, SpriteBatch, SpriteRenderer};
use slam_render::textures::Textures;
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
use slam_ui::song_select::{
    SongList, SongSelect, SongSelectAction, SongSelectInput, SongSelectLabels, SongSelectLayout,
    SongSelectSnapshot, SongSelectStatus, SongSelectView,
};
use slam_ui::theme::Theme;

use crate::loader::{LoaderConfig, LoaderLink, SceneTrash, Trash};
use crate::play::scene::{CHANNELS, PlayScene, PlaySettings, Prepared, SAMPLE_RATE};
use crate::play::{device_limits, pack_size};
use crate::settings::{Dirs, Settings};
use crate::songs::{SongFiles, SongsLink};

mod results;

pub const USAGE: &str = "usage: slam-app [--songs DIR]\n\
                         slam-app [bench|click|click-analyze|play|i18n] ...\n\
                         without a command, opens the window with the main menu; --songs is \
                         the folder of beatmap sets (as stable's Songs) and overrides \
                         paths.songs of slam.toml";

const WINDOW: (i32, i32) = (1280, 720);
/// Textures of the draw thread: the white square and the glyph pages.
const MAX_TEXTURES: usize = 16;
/// Rectangles of one frame: the buttons or rows of up to two layers.
const MAX_SHAPES: usize = 256;
/// Glyph sprites of one frame: the texts of up to two layers.
const MAX_GLYPH_SPRITES: usize = 8192;
/// How long the draw thread waits for the glyphs before the first frame.
const GLYPH_WARM_TIMEOUT: Duration = Duration::from_secs(1);

// USB HID usage ids, as `SDL_Scancode`.
const KEY_ENTER: u16 = 40;
const KEY_ESCAPE: u16 = 41;
const KEY_R: u16 = 21;
const KEY_HOME: u16 = 74;
const KEY_PAGE_UP: u16 = 75;
const KEY_END: u16 = 77;
const KEY_PAGE_DOWN: u16 = 78;
const KEY_RIGHT: u16 = 79;
const KEY_LEFT: u16 = 80;
const KEY_DOWN: u16 = 81;
const KEY_UP: u16 = 82;
const KEY_KEYPAD_ENTER: u16 = 88;

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
pub enum ScreenView {
    Menu(MenuSnapshot),
    SongSelect(SongSelectSnapshot),
    /// The play of this id, drawn from [`ShellSnapshot::play`]. Only while it is on top:
    /// a play going out leaves at once instead of sliding with a stale snapshot.
    Play(u64),
    Results(ResultsSnapshot),
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
}

/// The update logic without the window: input to navigation to snapshot.
pub struct ShellState {
    stack: ScreenStack<AppScreen>,
    theme: Theme,
    /// Layouts in pixels, the unit the draw thread lays out in.
    layout: MenuLayout,
    song_layout: SongSelectLayout,
    results_layout: ResultsLayout,
    song_select: SongSelect,
    /// The library's end, without a songs folder `None`.
    songs: Option<SongsLink>,
    /// The `.osu` files of the song select's list, by set and difficulty.
    files: Arc<SongFiles>,
    /// The lists before the first publication, kept so that replacing them frees nothing
    /// on the update thread.
    _empty: (Arc<SongList>, Arc<SongFiles>, Arc<str>),
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
    /// Last cursor position in pixels.
    cursor: [f32; 2],
    /// Input arrives in window points, layout is in pixels.
    pixels_per_point: f32,
}

impl ShellState {
    pub fn new(theme: Theme, width: f32, height: f32, pixels_per_point: f32) -> ShellState {
        let empty: (Arc<SongList>, Arc<SongFiles>, Arc<str>) =
            (Arc::default(), Arc::from([]), Arc::from(""));
        let mut song_select = SongSelect::new();
        song_select.set_list(Arc::clone(&empty.0));
        song_select.set_scanning_text(Arc::clone(&empty.2));
        ShellState {
            stack: ScreenStack::new(AppScreen::Menu(MainMenu::new())),
            layout: MenuLayout::new(width, height, &theme),
            song_layout: SongSelectLayout::new(width, height, &theme),
            results_layout: ResultsLayout::new(width, height, &theme),
            song_select,
            songs: None,
            files: Arc::clone(&empty.1),
            _empty: empty,
            library_status: SongSelectStatus::NoFolder,
            loading: false,
            play_request: None,
            last_file: None,
            last_title: String::new(),
            held_results: Vec::new(),
            retry_request: false,
            track_stop: false,
            theme,
            cursor: [0.0, 0.0],
            pixels_per_point,
        }
    }

    /// A shell whose song select shows what the library thread publishes; without a
    /// link, it says that no songs folder was given.
    pub fn with_songs(mut self, songs: SongsLink) -> ShellState {
        self.library_status = SongSelectStatus::Scanning { sets: 0 };
        self.song_select.set_status(self.library_status);
        self.songs = Some(songs);
        self
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

    /// Follows the pointer while the play has the input, so that the screen under it
    /// knows where it is when the play goes.
    pub fn follow_pointer(&mut self, events: &[InputEvent]) {
        for event in events {
            if let InputKind::MouseMove { x, y } = event.kind {
                self.cursor = [x * self.pixels_per_point, y * self.pixels_per_point];
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
        self.files.get(set)?.get(difficulty)
    }

    /// Lays the screens out for a window of `width` × `height` pixels.
    pub fn set_size(&mut self, width: f32, height: f32) {
        self.layout = MenuLayout::new(width, height, &self.theme);
        self.song_layout = SongSelectLayout::new(width, height, &self.theme);
        self.results_layout = ResultsLayout::new(width, height, &self.theme);
    }

    /// The input of the screens, as the song select (the larger set) understands it, and
    /// the key only the results use.
    fn screen_input(&mut self, kind: InputKind) -> Option<ScreenInput> {
        Some(ScreenInput::Common(match kind {
            InputKind::Key {
                scancode,
                down: true,
            } => match scancode {
                KEY_R => return Some(ScreenInput::Retry),
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
                _ => return None,
            },
            InputKind::MouseButton {
                button: MouseButton::X1,
                down: true,
            } => SongSelectInput::Back,
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
        self.song_select.set_list(published.list);
        self.library_status = published.status;
        if !self.loading {
            self.song_select.set_status(published.status);
        }
        self.song_select.set_scanning_text(published.scanning_text);
        self.files = published.files;
    }

    /// Gives `events` to the top screen and applies what it asks for at `now_ns`. Stops at
    /// the first exit.
    pub fn handle(&mut self, events: &[InputEvent], now_ns: u64) -> Navigated {
        for event in events {
            let Some(input) = self.screen_input(event.kind) else {
                continue;
            };
            let common = input.common();
            let nav = match self.stack.top_mut() {
                AppScreen::Menu(menu) => {
                    match common
                        .and_then(menu_input)
                        .and_then(|input| menu.input(input, &self.layout))
                    {
                        Some(MenuAction::Play) => Some(Nav::Push(AppScreen::SongSelect)),
                        Some(MenuAction::Exit) => Some(Nav::Exit),
                        None => None,
                    }
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
        navigated
    }
}

/// The input of a screen: the song select's set, plus the key of the results.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ScreenInput {
    Common(SongSelectInput),
    Retry,
}

impl ScreenInput {
    fn common(self) -> Option<SongSelectInput> {
        match self {
            ScreenInput::Common(input) => Some(input),
            ScreenInput::Retry => None,
        }
    }
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
}

/// A quit play whose voices the mixer silences. The session keeps its samples until the
/// mixer has handed all of theirs back, so that the update thread never drops the last
/// reference to one.
struct Retiring {
    session: Box<PlaySession>,
    /// Since when the mixer has had no voice.
    idle_since: Option<u64>,
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

    /// Takes requests, prepared plays and track events; ends a quit play. Allocates only
    /// when a play is asked for, starts or ends; logs and frees on the loader thread.
    fn advance(&mut self, now_ns: u64, state: &mut ShellState) {
        self.end_game(now_ns, state);
        if state.take_track_stop() {
            // A failed command leaves the track playing under the song select.
            let _ = self.audio.player.pause();
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
                Some(map) if self.game.is_none() => self.loader.request(self.next_request, map),
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
                self.loader.free(Trash::Other(old.session));
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
                        self.loader.free(Trash::Other(retiring.session));
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
            ..
        } = prepared;
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
    /// Input arrives in window points, the surface is in pixels.
    points_per_pixel: f32,
    /// Set when the menu quits; the window closes then.
    exited: Arc<AtomicBool>,
}

impl Update for Shell {
    type Snapshot = ShellSnapshot;

    fn tick(&mut self, info: &TickInfo, events: &[InputEvent], snapshot: &mut ShellSnapshot) {
        let size = self.surface.load(Ordering::Relaxed);
        // A minimised window may report 0×0: keep the last layout.
        if size != self.layout_size && size >> 32 != 0 && size & 0xffff_ffff != 0 {
            self.layout_size = size;
            let (w, h) = ((size >> 32) as f32, (size & 0xffff_ffff) as f32);
            self.state.set_size(w, h);
            if let Some(host) = &mut self.host {
                let ppp = self.points_per_pixel;
                host.set_size(w * ppp, h * ppp);
            }
        }
        // Between the exit and the window closing, input no longer navigates.
        if self.exited.load(Ordering::Relaxed) {
            return;
        }
        let now = info.now_ns;
        let playing = self.state.playing();
        let navigated = match &mut self.host {
            Some(host) if playing => {
                host.tick_game(now, events, &mut snapshot.play);
                self.state.follow_pointer(events);
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
            scene.start(&mut r.device, Duration::ZERO);
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
        self.glyphs.process(&mut r.device, &mut r.textures);
        let (width, height) = (size.width as f32, size.height as f32);
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
                ScreenView::SongSelect(song_select) => self.song_select.push(
                    &mut self.shapes,
                    &mut self.text,
                    &mut self.glyphs,
                    width,
                    height,
                    song_select,
                    layer.pose,
                    r.white,
                ),
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
            let play = matches!(layer.view, ScreenView::Play(_));
            ends[layers] = (self.shapes.len() as u32, self.text.len() as u32, play);
            layers += 1;
        }
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
                    scene.draw(&mut r.device, size, info.wait.woke_ns, &snapshot.play);
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
    // The mixer's master gain leaves the music alone, so the track takes both.
    let (master, music) = volumes(settings);
    mixer_handle
        .set_master_gain(master)
        .map_err(|e| e.to_string())?;
    player.set_volume(music).map_err(|e| e.to_string())?;
    mixer_handle
        .set_group_gain(VoiceGroup::Hitsounds, settings.hitsound_volume as f32)
        .map_err(|e| e.to_string())?;
    mixer_handle
        .set_group_gain(VoiceGroup::Effects, settings.effects_volume as f32)
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
    };
    Ok((audio, stream))
}

/// The mixer's master gain and the track's volume.
fn volumes(settings: &crate::settings::Audio) -> (f32, f32) {
    let master = settings.master_volume as f32;
    (master, master * settings.music_volume as f32)
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
/// `songs` from the command line overrides the file's folder. Without a settings folder or
/// with a broken file, the defaults.
pub fn load_settings(songs: Option<PathBuf>) -> Settings {
    let settings = match Dirs::detect() {
        Some(dirs) => crate::settings::load_or_create(&dirs.settings_file()).settings,
        None => {
            tracing::warn!("settings: no settings folder, using the defaults");
            Settings::default()
        }
    };
    with_songs(settings, songs)
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
        InputWindow::new_opengl("SLAM", WINDOW.0, WINDOW.1).map_err(|e| e.to_string())?;
    let surface = SdlGlSurface(window.create_gl_context().map_err(|e| e.to_string())?);
    let (pixels_w, pixels_h) = surface.0.size_in_pixels();
    let pixels_per_point = if pixels_w > 0 {
        pixels_w as f32 / WINDOW.0 as f32
    } else {
        1.0
    };
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
        hint: text("song-select-hint"),
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

    let mut state = ShellState::new(theme, pixels_w as f32, pixels_h as f32, pixels_per_point);
    // Joined when `run` returns, after the engine.
    let mut _library = None;
    if let Some(songs) = settings.paths.songs.clone() {
        let localizer = Arc::clone(&localizer);
        let scanning = Box::new(move |count: usize| {
            let mut args = slam_ui::i18n::FluentArgs::new();
            args.set("count", count);
            localizer
                .format("song-select-scanning", Some(&args))
                .into_owned()
        });
        let (link, thread) = crate::songs::spawn(songs, scanning).map_err(|e| e.to_string())?;
        state = state.with_songs(link);
        _library = Some(thread);
    }
    // The play: a loader thread, scenes for the draw thread, the audio output.
    let (loader, scene_trash, _loader) = crate::loader::spawn(LoaderConfig {
        settings: PlaySettings {
            beatmap_colours: settings.gameplay.beatmap_colours,
            dim: settings.gameplay.dim,
            ..PlaySettings::default()
        },
        limits,
        registry: Arc::clone(&registry),
        hud_labels: crate::play::scene::hud_labels(&localizer),
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
    let points_per_pixel = 1.0 / pixels_per_point;
    let host = audio.map(|audio| PlayHost {
        audio,
        loader,
        scenes: scene_tx,
        os_cursor: window.cursor_visibility(),
        pending: Pending::Idle,
        game: None,
        retiring: None,
        next_request: 0,
        points: [
            pixels_w as f32 * points_per_pixel,
            pixels_h as f32 * points_per_pixel,
        ],
        input: PlayInput::new(&settings),
        beatmap_offsets: HashMap::new(),
    });

    let exited = Arc::new(AtomicBool::new(false));
    let shell = Shell {
        state,
        host,
        surface: Arc::clone(&surface_size),
        layout_size: initial_size,
        points_per_pixel,
        exited: Arc::clone(&exited),
    };
    let draw = View {
        surface: Some(surface),
        resources: None,
        theme,
        glyphs,
        menu,
        song_select,
        results,
        shapes: SpriteBatch::with_capacity(MAX_SHAPES),
        text: SpriteBatch::with_capacity(MAX_GLYPH_SPRITES),
        scenes: Some(scene_rx),
        scene: None,
        scene_shown: false,
        scene_trash: Some(scene_trash),
        surface_size,
        _quit: crate::bench::QuitOnDrop(window.quit_handle()),
    };
    let (mut sink, source) = event_ring(1024);
    let engine = Engine::spawn(
        EngineConfig {
            update_hz: settings.frame.update_hz,
            draw: settings.frame.draw.to_limiter(),
            ..EngineConfig::default()
        },
        shell,
        draw,
        ShellSnapshot::default(),
        source,
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
    drop(shell);
    drop(window);
    ran.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
