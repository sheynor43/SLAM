//! Streaming real files through the music decoder thread: the result must match the
//! whole-file decode of the same file (`Sample::load`), from the start and after seeks.
//!
//! The files are the hitsound fixtures (see `sample_decode.rs`): 100 ms at 44.1 kHz,
//! a 1 kHz sine from 10 ms.
//!
//! The music path corrects the resampler delay by half a source frame, which
//! `Sample::convert` does not (its output runs that much early, ADR-0033). At
//! 44.1 → 48 kHz that is 0.54 output frames, which rounds to one whole frame: the
//! reference is delayed by one frame before comparing.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use slam_audio::{
    AudioCallback, CallbackInfo, MUSIC_FADE_MS, MusicConfig, MusicEvent, MusicPlayer, MusicSource,
    Sample, TrackSource,
};

const RATE: u32 = 48_000;
const BUFFER: usize = 128;
const FADE: usize = (RATE * MUSIC_FADE_MS / 1_000) as usize;

fn path(name: &str) -> PathBuf {
    PathBuf::from(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR")))
}

/// The whole-file decode at 48 kHz (left channel), delayed by one frame.
fn reference(name: &str) -> Vec<f32> {
    let bytes = std::fs::read(path(name)).unwrap();
    let sample = Sample::load(&bytes, name.rsplit('.').next(), RATE, 2).unwrap();
    let left = sample.data().iter().step_by(2).copied();
    std::iter::once(0.0)
        .chain(left)
        .take(sample.frames())
        .collect()
}

struct Stream {
    source: MusicSource,
    player: MusicPlayer,
    frame: u64,
}

impl Stream {
    fn new() -> Self {
        let (source, player) = slam_audio::music(&MusicConfig::default()).unwrap();
        Self {
            source,
            player,
            frame: 0,
        }
    }

    fn load(&mut self, name: &str) -> Option<u64> {
        let generation = self.player.load(TrackSource::File(path(name))).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.player.poll_event() {
                Some(MusicEvent::Loaded {
                    generation: g,
                    length,
                }) if g == generation => {
                    return length;
                }
                Some(other) => panic!("{name}: {other:?}"),
                None if Instant::now() > deadline => panic!("{name}: no Loaded event"),
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }

    /// Lets the decoder thread fill the pool (the fixtures fit in it): waits until it
    /// stops queueing blocks.
    fn settle(&mut self) {
        self.run(0);
        let decoded = || {
            self.player
                .stats()
                .decoded_blocks
                .load(std::sync::atomic::Ordering::Relaxed)
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut last, mut stable) = (decoded(), 0);
        while stable < 5 {
            assert!(Instant::now() < deadline, "the decoder never settled");
            std::thread::sleep(Duration::from_millis(10));
            let now = decoded();
            stable = if now == last && now > 0 {
                stable + 1
            } else {
                0
            };
            last = now;
        }
    }

    /// Plays until the track ends; returns the left channel.
    fn play_to_end(&mut self) -> Vec<f32> {
        let mut left = Vec::new();
        let mut out = [0.0f32; 2 * BUFFER];
        for _ in 0..10_000 {
            let info = CallbackInfo {
                frame: self.frame,
                timestamp_ns: 0,
                latency_ns: 0,
                sample_rate: RATE,
                channels: 2,
            };
            self.source.process(&mut out, &info);
            self.frame += BUFFER as u64;
            left.extend(out.iter().step_by(2));
            if self.player.position().read().is_some_and(|s| s.ended) {
                return left;
            }
        }
        panic!("the track never ended");
    }

    fn run(&mut self, frames: usize) {
        let mut out = vec![0.0f32; 2 * frames];
        self.source.process(
            &mut out,
            &CallbackInfo {
                frame: self.frame,
                timestamp_ns: 0,
                latency_ns: 0,
                sample_rate: RATE,
                channels: 2,
            },
        );
        self.frame += frames as u64;
    }
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

#[test]
fn streamed_files_match_the_whole_file_decode() {
    for name in [
        "tone-s16-mono.wav",
        "tone-vorbis-stereo.ogg",
        "tone-mp3-mono.mp3",
    ] {
        let want = reference(name);
        let mut stream = Stream::new();
        let length = stream.load(name);
        if let Some(length) = length {
            assert!(
                length.abs_diff(want.len() as u64) <= 1,
                "{name}: length {length}"
            );
        }
        stream.settle();
        stream.player.play().unwrap();
        let got = stream.play_to_end();
        let end = stream.player.position().read().unwrap().anchor.track_frame as usize;
        assert!(
            end.abs_diff(want.len()) <= 1,
            "{name}: ended at {end}, want {} length {length:?}",
            want.len()
        );
        // The fade-in only touches the leading silence.
        let diff = max_diff(&got[..end.min(want.len())], &want);
        // Float rounding of chunked vs whole-file resampling, under -60 dB.
        assert!(diff < 1e-3, "{name}: differs by {diff}");
        assert_eq!(
            stream.source_stats_underruns(),
            0,
            "{name}: underran with a full pool"
        );
    }
}

#[test]
fn streamed_seek_lands_on_the_same_audio() {
    for name in [
        "tone-s16-mono.wav",
        "tone-vorbis-stereo.ogg",
        "tone-mp3-mono.mp3",
    ] {
        let want = reference(name);
        let mut stream = Stream::new();
        stream.load(name);
        stream.settle();
        // 50 ms in, inside the sine; while paused, then play.
        let target = 2_400;
        stream.player.seek(target as u64).unwrap();
        stream.settle();
        stream.player.play().unwrap();
        let got = stream.play_to_end();
        let s = stream.player.position().read().unwrap();
        assert_eq!(s.generation, stream.player.generation());
        // Past the fade-in and the resampler restart transient.
        let from = FADE + 64;
        let n = want.len() - target - from - 64;
        let diff = max_diff(
            &got[from..from + n],
            &want[target + from..target + from + n],
        );
        assert!(diff < 0.02, "{name}: differs by {diff} after seek");
    }
}

impl Stream {
    fn source_stats_underruns(&self) -> u64 {
        self.player
            .stats()
            .underruns
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}
