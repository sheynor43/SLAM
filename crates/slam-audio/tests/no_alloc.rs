//! Audio-thread code paths that can be checked without a device: none may touch the
//! allocator.

use slam_audio::{AudioCallback, AudioPosition, CallbackInfo, PositionSnapshot, SineTone};
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
