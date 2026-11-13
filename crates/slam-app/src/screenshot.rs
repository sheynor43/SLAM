//! Screenshots of a play at fixed beatmap times (ADR-0111): the play runs without a window,
//! sound or real time ([`offline`]), and its scene is drawn on a headless OpenGL window on
//! Mesa's software renderer, so the same beatmap, skin, times and Mesa give the same
//! pixels. The skin screenshot tests compare them with references ([`compare`]).
//! Developer command: its console output is not localised.

pub mod check;
pub mod compare;
pub mod corpus;
mod intro;
mod offline;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use slam_engine::gl::SdlGlSurface;
use slam_input::{InputWindow, PriorityRequest};
use slam_play::{PlaySnapshot, PlayfieldLayout};
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::{Color, Device, Extent};
use slam_text::FontRegistry;
use slam_ui::i18n::Localizer;

use crate::play::scene::{self, PlaySettings, Prepared};
pub use intro::render_intro;
use offline::OfflinePlay;

pub const USAGE: &str = "usage: slam-app screenshot MAP.osu --time MS [--time MS ...] \
                         [--skin DIR] [--size WxH] [--dim PERCENT] [--out DIR] [--any-renderer]";

/// The renderer the references are drawn with.
pub const REFERENCE_RENDERER: &str = "llvmpipe";
/// Size of a screenshot unless given.
pub const DEFAULT_SIZE: (u32, u32) = (640, 480);
/// How long one screenshot may wait for the play's images and glyphs.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Engine time between the frames drawn on the way to a shot: 60 frames a second, on a
/// grid of engine time that does not depend on the shots; a frame is drawn at the first
/// tick of each period.
const FRAME_NS: u64 = 1_000_000_000 / 60;
/// How long the scene's start waits for the HUD's glyphs; the frames after it wait for the
/// rest.
const GLYPH_WARM_TIMEOUT: Duration = Duration::from_secs(1);

/// An `Rgba8` image, tight rows, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    /// Writes the frame as an RGBA PNG.
    pub fn write_png(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        writer
            .write_image_data(&self.pixels)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        writer
            .finish()
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Reads an 8-bit RGBA PNG, as [`Frame::write_png`] writes them.
    pub fn read_png(path: &Path) -> Result<Frame, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut reader = png::Decoder::new(std::io::BufReader::new(file))
            .read_info()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let info = reader.info();
        if (info.color_type, info.bit_depth) != (png::ColorType::Rgba, png::BitDepth::Eight) {
            return Err(format!("{}: not an 8-bit RGBA PNG", path.display()));
        }
        let (width, height) = (info.width, info.height);
        let mut pixels = vec![0; width as usize * height as usize * 4];
        reader
            .next_frame(&mut pixels)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Frame {
            width,
            height,
            pixels,
        })
    }
}

/// What to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Shots {
    pub map: PathBuf,
    pub settings: PlaySettings,
    /// Beatmap times in milliseconds, increasing; each is shot at the nearest tick (whole
    /// milliseconds from the play's start time), so times on the same tick give the same
    /// shot. A time before the play's start shows the start.
    pub times: Vec<f64>,
    pub size: (u32, u32),
    /// Draw on whatever OpenGL the machine has instead of refusing anything but
    /// [`REFERENCE_RENDERER`]: for looking, not for comparing with references.
    pub any_renderer: bool,
}

/// Draws the play of `shots` at each of its times. Opens SDL video: one call at a time in
/// a process, on its main thread.
pub fn render(shots: &Shots) -> Result<Vec<Frame>, String> {
    let (width, height) = shots.size;
    let to_i32 = |v: u32| i32::try_from(v).map_err(|_| format!("size {v} is too large"));
    let mut window =
        InputWindow::new_headless_opengl("SLAM screenshot", to_i32(width)?, to_i32(height)?)
            .map_err(|e| e.to_string())?;
    let context = window
        .create_gl_context_with(PriorityRequest::Default)
        .map_err(|e| e.to_string())?;
    let shots = shots.clone();
    // The context is used from another thread, as the draw thread does (ADR-0018); the
    // surface comes back so it is released before the window.
    let (frames, surface) = std::thread::spawn(move || draw(SdlGlSurface(context), &shots))
        .join()
        .map_err(|_| "the draw thread panicked".to_owned())?;
    drop(surface);
    drop(window);
    frames
}

fn draw(
    surface: SdlGlSurface,
    shots: &Shots,
) -> (Result<Vec<Frame>, String>, Option<SdlGlSurface>) {
    let mut device = match GlDevice::new(surface, GlDebug::Off) {
        Ok(device) => device,
        Err(e) => return (Err(format!("OpenGL: {e}")), None),
    };
    let frames = draw_on(&mut device, shots);
    match device.into_surface() {
        Ok(surface) => (frames, Some(surface)),
        Err(e) => (frames.and(Err(format!("releasing the context: {e}"))), None),
    }
}

fn draw_on(device: &mut GlDevice<SdlGlSurface>, shots: &Shots) -> Result<Vec<Frame>, String> {
    let driver = device.driver_info();
    tracing::info!("screenshots on {driver}");
    if !shots.any_renderer && !driver.contains(REFERENCE_RENDERER) {
        return Err(format!(
            "the references are drawn on {REFERENCE_RENDERER}, not {driver}"
        ));
    }
    check(shots)?;
    let size = Extent::new(shots.size.0, shots.size.1);
    let registry = Arc::new(FontRegistry::with_builtin());
    // English, as the play command (#98).
    let localizer = Localizer::builtin().map_err(|e| e.to_string())?;
    let labels = scene::hud_labels(&localizer);
    let Prepared {
        beatmap,
        mods,
        sounds,
        mut scene,
        ..
    } = scene::prepare(
        &shots.map,
        &shots.settings,
        device.limits(),
        registry,
        &labels,
    )?;
    // The offscreen driver has one pixel per point.
    let layout = PlayfieldLayout::new(size.width as f32, size.height as f32);
    let mut play = OfflinePlay::new(&beatmap, &mods, sounds, layout);
    let mut gpu = scene.start(device, GLYPH_WARM_TIMEOUT, None);
    let frames = shoot(device, &mut scene, &mut gpu, &mut play, &shots.times, size);
    gpu.destroy(device);
    frames
}

/// Loads the scene, then runs the play through `times` and shoots each.
fn shoot(
    device: &mut GlDevice<SdlGlSurface>,
    scene: &mut scene::PlayScene,
    gpu: &mut scene::SceneGpu<GlDevice<SdlGlSurface>>,
    play: &mut OfflinePlay,
    times: &[f64],
    size: Extent,
) -> Result<Vec<Frame>, String> {
    // Images that arrive during the play fade in from then on (the background): load them
    // all before it starts, as the shell's loading screen waits for them.
    let deadline = Instant::now() + LOAD_TIMEOUT;
    while !scene.loaded() {
        if Instant::now() > deadline {
            return Err("images or glyphs did not load".to_owned());
        }
        device.begin_frame(Color::BLACK);
        scene.draw(gpu, device, size, 0, &PlaySnapshot::default(), None, false);
        device.present().map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut frames = Vec::with_capacity(times.len());
    let mut last_period = 0;
    let mut presented = Ok(());
    for &time in times {
        // Frames in between, as a play draws them: the scene animates some things from
        // what earlier frames saw (the background's dim), so a shot then does not depend on
        // which shots came before. Each is drawn after the first tick of its period, with
        // that tick's time.
        play.run_to(play.target_ns(time), |play| {
            let period = play.now_ns() / FRAME_NS;
            if period == last_period || presented.is_err() {
                return;
            }
            last_period = period;
            device.begin_frame(Color::BLACK);
            scene.draw(
                gpu,
                device,
                size,
                play.now_ns(),
                play.snapshot(),
                None,
                false,
            );
            presented = device.present().map_err(|e| e.to_string());
        });
        presented.clone()?;
        let frame =
            settle(device, scene, gpu, play, size).map_err(|e| format!("{time} ms: {e}"))?;
        frames.push(frame);
    }
    Ok(frames)
}

/// Draws the play's current snapshot until everything it shows is loaded and two frames in
/// a row are the same, and gives that frame.
fn settle(
    device: &mut GlDevice<SdlGlSurface>,
    scene: &mut scene::PlayScene,
    gpu: &mut scene::SceneGpu<GlDevice<SdlGlSurface>>,
    play: &OfflinePlay,
    size: Extent,
) -> Result<Frame, String> {
    let deadline = Instant::now() + LOAD_TIMEOUT;
    let mut last: Option<Vec<u8>> = None;
    loop {
        let drawn = device.begin_frame(Color::BLACK);
        if drawn != size {
            return Err(format!("drew {drawn:?}, not {size:?}"));
        }
        // Loaded before this frame's uploads: the frame shows all of it.
        let loaded = scene.loaded();
        scene.draw(
            gpu,
            device,
            size,
            play.now_ns(),
            play.snapshot(),
            None,
            false,
        );
        let pixels = loaded.then(|| device.read_frame(size));
        device.present().map_err(|e| e.to_string())?;
        if let Some(pixels) = pixels {
            if last.as_ref() == Some(&pixels) {
                return Ok(Frame {
                    width: size.width,
                    height: size.height,
                    pixels,
                });
            }
            last = Some(pixels);
        }
        if Instant::now() > deadline {
            return Err(if last.is_some() {
                "frames of the same moment keep changing".to_owned()
            } else {
                "images or glyphs did not load".to_owned()
            });
        }
        if !loaded {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Refuses what the offline play does not model: speed mods (it ticks at rate 1 and does
/// not follow the track's frequency), and times out of order (the play only runs forward).
fn check(shots: &Shots) -> Result<(), String> {
    let rate = shots.settings.mods.true_gameplay_rate();
    if rate != 1.0 {
        return Err(format!(
            "mods that change the speed (rate {rate}) are not supported"
        ));
    }
    if shots.times.is_empty() {
        return Err("no times given".to_owned());
    }
    if let Some(pair) = shots.times.windows(2).find(|pair| pair[1] <= pair[0]) {
        return Err(format!(
            "times must increase: {} after {}",
            pair[1], pair[0]
        ));
    }
    Ok(())
}

/// The `screenshot` command's options.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub shots: Shots,
    /// Folder the PNGs go to, named `<map>-<time>.png`.
    pub out: PathBuf,
}

impl Options {
    /// Parses the arguments after `screenshot`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut map = None;
        let mut settings = PlaySettings::default();
        let mut times = Vec::new();
        let mut size = DEFAULT_SIZE;
        let mut out = PathBuf::from(".");
        let mut any_renderer = false;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--time" => {
                    let v = value()?;
                    let time: f64 = v.parse().map_err(|_| format!("bad --time {v}"))?;
                    if !time.is_finite() {
                        return Err(format!("bad --time {v}"));
                    }
                    times.push(time);
                }
                "--skin" => settings.skin = Some(value()?.into()),
                "--size" => size = parse_size(&value()?)?,
                "--dim" => {
                    let v = value()?;
                    let percent: f64 = v.parse().map_err(|_| format!("bad --dim {v}"))?;
                    if !(0.0..=100.0).contains(&percent) {
                        return Err(format!("--dim {v} is not 0..=100"));
                    }
                    settings.dim = percent / 100.0;
                }
                "--out" => out = value()?.into(),
                "--any-renderer" => any_renderer = true,
                other if other.starts_with("--") => return Err(format!("unknown option {other}")),
                other if map.is_none() => map = Some(PathBuf::from(other)),
                other => return Err(format!("unexpected argument {other}")),
            }
        }
        let map = map.ok_or("no beatmap given")?;
        if times.is_empty() {
            return Err("no --time given".to_owned());
        }
        let shots = Shots {
            map,
            settings,
            times,
            size,
            any_renderer,
        };
        check(&shots)?;
        Ok(Self { shots, out })
    }
}

fn parse_size(v: &str) -> Result<(u32, u32), String> {
    let bad = || format!("bad --size {v}, expected WxH");
    let (w, h) = v.split_once('x').ok_or_else(bad)?;
    let (w, h): (u32, u32) = (w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?);
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err(bad());
    }
    Ok((w, h))
}

/// Draws the screenshots and writes them to `options.out`.
pub fn run(options: &Options) -> Result<(), String> {
    let frames = render(&options.shots)?;
    let stem = options
        .shots
        .map
        .file_stem()
        .map_or_else(|| "map".into(), |s| s.to_string_lossy());
    std::fs::create_dir_all(&options.out).map_err(|e| format!("{}: {e}", options.out.display()))?;
    for (frame, time) in frames.iter().zip(&options.shots.times) {
        let path = options.out.join(format!("{stem}-{time}.png"));
        frame.write_png(&path)?;
        println!("{}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn parses_times_size_and_skin() {
        let options = parse(&[
            "map.osu", "--time", "1000", "--time", "2500.5", "--size", "320x240", "--skin",
            "skins/a", "--dim", "50", "--out", "shots",
        ])
        .unwrap();
        assert_eq!(options.shots.map, PathBuf::from("map.osu"));
        assert_eq!(options.shots.times, [1000.0, 2500.5]);
        assert_eq!(options.shots.size, (320, 240));
        assert_eq!(options.shots.settings.skin, Some(PathBuf::from("skins/a")));
        assert_eq!(options.shots.settings.dim, 0.5);
        assert_eq!(options.out, PathBuf::from("shots"));
        assert!(!options.shots.any_renderer);
    }

    #[test]
    fn refuses_missing_and_bad_values() {
        assert!(parse(&["--time", "1"]).is_err(), "no map");
        assert!(parse(&["map.osu"]).is_err(), "no time");
        assert!(parse(&["map.osu", "--time", "x"]).is_err());
        assert!(parse(&["map.osu", "--time", "NaN"]).is_err());
        assert!(parse(&["map.osu", "--time", "1", "--size", "0x10"]).is_err());
        assert!(parse(&["map.osu", "--time", "1", "--size", "640"]).is_err());
        assert!(parse(&["map.osu", "--time", "1", "--dim", "101"]).is_err());
        assert!(parse(&["map.osu", "--time", "1", "--frobnicate"]).is_err());
        assert!(
            parse(&["map.osu", "--time", "2", "--time", "1"]).is_err(),
            "times out of order"
        );
        assert!(parse(&["map.osu", "--time", "1", "--time", "1"]).is_err());
    }

    #[test]
    fn png_round_trip_keeps_pixels() {
        let dir = std::env::temp_dir().join(format!("slam-screenshot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frame.png");
        let frame = Frame {
            width: 3,
            height: 2,
            pixels: (0..24).map(|i| i * 10).collect(),
        };
        frame.write_png(&path).unwrap();
        assert_eq!(Frame::read_png(&path).unwrap(), frame);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
