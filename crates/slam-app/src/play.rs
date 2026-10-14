//! Plays a beatmap given on the command line: hit circles, a stand-in cursor and HUD, music
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
use slam_engine::{Draw, Engine, EngineConfig, FrameInfo, TickInfo, Update};
use slam_input::{InputEvent, InputWindow, TickMapper, event_ring, sdl_ticks_ns};
use slam_osu::judgement::HitResult;
use slam_osu::{Beatmap, ModSet};
use slam_play::circles::{
    CircleMods, CircleSettings, CircleSkinConfig, CircleStyle, CircleVisuals, HitCircles,
};
use slam_play::{
    BackgroundSettings, BackgroundView, Bindings, GameplaySounds, Hitsounds, HudView, LiveInput,
    Offsets, PlaySession, PlaySnapshot, PlayTextures, PlayView, PlayfieldLayout, SampleCache,
    SampleSkins, SoundSettings,
};
use slam_render::gl::{GlDebug, GlDevice, GlTexture};
use slam_render::sprite::{DEFAULT_SLOTS, Projection, SpriteBatch, SpriteRenderer};
use slam_render::textures::Textures;
use slam_render::upload::{
    TextureHandle, TextureKind, TextureLoaderConfig, TextureRequests, TextureState, TextureUploads,
    texture_loader,
};
use slam_render::{Color, Device, shaders};
use slam_skin::{Skin, SkinChain};
use slam_text::{FontRegistry, GlyphAtlasConfig, GlyphCache, glyph_atlas};
use slam_ui::i18n::Localizer;

use crate::bench::number;

pub const USAGE: &str = "usage: slam-app play MAP.osu [--skin DIR] [--backend NAME] \
                         [--device ID] [--buffer FRAMES]";

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const WINDOW: (i32, i32) = (1280, 720);
/// How long the track may take to open.
const LOAD_TIMEOUT: Duration = Duration::from_secs(10);
/// Sprites of one frame: every live circle's pieces, numbers and approach circle.
const MAX_SPRITES: usize = 4096;
/// Textures of the draw thread: atlas pages of the skin and the glyphs, and images too
/// large for a page.
const MAX_TEXTURES: usize = 64;
/// Glyph sprites of one frame: the HUD's numbers and the skip button.
const MAX_GLYPH_SPRITES: usize = 256;
/// How long the draw thread waits for the HUD's glyphs before the first frame.
const GLYPH_WARM_TIMEOUT: Duration = Duration::from_secs(1);

/// The bundled default skin, in the source tree.
fn default_skin_dir() -> PathBuf {
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
}

impl Options {
    /// Parses the arguments after `play`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut map = None;
        let mut options = Self {
            map: PathBuf::new(),
            skin: None,
            backend: None,
            device: None,
            buffer_frames: StreamConfig::default().buffer_frames,
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
}

fn pack_size(width: u32, height: u32) -> u64 {
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
        if self.session.has_exited() {
            self.exited.store(true, Ordering::Relaxed);
        }
    }
}

struct Resources {
    device: GlDevice<SdlGlSurface>,
    textures: Textures<GlTexture>,
    renderer: SpriteRenderer<GlDevice<SdlGlSurface>>,
    /// Draws MSDF glyphs.
    text_renderer: SpriteRenderer<GlDevice<SdlGlSurface>>,
}

/// The draw thread's side. The device and its resources exist between `start` and
/// `stop`, on the draw thread.
struct View {
    surface: Option<SdlGlSurface>,
    resources: Option<Resources>,
    uploads: TextureUploads,
    textures: PlayTextures,
    glyphs: GlyphCache,
    view: PlayView,
    hud: HudView,
    /// The beatmap's background, if it has one.
    background: Option<TextureHandle>,
    background_view: BackgroundView,
    /// The background, the playfield and the HUD's shapes.
    batch: SpriteBatch,
    /// The HUD's text.
    text: SpriteBatch,
    /// The cursor, on top of the HUD.
    cursor: SpriteBatch,
    /// Read by the update thread for the input layout.
    surface_size: Arc<AtomicU64>,
    _quit: crate::bench::QuitOnDrop,
}

impl Draw<PlaySnapshot> for View {
    fn start(&mut self) {
        // The window thread made the context and released it (ADR-0018).
        let surface = self.surface.take().expect("surface is set before start");
        // The same context made a device in `device_limits` already; failing now is a
        // driver fault this developer command does not recover from.
        let mut device = GlDevice::new(surface, GlDebug::for_build()).expect("GL device");
        // Two draws a frame (playfield and HUD, cursor), each with its spare slots.
        let renderer = SpriteRenderer::new(&mut device, 2 * DEFAULT_SLOTS).expect("renderer");
        let text_renderer = SpriteRenderer::with_shader(&mut device, DEFAULT_SLOTS, &shaders::MSDF)
            .expect("text renderer");
        let mut textures = Textures::with_capacity(MAX_TEXTURES);
        // The HUD's glyphs are made before the first frame: a glyph takes several frames
        // to appear. The update thread does not wait for this (the lead-in runs on); late
        // glyphs show when ready.
        let deadline = Instant::now() + GLYPH_WARM_TIMEOUT;
        while self.hud.request_glyphs(&mut self.glyphs) > 0 && Instant::now() < deadline {
            self.glyphs.process(&mut device, &mut textures);
            std::thread::sleep(Duration::from_millis(1));
        }
        self.resources = Some(Resources {
            device,
            textures,
            renderer,
            text_renderer,
        });
    }

    fn frame(&mut self, info: &FrameInfo, snapshot: &PlaySnapshot) {
        let r = self.resources.as_mut().expect("started");
        let size = r.device.begin_frame(Color::rgb(0.0, 0.0, 0.0));
        self.surface_size
            .store(pack_size(size.width, size.height), Ordering::Relaxed);
        self.uploads.process(&mut r.device, &mut r.textures);
        self.glyphs.process(&mut r.device, &mut r.textures);
        let textures = self.textures.resolve(&self.uploads);
        let (width, height) = (size.width as f32, size.height as f32);
        let layout = PlayfieldLayout::new(width, height);
        self.hud.update(width, height, &snapshot.hud);
        self.batch.clear();
        self.text.clear();
        self.cursor.clear();
        let background = self
            .background
            .and_then(|handle| match self.uploads.get(handle) {
                TextureState::Ready(region) => Some(region),
                _ => None,
            });
        self.background_view.push(
            &mut self.batch,
            width,
            height,
            snapshot,
            info.wait.woke_ns,
            background,
        );
        self.view.push(
            &mut self.batch,
            &layout,
            snapshot,
            info.wait.woke_ns,
            &textures,
        );
        if let Some(white) = textures.white {
            self.hud
                .push_shapes(&mut self.batch, &layout, snapshot, white);
        }
        self.hud
            .push_text(&mut self.text, snapshot, &mut self.glyphs);
        self.view
            .push_cursor(&mut self.cursor, &layout, snapshot, &textures);
        r.renderer.begin_frame();
        r.text_renderer.begin_frame();
        let gpu_textures = &r.textures;
        let projection = Projection::pixels(size);
        // A failed draw or present loses one frame; the next one tries again.
        let texture = |id| gpu_textures.get(id);
        let _ = r
            .renderer
            .draw(&mut r.device, &self.batch, &projection, texture);
        let _ = r
            .text_renderer
            .draw(&mut r.device, &self.text, &projection, texture);
        let _ = r
            .renderer
            .draw(&mut r.device, &self.cursor, &projection, texture);
        let _ = r.device.present();
    }

    fn stop(&mut self) {
        let Resources {
            mut device,
            mut textures,
            renderer,
            text_renderer,
        } = self.resources.take().expect("started");
        renderer.destroy(&mut device);
        text_renderer.destroy(&mut device);
        for texture in textures.drain() {
            device.destroy_texture(texture);
        }
        self.surface = Some(device.into_surface().expect("release GL context"));
    }
}

/// Everything loaded before the threads start.
struct Loaded {
    beatmap: Beatmap,
    audio: PathBuf,
    beatmap_skin: Skin,
    skins: SkinChain,
    /// The background image's name and bytes; `None` when the beatmap names none or it
    /// cannot be read.
    background: Option<(String, Vec<u8>)>,
}

fn load(options: &Options) -> Result<Loaded, String> {
    let map = &options.map;
    let bytes = std::fs::read(map).map_err(|e| format!("{}: {e}", map.display()))?;
    let file = slam_formats::osu::decode(&bytes)
        .map_err(|e| format!("{}: {e}", map.display()))?
        .beatmap;
    let dir = map.parent().unwrap_or(Path::new("."));
    let audio = dir.join(&file.general.audio_filename);
    let background = file.events.background.as_deref().and_then(|name| {
        let bytes = find_beatmap_file(dir, name).and_then(|path| std::fs::read(path).ok());
        if bytes.is_none() {
            tracing::warn!("background {name}: not found or unreadable");
        }
        bytes.map(|bytes| (name.to_owned(), bytes))
    });
    let beatmap = Beatmap::from_file(file).map_err(|e| format!("{}: {e}", map.display()))?;
    let beatmap_skin = Skin::load_beatmap(dir, &bytes).map_err(|e| format!("beatmap skin: {e}"))?;
    let default = Skin::load_default(default_skin_dir())
        .map_err(|e| format!("default skin {}: {e}", default_skin_dir().display()))?;
    let user = options
        .skin
        .as_ref()
        .map(|dir| Skin::load(dir).map_err(|e| format!("--skin {}: {e}", dir.display())))
        .transpose()?;
    Ok(Loaded {
        beatmap,
        audio,
        beatmap_skin,
        skins: SkinChain::new(user, default),
        background,
    })
}

/// Finds a file of the beatmap's folder by its name in the `.osu` file (with `/`), matching
/// each part without regard to case as lazer's file lookup does; the exact name wins, then
/// the smallest matching name. `None` when it is missing or the name leaves the folder
/// (absolute, empty, `.` or `..` parts): lazer only looks among the set's files.
fn find_beatmap_file(dir: &Path, name: &str) -> Option<PathBuf> {
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
fn load_track(player: &mut MusicPlayer, path: &Path) -> Result<(), String> {
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
fn device_limits(surface: SdlGlSurface) -> Result<(SdlGlSurface, slam_render::Limits), String> {
    let device = GlDevice::new(surface, GlDebug::for_build()).map_err(|e| e.to_string())?;
    let limits = device.limits();
    let surface = device.into_surface().map_err(|e| e.to_string())?;
    Ok((surface, limits))
}

/// Plays the beatmap in a window until it is closed and prints the judgements.
pub fn run(options: &Options) -> Result<(), String> {
    let loaded = load(options)?;
    let mods = ModSet::default();
    let beatmap = &loaded.beatmap;

    let mut cache = SampleCache::new(SAMPLE_RATE, CHANNELS);
    let hitsounds = Hitsounds::new(
        beatmap,
        SampleSkins {
            beatmap: Some(&loaded.beatmap_skin),
            chain: &loaded.skins,
        },
        &mut cache,
    );
    let sounds = GameplaySounds::new(beatmap, hitsounds, SoundSettings::for_mods(&mods));
    let circle_mods = CircleMods::new(&mods);
    let windows = slam_osu::hit_windows::HitWindows::new(beatmap.difficulty.overall_difficulty);
    let view = PlayView {
        circles: HitCircles {
            visuals: CircleVisuals::new(
                beatmap,
                circle_mods,
                CircleSettings::default(),
                Some(&loaded.beatmap_skin),
                &loaded.skins,
            ),
            style: CircleStyle::new(circle_mods, &windows, &loaded.skins),
            config: CircleSkinConfig::new(&loaded.skins),
        },
    };

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
    mixer.set_music(music).map_err(|e| e.to_string())?;
    load_track(&mut player, &loaded.audio)?;
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
    let (mut requests, uploads): (TextureRequests, TextureUploads) =
        texture_loader(&TextureLoaderConfig::new(limits)).map_err(|e| e.to_string())?;
    let textures =
        PlayTextures::request(&loaded.skins, &mut requests).map_err(|e| e.to_string())?;
    let background_name = loaded.background.as_ref().map(|(name, _)| name.clone());
    // The background is optional: a refused request plays without it.
    let background = loaded.background.and_then(|(name, bytes)| {
        requests
            .load(bytes, TextureKind::Standalone)
            .inspect_err(|e| tracing::warn!("background {name}: {e}"))
            .ok()
    });
    let registry = Arc::new(FontRegistry::with_builtin());
    let glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits))
        .map_err(|e| e.to_string())?;
    // English until the language setting exists (#98).
    let localizer = Localizer::builtin().map_err(|e| e.to_string())?;
    let hud = HudView::new(registry, &localizer.format("play-skip-button", None));

    let input = LiveInput::new(
        Bindings::default(),
        PlayfieldLayout::new(
            pixels_w as f32 * points_per_pixel,
            pixels_h as f32 * points_per_pixel,
        ),
    );
    let session = PlaySession::new(beatmap, &mods, &player, input, sounds, Offsets::default());
    let exited = Arc::new(AtomicBool::new(false));
    let gameplay = Gameplay {
        session,
        output: Arc::clone(stream.position()),
        surface: Arc::clone(&surface_size),
        layout_size: initial_size,
        points_per_pixel,
        exited: Arc::clone(&exited),
        music: Arc::clone(player.position()),
        player,
        mixer: mixer_handle,
    };
    let draw = View {
        surface: Some(surface),
        resources: None,
        uploads,
        textures,
        glyphs,
        view,
        hud,
        background,
        background_view: BackgroundView::new(BackgroundSettings::default()),
        batch: SpriteBatch::with_capacity(MAX_SPRITES),
        text: SpriteBatch::with_capacity(MAX_GLYPH_SPRITES),
        cursor: SpriteBatch::with_capacity(1),
        surface_size,
        _quit: crate::bench::QuitOnDrop(window.quit_handle()),
    };
    let (mut sink, source) = event_ring(1024);
    let engine = Engine::spawn(
        EngineConfig::default(),
        gameplay,
        draw,
        PlaySnapshot::default(),
        source,
    )
    .map_err(|e| e.to_string())?;
    sink.set_notify(engine.waker().as_notify());

    println!(
        "playing {} ({} objects), audio: {}, background: {}; Escape pauses, hold ` to retry, \
         Ctrl+` to quit",
        options.map.display(),
        beatmap.hit_objects.len(),
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
    let mut mapper = TickMapper::new(now_ns, sdl_ticks_ns);
    let ran = window.run(&mut sink, &mut mapper);
    closed.store(true, Ordering::Relaxed);
    let _ = watcher.join();

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
    drop(requests);

    let session = &gameplay.session;
    if session.health().has_failed() {
        println!("failed after {} judgements", session.counted_judgements());
    }
    let statistics = session.log().statistics();
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

        assert_eq!(find_beatmap_file(&set, "BG.jpg"), Some(set.join("BG.jpg")));
        assert_eq!(find_beatmap_file(&set, "bg.JPG"), Some(set.join("BG.jpg")));
        assert_eq!(
            find_beatmap_file(&set, "sb/back.png"),
            Some(set.join("SB").join("Back.PNG"))
        );
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

        let o = parse(&[
            "--skin", "s", "map.osu", "--device", "out", "--buffer", "64",
        ])
        .unwrap();
        assert_eq!(o.skin, Some(PathBuf::from("s")));
        assert_eq!(o.device.as_deref(), Some("out"));
        assert_eq!(o.buffer_frames, 64);

        assert!(parse(&[]).is_err());
        assert!(parse(&["a.osu", "b.osu"]).is_err());
        assert!(parse(&["map.osu", "--skin"]).is_err());
        assert!(parse(&["map.osu", "--buffer", "0"]).is_err());
        assert!(parse(&["map.osu", "--what"]).is_err());
        assert!(parse(&["map.osu", "--backend", "asio"]).is_err());
    }

    #[test]
    fn the_default_skin_is_in_the_source_tree() {
        assert!(Skin::load_default(default_skin_dir()).is_ok());
    }
}
