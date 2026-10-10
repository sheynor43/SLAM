//! Real PipeWire output stream. Needs a running PipeWire server and plays a quiet tone
//! for about a second, so it is ignored by default:
//! `cargo test -p slam-audio --test pipewire -- --ignored`.
#![cfg(target_os = "linux")]

mod common;

use slam_audio::{Backend, StreamConfig};

slam_testkit::install_counting_allocator!();

#[test]
#[ignore = "needs a running PipeWire server"]
fn pipewire_stream_plays_without_allocating() {
    let devices = slam_audio::devices(Backend::PipeWire).expect("listing devices");
    assert!(!devices.is_empty(), "no Audio/Sink nodes");

    let (checker, shared) = common::Checker::new();
    let stream = slam_audio::open_output(
        Backend::PipeWire,
        StreamConfig {
            name: "SLAM test".to_owned(),
            ..StreamConfig::default()
        },
        Box::new(checker),
    )
    .expect("opening the stream");
    assert_eq!(stream.config().sample_rate, 48_000);
    common::play_and_check(stream, &shared);
}
