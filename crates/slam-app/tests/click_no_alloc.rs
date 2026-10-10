//! The click test's update tick (queue a hitsound, record the press, collect voice
//! starts) must not allocate. The mixer side is covered in `slam-audio`.

use std::sync::Arc;

use slam_app::click::{Clicker, tone_burst};
use slam_audio::{AudioCallback, CallbackInfo, MixerConfig};
use slam_engine::{TickInfo, Update};
use slam_input::{InputEvent, InputKind, MouseButton};

slam_testkit::install_counting_allocator!();

#[test]
fn click_tick_does_not_allocate() {
    let (mut mixer, handle) = slam_audio::mixer(&MixerConfig {
        start_reports: 64,
        ..MixerConfig::default()
    })
    .unwrap();
    let sample = Arc::new(tone_burst(48_000, 2, 1000.0, 30.0));
    let mut clicker = Clicker::new(handle, Arc::clone(&sample), 1_000);
    let events = [
        InputEvent {
            time_ns: 10,
            kind: InputKind::MouseButton {
                button: MouseButton::Left,
                down: true,
            },
        },
        InputEvent {
            time_ns: 11,
            kind: InputKind::MouseMove { x: 1.0, y: 2.0 },
        },
        InputEvent {
            time_ns: 12,
            kind: InputKind::Key {
                scancode: 4,
                down: true,
            },
        },
        InputEvent {
            time_ns: 13,
            kind: InputKind::Key {
                scancode: 4,
                down: false,
            },
        },
    ];
    let mut out = vec![0.0f32; 2 * 128];
    let mut info = CallbackInfo {
        frame: 0,
        timestamp_ns: 1,
        latency_ns: 1_000_000,
        sample_rate: 48_000,
        channels: 2,
    };
    let tick = TickInfo {
        now_ns: 20,
        deadline_ns: 1_000_000,
        by_input: true,
    };
    let mut presses = 0;
    slam_testkit::assert_no_alloc(|| {
        // Past the recording capacity too: counted, not recorded.
        for _ in 0..600 {
            clicker.tick(&tick, &events, &mut presses);
            mixer.process(&mut out, &info);
            info.frame += 128;
        }
        clicker.tick(&tick, &[], &mut presses);
    });
    assert_eq!(presses, 1_200);
    assert_eq!(clicker.clicks.clicks().len(), 1_000);
    assert_eq!(clicker.clicks.starts().len(), 1_000);
    assert_eq!(clicker.clicks.unrecorded_clicks, 200);
    assert_eq!(clicker.clicks.unrecorded_starts, 200);
    assert_eq!(clicker.clicks.rejected, 0);
}
