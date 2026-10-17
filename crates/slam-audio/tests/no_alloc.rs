//! Audio-thread code paths that can be checked without a device: none may touch the
//! allocator.

use std::sync::Arc;

use slam_audio::{
    AudioCallback, AudioPosition, CallbackInfo, ClockError, FilterKind, GameClock, MixerConfig,
    MusicConfig, MusicEvent, PlayError, PositionSnapshot, Sample, SineTone, TrackAdjustment,
    TrackSource, VoiceGroup, VoiceId, VoiceParams,
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
        start_reports: 32,
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
    let mut reports = 0u64;
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
            handle.drain_started(|_| reports += 1);
        }
        // A full queue rejects without allocating or freeing.
        while handle.play(&short, 1.0).is_ok() {}
        mixer.process(&mut out, &info);
        handle.collect();
        handle.drain_started(|_| reports += 1);
    });
    assert_eq!(reports, handle.queued_plays());
    drop(mixer);
    handle.collect();
    assert_eq!(Arc::strong_count(&short), 1);
    assert_eq!(Arc::strong_count(&long), 1);
}

#[test]
fn voice_controls_do_not_allocate() {
    let (mut mixer, mut handle) = slam_audio::mixer(&MixerConfig {
        max_voices: 8,
        queue_capacity: 64,
        ..MixerConfig::default()
    })
    .unwrap();
    let short = Arc::new(Sample::from_interleaved(vec![0.25; 2 * 100], 48_000, 2).unwrap());
    let long = Arc::new(Sample::from_interleaved(vec![0.25; 2 * 5_000], 48_000, 2).unwrap());
    let empty = Arc::new(Sample::from_interleaved(Vec::new(), 48_000, 2).unwrap());
    let mut out = vec![0.0f32; 2 * 1024];
    let info = CallbackInfo {
        frame: 0,
        timestamp_ns: 0,
        latency_ns: 0,
        sample_rate: 48_000,
        channels: 2,
    };
    let mut voices = [VoiceId::NONE; 12];
    assert_no_alloc(|| {
        for round in 0..50 {
            // More voices than the pool: earlier ids go stale by stealing or ending.
            for (i, voice) in voices.iter_mut().enumerate() {
                let sample = match i % 3 {
                    0 => &short,
                    1 => &long,
                    _ => &empty,
                };
                let params = VoiceParams {
                    gain: 0.5,
                    balance: -0.4,
                    rate: 0.5 + i as f64 * 0.25,
                    looping: i % 2 == 1,
                    group: VoiceGroup::ALL[i % 2],
                };
                *voice = handle.play_voice(sample, params).unwrap();
            }
            for (i, &voice) in voices.iter().enumerate() {
                match (round + i) % 4 {
                    0 => handle.set_voice_gain(voice, 1.0, 5.0).unwrap(),
                    1 => handle.set_voice_balance(voice, 0.7).unwrap(),
                    2 => handle.set_voice_rate(voice, 1.0).unwrap(),
                    _ => handle.stop_voice(voice).unwrap(),
                }
            }
            // Bus ramps over the voices, the master one too.
            handle
                .set_group_gain(VoiceGroup::ALL[round % 2], 0.25 * (round % 5) as f32)
                .unwrap();
            if round % 3 == 0 {
                handle.set_master_gain(0.9).unwrap();
            }
            for frames in [1usize, 64, 128, 1024] {
                mixer.process(&mut out[..2 * frames], &info);
            }
            handle.collect();
        }
        handle.stop_all().unwrap();
        mixer.process(&mut out, &info);
        handle.collect();
    });
    drop(mixer);
    handle.collect();
    assert_eq!(Arc::strong_count(&short), 1);
    assert_eq!(Arc::strong_count(&long), 1);
}

/// Accepts a command result where only a full queue may fail it.
fn queued(result: Result<(), PlayError>) {
    assert!(
        matches!(result, Ok(()) | Err(PlayError::QueueFull)),
        "{result:?}"
    );
}

#[test]
fn music_commands_and_mixing_do_not_allocate() {
    let (mut mixer, mut handle) = slam_audio::mixer(&MixerConfig::default()).unwrap();
    let (music, mut player) = slam_audio::music(&MusicConfig {
        block_frames: 256,
        blocks: 4,
        ..MusicConfig::default()
    })
    .unwrap();
    mixer.set_music(music).unwrap();
    let path = format!(
        "{}/tests/data/tone-vorbis-stereo.ogg",
        env!("CARGO_MANIFEST_DIR")
    );
    let generation = player.load(TrackSource::File(path.into())).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !matches!(player.poll_event(), Some(MusicEvent::Loaded { generation: g, .. }) if g == generation)
    {
        assert!(std::time::Instant::now() < deadline, "track did not load");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let hit = Arc::new(Sample::from_interleaved(vec![0.25; 2 * 100], 48_000, 2).unwrap());
    let mut out = vec![0.0f32; 2 * 1024];
    let mut info = CallbackInfo {
        frame: 0,
        timestamp_ns: 0,
        latency_ns: 0,
        sample_rate: 48_000,
        channels: 2,
    };
    // The decoder thread allocates freely; only this thread is counted.
    assert_no_alloc(|| {
        for round in 0..200u64 {
            match round % 7 {
                0 => player.play().unwrap(),
                // The decoder may lag behind this burst of commands: a full queue
                // is a valid answer, not an allocation.
                1 => queued(player.seek(round * 37 % 4_000)),
                2 => player.set_volume(0.5).unwrap(),
                3 => player.pause().unwrap(),
                4 => queued(player.set_rate(if round % 14 == 4 { 1.5 } else { 1.0 })),
                // Frequency moves and filters: the callback reads between frames and
                // filters in chunks.
                5 => queued(player.adjust(&[
                    TrackAdjustment::Frequency {
                        frequency: if round % 14 == 5 { 0.0 } else { 1.0 },
                        duration_ms: 20.0,
                    },
                    TrackAdjustment::Volume(0.5),
                    TrackAdjustment::Filter {
                        kind: FilterKind::LowPass,
                        cutoff_hz: if round % 14 == 5 { 300.0 } else { 22_049.0 },
                        duration_ms: 20.0,
                    },
                    TrackAdjustment::Filter {
                        kind: FilterKind::HighPass,
                        cutoff_hz: if round % 14 == 5 { 300.0 } else { 1.0 },
                        duration_ms: 0.0,
                    },
                ])),
                _ => handle.play(&hit, 0.5).unwrap(),
            }
            for frames in [1usize, 64, 128, 1024] {
                mixer.process(&mut out[..2 * frames], &info);
                info.frame += frames as u64;
            }
            player.position().read();
            handle.collect();
            // Give the decoder a moment, as a real callback period would.
            std::thread::yield_now();
        }
    });
    let stats = player.stats();
    assert_eq!(
        stats
            .leaked_blocks
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[test]
fn game_clock_does_not_allocate() {
    let (mut mixer, _handle) = slam_audio::mixer(&MixerConfig::default()).unwrap();
    let (music, mut player) = slam_audio::music(&MusicConfig {
        block_frames: 256,
        blocks: 4,
        ..MusicConfig::default()
    })
    .unwrap();
    mixer.set_music(music).unwrap();
    let path = format!(
        "{}/tests/data/tone-vorbis-stereo.ogg",
        env!("CARGO_MANIFEST_DIR")
    );
    player.load(TrackSource::File(path.into())).unwrap();
    let mut clock = GameClock::new(&player);
    let mut out = vec![0.0f32; 2 * 256];
    let mut info = CallbackInfo {
        frame: 0,
        timestamp_ns: 1_000_000_000,
        latency_ns: 5_000_000,
        sample_rate: 48_000,
        channels: 2,
    };
    let mut time = 0.0;
    assert_no_alloc(|| {
        for round in 0..2_000u64 {
            mixer.process(&mut out, &info);
            let report = PositionSnapshot {
                frame: info.frame,
                timestamp_ns: info.timestamp_ns,
                latency_ns: info.latency_ns,
                sample_rate: info.sample_rate,
            };
            let now = info.timestamp_ns + 1_000_000;
            if let Some(s) = clock.update(now, Some(report), player.position().read()) {
                time += s.track_time_at(now - 500_000);
                let result = match round % 50 {
                    1 => clock.seek(now, -100.0, &mut player),
                    2 => clock.start(now, &mut player),
                    20 => clock.stop(now, &mut player),
                    21 => clock.seek(now, 500.0, &mut player),
                    22 => clock.set_rate(now, 1.5, &mut player),
                    23 => clock.start(now, &mut player),
                    _ => Ok(()),
                };
                // A full queue (the decoder lagging behind the test's pace) is fine.
                match result {
                    Ok(()) | Err(ClockError::Track(PlayError::QueueFull)) => {}
                    Err(e) => panic!("round {round}: {e:?}"),
                }
            }
            info.frame += 256;
            info.timestamp_ns += 5_333_333;
            // Let the decoder take its seeks, as real ticks would.
            std::thread::sleep(std::time::Duration::from_micros(100));
        }
    });
    assert!(time.is_finite());
}
