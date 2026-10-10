//! Click test: every key or mouse button press plays a hitsound through the whole
//! path (window → input thread → update → mixer queue → audio device), and the
//! report splits the latency into its parts. Developer tool: its console output is
//! not localised. Method of the physical check: `docs/perf/2026-10-10-click-to-sound.md`.

pub mod analyze;
mod clicker;
mod flash;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use slam_audio::{Backend, MixerConfig, Sample, StreamConfig};
use slam_engine::clock::now_ns;
use slam_engine::{Engine, EngineConfig};
use slam_input::{InputWindow, TickMapper, event_ring, sdl_ticks_ns};

pub use clicker::{Click, ClickReport, Clicker, Clicks};
pub use flash::Flash;

use crate::bench::{MAX_SECONDS, number};

pub const USAGE: &str = "usage: slam-app click [--seconds S] [--device ID] [--buffer FRAMES] \
                         [--tone-hz HZ] [--sample FILE] [--dump FILE.csv] | --devices";

/// Presses recorded at most; more are counted, not recorded.
pub const MAX_PRESSES: usize = 100_000;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const TONE_MS: f64 = 30.0;

/// Click test settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// The window closes by itself after this long.
    pub seconds: f64,
    /// Output device id (`--devices`); `None` follows the system default.
    pub device: Option<String>,
    /// Requested frames per audio buffer (PipeWire quantum).
    pub buffer_frames: u32,
    /// Frequency of the generated tone burst.
    pub tone_hz: f64,
    /// Plays this file instead of the tone.
    pub sample: Option<PathBuf>,
    /// Where to write per-press timings as CSV.
    pub dump: Option<PathBuf>,
    /// Lists output devices and exits.
    pub list_devices: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            seconds: 60.0,
            device: None,
            buffer_frames: StreamConfig::default().buffer_frames,
            tone_hz: 1000.0,
            sample: None,
            dump: None,
            list_devices: false,
        }
    }
}

impl Options {
    /// Parses the arguments after `click`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--seconds" => options.seconds = number(&flag, &value()?)?,
                "--device" => options.device = Some(value()?),
                "--buffer" => options.buffer_frames = number(&flag, &value()?)?,
                "--tone-hz" => options.tone_hz = number(&flag, &value()?)?,
                "--sample" => options.sample = Some(PathBuf::from(value()?)),
                "--dump" => options.dump = Some(PathBuf::from(value()?)),
                "--devices" => options.list_devices = true,
                _ => return Err(format!("unknown argument {flag}")),
            }
        }
        if !(options.seconds > 0.0 && options.seconds <= MAX_SECONDS) {
            return Err(format!("--seconds must be in (0, {MAX_SECONDS}]"));
        }
        let nyquist = f64::from(SAMPLE_RATE) / 2.0;
        if !(options.tone_hz > 0.0 && options.tone_hz < nyquist) {
            return Err(format!("--tone-hz must be in (0, {nyquist})"));
        }
        Ok(options)
    }
}

/// A sine burst of `hz` at full scale ×0.5 in every channel, `ms` long, with a 1 ms
/// raised-cosine fade-in and a 5 ms fade-out so that it has no broadband click.
pub fn tone_burst(rate: u32, channels: u16, hz: f64, ms: f64) -> Sample {
    let frames = (f64::from(rate) * ms / 1000.0).round() as usize;
    let fade_in = (f64::from(rate) / 1000.0) as usize;
    let fade_out = fade_in * 5;
    let mut data = Vec::with_capacity(frames * usize::from(channels));
    for i in 0..frames {
        let ramp =
            |n: usize, len: usize| 0.5 - 0.5 * (std::f64::consts::PI * n as f64 / len as f64).cos();
        let envelope = if i < fade_in {
            ramp(i, fade_in)
        } else if frames - i <= fade_out {
            ramp(frames - i, fade_out)
        } else {
            1.0
        };
        let t = i as f64 / f64::from(rate);
        let value = (0.5 * envelope * (std::f64::consts::TAU * hz * t).sin()) as f32;
        data.extend(std::iter::repeat_n(value, usize::from(channels)));
    }
    Sample::from_interleaved(data, rate, channels).expect("tone burst is a valid sample")
}

fn load_sample(options: &Options) -> Result<Sample, String> {
    let Some(path) = &options.sample else {
        return Ok(tone_burst(SAMPLE_RATE, CHANNELS, options.tone_hz, TONE_MS));
    };
    let bytes = std::fs::read(path).map_err(|e| format!("--sample {}: {e}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str());
    let sample = Sample::load(&bytes, ext, SAMPLE_RATE, CHANNELS)
        .map_err(|e| format!("--sample {}: {e}", path.display()))?;
    // The mixer ignores an empty sample: presses would play and record nothing.
    if sample.data().is_empty() {
        return Err(format!("--sample {}: no audio", path.display()));
    }
    Ok(sample)
}

/// Runs the click test in a window and prints the report.
pub fn run(options: &Options) -> Result<(), String> {
    let backend = *Backend::available()
        .first()
        .ok_or("no audio backend in this build")?;
    if options.list_devices {
        for device in slam_audio::devices(backend).map_err(|e| e.to_string())? {
            println!("{}\t{}", device.id, device.name);
        }
        return Ok(());
    }
    let sample = Arc::new(load_sample(options)?);
    let (mixer, handle) = slam_audio::mixer(&MixerConfig {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        start_reports: 1024,
        ..MixerConfig::default()
    })
    .map_err(|e| e.to_string())?;
    let stream = slam_audio::open_output(
        backend,
        StreamConfig {
            device: options.device.clone(),
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            buffer_frames: options.buffer_frames,
            name: "SLAM click test".to_owned(),
        },
        Box::new(mixer),
    )
    .map_err(|e| e.to_string())?;

    let mut window =
        InputWindow::new_opengl("SLAM click test", 640, 360).map_err(|e| e.to_string())?;
    let surface =
        slam_engine::gl::SdlGlSurface(window.create_gl_context().map_err(|e| e.to_string())?);
    let (mut sink, source) = event_ring(1024);
    let engine = Engine::spawn(
        EngineConfig::default(),
        Clicker::new(handle, Arc::clone(&sample), MAX_PRESSES),
        Flash::new(surface, window.quit_handle()),
        0,
        source,
    )
    .map_err(|e| e.to_string())?;
    sink.set_notify(engine.waker().as_notify());

    let end_ns = now_ns() + (options.seconds * 1e9) as u64;
    let quit = window.quit_handle();
    let timer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_nanos(end_ns.saturating_sub(now_ns())));
        quit.request();
    });
    println!(
        "click or press keys in the window; it closes after {} s",
        options.seconds
    );
    let mut mapper = TickMapper::new(now_ns, sdl_ticks_ns);
    let ran = window.run(&mut sink, &mut mapper);

    let slam_engine::Stopped {
        update: mut clicker,
        draw,
        ..
    } = engine.shutdown();
    // The GL context goes before the window.
    drop(draw);
    // Let the last queued voices start before the stream closes.
    std::thread::sleep(Duration::from_millis(100));
    let counters = stream.counters();
    let missed = counters.missed_buffers.load(Ordering::Relaxed);
    let failed = counters.failed.load(Ordering::Relaxed);
    drop(stream);
    clicker.finish();
    let lost = clicker
        .mixer()
        .stats()
        .start_report_overflows
        .load(Ordering::Relaxed);

    if cfg!(debug_assertions) {
        println!("warning: debug build, numbers are not representative");
    }
    let source = options.sample.as_ref().map_or_else(
        || format!("{} Hz tone", options.tone_hz),
        |p| p.display().to_string(),
    );
    println!(
        "PipeWire, {} Hz, {} frames requested, device {}, {source}, video: {}",
        SAMPLE_RATE,
        options.buffer_frames,
        options.device.as_deref().unwrap_or("default"),
        window.video_driver()
    );
    if missed > 0 || failed || lost > 0 {
        println!(
            "warning: {missed} missed audio buffers, stream failed: {failed}, \
             {lost} voice-start reports lost"
        );
    }
    print!("{}", clicker.clicks.report());
    let dumped = options.dump.as_ref().map(|path| {
        std::fs::File::create(path)
            .and_then(|file| clicker.clicks.write_csv(std::io::BufWriter::new(file)))
            .map_err(|e| format!("--dump {}: {e}", path.display()))
    });
    drop(window);
    drop(timer);
    ran.map_err(|e| e.to_string())?;
    dumped.transpose().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn flags() {
        assert_eq!(parse(&[]), Ok(Options::default()));
        let o = parse(&[
            "--seconds",
            "30",
            "--device",
            "out",
            "--buffer",
            "64",
            "--tone-hz",
            "2000",
            "--dump",
            "c.csv",
        ])
        .unwrap();
        assert_eq!(o.seconds, 30.0);
        assert_eq!(o.device.as_deref(), Some("out"));
        assert_eq!((o.buffer_frames, o.tone_hz), (64, 2000.0));
        assert!(parse(&["--devices"]).unwrap().list_devices);
        assert!(parse(&["--seconds", "0"]).is_err());
        assert!(parse(&["--tone-hz", "24000"]).is_err());
        assert!(parse(&["--buffer"]).is_err());
        assert!(parse(&["--what"]).is_err());
    }

    #[test]
    fn tone_burst_fades_and_fills_all_channels() {
        let s = tone_burst(48_000, 2, 1000.0, 30.0);
        let data = s.data();
        assert_eq!(data.len(), 2 * 1440);
        assert_eq!(data[0], 0.0);
        assert!(data.chunks(2).all(|f| f[0] == f[1]));
        let peak = data.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 1e-3, "{peak}");
        assert!(data[data.len() - 1].abs() < 1e-3);
    }
}
