//! Benchmark scene: an empty frame plus moving sprites, drawn for a fixed time, then
//! a report of frame rate, frame-time percentiles, limiter lateness and draw calls.
//! Developer tool: its console output is not localised.

mod record;
mod scene;

use std::sync::Arc;
use std::time::Duration;

use slam_engine::clock::now_ns;
use slam_engine::gl::SdlGlSurface;
use slam_engine::limiter::LimiterMode;
use slam_engine::{Engine, EngineConfig, EngineCounters, TickInfo, Update};
use slam_input::{InputEvent, InputWindow, TickMapper, event_ring, sdl_ticks_ns};

pub use record::{FrameWork, MAX_RECORDED_FPS, Percentiles, Recorder, Report};
pub use scene::{Scene, fill};

/// Longest measurement or warm-up accepted: buffers for the whole window are
/// allocated up front (16 bytes per frame at [`MAX_RECORDED_FPS`]).
pub const MAX_SECONDS: f64 = 300.0;

pub const USAGE: &str = "usage: slam-app bench [--seconds S] [--warmup S] [--sprites N] \
                         [--hz HZ (0 = unlimited)] [--spin-us US]";

/// Benchmark settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// Length of the measurement window.
    pub seconds: f64,
    /// Time from start that is not measured: GL setup, driver warm-up.
    pub warmup: f64,
    pub sprites: usize,
    /// Draw rate limit; `None` draws as fast as possible.
    pub hz: Option<f64>,
    /// Limiter busy-wait threshold; `None` keeps the engine default.
    pub spin_us: Option<u64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            seconds: 10.0,
            warmup: 1.0,
            sprites: 500,
            hz: None,
            spin_us: None,
        }
    }
}

impl Options {
    /// Parses the arguments after `bench`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--seconds" => options.seconds = number(&flag, &value()?)?,
                "--warmup" => options.warmup = number(&flag, &value()?)?,
                "--sprites" => options.sprites = number(&flag, &value()?)?,
                "--hz" => {
                    let hz: f64 = number(&flag, &value()?)?;
                    options.hz = (hz != 0.0).then_some(hz);
                }
                "--spin-us" => options.spin_us = Some(number(&flag, &value()?)?),
                _ => return Err(format!("unknown argument {flag}")),
            }
        }
        let valid = |x: f64| (0.0..=MAX_SECONDS).contains(&x);
        if !valid(options.seconds) || options.seconds == 0.0 || !valid(options.warmup) {
            return Err(format!(
                "--seconds must be in (0, {MAX_SECONDS}] and --warmup in [0, {MAX_SECONDS}]"
            ));
        }
        Ok(options)
    }
}

fn number<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag}: invalid value {value}"))
}

struct Idle;

impl Update for Idle {
    type Snapshot = ();

    fn tick(&mut self, _: &TickInfo, _: &[InputEvent], _: &mut ()) {}
}

/// Runs the benchmark in a window and prints the report.
pub fn run(options: &Options) -> Result<(), String> {
    let mut window =
        InputWindow::new_opengl("SLAM benchmark", 1280, 720).map_err(|e| e.to_string())?;
    let surface = SdlGlSurface(window.create_gl_context().map_err(|e| e.to_string())?);

    let epoch_ns = now_ns();
    let to_ns = |s: f64| (s * 1e9) as u64;
    let start_ns = epoch_ns + to_ns(options.warmup);
    let end_ns = start_ns + to_ns(options.seconds);
    let recorder = Recorder::new(start_ns, end_ns, Recorder::capacity_for(options.seconds));

    let mut config = EngineConfig {
        draw: options.hz.map_or(LimiterMode::Unlimited, LimiterMode::Hz),
        ..EngineConfig::default()
    };
    if let Some(spin_us) = options.spin_us {
        config.spin_threshold_ns = spin_us.saturating_mul(1000);
    }
    let counters = Arc::new(EngineCounters::default());
    let (mut sink, source) = event_ring(1024);
    let engine = Engine::spawn_with_counters(
        config,
        Idle,
        Scene::new(
            surface,
            options.sprites,
            epoch_ns,
            recorder,
            window.quit_handle(),
        ),
        (),
        source,
        counters,
    )
    .map_err(|e| e.to_string())?;
    sink.set_notify(engine.waker().as_notify());

    // Close the window shortly after the window ends; closing it earlier stops the
    // benchmark early.
    let quit = window.quit_handle();
    let timer = std::thread::spawn(move || {
        let until = end_ns + 100_000_000;
        std::thread::sleep(Duration::from_nanos(until.saturating_sub(now_ns())));
        quit.request();
    });

    let mut mapper = TickMapper::new(now_ns, sdl_ticks_ns);
    let ran = window.run(&mut sink, &mut mapper);

    // Stop drawing and destroy the GL context before the window.
    let mut stopped = engine.shutdown();
    let scene = &mut stopped.draw;
    let report = scene.recorder.report();
    let sprites = options.sprites;
    let mode = options
        .hz
        .map_or_else(|| "unlimited".to_owned(), |hz| format!("{hz} Hz"));
    if cfg!(debug_assertions) {
        println!("warning: debug build with GL debug output, numbers are not representative");
    }
    println!("driver: {}", scene.driver);
    println!(
        "{sprites} sprites, draw {mode}, {} s after {} s warm-up",
        options.seconds, options.warmup
    );
    print!("{report}");
    drop(stopped);
    drop(window);
    // The timer may still be sleeping if the window was closed early.
    drop(timer);
    ran.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn defaults() {
        assert_eq!(parse(&[]), Ok(Options::default()));
        assert_eq!(Options::default().sprites, 500);
    }

    #[test]
    fn flags() {
        let o = parse(&[
            "--seconds",
            "5",
            "--warmup",
            "0",
            "--sprites",
            "0",
            "--hz",
            "4000",
            "--spin-us",
            "50",
        ])
        .unwrap();
        assert_eq!(
            o,
            Options {
                seconds: 5.0,
                warmup: 0.0,
                sprites: 0,
                hz: Some(4000.0),
                spin_us: Some(50),
            }
        );
        assert_eq!(parse(&["--hz", "0"]).unwrap().hz, None);
    }

    #[test]
    fn errors() {
        assert!(parse(&["--sprites"]).is_err());
        assert!(parse(&["--sprites", "-1"]).is_err());
        assert!(parse(&["--seconds", "0"]).is_err());
        assert!(parse(&["--seconds", "inf"]).is_err());
        assert!(parse(&["--seconds", "NaN"]).is_err());
        assert!(parse(&["--seconds", "301"]).is_err());
        assert!(parse(&["--warmup", "1e12"]).is_err());
        assert!(parse(&["--warmup", "-1"]).is_err());
        assert!(parse(&["--frobnicate"]).is_err());
    }
}
