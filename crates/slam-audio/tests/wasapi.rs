//! Real WASAPI output streams in every mode. Plays a quiet tone for about a second per
//! mode on the default render endpoint; skipped (passes) when the machine has no active
//! render endpoint, as on CI runners.
#![cfg(windows)]

mod common;

use slam_audio::{Backend, StreamConfig, WasapiMode};

slam_testkit::install_counting_allocator!();

fn check_mode(mode: WasapiMode) {
    let backend = Backend::Wasapi(mode);
    // CI runners may lack the audio service as well as a device.
    match slam_audio::devices(backend) {
        Ok(devices) if !devices.is_empty() => {}
        Ok(_) => {
            eprintln!("no active render endpoint: skipping {mode:?}");
            return;
        }
        Err(err) => {
            eprintln!("WASAPI unavailable ({err}): skipping {mode:?}");
            return;
        }
    }

    let (checker, shared) = common::Checker::new();
    let stream = slam_audio::open_output(
        backend,
        StreamConfig {
            name: "SLAM test".to_owned(),
            ..StreamConfig::default()
        },
        Box::new(checker),
    )
    .expect("opening the stream");
    let Backend::Wasapi(opened) = stream.backend() else {
        panic!("opened as {:?}", stream.backend());
    };
    eprintln!(
        "{mode:?} opened as {opened:?}, {} frames per buffer",
        stream.config().buffer_frames
    );
    let fallback = match mode {
        WasapiMode::Exclusive => &[
            WasapiMode::Exclusive,
            WasapiMode::LowLatency,
            WasapiMode::Shared,
        ][..],
        WasapiMode::LowLatency => &[WasapiMode::LowLatency, WasapiMode::Shared][..],
        WasapiMode::Shared => &[WasapiMode::Shared][..],
    };
    assert!(fallback.contains(&opened), "{mode:?} opened as {opened:?}");
    if opened != mode {
        eprintln!("NOTE: {mode:?} fell back to {opened:?} on a free device");
    }
    assert_eq!(stream.config().sample_rate, 48_000);
    common::play_and_check(stream, &shared);
}

// One test for all modes: streams on the same device in parallel would fight over it
// (exclusive locks it, low-latency periods lock the engine).
#[test]
fn streams_play_without_allocating() {
    for mode in [
        WasapiMode::Shared,
        WasapiMode::LowLatency,
        WasapiMode::Exclusive,
    ] {
        check_mode(mode);
    }
}
