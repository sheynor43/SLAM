//! Audio-thread code paths that can be checked without a device: none may touch the
//! allocator.

use std::sync::Arc;

use slam_audio::{
    AudioCallback, AudioPosition, CallbackInfo, MixerConfig, PositionSnapshot, Sample, SineTone,
};
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

#[test]
fn sine_tone_does_not_allocate() {
    let mut tone = SineTone::new(440.0, 0.5);
    let mut out = vec![0.0f32; 2 * 1024];
    let mut info = CallbackInfo {
        frame: 0,
        timestamp_ns: 0,
        latency_ns: 0,
        sample_rate: 48_000,
        channels: 2,
    };
    assert_no_alloc(|| {
        for frames in [1usize, 64, 128, 1024] {
            tone.process(&mut out[..2 * frames], &info);
            info.frame += frames as u64;
        }
    });
}

#[test]
fn position_publish_and_read_do_not_allocate() {
    let pos = AudioPosition::new();
    let read = assert_no_alloc(|| {
        for frame in 0..1_000u64 {
            pos.publish(PositionSnapshot {
                frame,
                timestamp_ns: frame,
                latency_ns: 0,
                sample_rate: 48_000,
            });
        }
        pos.read()
    });
    assert_eq!(read.map(|s| s.frame), Some(999));
}

#[test]
fn mixer_commands_and_process_do_not_allocate() {
    let (mut mixer, mut handle) = slam_audio::mixer(&MixerConfig {
        max_voices: 8,
        queue_capacity: 16,
        ..MixerConfig::default()
    })
    .unwrap();
    // Held here like a sample bank would, so released references are decrements.
    let short = Arc::new(Sample::from_interleaved(vec![0.25; 2 * 100], 48_000, 2).unwrap());
    let long = Arc::new(Sample::from_interleaved(vec![0.25; 2 * 5_000], 48_000, 2).unwrap());
    let mut out = vec![0.0f32; 2 * 1024];
    let mut info = CallbackInfo {
        frame: 0,
        timestamp_ns: 0,
        latency_ns: 0,
        sample_rate: 48_000,
        channels: 2,
    };
    assert_no_alloc(|| {
        for round in 0..50 {
            // More plays than voices: steals; short voices finish inside a buffer.
            for i in 0..12 {
                let sample = if i % 2 == 0 { &short } else { &long };
                handle.play(sample, 0.5).unwrap();
            }
            if round % 10 == 9 {
                handle.stop_all().unwrap();
                handle.set_master_gain(0.8).unwrap();
            }
            for frames in [1usize, 64, 128, 1024] {
                mixer.process(&mut out[..2 * frames], &info);
                info.frame += frames as u64;
            }
            handle.collect();
        }
        // A full queue rejects without allocating or freeing.
        while handle.play(&short, 1.0).is_ok() {}
        mixer.process(&mut out, &info);
        handle.collect();
    });
    drop(mixer);
    handle.collect();
    assert_eq!(Arc::strong_count(&short), 1);
    assert_eq!(Arc::strong_count(&long), 1);
}
