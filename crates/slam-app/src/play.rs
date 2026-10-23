//! Plays a beatmap given on the command line: hit objects, the skinned cursor and HUD, music
//! and hitsounds, no menus. Developer command until the screen stack and song select exist:
//! its console output is not localised.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use slam_audio::{
    AudioPosition, Backend, MixerConfig, MixerHandle, MusicConfig, MusicEvent, MusicPlayer,
    MusicPosition, StreamConfig, TrackSource,
};
use slam_engine::clock::now_ns;
use slam_engine::gl::SdlGlSurface;
use slam_engine::limiter::{LimiterMode, MAX_HZ, MIN_HZ, is_valid_hz};
use slam_engine::{Draw, Engine, EngineConfig, FrameInfo, TickInfo, Update};
use slam_input::{CursorVisibility, InputEvent, InputWindow, TickMapper, event_ring, sdl_ticks_ns};
use slam_osu::judgement::HitResult;
use slam_osu::{Mod, ModSet};
use slam_play::{
    BackgroundSettings, Bindings, LiveInput, Offsets, PlaySession, PlaySnapshot, PlayfieldLayout,
};
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::{Color, Device};
use slam_text::FontRegistry;
use slam_ui::i18n::Localizer;

use crate::bench::number;

pub mod fps;
mod perf;
pub mod scene;
use perf::{FramePerf, ReportSources, SharedFrameTimes};
use scene::{CHANNELS, PlayScene, PlaySettings, Prepared, SAMPLE_RATE};

pub const USAGE: &str = "usage: slam-app play MAP.osu [--skin DIR] [--backend NAME] \
                         [--device ID] [--buffer FRAMES] [--fps HZ|unlimited] \
                         [--update-hz HZ] [--mods HDHR] [--dim PERCENT] [--no-beatmap-skins] [--no-beatmap-colours] [--no-beatmap-hitsounds] [--show-fps] [--no-mouse-buttons] [--no-stats]";

const WINDOW: (i32, i32) = (1280, 720);
/// How long the track may take to open.
const LOAD_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the draw thread waits for the HUD's glyphs before the first frame.
const GLYPH_WARM_TIMEOUT: Duration = Duration::from_secs(1);

/// The bundled default skin, in the source tree.
pub(crate) fn default_skin_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/default-skin")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub map: PathBuf,
    /// User skin folder; without it, the default skin.
    pub skin: Option<PathBuf>,
    /// Audio backend (`Backend::name`); `None` takes the first available.
    pub backend: Option<Backend>,
    /// Output device id; `None` follows the system default.
    pub device: Option<String>,
    /// Requested frames per audio buffer.
    pub buffer_frames: u32,
    /// Print the once-a-second performance line to stderr; on by default.
    pub stats: bool,
    /// Draw rate.
    pub draw: LimiterMode,
    /// Update rate in Hz.
    pub update_hz: f64,
    /// Use the beatmap's own skin textures and configuration (lazer's `BeatmapSkins`); on by default.
    pub beatmap_skins: bool,
    /// Take colours from the beatmap's skin (lazer's `BeatmapColours`); on by default.
    pub beatmap_colours: bool,
    /// Play the beatmap's own hitsound samples (lazer's `BeatmapHitsounds`); on by default.
    pub beatmap_hitsounds: bool,
    /// Show the FPS counter (lazer's `ShowFpsDisplay`); off by default.
    pub show_fps: bool,
    /// Mouse buttons press in gameplay (lazer's `MouseDisableButtons` off); on by default,
    /// toggled with F10.
    pub mouse_buttons: bool,
    /// Mods of the play; without `--mods`, none.
    pub mods: ModSet,
    /// How dark the background is in gameplay, 0 ..= 1 (lazer's `DimLevel`); `--dim` takes
    /// it in whole percent.
    pub dim: f64,
    /// Size multiplier of the hit error meter.
    pub hit_error_scale: f64,
}

impl Options {
    /// What the play takes from the options.
    pub fn settings(&self) -> PlaySettings {
        PlaySettings {
            skin: self.skin.clone(),
            beatmap_skins: self.beatmap_skins,
            beatmap_colours: self.beatmap_colours,
            beatmap_hitsounds: self.beatmap_hitsounds,
            mods: self.mods.clone(),
            dim: self.dim,
            hit_error_scale: self.hit_error_scale,
        }
    }

    /// Parses the arguments after `play`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut map = None;
        let mut options = Self {
            map: PathBuf::new(),
            skin: None,
            backend: None,
            device: None,
            buffer_frames: StreamConfig::default().buffer_frames,
            stats: true,
            draw: EngineConfig::default().draw,
            update_hz: EngineConfig::default().update_hz,
            beatmap_skins: true,
            beatmap_colours: true,
            beatmap_hitsounds: true,
            show_fps: false,
            mouse_buttons: true,
            mods: ModSet::default(),
            dim: BackgroundSettings::default().dim_level,
            hit_error_scale: 1.0,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--skin" => options.skin = Some(value()?.into()),
                "--backend" => {
                    let name = value()?;
                    options.backend = Some(
                        Backend::from_name(&name)
                            .ok_or_else(|| format!("unknown audio backend {name}"))?,
                    );
                }
                "--device" => options.device = Some(value()?),
                "--no-stats" => options.stats = false,
                "--no-beatmap-skins" => options.beatmap_skins = false,
                "--no-beatmap-colours" => options.beatmap_colours = false,
                "--no-beatmap-hitsounds" => options.beatmap_hitsounds = false,
                "--show-fps" => options.show_fps = true,
                "--no-mouse-buttons" => options.mouse_buttons = false,
                "--mods" => options.mods = parse_mods(&value()?)?,
                "--fps" => {
                    let fps = value()?;
                    options.draw = if fps == "unlimited" {
                        LimiterMode::Unlimited
                    } else {
                        LimiterMode::Hz(rate(&arg, &fps)?)
                    };
                }
                "--update-hz" => options.update_hz = rate(&arg, &value()?)?,
                "--dim" => {
                    // Lazer's setting moves in steps of 0.01.
                    let percent: f64 = number(&arg, &value()?)?;
                    if !(0.0..=100.0).contains(&percent) || percent.fract() != 0.0 {
                        return Err("--dim must be a whole percent in [0, 100]".into());
                    }
                    options.dim = percent / 100.0;
                }
                "--buffer" => {
                    let frames: f64 = number(&arg, &value()?)?;
                    if frames < 1.0 || frames.fract() != 0.0 || frames > 8192.0 {
                        return Err("--buffer must be a whole number in [1, 8192]".into());
                    }
                    options.buffer_frames = frames as u32;
                }
                flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
                _ if map.is_none() => map = Some(PathBuf::from(arg)),
                _ => return Err(format!("unexpected argument {arg}")),
            }
        }
        options.map = map.ok_or("no beatmap given")?;
        Ok(options)
    }
}

/// The mods of `--mods`: two-letter acronyms run together, in any letter case and order.
/// The mods take their default settings; the set must be one lazer accepts.
fn parse_mods(value: &str) -> Result<ModSet, String> {
    if value.is_empty() || !value.is_ascii() || !value.len().is_multiple_of(2) {
        return Err(format!(
            "--mods {value}: expected two-letter acronyms run together, like HDHR"
        ));
    }
    let mut mods = Vec::new();
    for i in (0..value.len()).step_by(2) {
        let acronym = value[i..i + 2].to_ascii_uppercase();
        let m = Mod::from_acronym(&acronym);
        match m {
            Mod::NoFail
            | Mod::Easy { .. }
            | Mod::HardRock
            | Mod::Hidden { .. }
            | Mod::SuddenDeath { .. }
            | Mod::Perfect { .. }
            | Mod::HalfTime(_)
            | Mod::Daycore { .. }
            | Mod::DoubleTime(_)
            | Mod::Nightcore { .. }
            | Mod::DifficultyAdjust(_)
            | Mod::Mirror { .. }
            | Mod::Classic(_) => mods.push(m),
            _ => {
                return Err(format!(
                    "--mods: {acronym} is not supported \
                     (NF, EZ, HT, DC, HR, SD, PF, DT, NC, HD, DA, CL, MR are)"
                ));
            }
        }
    }
    let mods = ModSet::new(mods).map_err(|e| format!("--mods: {e}"))?;
    mods.check_compatible()
        .map_err(|e| format!("--mods: {e}"))?;
    Ok(mods)
}

/// A draw or update rate in Hz.
fn rate(flag: &str, value: &str) -> Result<f64, String> {
    let hz: f64 = number(flag, value)?;
    if is_valid_hz(hz) {
        Ok(hz)
    } else {
        Err(format!("{flag} must be in [{MIN_HZ}, {MAX_HZ}] Hz"))
    }
}

/// The update thread's side.
struct Gameplay {
    session: PlaySession,
    player: MusicPlayer,
    mixer: MixerHandle,
    output: Arc<AudioPosition>,
    music: Arc<MusicPosition>,
    /// Surface size the draw thread last drew at, see [`pack_size`].
    surface: Arc<AtomicU64>,
    /// The size the input layout was made for.
    layout_size: u64,
    /// Input arrives in window points, the surface is in pixels.
    points_per_pixel: f32,
    /// Set once the player quits; the window closes then.
    exited: Arc<AtomicBool>,
    /// Hides the OS cursor while gameplay draws its own.
    os_cursor: CursorVisibility,
}

pub(crate) fn pack_size(width: u32, height: u32) -> u64 {
    (u64::from(width) << 32) | u64::from(height)
}

impl Update for Gameplay {
    type Snapshot = PlaySnapshot;

    fn tick(&mut self, info: &TickInfo, events: &[InputEvent], snapshot: &mut PlaySnapshot) {
        // Music events are not read here: dropping a failure frees its message. A full
        // event queue only counts the events it drops.
        let size = self.surface.load(Ordering::Relaxed);
        // A minimised window may report 0×0: keep the last layout.
        if size != self.layout_size && size >> 32 != 0 && size & 0xffff_ffff != 0 {
            self.layout_size = size;
            let (w, h) = ((size >> 32) as f32, (size & 0xffff_ffff) as f32);
            let ppp = self.points_per_pixel;
            self.session
                .set_layout(PlayfieldLayout::new(w * ppp, h * ppp));
        }
        let (output, music) = (self.output.read(), self.music.read());
        self.session.tick(
            info.now_ns,
            events,
            output,
            music,
            &mut self.player,
            &mut self.mixer,
            snapshot,
        );
        self.os_cursor.set_hidden(snapshot.state.hides_os_cursor());
        if self.session.has_exited() {
            self.exited.store(true, Ordering::Relaxed);
        }
    }
}

/// The draw thread's side. The device exists between `start` and `stop`, on the draw thread.
struct View {
    surface: Option<SdlGlSurface>,
    device: Option<GlDevice<SdlGlSurface>>,
    scene: PlayScene,
    /// The FPS counter with `--show-fps` (ADR-0053).
    fps: Option<fps::FpsOverlay>,
    /// Read by the update thread for the input layout.
    surface_size: Arc<AtomicU64>,
    /// Frame times for the console report; `None` with `--no-stats`.
    perf: Option<FramePerf>,
    _quit: crate::bench::QuitOnDrop,
}

impl Draw<PlaySnapshot> for View {
    fn start(&mut self) {
        // The window thread made the context and released it (ADR-0018).
        let surface = self.surface.take().expect("surface is set before start");
        // The same context made a device in `device_limits` already; failing now is a
        // driver fault this developer command does not recover from.
        let mut device = GlDevice::new(surface, GlDebug::for_build()).expect("GL device");
        // The update thread does not wait for the glyphs (the lead-in runs on).
        self.scene
            .start(&mut device, GLYPH_WARM_TIMEOUT, self.fps.as_ref());
        self.device = Some(device);
    }

    fn frame(&mut self, info: &FrameInfo, snapshot: &PlaySnapshot) {
        if let Some(perf) = &mut self.perf {
            perf.record(info.wait.woke_ns);
        }
        let device = self.device.as_mut().expect("started");
        let size = device.begin_frame(Color::rgb(0.0, 0.0, 0.0));
        self.surface_size
            .store(pack_size(size.width, size.height), Ordering::Relaxed);
        let fps = self.fps.as_mut().map(|overlay| {
            let display = fps::FpsDisplay {
                show: true,
                ..fps::FpsDisplay::default()
            };
            let (width, height) = (size.width as f32, size.height as f32);
            overlay.frame(info.wait.woke_ns, width, height, display, true);
            &*overlay
        });
        self.scene.draw(
            device,
            size,
            info.wait.woke_ns,
            snapshot,
            fps.and_then(fps::FpsOverlay::view),
        );
        // A failed present loses one frame; the next one tries again.
        let _ = device.present();
    }

    fn stop(&mut self) {
        let mut device = self.device.take().expect("started");
        self.scene.stop(&mut device);
        self.surface = Some(device.into_surface().expect("release GL context"));
    }
}

/// Finds a file of the beatmap's folder by its name in the `.osu` file (with `/`), matching
/// each part without regard to case as lazer's file lookup does; the exact name wins, then
/// the smallest matching name. `None` when it is missing or the name leaves the folder
/// (absolute, empty, `.` or `..` parts): lazer only looks among the set's files.
pub(crate) fn find_beatmap_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut path = dir.to_path_buf();
    for part in name.split('/') {
        let mut components = Path::new(part).components();
        if !matches!(
            (components.next(), components.next()),
            (Some(Component::Normal(_)), None)
        ) {
            return None;
        }
        let exact = path.join(part);
        path = if exact.exists() {
            exact
        } else {
            let lower = part.to_lowercase();
            std::fs::read_dir(&path)
                .ok()?
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().to_lowercase() == lower)
                .map(|entry| entry.path())
                .min()?
        };
    }
    Some(path)
}

/// Opens the track and waits until it is ready to play.
pub(crate) fn load_track(player: &mut MusicPlayer, path: &Path) -> Result<(), String> {
    let generation = player
        .load(TrackSource::File(path.to_owned()))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let deadline = Instant::now() + LOAD_TIMEOUT;
    while Instant::now() < deadline {
        match player.poll_event() {
            Some(MusicEvent::Loaded { generation: g, .. }) if g == generation => return Ok(()),
            Some(MusicEvent::Failed {
                generation: g,
                error,
            }) if g == generation => return Err(format!("{}: {error}", path.display())),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    Err(format!("{}: not opened in time", path.display()))
}

/// The device limits the texture loader needs; the context is released again for the
/// draw thread.
pub(crate) fn device_limits(
    surface: SdlGlSurface,
) -> Result<(SdlGlSurface, slam_render::Limits), String> {
    let device = GlDevice::new(surface, GlDebug::for_build()).map_err(|e| e.to_string())?;
    let limits = device.limits();
    let surface = device.into_surface().map_err(|e| e.to_string())?;
    Ok((surface, limits))
}

/// Plays the beatmap in a window until it is closed and prints the judgements.
pub fn run(options: &Options) -> Result<(), String> {
    let backend = match options.backend {
        Some(backend) => backend,
        None => *Backend::available()
            .first()
            .ok_or("no audio backend in this build")?,
    };
    let (mut mixer, mixer_handle) = slam_audio::mixer(&MixerConfig {
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
    let mixer_stats = Arc::clone(mixer_handle.stats());
    let music_stats = Arc::clone(player.stats());
    mixer.set_music(music).map_err(|e| e.to_string())?;
    let stream = slam_audio::open_output(
        backend,
        StreamConfig {
            device: options.device.clone(),
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            buffer_frames: options.buffer_frames,
            name: "SLAM".to_owned(),
        },
        Box::new(mixer),
    )
    .map_err(|e| e.to_string())?;

    let mut window =
        InputWindow::new_opengl("SLAM", WINDOW.0, WINDOW.1).map_err(|e| e.to_string())?;
    let surface = SdlGlSurface(window.create_gl_context().map_err(|e| e.to_string())?);
    let (pixels_w, pixels_h) = surface.0.size_in_pixels();
    let points_per_pixel = if pixels_w > 0 {
        WINDOW.0 as f32 / pixels_w as f32
    } else {
        1.0
    };
    let initial_size = pack_size(pixels_w, pixels_h);
    let surface_size = Arc::new(AtomicU64::new(initial_size));
    let (surface, limits) = device_limits(surface)?;
    let registry = Arc::new(FontRegistry::with_builtin());
    // English until the language setting exists (#98).
    let localizer = Localizer::builtin().map_err(|e| e.to_string())?;
    let labels = scene::hud_labels(&localizer);
    let Prepared {
        beatmap,
        mods,
        audio,
        sounds,
        scene,
        background_name,
        ..
    } = scene::prepare(
        &options.map,
        &options.settings(),
        limits,
        Arc::clone(&registry),
        &labels,
    )?;
    load_track(&mut player, &audio)?;
    let mut input = LiveInput::new(
        Bindings::default(),
        PlayfieldLayout::new(
            pixels_w as f32 * points_per_pixel,
            pixels_h as f32 * points_per_pixel,
        ),
    );
    input.set_mouse_buttons_disabled(!options.mouse_buttons);
    let session = PlaySession::new(&beatmap, &mods, &player, input, sounds, Offsets::default());
    let exited = Arc::new(AtomicBool::new(false));
    let gameplay = Gameplay {
        session,
        output: Arc::clone(stream.position()),
        surface: Arc::clone(&surface_size),
        layout_size: initial_size,
        points_per_pixel,
        exited: Arc::clone(&exited),
        os_cursor: {
            // Hidden from the first frame: the play starts without an event to apply it.
            let os_cursor = window.cursor_visibility();
            os_cursor.set_hidden(true);
            os_cursor
        },
        music: Arc::clone(player.position()),
        player,
        mixer: mixer_handle,
    };
    let frame_times = options.stats.then(|| Arc::new(SharedFrameTimes::default()));
    let objects = beatmap.hit_objects.len();
    let counters = Arc::<slam_engine::EngineCounters>::default();
    let update_rate = slam_engine::UpdateRateControl::new();
    let draw_rate = slam_engine::DrawRateControl::new();
    let draw = View {
        surface: Some(surface),
        device: None,
        scene,
        fps: options.show_fps.then(|| {
            fps::FpsOverlay::new(
                fps::FpsSource::new(
                    Arc::clone(&counters),
                    update_rate.clone(),
                    draw_rate.clone(),
                ),
                registry,
                &labels.fps,
            )
        }),
        surface_size,
        perf: frame_times
            .as_ref()
            .map(|shared| FramePerf::new(Arc::clone(shared))),
        _quit: crate::bench::QuitOnDrop(window.quit_handle()),
    };
    let (mut sink, source) = event_ring(1024);
    let engine = Engine::spawn_shared(
        EngineConfig {
            update_hz: options.update_hz,
            draw: options.draw,
            ..EngineConfig::default()
        },
        gameplay,
        draw,
        PlaySnapshot::default(),
        source,
        slam_engine::EngineShared {
            counters,
            update_rate,
            draw_rate,
        },
    )
    .map_err(|e| e.to_string())?;
    sink.set_notify(engine.waker().as_notify());

    println!(
        "playing {} ({} objects), audio: {}, background: {}; Escape pauses (or leaves the \
         results), hold ` to retry, Ctrl+` to quit",
        options.map.display(),
        objects,
        stream.backend().name(),
        background_name.as_deref().unwrap_or("none")
    );
    // The update thread must not take the window's lock: a watcher closes the window
    // once the player quits.
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
    let reporter = frame_times.map(|frame_times| {
        let sources = ReportSources {
            engine: Arc::clone(engine.counters()),
            input: Arc::clone(window.counters()),
            stream: Arc::clone(stream.counters()),
            position: Arc::clone(stream.position()),
            mixer: mixer_stats,
            music: music_stats,
            frame_times,
            buffer_frames: stream.config().buffer_frames,
            sample_rate: stream.config().sample_rate,
        };
        let closed = Arc::clone(&closed);
        std::thread::spawn(move || perf::report(&sources, &closed))
    });
    let mut mapper = TickMapper::new(now_ns, sdl_ticks_ns);
    let ran = window.run(&mut sink, &mut mapper);
    closed.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    if let Some(reporter) = reporter {
        let _ = reporter.join();
    }

    let slam_engine::Stopped {
        update: mut gameplay,
        draw,
        ..
    } = engine.shutdown();
    // The GL context goes before the window.
    drop(draw);
    let deadline = Instant::now() + Duration::from_secs(1);
    while !gameplay.session.stop_sounds(&mut gameplay.mixer) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(stream);

    let session = &gameplay.session;
    // Until the results screen exists, the results go to the console.
    if let Some(results) = session.results() {
        println!(
            "passed: score {}, accuracy {:.2}%, rank {:?}",
            results.score,
            results.accuracy * 100.0,
            results.rank
        );
    } else if session.health().has_failed() {
        println!("failed after {} judgements", session.counted_judgements());
    }
    let statistics = session.score().statistics();
    println!(
        "300: {}, 100: {}, 50: {}, miss: {}, max combo: {}",
        statistics.count(HitResult::Great),
        statistics.count(HitResult::Ok),
        statistics.count(HitResult::Meh),
        statistics.count(HitResult::Miss),
        statistics.max_combo
    );
    drop(window);
    ran.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temp directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!("slam-play-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn beatmap_files_are_found_without_regard_to_case_inside_the_folder() {
        let tmp = TempDir::new("find");
        let set = tmp.0.join("set");
        std::fs::create_dir_all(set.join("SB")).unwrap();
        std::fs::write(set.join("BG.jpg"), b"a").unwrap();
        std::fs::write(set.join("SB").join("Back.PNG"), b"b").unwrap();
        std::fs::write(tmp.0.join("outside.jpg"), b"c").unwrap();

        // Compared by contents: on a case-insensitive file system (Windows) the path keeps
        // the asked case.
        let read =
            |name: &str| find_beatmap_file(&set, name).map(|path| std::fs::read(path).unwrap());
        assert_eq!(read("BG.jpg"), Some(b"a".to_vec()));
        assert_eq!(read("bg.JPG"), Some(b"a".to_vec()));
        assert_eq!(read("sb/back.png"), Some(b"b".to_vec()));
        assert_eq!(find_beatmap_file(&set, "missing.jpg"), None);
        assert_eq!(find_beatmap_file(&set, "../outside.jpg"), None);
        assert_eq!(find_beatmap_file(&set, "./BG.jpg"), None);
        assert_eq!(find_beatmap_file(&set, "SB//Back.PNG"), None);
        let absolute = tmp.0.join("outside.jpg");
        assert_eq!(find_beatmap_file(&set, absolute.to_str().unwrap()), None);
    }

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn flags() {
        let o = parse(&["map.osu"]).unwrap();
        assert_eq!(o.map, PathBuf::from("map.osu"));
        assert_eq!(
            (o.skin.as_ref(), o.backend, o.device.as_ref()),
            (None, None, None)
        );
        assert_eq!(o.buffer_frames, StreamConfig::default().buffer_frames);
        assert!(o.stats);
        let defaults = EngineConfig::default();
        assert_eq!((o.draw, o.update_hz), (defaults.draw, defaults.update_hz));
        assert!(!parse(&["--no-stats", "map.osu"]).unwrap().stats);
        let o = parse(&["map.osu"]).unwrap();
        assert!(o.beatmap_skins && o.beatmap_colours && o.mouse_buttons);
        assert!(
            !parse(&["--no-beatmap-skins", "map.osu"])
                .unwrap()
                .beatmap_skins
        );
        let o = parse(&["--no-beatmap-colours", "map.osu", "--no-mouse-buttons"]).unwrap();
        assert!(!o.beatmap_colours && !o.mouse_buttons);
        assert!(o.beatmap_hitsounds);
        assert!(
            !parse(&["--no-beatmap-hitsounds", "map.osu"])
                .unwrap()
                .beatmap_hitsounds
        );
        assert!(!o.show_fps);
        assert!(parse(&["--show-fps", "map.osu"]).unwrap().show_fps);

        let o = parse(&[
            "--skin", "s", "map.osu", "--device", "out", "--buffer", "64",
        ])
        .unwrap();
        assert_eq!(o.skin, Some(PathBuf::from("s")));
        assert_eq!(o.device.as_deref(), Some("out"));
        assert_eq!(o.buffer_frames, 64);

        let o = parse(&["map.osu", "--fps", "4000", "--update-hz", "8000"]).unwrap();
        assert_eq!((o.draw, o.update_hz), (LimiterMode::Hz(4000.0), 8000.0));
        let o = parse(&["map.osu", "--fps", "unlimited", "--update-hz", "1"]).unwrap();
        assert_eq!((o.draw, o.update_hz), (LimiterMode::Unlimited, 1.0));

        assert!(parse(&[]).is_err());
        assert!(parse(&["a.osu", "b.osu"]).is_err());
        assert!(parse(&["map.osu", "--skin"]).is_err());
        assert!(parse(&["map.osu", "--buffer", "0"]).is_err());
        assert!(parse(&["map.osu", "--what"]).is_err());
        for bad in ["0", "8001", "-1", "NaN", "inf", "fast"] {
            assert!(parse(&["map.osu", "--fps", bad]).is_err(), "{bad}");
            assert!(parse(&["map.osu", "--update-hz", bad]).is_err(), "{bad}");
        }
        assert!(parse(&["map.osu", "--update-hz", "unlimited"]).is_err());

        // The background dim: lazer's default without the flag, whole percent with it.
        assert_eq!(parse(&["map.osu"]).unwrap().dim, 0.7);
        assert_eq!(parse(&["map.osu", "--dim", "70"]).unwrap().dim, 0.7);
        assert_eq!(parse(&["--dim", "0", "map.osu"]).unwrap().dim, 0.0);
        assert_eq!(parse(&["map.osu", "--dim", "100"]).unwrap().dim, 1.0);
        assert_eq!(parse(&["map.osu", "--dim", "35"]).unwrap().dim, 0.35);
        for bad in ["-1", "101", "50.5", "NaN", "inf", "dark"] {
            assert!(parse(&["map.osu", "--dim", bad]).is_err(), "{bad}");
        }
        assert!(parse(&["map.osu", "--dim"]).is_err());
        assert!(parse(&["map.osu", "--backend", "asio"]).is_err());
    }

    #[test]
    fn mods_are_parsed_in_any_case_and_order() {
        let acronyms = |args: &[&str]| -> Vec<String> {
            let mods = parse(args).unwrap().mods;
            mods.to_api().into_iter().map(|m| m.acronym).collect()
        };
        assert!(acronyms(&["map.osu"]).is_empty());
        assert_eq!(acronyms(&["map.osu", "--mods", "HDHR"]), ["HD", "HR"]);
        assert_eq!(acronyms(&["--mods", "hrHd", "map.osu"]), ["HR", "HD"]);
        assert_eq!(acronyms(&["map.osu", "--mods", "nf"]), ["NF"]);
        assert_eq!(
            acronyms(&["map.osu", "--mods", "EZHDSD"]),
            ["EZ", "HD", "SD"]
        );
        assert_eq!(acronyms(&["map.osu", "--mods", "PFHR"]), ["PF", "HR"]);
        assert_eq!(acronyms(&["map.osu", "--mods", "HDDT"]), ["HD", "DT"]);
        assert_eq!(acronyms(&["map.osu", "--mods", "nc"]), ["NC"]);
        assert_eq!(acronyms(&["map.osu", "--mods", "HTCL"]), ["HT", "CL"]);
        assert_eq!(
            acronyms(&["map.osu", "--mods", "DCMRDA"]),
            ["DC", "MR", "DA"]
        );
        // The mods reach the beatmap with their default settings.
        let mods = parse(&["map.osu", "--mods", "EZ"]).unwrap().mods;
        assert_eq!(mods, ModSet::new(vec![Mod::from_acronym("EZ")]).unwrap());
    }

    #[test]
    fn mods_that_play_cannot_take_are_rejected() {
        let bad = |value: &str| parse(&["map.osu", "--mods", value]).unwrap_err();
        // Malformed.
        for value in ["", "H", "HDH", "HDé", "HD HR"] {
            assert!(bad(value).contains("two-letter"), "{value}");
        }
        for value in ["FL", "HDTC", "XX", "RX"] {
            assert!(bad(value).contains("not supported"), "{value}");
        }
        // Sets lazer does not accept.
        for value in ["HDHD", "hdHD"] {
            assert!(bad(value).contains("more than once"), "{value}");
        }
        for value in [
            "HREZ", "EZHR", "SDPF", "NFSD", "PFNF", "HDNFHRPF", "DTHT", "NCDT", "DCHT", "HRMR",
            "DAHR", "EZDA",
        ] {
            assert!(bad(value).contains("incompatible"), "{value}");
        }
        assert!(parse(&["map.osu", "--mods"]).is_err());
    }

    #[test]
    fn mods_are_applied_to_the_loaded_beatmap() {
        let map = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../slam-osu/tests/data/slider-samples.osu")
            .to_string_lossy()
            .into_owned();
        let loaded = |mods: &[&str]| {
            let mut args = vec![map.as_str()];
            args.extend_from_slice(mods);
            let options = parse(&args).unwrap();
            let loaded = scene::load(&options.map, &options.settings()).unwrap();
            assert_eq!(loaded.mods, options.mods);
            loaded
        };
        let difficulty = |mods: &[&str]| loaded(mods).beatmap.difficulty;
        let plain = difficulty(&[]);
        let hard = difficulty(&["--mods", "HR"]);
        let easy = difficulty(&["--mods", "EZ"]);
        assert_eq!(hard.circle_size, (plain.circle_size * 1.3).min(10.0));
        assert_eq!(hard.approach_rate, (plain.approach_rate * 1.4).min(10.0));
        assert_eq!(easy.circle_size, plain.circle_size * 0.5);
        assert_eq!(easy.approach_rate, plain.approach_rate * 0.5);
        assert_eq!(difficulty(&["--mods", "HDNF"]), plain);
    }

    #[test]
    fn the_default_skin_is_in_the_source_tree() {
        assert!(slam_skin::Skin::load_default(default_skin_dir()).is_ok());
    }
}
