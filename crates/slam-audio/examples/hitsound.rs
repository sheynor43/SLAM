//! Plays a hitsound each time Enter is pressed, through the mixer, and prints the
//! mixer counters. A manual check of decoding, resampling and the command queue.
//!
//! Run: `cargo run --release -p slam-audio --example hitsound [file] [buffer_frames]`
//! (defaults: a built-in 5 ms click, 128 frames). Type `q` and Enter to quit.

use std::io::BufRead;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use slam_audio::{Backend, MixerConfig, Sample, StreamConfig};

/// A 5 ms decaying 2 kHz burst: short and sharp, easy to hear the latency of.
fn click(sample_rate: u32) -> Sample {
    let frames = sample_rate as usize / 200;
    let data = (0..frames)
        .flat_map(|n| {
            let t = n as f32 / sample_rate as f32;
            let v = (t * 2_000.0 * std::f32::consts::TAU).sin() * (-t * 800.0).exp() * 0.8;
            [v, v]
        })
        .collect();
    Sample::from_interleaved(data, sample_rate, 2).expect("whole stereo frames")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let file = args.next();
    let buffer_frames: u32 = args.next().map_or(Ok(128), |s| s.parse())?;

    let backend = *Backend::available()
        .first()
        .ok_or("no audio backend in this build")?;
    let config = StreamConfig {
        buffer_frames,
        name: "SLAM hitsound".to_owned(),
        ..StreamConfig::default()
    };
    let (rate, channels) = (config.sample_rate, config.channels);
    let sample = Arc::new(match &file {
        Some(path) => {
            let extension = std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str());
            Sample::load(&std::fs::read(path)?, extension, rate, channels)?
        }
        None => click(rate),
    });
    println!(
        "sample: {} frames ({:.1} ms) at {rate} Hz",
        sample.frames(),
        sample.frames() as f64 * 1e3 / f64::from(rate)
    );

    let (mixer, mut handle) = slam_audio::mixer(&MixerConfig {
        sample_rate: rate,
        channels,
        ..MixerConfig::default()
    })?;
    let stream = slam_audio::open_output(backend, config, Box::new(mixer))?;
    println!("Enter plays the sample, q quits");

    for line in std::io::stdin().lock().lines() {
        if line?.trim() == "q" {
            break;
        }
        if let Err(e) = handle.play(&sample, 1.0) {
            println!("not queued: {e}");
        }
        let stats = handle.stats();
        println!(
            "started {} | active {} | stolen {} | callbacks {} | missed {}",
            stats.started_voices.load(Ordering::Relaxed),
            stats.active_voices.load(Ordering::Relaxed),
            stats.stolen_voices.load(Ordering::Relaxed),
            stream.counters().callbacks.load(Ordering::Relaxed),
            stream.counters().missed_buffers.load(Ordering::Relaxed),
        );
    }
    Ok(())
}
