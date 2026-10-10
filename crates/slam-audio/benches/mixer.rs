//! Hitsound path costs: queuing a play command on the update thread, and mixing one
//! buffer on the audio thread with N active voices.
//!
//! Run: `cargo bench -p slam-audio --bench mixer`. Not built for Windows targets (no
//! criterion there, see `Cargo.toml`).

#[cfg(windows)]
fn main() {}

#[cfg(not(windows))]
mod bench {

    use std::hint::black_box;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use criterion::{BenchmarkId, Criterion, criterion_group};
    use slam_audio::{AudioCallback, CallbackInfo, MixerConfig, Sample};

    const RATE: u32 = 48_000;
    const BUFFER_FRAMES: usize = 128;

    fn info() -> CallbackInfo {
        CallbackInfo {
            frame: 0,
            timestamp_ns: 0,
            latency_ns: 0,
            sample_rate: RATE,
            channels: 2,
        }
    }

    /// Two seconds of stereo noise-like PCM: long enough that no voice ends mid-bench.
    fn sample() -> Arc<Sample> {
        let data = (0..2 * 2 * RATE as usize)
            .map(|i| ((i * 7_919) % 2_001) as f32 / 2_000.0 - 0.5)
            .collect();
        Arc::new(Sample::from_interleaved(data, RATE, 2).unwrap())
    }

    fn enqueue(c: &mut Criterion) {
        let capacity = 1_024;
        let (mut mixer, mut handle) = slam_audio::mixer(&MixerConfig {
            queue_capacity: capacity,
            ..MixerConfig::default()
        })
        .unwrap();
        let sample = sample();
        let mut out = [0.0f32; 0];
        c.bench_function("mixer/play_enqueue", |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                let mut left = iters;
                while left > 0 {
                    // One slot stays free for the `stop_all` below.
                    let batch = left.min(capacity as u64 - 1);
                    let start = Instant::now();
                    for _ in 0..batch {
                        handle.play(black_box(&sample), 0.5).unwrap();
                    }
                    total += start.elapsed();
                    // Untimed: let the mixer take the commands and give the samples back.
                    handle.stop_all().unwrap();
                    mixer.process(&mut out, &info());
                    left -= batch;
                }
                total
            });
        });
        handle.collect();
    }

    fn process(c: &mut Criterion) {
        let sample = sample();
        let mut group = c.benchmark_group("mixer/process_128_frames");
        for voices in [0usize, 1, 8, 32, 64] {
            let (mut mixer, mut handle) = slam_audio::mixer(&MixerConfig::default()).unwrap();
            let mut out = vec![0.0f32; 2 * BUFFER_FRAMES];
            group.bench_with_input(
                BenchmarkId::from_parameter(voices),
                &voices,
                |b, &voices| {
                    b.iter_custom(|iters| {
                        let mut total = Duration::ZERO;
                        // Restart the voices every 500 buffers (~1.3 s of audio) so they never end.
                        for chunk_start in (0..iters).step_by(500) {
                            handle.stop_all().unwrap();
                            for _ in 0..voices {
                                handle.play(&sample, 0.1).unwrap();
                            }
                            let n = (iters - chunk_start).min(500);
                            let start = Instant::now();
                            for _ in 0..n {
                                mixer.process(black_box(&mut out), &info());
                            }
                            total += start.elapsed();
                        }
                        total
                    });
                },
            );
            handle.collect();
        }
        group.finish();
    }

    criterion_group!(benches, enqueue, process);
}

// What `criterion_main!` expands to; the macro cannot sit inside the module.
#[cfg(not(windows))]
fn main() {
    bench::benches();
    criterion::Criterion::default()
        .configure_from_args()
        .final_summary();
}
