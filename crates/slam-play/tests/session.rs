//! A play session on a simulated output stream and track: the clock starts on the first
//! position report, ticks are deterministic, the play finishes, and nothing allocates.

mod common;

use std::sync::Arc;

use common::skin::hitsounds;
use common::{beatmap, layout, play};
use slam_audio::{
    MusicAnchor, MusicSnapshot, PlayError, PositionSnapshot, Sample, TrackControl, VoiceId,
    VoiceParams,
};
use slam_input::InputEvent;
use slam_osu::judgement::JudgementLog;
use slam_osu::replay::Rules;
use slam_osu::rules::OsuRules;
use slam_osu::{Beatmap, ModSet};
use slam_play::circles::{
    CircleMods, CircleSettings, CircleSkinConfig, CircleStyle, CircleVisuals, HitCircles,
};
use slam_play::{
    Bindings, FrameTextures, GameTime, GameplaySounds, LiveInput, Offsets, PlaySession,
    PlaySnapshot, PlayState, PlayView, SoundSettings, VoiceSink,
};
use slam_render::sprite::SpriteBatch;
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

const RATE: u32 = 48_000;
const MS: u64 = 1_000_000;
/// Engine time of the first position report.
const T0: u64 = 5_000 * MS;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Command {
    Play,
    PlayAt(u64),
    Pause,
    Seek(u64),
}

#[derive(Default)]
struct Track {
    commands: Vec<Command>,
    generation: u32,
    /// Refuse the next `play_at` once.
    refuse_play_at: bool,
}

impl TrackControl for Track {
    fn play(&mut self) -> Result<(), PlayError> {
        self.commands.push(Command::Play);
        Ok(())
    }

    fn play_at(&mut self, device_frame: u64) -> Result<(), PlayError> {
        if std::mem::take(&mut self.refuse_play_at) {
            return Err(PlayError::QueueFull);
        }
        self.commands.push(Command::PlayAt(device_frame));
        Ok(())
    }

    fn pause(&mut self) -> Result<(), PlayError> {
        self.commands.push(Command::Pause);
        Ok(())
    }

    fn seek(&mut self, frame: u64) -> Result<(), PlayError> {
        self.commands.push(Command::Seek(frame));
        self.generation += 1;
        Ok(())
    }

    fn set_rate(&mut self, _: f64) -> Result<(), PlayError> {
        unreachable!("no rate mods")
    }

    fn generation(&self) -> u32 {
        self.generation
    }
}

/// Takes every command.
#[derive(Default)]
struct Sink {
    plays: u64,
}

impl VoiceSink for Sink {
    fn play_voice(&mut self, _: &Arc<Sample>, _: VoiceParams) -> Result<VoiceId, PlayError> {
        self.plays += 1;
        Ok(VoiceId::from_play(self.plays))
    }

    fn stop_voice(&mut self, _: VoiceId) -> Result<(), PlayError> {
        Ok(())
    }

    fn set_voice_balance(&mut self, _: VoiceId, _: f32) -> Result<(), PlayError> {
        Ok(())
    }

    fn set_voice_gain(&mut self, _: VoiceId, _: f32, _: f64) -> Result<(), PlayError> {
        Ok(())
    }

    fn set_voice_rate(&mut self, _: VoiceId, _: f64) -> Result<(), PlayError> {
        Ok(())
    }
}

/// A stream whose reports are exact (no latency, no jitter) from `T0` on, and the track
/// following the last `play_at`.
fn reports(now_ns: u64, track: &Track) -> (Option<PositionSnapshot>, Option<MusicSnapshot>) {
    if now_ns < T0 {
        return (None, None);
    }
    let output = PositionSnapshot {
        frame: (now_ns - T0) * u64::from(RATE) / 1_000_000_000,
        timestamp_ns: T0
            + (now_ns - T0) / (1_000_000_000 / u64::from(RATE)) * (1_000_000_000 / u64::from(RATE)),
        latency_ns: 0,
        sample_rate: RATE,
    };
    let music = track.commands.iter().rev().find_map(|c| match *c {
        Command::PlayAt(device_frame) if output.frame >= device_frame => {
            let anchor = MusicAnchor {
                device_frame,
                track_frame: 0.0,
                speed: 1.0,
            };
            Some(MusicSnapshot {
                anchor,
                previous: anchor,
                generation: track.generation,
                ended: false,
            })
        }
        _ => None,
    });
    (Some(output), music)
}

fn session(map: &Beatmap, track: &Track) -> PlaySession {
    session_with(map, track, Offsets::default())
}

fn session_with(map: &Beatmap, track: &Track, offsets: Offsets) -> PlaySession {
    let (sounds, _) = hitsounds(map, &["normal-hitnormal.wav", "soft-hitnormal.wav"]);
    let mods = ModSet::default();
    PlaySession::new(
        map,
        &mods,
        track,
        LiveInput::new(Bindings::default(), layout()),
        GameplaySounds::new(map, sounds, SoundSettings::for_mods(&mods)),
        offsets,
    )
}

/// The scripted play's events in engine time, for a clock started at `start_ns`.
fn events(map: &Beatmap, start_time: f64, start_ns: u64) -> Vec<InputEvent> {
    play(map, 3)
        .events
        .into_iter()
        .map(|mut e| {
            let game_ms = e.time_ns as f64 / MS as f64;
            e.time_ns = start_ns + ((game_ms - start_time) * MS as f64) as u64;
            e
        })
        .collect()
}

/// Runs a play ticking every `step_ns` until `end_ns`, recording the snapshot of every
/// tick with `record`.
fn run(
    map: &Beatmap,
    step_ns: u64,
    end_ns: u64,
    record: impl FnMut(&PlaySnapshot),
) -> (PlaySession, Track, Sink) {
    let track = Track::default();
    let session = session(map, &track);
    run_with(map, session, track, step_ns, end_ns, record)
}

fn run_with(
    map: &Beatmap,
    mut session: PlaySession,
    mut track: Track,
    step_ns: u64,
    end_ns: u64,
    mut record: impl FnMut(&PlaySnapshot),
) -> (PlaySession, Track, Sink) {
    let mut sink = Sink::default();
    // The clock starts on the first tick with a report, at `T0`.
    let events = events(map, session.start_time(), T0);
    let mut snapshot = PlaySnapshot::default();
    let mut next = 0;
    let mut now = T0 - 10 * MS;
    while now <= end_ns {
        let start = next;
        while next < events.len() && events[next].time_ns <= now {
            next += 1;
        }
        let (output, music) = reports(now, &track);
        session.tick(
            now,
            &events[start..next],
            output,
            music,
            &mut track,
            &mut sink,
            &mut snapshot,
        );
        record(&snapshot);
        now += step_ns;
    }
    (session, track, sink)
}

/// Game time of a clock started at `T0` from `start_time`, exactly.
struct Started {
    start_time: f64,
}

impl GameTime for Started {
    fn game_time(&self, time_ns: u64) -> f64 {
        self.start_time + (time_ns as f64 - T0 as f64) / MS as f64
    }
}

fn end_ns(map: &Beatmap, start_time: f64) -> u64 {
    let end = map.hit_objects.last().unwrap().end_time();
    T0 + ((end + 1000.0 - start_time) * MS as f64) as u64
}

#[test]
fn the_clock_starts_on_the_first_report_before_the_first_object() {
    let map = beatmap();
    let track = Track::default();
    let start_time = session(&map, &track).start_time();
    // AR 9: 600 ms preempt, first object at 1000 ms, one second of lead-in.
    assert_eq!(start_time, -600.0);

    let mut states = Vec::new();
    let (_, track, _) = run(&map, MS, T0 + 1000 * MS, |s| {
        states.push((s.state, s.time));
    });
    // Waiting before the first report and on the tick that starts the clock.
    assert!(
        states[..11]
            .iter()
            .all(|&(state, time)| state == PlayState::Waiting && time == start_time)
    );
    assert_eq!(states[11].0, PlayState::Playing);
    // The track starts at frame 0 when time reaches 0: 600 ms after the start.
    assert_eq!(
        track.commands,
        [Command::Pause, Command::Seek(0), Command::PlayAt(600 * 48)]
    );
    // Then time runs with the output stream.
    let (_, time) = states[11 + 500];
    assert!((time - (start_time + 501.0)).abs() < 0.1, "{time}");
    assert!(states.windows(2).all(|w| w[0].1 <= w[1].1));
}

#[test]
fn ticks_are_deterministic_and_the_play_finishes() {
    let map = beatmap();
    let track = Track::default();
    let end = end_ns(&map, session(&map, &track).start_time());
    let snapshots = || {
        let mut out = Vec::new();
        let (session, _, sink) = run(&map, MS, end, |s| {
            out.push((s.state, s.time, s.cursor, s.circles.as_slice().to_vec()));
        });
        (out, session.log().events().to_vec(), sink.plays)
    };
    let (a, log, plays) = snapshots();
    assert_eq!(a.last().unwrap().0, PlayState::Finished);
    assert!(a.iter().any(|s| !s.3.is_empty()), "no circle was alive");
    assert!(!log.is_empty());
    assert!(plays > 0, "no hitsound was played");
    assert_eq!(snapshots(), (a, log, plays));
}

#[test]
fn ticks_and_frames_do_not_allocate() {
    let map = beatmap();
    let mut track = Track::default();
    // The fake's own log must not allocate inside the check.
    track.commands.reserve(16);
    let mut sink = Sink::default();
    let mut session = session(&map, &track);
    let events = events(&map, session.start_time(), T0);
    let end = end_ns(&map, session.start_time());
    let mut snapshot = PlaySnapshot::default();
    let view = view(&map);
    let textures = FrameTextures::default();
    let mut batch = SpriteBatch::with_capacity(4096);
    let mut drawn = 0;

    assert_no_alloc(|| {
        let mut next = 0;
        let mut now = T0 - 10 * MS;
        while now <= end {
            let start = next;
            while next < events.len() && events[next].time_ns <= now {
                next += 1;
            }
            let (output, music) = reports(now, &track);
            session.tick(
                now,
                &events[start..next],
                output,
                music,
                &mut track,
                &mut sink,
                &mut snapshot,
            );
            batch.clear();
            view.push(&mut batch, &layout(), &snapshot, now + MS / 2, &textures);
            drawn += snapshot.circles.as_slice().len();
            now += 4 * MS;
        }
    });
    assert_eq!(snapshot.state, PlayState::Finished);
    assert!(drawn > 0);
}

fn view(map: &Beatmap) -> PlayView {
    let dir = common::skin::SampleDir::new(&[]);
    let chain = slam_skin::SkinChain::new(None, slam_skin::Skin::load(&dir.0).unwrap());
    let mods = CircleMods::new(&ModSet::default());
    let windows = slam_osu::hit_windows::HitWindows::new(map.difficulty.overall_difficulty);
    PlayView {
        circles: HitCircles {
            visuals: CircleVisuals::new(map, mods, CircleSettings::default(), None, &chain),
            style: CircleStyle::new(mods, &windows, &chain),
            config: CircleSkinConfig::new(&chain),
        },
    }
}

#[test]
fn a_frame_is_placed_between_ticks_by_the_clock() {
    let map = beatmap();
    let mut last = None;
    run(&map, 10 * MS, T0 + 500 * MS, |s| last = Some(s.clone()));
    let s = last.unwrap();
    assert_eq!(s.state, PlayState::Playing);
    // 4 ms after the tick, time is 4 ms later; never before the tick.
    let tick_ns = T0 + 500 * MS;
    assert!((s.time_at(tick_ns + 4 * MS) - (s.time + 4.0)).abs() < 0.01);
    assert_eq!(s.time_at(tick_ns - 4 * MS), s.time);
}

#[test]
fn the_play_judges_like_its_input_on_the_clock() {
    let map = beatmap();
    let track = Track::default();
    let start_time = session(&map, &track).start_time();
    let end = end_ns(&map, start_time);
    let (session, _, _) = run(&map, MS, end, |_| {});

    // The same input stepped on the same ticks, on the time the clock should give.
    let events = events(&map, start_time, T0);
    let mut live = LiveInput::new(Bindings::default(), layout());
    let mut rules = OsuRules::new(&map, &ModSet::default());
    let mut log = JudgementLog::with_capacity(rules.max_judgements());
    let mut next = 0;
    // The tick at `T0` starts the clock; the play steps from the next one.
    let mut now = T0 + MS;
    while now <= end {
        let start = next;
        while next < events.len() && events[next].time_ns <= now {
            next += 1;
        }
        let step = live.step(&events[start..next], now, &Started { start_time });
        rules.update(&step, &mut log);
        now += MS;
    }
    assert!(rules.is_complete());
    assert!(log.events().len() > 5);
    // The simulated reports truncate the nanoseconds per frame: the clock runs 16 ppm off
    // the exact time.
    let (got, expected) = (session.log().events(), log.events());
    assert_eq!(got.len(), expected.len());
    for (g, e) in got.iter().zip(expected) {
        assert_eq!((g.object, g.result), (e.object, e.result));
        assert!((g.time - e.time).abs() < 0.05, "{g:?} vs {e:?}");
    }
}

#[test]
fn a_refused_start_is_tried_again_on_the_next_tick() {
    let map = beatmap();
    let track = Track {
        refuse_play_at: true,
        ..Track::default()
    };
    let session = session(&map, &track);
    let mut states = Vec::new();
    let (session, track, _) = run_with(&map, session, track, MS, T0 + 100 * MS, |s| {
        states.push((s.state, s.time));
    });
    assert_eq!(session.clock_errors(), 1);
    // Waiting through the refused tick at `T0` and the retry 1 ms later.
    assert!(
        states[..12]
            .iter()
            .all(|&(state, _)| state == PlayState::Waiting)
    );
    assert_eq!(states[12].0, PlayState::Playing);
    assert!(
        (states[12].1 - (-600.0 + 1.0)).abs() < 0.1,
        "{}",
        states[12].1
    );
    assert_eq!(
        track.commands,
        [
            Command::Pause,
            Command::Seek(0),
            Command::Pause,
            Command::Seek(0),
            Command::PlayAt(601 * 48),
        ]
    );
}

#[test]
fn the_play_starts_at_its_start_time_with_offsets() {
    let map = beatmap();
    for offsets in [
        Offsets {
            global_ms: 25.0,
            beatmap_ms: 0.0,
        },
        Offsets {
            global_ms: 0.0,
            beatmap_ms: -40.0,
        },
    ] {
        let track = Track::default();
        let session = session_with(&map, &track, offsets);
        let mut states = Vec::new();
        let (_, track, _) = run_with(&map, session, track, MS, T0 + 100 * MS, |s| {
            states.push((s.state, s.time));
        });
        assert_eq!(states[11].0, PlayState::Playing);
        assert!(
            (states[11].1 - (-600.0 + 1.0)).abs() < 0.1,
            "{offsets:?}: {}",
            states[11].1
        );
        // Game time is the track time plus the offsets: the track starts when game time
        // reaches the offsets.
        let total = offsets.global_ms + offsets.beatmap_ms;
        let frame = ((600.0 + total) * 48.0).round() as u64;
        assert_eq!(
            track.commands.last(),
            Some(&Command::PlayAt(frame)),
            "{offsets:?}"
        );
    }
}
