//! Plays a 440 Hz test tone and prints what the backend reports: devices, frames per
//! callback, latency, playback position and missed buffers.
//!
//! Run: `cargo run --release -p slam-audio --example tone [--backend NAME] [seconds]
//! [buffer_frames] [device]` (defaults: first available backend, 5 s, 128 frames, system
//! default device). `just tone` does the same. Backend names: `pipewire`,
//! `wasapi-shared`, `wasapi-low-latency`, `wasapi-exclusive`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use slam_audio::{AudioCallback, Backend, CallbackInfo, SineTone, StreamConfig};

/// Wraps the tone to expose the last buffer size and latency to the main thread.
struct Probe {
    tone: SineTone,
    frames: Arc<AtomicU32>,
    latency_ns: Arc<AtomicU64>,
}

impl AudioCallback for Probe {
    fn process(&mut self, out: &mut [f32], info: &CallbackInfo) {
        self.tone.process(out, info);
        let frames = out.len() / usize::from(info.channels);
        self.frames.store(frames as u32, Ordering::Relaxed);
        self.latency_ns.store(info.latency_ns, Ordering::Relaxed);
    }
}

/// Engine clock base on Linux (ADR-0022).
#[cfg(target_os = "linux")]
fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Engine clock base on Windows (ADR-0022): QPC in nanoseconds.
#[cfg(windows)]
fn monotonic_ns() -> u64 {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut ticks, mut freq) = (0i64, 0i64);
    // SAFETY: valid out pointers; never fails on Windows XP and later.
    unsafe {
        let _ = QueryPerformanceFrequency(&mut freq);
        let _ = QueryPerformanceCounter(&mut ticks);
    }
    (ticks as u128 * 1_000_000_000 / freq.max(1) as u128) as u64
}

/// No backend on other targets, so no position is ever read.
#[cfg(not(any(target_os = "linux", windows)))]
fn monotonic_ns() -> u64 {
    0
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let backend = match args.iter().position(|a| a == "--backend") {
        Some(i) => {
            let name = args.get(i + 1).ok_or("--backend needs a value")?.clone();
            args.drain(i..i + 2);
            Backend::from_name(&name).ok_or_else(|| format!("unknown backend {name}"))?
        }
        None => *Backend::available()
            .first()
            .ok_or("no audio backend in this build")?,
    };
    let mut args = args.into_iter();
    let seconds: u64 = args.next().map_or(Ok(5), |s| s.parse())?;
    let buffer_frames: u32 = args.next().map_or(Ok(128), |s| s.parse())?;
    let device = args.next();

    println!("backend: {}", backend.name());
    for d in slam_audio::devices(backend)? {
        println!("  device {:?}: {}", d.id, d.name);
    }

    let frames = Arc::new(AtomicU32::new(0));
    let latency_ns = Arc::new(AtomicU64::new(0));
    let config = StreamConfig {
        device,
        buffer_frames,
        name: "SLAM test tone".to_owned(),
        ..StreamConfig::default()
    };
    let probe = Probe {
        tone: SineTone::new(440.0, 0.2),
        frames: Arc::clone(&frames),
        latency_ns: Arc::clone(&latency_ns),
    };
    let stream = slam_audio::open_output(backend, config, Box::new(probe))?;
    let rate = stream.config().sample_rate;
    let granted = stream.config().buffer_frames;
    println!(
        "opened {}: requested {buffer_frames} frames ({:.2} ms), granted {granted} ({:.2} ms) \
         at {rate} Hz",
        stream.backend().name(),
        f64::from(buffer_frames) * 1e3 / f64::from(rate),
        f64::from(granted) * 1e3 / f64::from(rate),
    );

    for _ in 0..seconds * 2 {
        std::thread::sleep(Duration::from_millis(500));
        let n = frames.load(Ordering::Relaxed);
        let output = stream
            .position()
            .read()
            .map(|p| p.output_frame_at(monotonic_ns()) / f64::from(p.sample_rate));
        println!(
            "frames/callback {n} ({:.2} ms) | latency {:.2} ms | output {} | callbacks {} | missed {}",
            f64::from(n) * 1e3 / f64::from(rate),
            latency_ns.load(Ordering::Relaxed) as f64 / 1e6,
            output.map_or("-".to_owned(), |s| format!("{s:.3} s")),
            stream.counters().callbacks.load(Ordering::Relaxed),
            stream.counters().missed_buffers.load(Ordering::Relaxed),
        );
    }
    Ok(())
}
