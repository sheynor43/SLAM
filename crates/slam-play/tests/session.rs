//! A play session on a simulated output stream and track: the clock starts on the first
//! position report, ticks are deterministic, the play finishes, and nothing allocates.

mod common;

use std::cell::Cell;
use std::sync::Arc;

use common::skin::hitsounds;
use common::{beatmap, layout, play};
use slam_audio::{
    MusicAnchor, MusicSnapshot, PlayError, PositionSnapshot, Sample, TrackControl, VoiceId,
    VoiceParams,
};
use slam_input::{InputEvent, InputKind, MouseButton};
use slam_osu::judgement::{HitResult, JudgementLog};
use slam_osu::replay::{Actions, Rules};
use slam_osu::rules::OsuRules;
use slam_osu::scoring::ScoreProcessor;
use slam_osu::{Beatmap, ModSet, Vec2};
use slam_play::circles::{
    CircleMods, CircleSettings, CircleSkinConfig, CircleStyle, CircleVisuals, HitCircles,
};
use slam_play::slider_body::Snaking;
use slam_play::sliders::{HitSliders, SliderBodies, SliderVisuals};
use slam_play::{BackgroundSettings, BackgroundView, HudValues, HudView};
use slam_play::{
    Bindings, FrameTextures, GameTime, GameplaySounds, LiveInput, Offsets, PlaySession,
    PlaySnapshot, PlayState, PlayView, SoundSettings, VoiceSink,
};
use slam_render::Extent;
use slam_render::Limits;
use slam_render::Origin;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};

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
    /// The stream frame each command was given at.
    issued: Vec<u64>,
    /// The stream frame of the last report.
    frame: Cell<u64>,
    generation: u32,
    /// Refuse the next `play_at` once.
    refuse_play_at: bool,
    /// Refuse one `seek` once this many commands are logged.
    refuse_seek_after: Option<usize>,
    /// Stream frames the callback takes a seek after it is given (as if behind the queue).
    seek_delay: u64,
}

impl Track {
    /// Room for `n` commands, so that logging them does not allocate.
    fn reserve(&mut self, n: usize) {
        self.commands.reserve(n);
        self.issued.reserve(n);
    }

    fn log(&mut self, command: Command) {
        self.commands.push(command);
        self.issued.push(self.frame.get());
    }

    /// Where the callback has the track at stream frame `frame`: the commands take effect
    /// on the frame they were given at (no latency), a `play_at` on its frame. `None`
    /// before the first command.
    fn music(&self, frame: u64) -> Option<MusicSnapshot> {
        let mut anchors: Option<(MusicAnchor, MusicAnchor)> = None;
        let mut generation = 0;
        let mut scheduled = None;
        let push = |anchors: &mut Option<(MusicAnchor, MusicAnchor)>, anchor: MusicAnchor| {
            *anchors = Some(match *anchors {
                Some((_, last)) => (last, anchor),
                None => (anchor, anchor),
            });
        };
        let position = |anchors: &Option<(MusicAnchor, MusicAnchor)>, at: u64| {
            anchors.map_or((0.0, 0.0), |(_, a)| {
                (
                    a.track_frame + (at as f64 - a.device_frame as f64) * a.speed,
                    a.speed,
                )
            })
        };
        let fire = |anchors: &mut Option<(MusicAnchor, MusicAnchor)>,
                    scheduled: &mut Option<u64>,
                    upto: u64| {
            if let Some(at) = *scheduled
                && at <= upto
            {
                *scheduled = None;
                let (track_frame, _) = position(anchors, at);
                push(
                    anchors,
                    MusicAnchor {
                        device_frame: at,
                        track_frame,
                        speed: 1.0,
                    },
                );
            }
        };
        for (&command, &issued) in self.commands.iter().zip(&self.issued) {
            let at = match command {
                Command::Seek(_) => issued + self.seek_delay,
                _ => issued,
            };
            if at > frame {
                break;
            }
            fire(&mut anchors, &mut scheduled, at);
            let (track_frame, speed) = position(&anchors, at);
            match command {
                Command::PlayAt(device_frame) => scheduled = Some(device_frame),
                Command::Play | Command::Pause => {
                    scheduled = None;
                    let speed = if command == Command::Play { 1.0 } else { 0.0 };
                    push(
                        &mut anchors,
                        MusicAnchor {
                            device_frame: at,
                            track_frame,
                            speed,
                        },
                    );
                }
                Command::Seek(to) => {
                    generation += 1;
                    push(
                        &mut anchors,
                        MusicAnchor {
                            device_frame: at,
                            track_frame: to as f64,
                            speed,
                        },
                    );
                }
            }
        }
        fire(&mut anchors, &mut scheduled, frame);
        anchors.map(|(previous, anchor)| MusicSnapshot {
            anchor,
            previous,
            generation,
            ended: false,
        })
    }
}

impl TrackControl for Track {
    fn play(&mut self) -> Result<(), PlayError> {
        self.log(Command::Play);
        Ok(())
    }

    fn play_at(&mut self, device_frame: u64) -> Result<(), PlayError> {
        if std::mem::take(&mut self.refuse_play_at) {
            return Err(PlayError::QueueFull);
        }
        self.log(Command::PlayAt(device_frame));
        Ok(())
    }

    fn pause(&mut self) -> Result<(), PlayError> {
        self.log(Command::Pause);
        Ok(())
    }

    fn seek(&mut self, frame: u64) -> Result<(), PlayError> {
        if self
            .refuse_seek_after
            .is_some_and(|after| self.commands.len() >= after)
        {
            self.refuse_seek_after = None;
            return Err(PlayError::QueueFull);
        }
        self.log(Command::Seek(frame));
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

/// A stream whose reports are exact (no latency, no jitter) from `T0` on, and the track as
/// its commands left it.
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
    track.frame.set(output.frame);
    (Some(output), track.music(output.frame))
}

fn session(map: &Beatmap, track: &Track) -> PlaySession {
    session_with(map, track, Offsets::default())
}

/// A session with No Fail: the scripted player would run out of health.
fn session_with(map: &Beatmap, track: &Track, offsets: Offsets) -> PlaySession {
    let (sounds, _) = hitsounds(map, &["normal-hitnormal.wav", "soft-hitnormal.wav"]);
    let mods = mods(vec![slam_osu::mods::Mod::NoFail]);
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
    // First object at 1000 ms with a 600 ms preempt: gameplay starts two seconds before it.
    assert_eq!(start_time, -1000.0);

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
    // The track starts at frame 0 when time reaches 0: a second after the start.
    assert_eq!(
        track.commands,
        [Command::Pause, Command::Seek(0), Command::PlayAt(1000 * 48)]
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
    // A second after the last object: passed, the results wait.
    assert_eq!(a.last().unwrap().0, PlayState::Passed);
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
    track.reserve(16);
    let mut sink = Sink::default();
    let mut session = session(&map, &track);
    let events = events(&map, session.start_time(), T0);
    // On to the results.
    let end = end_ns(&map, session.start_time()) + 1_100 * MS;
    let mut snapshot = PlaySnapshot::default();
    let view = view(&map);
    let textures = FrameTextures::default();
    let mut batch = SpriteBatch::with_capacity(4096);
    let mut bodies = SliderBodies::new();
    let mut drawn = 0;
    let mut bodies_drawn = 0;
    // The HUD's glyphs are only requested here: without a device they stay pending.
    let registry = Arc::new(FontRegistry::with_builtin());
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
    let mut hud = HudView::new(registry, "Пропустить");
    hud.request_glyphs(&mut glyphs);
    let mut text = SpriteBatch::with_capacity(256);
    let mut pending = 0;
    let white = WHITE;
    let mut background = BackgroundView::new(BackgroundSettings::default());

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
            text.clear();
            background.push(&mut batch, 640.0, 480.0, &snapshot, now, Some(white));
            view.push(
                &mut batch,
                &mut bodies,
                &layout(),
                &snapshot,
                now + MS / 2,
                &textures,
            );
            bodies_drawn += bodies.as_slice().len();
            hud.update(1280.0, 720.0, &snapshot.hud);
            hud.push_shapes(&mut batch, &layout(), &snapshot, white);
            pending += hud.push_text(&mut text, &snapshot, &mut glyphs).pending;
            view.push_cursor(&mut batch, &layout(), &snapshot, &textures);
            drawn += snapshot.circles.as_slice().len();
            now += 4 * MS;
        }
    });
    assert_eq!(snapshot.state, PlayState::Results);
    assert!(drawn > 0);
    assert!(bodies_drawn > 0);
    assert!(pending > 0);
    assert!(snapshot.hud.score > 0);
    assert_eq!(hud.texts()[0], format!("{:06}", snapshot.hud.score));
}

const LIMITS: Limits = Limits {
    max_texture_size: 4096,
    max_uniform_block_size: 16384,
    max_uniform_buffer_slots: 8,
    max_texture_slots: 8,
};

const WHITE: AtlasRegion = AtlasRegion {
    texture: TextureId(0),
    origin: Origin { x: 0, y: 0 },
    size: Extent {
        width: 4,
        height: 4,
    },
    uv: UvRect::FULL,
};

/// A score processor of the sessions' No Fail play that took `log`'s judgements, as
/// lazer's HUD binds it.
fn scored(map: &Beatmap, log: &[slam_osu::judgement::Judgement]) -> HudValues {
    let mut score = ScoreProcessor::for_play(map, &mods(vec![slam_osu::mods::Mod::NoFail]));
    for judgement in log {
        score.apply(judgement, false);
    }
    HudValues {
        score: score.total_score(),
        accuracy: score.accuracy(),
        combo: score.statistics().combo,
    }
}

#[test]
fn the_hud_shows_the_score_of_the_play() {
    let map = beatmap();
    let track = Track::default();
    let end = end_ns(&map, session(&map, &track).start_time());
    let mut first_judged = None;
    let mut seen = 0;
    let (session, _, _) = run(&map, MS, end, |s| {
        seen += 1;
        if first_judged.is_none() && s.hud != HudValues::default() {
            first_judged = Some(s.hud);
        }
    });
    assert!(seen > 0);
    let events = session.log().events();
    assert!(!events.is_empty());
    let first = first_judged.expect("the play scores");
    assert_eq!(first, scored(&map, &events[..1]));
    let last = HudValues {
        score: session.score().total_score(),
        accuracy: session.score().accuracy(),
        combo: session.score().statistics().combo,
    };
    assert_eq!(last, scored(&map, events));
    assert!(last.score > 0);
    assert_eq!(last.combo, session.log().statistics().combo);
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
        sliders: HitSliders {
            visuals: SliderVisuals::new(map, mods, CircleSettings::default(), None, &chain),
            snaking: Snaking::default(),
            arrows: slam_play::sliders::ArrowStyle::new(&chain),
            ball: slam_play::sliders::BallStyle::new(&chain),
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
        (states[12].1 - (-1000.0 + 1.0)).abs() < 0.1,
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
            Command::PlayAt(1001 * 48),
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
            (states[11].1 - (-1000.0 + 1.0)).abs() < 0.1,
            "{offsets:?}: {}",
            states[11].1
        );
        // Game time is the track time plus the offsets: the track starts when game time
        // reaches the offsets.
        let total = offsets.global_ms + offsets.beatmap_ms;
        let frame = ((1000.0 + total) * 48.0).round() as u64;
        assert_eq!(
            track.commands.last(),
            Some(&Command::PlayAt(frame)),
            "{offsets:?}"
        );
    }
}

/// The test beatmap with its objects `delay` ms later, decoded anew so that nested objects
/// and spinner ends move too.
fn delayed(delay: f64) -> Beatmap {
    let (head, objects) = common::MAP.split_once("[HitObjects]\n").unwrap();
    let mut text = format!("{head}[HitObjects]\n");
    for line in objects.lines() {
        let mut fields: Vec<String> = line.split(',').map(str::to_owned).collect();
        let shift = |field: &str| (field.parse::<f64>().unwrap() + delay).to_string();
        fields[2] = shift(&fields[2]);
        // A spinner's end time.
        if fields[3].parse::<u32>().unwrap() & 8 != 0 {
            fields[5] = shift(&fields[5]);
        }
        text.push_str(&fields.join(","));
        text.push('\n');
    }
    Beatmap::decode(text.as_bytes()).unwrap()
}

fn press(time_ns: u64, kind: InputKind) -> InputEvent {
    InputEvent { time_ns, kind }
}

fn space(time_ns: u64) -> InputEvent {
    press(
        time_ns,
        InputKind::Key {
            scancode: 44,
            down: true,
        },
    )
}

/// Ticks a play of `map` every millisecond until `end_ns` with `presses` and no other
/// input, recording the time and skip availability of every playing tick.
fn run_presses(map: &Beatmap, presses: &[InputEvent], end_ns: u64) -> (Vec<(f64, bool)>, Track) {
    let track = Track::default();
    let session = session(map, &track);
    let (ticks, track, _) = run_presses_with(session, track, presses, end_ns);
    (ticks, track)
}

fn run_presses_with(
    mut session: PlaySession,
    mut track: Track,
    presses: &[InputEvent],
    end_ns: u64,
) -> (Vec<(f64, bool)>, Track, PlaySession) {
    let mut sink = Sink::default();
    let mut snapshot = PlaySnapshot::default();
    let mut ticks = Vec::new();
    let mut next = 0;
    let mut now = T0 - 10 * MS;
    while now <= end_ns {
        let start = next;
        while next < presses.len() && presses[next].time_ns <= now {
            next += 1;
        }
        let (output, music) = reports(now, &track);
        session.tick(
            now,
            &presses[start..next],
            output,
            music,
            &mut track,
            &mut sink,
            &mut snapshot,
        );
        if snapshot.state != PlayState::Waiting {
            ticks.push((snapshot.time, snapshot.skip_available));
        }
        now += MS;
    }
    (ticks, track, session)
}

#[test]
fn a_long_lead_in_is_skipped_to_a_second_before_gameplay_starts() {
    let mut map = beatmap();
    map.audio_lead_in = 10000.0;
    let track = Track::default();
    // The first object at 1000 ms: gameplay starts at −1000, the skip goes to −2000.
    assert_eq!(session(&map, &track).start_time(), -9000.0);

    let (ticks, track) = run_presses(&map, &[space(T0 + 100 * MS)], T0 + 200 * MS);
    // Playing ticks start at `T0` + 1 ms; the press is on the 100th.
    assert!(ticks[..99].iter().all(|&(_, available)| available));
    // The tick with the press still runs on the old clock; the next runs from the target.
    let (time, available) = ticks[100];
    assert!((time - (-2000.0 + 1.0)).abs() < 0.1, "{time}");
    assert!(!available);
    // Track frame 0 plays when time reaches 0: 2 s after the skip.
    assert_eq!(
        track.commands,
        [
            Command::Pause,
            Command::Seek(0),
            Command::PlayAt(9000 * 48),
            Command::Pause,
            Command::Seek(0),
            Command::PlayAt(2100 * 48),
        ]
    );
}

#[test]
fn a_late_first_object_is_skipped_on_the_track() {
    let map = delayed(29000.0);
    let track = Track::default();
    assert_eq!(session(&map, &track).start_time(), 0.0);

    let (ticks, track) = run_presses(&map, &[space(T0 + 100 * MS)], T0 + 200 * MS);
    // Playing ticks start at `T0` + 1 ms; the press is on the 100th.
    assert!(ticks[..99].iter().all(|&(_, available)| available));
    // Gameplay starts at 28 s: the track seeks to 27 s.
    assert_eq!(track.commands.last(), Some(&Command::Seek(27000 * 48)));
}

#[test]
fn the_second_extra_mouse_button_skips_too() {
    let map = delayed(29000.0);
    let x2 = press(
        T0 + 100 * MS,
        InputKind::MouseButton {
            button: MouseButton::X2,
            down: true,
        },
    );
    let (_, track) = run_presses(&map, &[x2], T0 + 200 * MS);
    assert_eq!(track.commands.last(), Some(&Command::Seek(27000 * 48)));
}

#[test]
fn nothing_is_skipped_without_time_to_skip() {
    // Gameplay starts at −1000, where the play starts: no skip.
    let map = beatmap();
    let (ticks, track) = run_presses(&map, &[space(T0 + 100 * MS)], T0 + 200 * MS);
    assert!(ticks.iter().all(|&(_, available)| !available));
    assert_eq!(
        track.commands,
        [Command::Pause, Command::Seek(0), Command::PlayAt(1000 * 48)]
    );

    // Released keys and other keys do not skip.
    let map = delayed(29000.0);
    let presses = [
        press(
            T0 + 100 * MS,
            InputKind::Key {
                scancode: 44,
                down: false,
            },
        ),
        press(
            T0 + 110 * MS,
            InputKind::Key {
                scancode: 29,
                down: true,
            },
        ),
    ];
    let (_, track) = run_presses(&map, &presses, T0 + 200 * MS);
    assert!(!track.commands.contains(&Command::Seek(27000 * 48)));
}

#[test]
fn a_skip_does_not_allocate() {
    let map = delayed(29000.0);
    let mut track = Track::default();
    track.reserve(16);
    let mut sink = Sink::default();
    let mut session = session(&map, &track);
    let mut snapshot = PlaySnapshot::default();
    let presses = [space(T0 + 100 * MS)];

    assert_no_alloc(|| {
        let mut now = T0 - 10 * MS;
        while now <= T0 + 200 * MS {
            let events: &[InputEvent] = if now == T0 + 100 * MS { &presses } else { &[] };
            let (output, music) = reports(now, &track);
            session.tick(
                now,
                events,
                output,
                music,
                &mut track,
                &mut sink,
                &mut snapshot,
            );
            now += MS;
        }
    });
    assert_eq!(track.commands.last(), Some(&Command::Seek(27000 * 48)));
}

/// The lead-in map of `a_long_lead_in_is_skipped_to_a_second_before_gameplay_starts`: the
/// play starts at −9000, the skip period ends at −2000.
fn long_lead_in() -> Beatmap {
    let mut map = beatmap();
    map.audio_lead_in = 10000.0;
    map
}

#[test]
fn the_skip_period_ends_a_second_before_gameplay_starts() {
    let map = long_lead_in();
    // Tick time −2001: skipped to −2000, the track plays 2 s later.
    let (_, track) = run_presses(&map, &[space(T0 + 6999 * MS)], T0 + 7100 * MS);
    assert_eq!(track.commands.last(), Some(&Command::PlayAt(8999 * 48)));
    // Tick time −2000: no skip.
    let (ticks, track) = run_presses(&map, &[space(T0 + 7000 * MS)], T0 + 7100 * MS);
    assert_eq!(track.commands.len(), 3);
    // Ticks start at `T0` + 1 ms, at −8999.
    assert!(ticks[6998].1);
    assert!(!ticks[6999].1 && !ticks[7000].1);
}

#[test]
fn a_press_before_the_clock_starts_does_not_skip() {
    let map = long_lead_in();
    let (ticks, track) = run_presses(&map, &[space(T0 - 5 * MS)], T0 + 100 * MS);
    assert_eq!(track.commands.len(), 3);
    assert!(ticks.iter().all(|&(_, available)| available));
}

#[test]
fn a_skip_lands_on_its_target_with_offsets() {
    let map = delayed(29000.0);
    let offsets = Offsets {
        global_ms: 25.0,
        beatmap_ms: -40.0,
    };
    let track = Track::default();
    let session = session_with(&map, &track, offsets);
    let (ticks, track, _) =
        run_presses_with(session, track, &[space(T0 + 100 * MS)], T0 + 200 * MS);
    // Game time 27000 is track time 27015.
    assert_eq!(track.commands.last(), Some(&Command::Seek(27015 * 48)));
    assert!(!ticks[99].1);
}

#[test]
fn a_refused_skip_is_tried_again_on_the_next_tick() {
    let map = long_lead_in();
    // Start: pause, seek, play_at; the skip's seek back to track frame 0 is refused.
    let track = Track {
        refuse_seek_after: Some(3),
        ..Track::default()
    };
    let session = session(&map, &track);
    let (ticks, track, session) =
        run_presses_with(session, track, &[space(T0 + 100 * MS)], T0 + 200 * MS);
    assert_eq!(session.clock_errors(), 1);
    assert!(!ticks[99].1 && !ticks[100].1);
    // Retried 1 ms later: the clock runs again from −2000.
    assert_eq!(track.commands.last(), Some(&Command::PlayAt(2101 * 48)));
    let (time, _) = ticks[150];
    assert!((time - (-2000.0 + 50.0)).abs() < 0.1, "{time}");
}

// ---- Pausing, failing, restarting and quitting ----

const ESCAPE: u16 = 41;
const GRAVE: u16 = 53;
const LCTRL: u16 = 224;
const Z: u16 = 29;

/// A play ticked every millisecond by hand, with the events of each tick given.
struct Driver {
    session: PlaySession,
    track: Track,
    sink: Sink,
    snapshot: PlaySnapshot,
    now: u64,
}

impl Driver {
    /// A play of `map` with `mods`, ticked until the clock started.
    fn new(map: &Beatmap, mods: ModSet) -> Driver {
        let mut track = Track::default();
        track.reserve(256);
        let (sounds, _) = hitsounds(map, &["normal-hitnormal.wav", "soft-hitnormal.wav"]);
        let session = PlaySession::new(
            map,
            &mods,
            &track,
            LiveInput::new(Bindings::default(), layout()),
            GameplaySounds::new(map, sounds, SoundSettings::for_mods(&mods)),
            Offsets::default(),
        );
        let mut driver = Driver {
            session,
            track,
            sink: Sink::default(),
            snapshot: PlaySnapshot::default(),
            now: T0,
        };
        driver.tick(&[]);
        driver.tick(&[]);
        assert_eq!(driver.state(), PlayState::Playing);
        driver
    }

    fn tick(&mut self, events: &[InputEvent]) {
        let (output, music) = reports(self.now, &self.track);
        self.session.tick(
            self.now,
            events,
            output,
            music,
            &mut self.track,
            &mut self.sink,
            &mut self.snapshot,
        );
        self.now += MS;
    }

    /// Ticks for `ms` milliseconds without input.
    fn run(&mut self, ms: u64) {
        for _ in 0..ms {
            self.tick(&[]);
        }
    }

    /// Ticks without input until the game time reaches `time`.
    fn run_to(&mut self, time: f64) {
        while self.snapshot.time < time {
            self.tick(&[]);
        }
    }

    /// Ticks without input until `done` holds, at most `ms` milliseconds.
    fn run_until(&mut self, ms: u64, done: impl Fn(&PlaySnapshot) -> bool) {
        for _ in 0..ms {
            if done(&self.snapshot) {
                return;
            }
            self.tick(&[]);
        }
        assert!(done(&self.snapshot), "not reached in {ms} ms");
    }

    fn state(&self) -> PlayState {
        self.snapshot.state
    }

    fn key(&self, scancode: u16, down: bool) -> InputEvent {
        press(self.now, InputKind::Key { scancode, down })
    }

    /// Moves the cursor to osu!pixel `at`.
    fn mouse(&self, at: Vec2) -> InputEvent {
        let window = layout().to_window(at);
        press(
            self.now,
            InputKind::MouseMove {
                x: window.x,
                y: window.y,
            },
        )
    }

    /// Presses and releases `scancode` on one tick.
    fn tap(&mut self, scancode: u16) {
        let events = [self.key(scancode, true), self.key(scancode, false)];
        self.tick(&events);
    }

    /// Holds `scancode` (with `modifier` if any) for `ms` milliseconds, then releases.
    fn hold(&mut self, modifier: Option<u16>, scancode: u16, ms: u64) {
        let mut down = Vec::new();
        down.extend(modifier.map(|m| self.key(m, true)));
        down.push(self.key(scancode, true));
        self.tick(&down);
        self.run(ms);
        let mut up = vec![self.key(scancode, false)];
        up.extend(modifier.map(|m| self.key(m, false)));
        self.tick(&up);
    }
}

fn mods(list: Vec<slam_osu::mods::Mod>) -> ModSet {
    ModSet::new(list).unwrap()
}

#[test]
fn a_pause_stops_the_clock_and_a_click_on_the_cursor_resumes_it_where_it_was() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    let at = Vec2::new(300.0, 300.0);
    let moved = d.mouse(at);
    d.tick(&[moved]);
    d.run_to(1500.0);

    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Paused);
    assert_eq!(d.track.commands.last(), Some(&Command::Pause));
    let paused = d.snapshot.time;
    d.run(500);
    assert_eq!(d.state(), PlayState::Paused);
    assert_eq!(d.snapshot.time, paused);
    assert!((d.snapshot.time_at(d.now) - paused).abs() < 1.0);

    // Continue: the cursor where gameplay paused has to be clicked.
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Resuming);
    assert_eq!(d.snapshot.resume_target, at);
    // Clicking 14 osu!pixels off misses the cursor (its right edge is outside).
    let off = d.mouse(Vec2::new(at.x + 14.0, at.y));
    d.tick(&[off]);
    d.tap(Z);
    assert_eq!(d.state(), PlayState::Resuming);
    // Smoke does not resume.
    let on = d.mouse(Vec2::new(at.x - 14.0, at.y + 13.0));
    d.tick(&[on]);
    d.tap(6);
    assert_eq!(d.state(), PlayState::Resuming);
    d.run(300);
    assert_eq!(d.snapshot.time, paused);

    let log = d.session.log().events().len();
    let click = d.key(Z, true);
    d.tick(&[click]);
    assert_eq!(d.state(), PlayState::Playing);
    assert_eq!(d.track.commands.last(), Some(&Command::Play));
    // The press that resumed holds the left button without pressing it.
    d.run(1);
    assert!(d.session.input().held().contains(Actions::LEFT));
    assert_eq!(d.session.log().events().len(), log);
    d.run(99);
    let time = d.snapshot.time;
    assert!(
        (time - (paused + 100.0)).abs() < 2.0,
        "{time} after {paused}"
    );
}

#[test]
fn releases_while_paused_reach_gameplay_on_resume() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_to(1500.0);
    let down = d.key(Z, true);
    d.tick(&[down]);
    d.run(5);
    assert!(d.session.input().held().contains(Actions::LEFT));
    d.tap(ESCAPE);
    let up = d.key(Z, false);
    d.tick(&[up]);
    // A press while paused is not delivered.
    let x = d.key(27, true);
    d.tick(&[x]);
    // The cursor never moved: it is hidden, and gameplay resumes at once.
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Playing);
    d.run(2);
    assert_eq!(d.session.input().held(), Actions::NONE);
}

#[test]
fn a_second_pause_waits_for_the_cooldown() {
    let map = beatmap();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    d.run_to(1500.0);
    d.tap(ESCAPE);
    let paused = d.snapshot.time;
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Playing);
    d.run_to(paused + 990.0);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Playing);
    d.run_to(paused + 1000.0);
    // The middle mouse button pauses too.
    let middle = press(
        d.now,
        InputKind::MouseButton {
            button: MouseButton::Middle,
            down: true,
        },
    );
    d.tick(&[middle]);
    assert_eq!(d.state(), PlayState::Paused);
}

#[test]
fn the_intro_is_a_break_without_cooldown_or_resume_cursor() {
    let map = delayed(5000.0);
    let mut d = Driver::new(&map, ModSet::default());
    let moved = d.mouse(Vec2::new(100.0, 100.0));
    d.tick(&[moved]);
    d.run_to(500.0);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Paused);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Playing);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Paused);
}

#[test]
fn escape_while_waiting_for_the_click_pauses_again() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    let moved = d.mouse(Vec2::new(200.0, 200.0));
    d.tick(&[moved]);
    d.run_to(1500.0);
    d.tap(ESCAPE);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Resuming);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Paused);
}

#[test]
fn running_out_of_health_fails_and_stops_after_the_fail_animation() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_until(10_000, |s| s.state == PlayState::Failing);
    assert_eq!(d.snapshot.health, 0.0);
    assert!(d.session.health().has_failed());
    let failed_at = d.session.counted_judgements();
    // Gameplay runs on for 2.5 s, without counting judgements or pausing.
    d.tap(ESCAPE);
    d.run(2_490);
    assert_eq!(d.state(), PlayState::Failed);
    let mut d = Driver::new(&map, ModSet::default());
    d.run_until(10_000, |s| s.state == PlayState::Failing);
    let time = d.snapshot.time;
    let hud = d.snapshot.hud;
    // The judgement that fails scores; the score is failed.
    let score = d.session.score();
    assert!(score.has_failed());
    let scored: u32 = HitResult::ALL
        .iter()
        .map(|&r| score.statistics().count(r))
        .sum();
    assert_eq!(scored as usize, failed_at);
    d.run(2_490);
    assert_eq!(d.state(), PlayState::Failing);
    // Judgements after the fail do not score: the misses leave the accuracy.
    assert_eq!(d.snapshot.hud, hud);
    assert!(d.session.log().events().len() > failed_at);
    assert!(d.snapshot.time > time + 2_400.0);
    assert_eq!(d.session.counted_judgements(), failed_at);
    d.run(20);
    assert_eq!(d.state(), PlayState::Failed);
    assert_eq!(d.track.commands.last(), Some(&Command::Pause));
    let stopped = d.snapshot.time;
    d.run(100);
    assert_eq!(d.snapshot.time, stopped);
    // Back now quits.
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Exited);
    assert!(d.session.has_exited());
}

#[test]
fn no_fail_plays_to_the_end() {
    let map = beatmap();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    d.run_until(12_000, |s| s.state == PlayState::Passed);
    assert!(!d.session.health().has_failed());
    assert_eq!(d.snapshot.health, 0.0);
}

// ---- Passing and the results ----

/// A play with No Fail ticked until it passed: every object missed.
fn passed(map: &Beatmap) -> Driver {
    let mut d = Driver::new(map, mods(vec![slam_osu::mods::Mod::NoFail]));
    d.run_until(12_000, |s| s.state == PlayState::Passed);
    d
}

#[test]
fn passing_shows_the_results_a_second_later() {
    let map = beatmap();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    let mut last = None;
    for _ in 0..12_000 {
        d.tick(&[]);
        if d.state() == PlayState::Passed {
            break;
        }
        assert!(d.session.results().is_none());
        last = d.session.log().events().last().map(|j| j.time);
    }
    assert_eq!(d.state(), PlayState::Passed);
    // Every judgement is in, the last one on the previous step: passed on the next step.
    let last = last.unwrap();
    assert_eq!(
        d.session.counted_judgements(),
        d.session.log().events().len()
    );
    assert!(
        d.snapshot.time > last && d.snapshot.time - last < 1.5,
        "{last}"
    );
    let results = d.session.results().unwrap();
    assert_eq!(results.statistics, *d.session.log().statistics());
    assert_eq!(results.score, d.snapshot.hud.score);
    assert_eq!(results.rank, slam_osu::scoring::ScoreRank::D);

    let time = d.snapshot.time;
    let commands = d.track.commands.len();
    assert_no_alloc(|| {
        // Gameplay runs on for a second of real time.
        d.run(999);
        assert_eq!(d.state(), PlayState::Passed);
        assert!(d.snapshot.time > time + 990.0);
        assert!(d.snapshot.clock.is_some());
        d.run(1);
        assert_eq!(d.state(), PlayState::Results);
        d.run(100);
    });
    assert_eq!(d.state(), PlayState::Results);
    assert_eq!(d.session.results(), Some(results));
    // Gameplay stopped, the track plays on.
    assert!(d.snapshot.clock.is_none());
    assert!(d.snapshot.time < time + 1_001.0);
    assert_eq!(d.track.commands.len(), commands);
    // Back leaves the results, which stay.
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Exited);
    assert!(d.session.has_exited());
    assert_eq!(d.session.results(), Some(results));
}

#[test]
fn pausing_after_passing_shows_the_results_at_once() {
    let map = beatmap();
    let mut d = passed(&map);
    d.run(100);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Results);
    assert!(d.session.results().is_some());
    assert_ne!(d.track.commands.last(), Some(&Command::Pause));
    // Quick exit leaves the results.
    d.hold(Some(LCTRL), GRAVE, 200);
    assert_eq!(d.state(), PlayState::Exited);
}

#[test]
fn quick_exit_after_passing_shows_the_results() {
    let map = beatmap();
    let mut d = passed(&map);
    d.hold(Some(LCTRL), GRAVE, 200);
    assert_eq!(d.state(), PlayState::Results);
    d.run(2_000);
    assert_eq!(d.state(), PlayState::Results);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Exited);
}

#[test]
fn quick_retry_restarts_after_passing_and_from_the_results() {
    let map = beatmap();
    let mut d = passed(&map);
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    assert_eq!(d.state(), PlayState::Playing);
    assert!(d.session.results().is_none());

    d.run_until(12_000, |s| s.state == PlayState::Results);
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 2);
    assert_eq!(d.state(), PlayState::Playing);
    assert!(d.session.log().events().is_empty());
    // The next play passes again.
    d.run_until(12_000, |s| s.state == PlayState::Passed);
}

#[test]
fn back_in_the_tick_that_passes_shows_the_results() {
    let map = beatmap();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    // Find the tick that passes, then play it again with Back after its step.
    let mut probe = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    probe.run_until(12_000, |s| s.state == PlayState::Passed);
    let ticks = (probe.now - d.now) / MS - 1;
    d.run(ticks);
    assert_eq!(d.state(), PlayState::Playing);
    // The step before Back passes; Back then shows the results instead of pausing.
    let back = d.key(ESCAPE, true);
    d.tick(&[back]);
    assert_eq!(d.state(), PlayState::Results);
    assert!(d.session.results().is_some());
}

#[test]
fn sudden_death_with_restart_restarts_on_failing_on_the_last_judgement() {
    let mut map = beatmap();
    map.hit_objects.truncate(1);
    let mut d = Driver::new(
        &map,
        mods(vec![slam_osu::mods::Mod::SuddenDeath {
            restart: true,
            fail_on_slider_tail: false,
        }]),
    );
    d.run_until(5_000, |s| s.restarts == 1);
    assert_eq!(d.state(), PlayState::Playing);
    assert!(d.session.results().is_none());
}

#[test]
fn failing_on_the_last_judgement_shows_no_results() {
    let mut map = beatmap();
    map.hit_objects.truncate(1);
    // Missing the only circle with No Fail passes.
    let mut d = passed(&map);
    d.run(1_100);
    assert_eq!(d.state(), PlayState::Results);

    // With Sudden Death, the same miss fails first.
    let sudden_death = slam_osu::mods::Mod::SuddenDeath {
        restart: false,
        fail_on_slider_tail: false,
    };
    let mut d = Driver::new(&map, mods(vec![sudden_death]));
    d.run_until(5_000, |s| s.state == PlayState::Failing);
    assert_eq!(d.session.log().events().len(), 1);
    d.run(3_000);
    assert_eq!(d.state(), PlayState::Failed);
    assert!(d.session.results().is_none());
}

#[test]
fn sudden_death_with_restart_restarts_on_the_first_miss() {
    let map = beatmap();
    let mut d = Driver::new(
        &map,
        mods(vec![slam_osu::mods::Mod::SuddenDeath {
            restart: true,
            fail_on_slider_tail: false,
        }]),
    );
    d.run_until(3_000, |s| s.restarts == 1);
    assert_eq!(d.state(), PlayState::Playing);
    assert!(d.session.log().events().is_empty());
    assert_eq!(d.snapshot.health, 1.0);
    // Back at the start: the first circle (1000 ms) is missed again a second later.
    d.run(10);
    assert!(d.snapshot.time < 0.0, "{}", d.snapshot.time);
    d.run_until(3_000, |s| s.restarts == 2);
}

#[test]
fn holding_quick_retry_restarts_from_the_start() {
    let map = beatmap();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    d.run_to(2500.0);
    assert!(!d.session.log().events().is_empty());
    // Released before 200 ms: nothing.
    d.hold(None, GRAVE, 150);
    assert_eq!(d.snapshot.restarts, 0);
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.retry_progress, 0.0);
    assert_eq!(d.snapshot.restarts, 1);
    assert!(d.session.log().events().is_empty());
    assert_eq!(d.snapshot.hud, HudValues::default());
    let seek = d
        .track
        .commands
        .iter()
        .rposition(|c| *c == Command::Seek(0));
    assert!(seek.is_some());
    d.run(5);
    let since_start = d.snapshot.time - d.session.start_time();
    assert!((0.0..10.0).contains(&since_start), "{since_start}");
    // The second play judges like a fresh one: the first circle is missed at its time.
    d.run_to(1_200.0);
    assert_eq!(d.session.log().events().len(), 1);
}

#[test]
fn a_quick_restart_skips_the_intro() {
    let map = delayed(29000.0);
    let mut d = Driver::new(&map, ModSet::default());
    d.run(100);
    d.hold(Some(LCTRL), 21, 200);
    assert_eq!(d.snapshot.restarts, 1);
    d.run(5);
    // Gameplay starts at 28 000 ms: the skip goes to a second before.
    assert!(d.track.commands.contains(&Command::Seek(27000 * 48)));
    assert!(d.snapshot.time >= 27_000.0, "{}", d.snapshot.time);
}

#[test]
fn quick_exit_and_back_after_failing_quit() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_to(500.0);
    d.hold(Some(LCTRL), GRAVE, 200);
    assert_eq!(d.state(), PlayState::Exited);
    assert_eq!(d.track.commands.last(), Some(&Command::Pause));
    d.run(10);
    assert_eq!(d.state(), PlayState::Exited);

    // Back during the fail animation cuts it short; again, it quits.
    let mut d = Driver::new(&map, ModSet::default());
    d.run_until(10_000, |s| s.state == PlayState::Failing);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Failed);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Exited);
}

#[test]
fn pausing_failing_and_ticks_after_a_restart_do_not_allocate() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    let moved = d.mouse(Vec2::new(300.0, 300.0));
    let ticks = |d: &mut Driver| {
        d.run_to(1500.0);
        d.tap(ESCAPE);
        d.run(50);
        d.tap(ESCAPE);
        let click = d.key(Z, true);
        d.tick(&[click]);
        d.run_until(10_000, |s| s.state == PlayState::Failed);
        d.run(10);
    };
    d.tick(&[moved]);
    assert_no_alloc(|| ticks(&mut d));
    // The restart builds the play anew; what follows does not allocate.
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    assert_no_alloc(|| {
        d.run(10);
        ticks(&mut d)
    });
}

#[test]
fn a_restart_waits_for_its_seek_in_the_track_to_be_heard() {
    // Starts at 0, in the track: the restart seeks back inside it.
    let map = delayed(5000.0);
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    assert_eq!(d.session.start_time(), 0.0);
    d.run_to(7_000.0);
    assert!(!d.session.log().events().is_empty());
    // The seek is heard 20 ms after it is sent.
    d.track.seek_delay = 20 * 48;
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    for _ in 0..30 {
        d.tick(&[]);
        // Fresh rules never see the old time: nothing is judged, frames do not move.
        assert!(d.session.log().events().is_empty());
        assert!(d.snapshot.time < 100.0, "{}", d.snapshot.time);
    }
    d.run_to(1_000.0);
    assert!(d.session.log().events().is_empty());
}

#[test]
fn failing_comes_before_a_pause_in_the_same_tick() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    // Find the tick that fails, then replay up to it with Escape on that tick.
    let mut probe = Driver::new(&map, ModSet::default());
    let mut ticks = 0;
    while probe.state() != PlayState::Failing {
        probe.tick(&[]);
        ticks += 1;
    }
    d.run(ticks - 1);
    // The step before Escape fails; Back then cuts the fail animation short instead of
    // pausing.
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Failed);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Exited);
}

#[test]
fn hud_updates_and_frames_do_not_allocate() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
    let mut hud = HudView::new(registry, "Пропустить");
    hud.request_glyphs(&mut glyphs);
    let mut text = SpriteBatch::with_capacity(256);
    let mut batch = SpriteBatch::with_capacity(256);
    let mut snapshot = PlaySnapshot {
        skip_available: true,
        state: PlayState::Resuming,
        ..PlaySnapshot::default()
    };
    let values = [
        HudValues::default(),
        HudValues {
            score: 1_234_567,
            accuracy: 0.987_65,
            combo: 1234,
        },
        HudValues {
            score: i64::MAX,
            accuracy: f64::NAN,
            combo: u32::MAX,
        },
        HudValues {
            score: -1,
            accuracy: 0.0,
            combo: 0,
        },
    ];
    assert_no_alloc(|| {
        for (index, values) in values.iter().cycle().take(16).enumerate() {
            let height = [720.0, 1080.0, 1.0, 0.0][index % 4];
            hud.update(1280.0, height, values);
            snapshot.hud = *values;
            batch.clear();
            text.clear();
            hud.push_shapes(&mut batch, &layout(), &snapshot, WHITE);
            hud.push_text(&mut text, &snapshot, &mut glyphs);
        }
    });
    // Health bar (two), skip button and resume target.
    assert_eq!(batch.len(), 4);
}

#[test]
fn the_snapshot_tells_breaks_the_intro_and_the_end() {
    let mut map = delayed(3000.0);
    // Effective from 4500 to 5300 - 325 ms of fade.
    map.breaks.push(slam_osu::breaks::Break {
        start: 4500.0,
        end: 5300.0,
    });
    // No Fail: nobody plays and the play has to finish.
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    // Gameplay starts at 4000 - max(2000, preempt).
    assert!(d.snapshot.is_break_time);
    d.run_to(1999.0);
    assert!(d.snapshot.is_break_time);
    d.run_to(2000.0);
    assert!(!d.snapshot.is_break_time);
    d.run_to(4500.0);
    assert!(d.snapshot.is_break_time);
    d.run_to(4974.0);
    assert!(d.snapshot.is_break_time);
    // Ticks fall between whole milliseconds, so 4975 itself is not hit (the unit tests of
    // `BreakTracker` check the inclusive end).
    d.run_until(1_000, |s| s.time > 4975.0);
    assert!(!d.snapshot.is_break_time);
    d.run_until(20_000, |s| s.state == PlayState::Passed);
    assert!(d.snapshot.is_break_time);
}

/// `delayed(3000)` with a break effective from 4500 to 4975.
fn with_break() -> Beatmap {
    let mut map = delayed(3000.0);
    map.breaks.push(slam_osu::breaks::Break {
        start: 4500.0,
        end: 5300.0,
    });
    map
}

#[test]
fn a_break_mid_play_pauses_without_cooldown_and_resumes_at_once() {
    let map = with_break();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    let moved = d.mouse(Vec2::new(100.0, 100.0));
    d.tick(&[moved]);
    d.run_to(4600.0);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Paused);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Playing);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Paused);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Playing);
    // Out of the break and the cooldown, the resume cursor is back.
    d.run_to(5700.0);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Paused);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Resuming);
}

#[test]
fn after_a_restart_the_intro_and_the_breaks_count_again() {
    let map = with_break();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    d.run_to(6000.0);
    assert!(!d.snapshot.is_break_time);
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    d.run(5);
    assert!(d.snapshot.time < 2000.0, "{}", d.snapshot.time);
    assert!(d.snapshot.is_break_time);
    d.run_to(3000.0);
    assert!(!d.snapshot.is_break_time);
    d.run_to(4600.0);
    assert!(d.snapshot.is_break_time);
}
