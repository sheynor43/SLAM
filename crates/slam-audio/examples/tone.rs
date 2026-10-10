//! Plays a 440 Hz test tone and prints what the backend reports: devices, frames per
//! callback, latency, playback position and missed buffers.
//!
//! Run: `cargo run --release -p slam-audio --example tone [seconds] [buffer_frames] [device]`
//! (defaults: 5 s, 128 frames, system default device). `just tone` does the same.

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

/// No backend outside Linux yet, so no position is ever read.
#[cfg(not(target_os = "linux"))]
fn monotonic_ns() -> u64 {
    0
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().map_or(Ok(5), |s| s.parse())?;
    let buffer_frames: u32 = args.next().map_or(Ok(128), |s| s.parse())?;
    let device = args.next();

    let backend = *Backend::available()
        .first()
        .ok_or("no audio backend in this build")?;
    println!("backend: {backend:?}");
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
    println!(
        "requested {buffer_frames} frames ({:.2} ms) at {rate} Hz",
        f64::from(buffer_frames) * 1e3 / f64::from(rate)
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
