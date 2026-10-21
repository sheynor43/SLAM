//! A play session on a simulated output stream and track: the clock starts on the first
//! position report, ticks are deterministic, the play finishes, and nothing allocates.

mod common;

use std::cell::Cell;
use std::sync::Arc;

use common::skin::hitsounds;
use common::{beatmap, layout, play};
use slam_audio::{
    FilterKind, MusicAnchor, MusicSnapshot, PlayError, PositionSnapshot, Sample, TrackAdjustment,
    TrackControl, VoiceGroup, VoiceId, VoiceParams,
};
use slam_input::{InputEvent, InputKind, MouseButton};
use slam_osu::judgement::{HitResult, JudgementLog};
use slam_osu::replay::{Actions, Rules};
use slam_osu::rules::OsuRules;
use slam_osu::scoring::ScoreProcessor;
use slam_osu::{Beatmap, ModSet, Vec2};
use slam_play::circles::{
    CircleMods, CircleSettings, CircleSkinConfig, CircleStyle, CircleVisuals, HitCircles,
    SkinSprite,
};
use slam_play::cursor::{
    CursorSkinConfig, CursorTextures, CursorView, ExpandTimes, MAX_CURSOR_SPRITES,
};
use slam_play::hud::OFFSET_PLACEHOLDER;
use slam_play::menu_overlay::{
    GameplayMenu, MenuInfo, MenuKind, MenuLayout, MenuView, song_progress,
};
use slam_play::offset::OffsetPanel;
use slam_play::slider_body::Snaking;
use slam_play::sliders::{AnimationSprites, HitSliders, SliderBodies, SliderVisuals};
use slam_play::{
    BackgroundSettings, BackgroundView, FpsCounter, FpsLabels, FpsView, HudLabels, HudValues,
    HudView, MenuLabels,
};
use slam_play::{
    Bindings, FrameTextures, GameTime, GameplaySounds, LiveInput, Offsets, PlaySession,
    PlaySnapshot, PlayState, PlayView, PlayfieldLayout, SoundSettings, VoiceSink,
};
use slam_render::Extent;
use slam_render::Limits;
use slam_render::Origin;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_skin::SIXTY_FRAME_TIME;
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
    /// Every adjustment given, in order.
    adjustments: Vec<TrackAdjustment>,
    /// Refuse the next adjustment (queue full).
    refuse_adjust: bool,
    /// Frequency moves given: stream frame, target, frames.
    frequencies: Vec<(u64, f64, u64)>,
    /// Tempos given; the last one holds from the start (sessions set it before playing).
    rates: Vec<f64>,
    /// Refuse the next tempo once.
    refuse_rate: bool,
}

impl Track {
    /// Room for `n` commands, so that logging them does not allocate.
    fn reserve(&mut self, n: usize) {
        self.commands.reserve(n);
        self.issued.reserve(n);
        self.adjustments.reserve(n);
        self.frequencies.reserve(n);
        self.rates.reserve(n);
    }

    fn log(&mut self, command: Command) {
        self.commands.push(command);
        self.issued.push(self.frame.get());
    }

    /// Where the callback has the track at stream frame `frame`: the commands take effect
    /// on the frame they were given at (no latency), a `play_at` on its frame, a frequency
    /// move as the callback makes it (one anchor with an acceleration). `None` before the
    /// first command.
    fn music(&self, frame: u64) -> Option<MusicSnapshot> {
        #[derive(Clone, Copy)]
        struct Move {
            from: u64,
            start: f64,
            step: f64,
            end: u64,
            target: f64,
        }
        impl Move {
            fn at(&self, x: u64) -> (f64, f64) {
                if x >= self.end {
                    (self.target, 0.0)
                } else {
                    (self.start + self.step * (x - self.from) as f64, self.step)
                }
            }
        }
        #[derive(Clone, Copy)]
        enum Event {
            Command(Command),
            Frequency(f64, u64),
        }
        let mut anchors: Option<(MusicAnchor, MusicAnchor)> = None;
        let mut generation = 0;
        let mut scheduled = None;
        let mut playing = false;
        let mut frequency = Move {
            from: 0,
            start: 1.0,
            step: 0.0,
            end: 0,
            target: 1.0,
        };
        let mut move_end = None;
        let tempo = self.rates.last().copied().unwrap_or(1.0);
        let position = |anchors: &Option<(MusicAnchor, MusicAnchor)>, at: u64| {
            anchors.map_or(0.0, |(_, a)| a.at(at as f64))
        };
        let push = |anchors: &mut Option<(MusicAnchor, MusicAnchor)>,
                    at: u64,
                    track_frame: f64,
                    playing: bool,
                    frequency: &Move| {
            let (speed, accel) = if playing {
                let (speed, accel) = frequency.at(at);
                (speed * tempo, accel * tempo)
            } else {
                (0.0, 0.0)
            };
            let anchor = MusicAnchor {
                device_frame: at,
                track_frame,
                speed,
                accel,
            };
            *anchors = Some(match *anchors {
                Some((_, last)) => (last, anchor),
                None => (anchor, anchor),
            });
        };
        // Starts and move ends due by `upto`, in order.
        let fire = |anchors: &mut Option<(MusicAnchor, MusicAnchor)>,
                    scheduled: &mut Option<u64>,
                    move_end: &mut Option<u64>,
                    playing: &mut bool,
                    frequency: &Move,
                    upto: u64| {
            loop {
                let next = [*scheduled, *move_end]
                    .into_iter()
                    .flatten()
                    .filter(|&at| at <= upto)
                    .min();
                let Some(at) = next else { break };
                let track_frame = position(anchors, at);
                if *scheduled == Some(at) {
                    *scheduled = None;
                    *playing = true;
                } else {
                    *move_end = None;
                }
                push(anchors, at, track_frame, *playing, frequency);
            }
        };
        // Both logs are in the order given; commands of one frame come first.
        let command_at = |k: usize| match self.commands[k] {
            Command::Seek(_) => self.issued[k] + self.seek_delay,
            _ => self.issued[k],
        };
        let (mut c, mut f) = (0, 0);
        let events = std::iter::from_fn(|| {
            let next_command = (c < self.commands.len()).then(|| command_at(c));
            let next_frequency = self.frequencies.get(f).map(|&(at, ..)| at);
            match (next_command, next_frequency) {
                (Some(at), Some(fat)) if at <= fat => {
                    c += 1;
                    Some((at, Event::Command(self.commands[c - 1])))
                }
                (Some(at), None) => {
                    c += 1;
                    Some((at, Event::Command(self.commands[c - 1])))
                }
                (_, Some(at)) => {
                    let (_, target, frames) = self.frequencies[f];
                    f += 1;
                    Some((at, Event::Frequency(target, frames)))
                }
                (None, None) => None,
            }
        });
        for (at, event) in events {
            if at > frame {
                break;
            }
            fire(
                &mut anchors,
                &mut scheduled,
                &mut move_end,
                &mut playing,
                &frequency,
                at,
            );
            let track_frame = position(&anchors, at);
            match event {
                Event::Command(Command::PlayAt(device_frame)) => scheduled = Some(device_frame),
                Event::Command(command @ (Command::Play | Command::Pause)) => {
                    scheduled = None;
                    playing = command == Command::Play;
                    push(&mut anchors, at, track_frame, playing, &frequency);
                }
                Event::Command(Command::Seek(to)) => {
                    generation += 1;
                    push(&mut anchors, at, to as f64, playing, &frequency);
                }
                Event::Frequency(target, frames) => {
                    let (start, _) = frequency.at(at);
                    frequency = Move {
                        from: at,
                        start,
                        step: if frames == 0 {
                            0.0
                        } else {
                            (target - start) / frames as f64
                        },
                        end: at + frames,
                        target,
                    };
                    move_end = (frames > 0).then_some(at + frames);
                    push(&mut anchors, at, track_frame, playing, &frequency);
                }
            }
        }
        fire(
            &mut anchors,
            &mut scheduled,
            &mut move_end,
            &mut playing,
            &frequency,
            frame,
        );
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

    fn set_rate(&mut self, rate: f64) -> Result<(), PlayError> {
        if std::mem::take(&mut self.refuse_rate) {
            return Err(PlayError::QueueFull);
        }
        self.rates.push(rate);
        Ok(())
    }

    fn adjust(&mut self, adjustments: &[TrackAdjustment]) -> Result<(), PlayError> {
        if std::mem::take(&mut self.refuse_adjust) {
            return Err(PlayError::QueueFull);
        }
        self.adjustments.extend_from_slice(adjustments);
        for adjustment in adjustments {
            if let TrackAdjustment::Frequency {
                frequency,
                duration_ms,
            } = *adjustment
            {
                let frames = (duration_ms * f64::from(RATE) / 1_000.0).round() as u64;
                self.frequencies.push((self.frame.get(), frequency, frames));
            }
        }
        Ok(())
    }

    fn generation(&self) -> u32 {
        self.generation
    }
}

/// Takes every command.
#[derive(Default)]
struct Sink {
    plays: u64,
    /// Voices started in the effects group, and the last of them.
    effects: u64,
    last_effect: Option<VoiceId>,
    /// The last voice stopped.
    last_stopped: Option<VoiceId>,
}

impl VoiceSink for Sink {
    fn play_voice(&mut self, _: &Arc<Sample>, params: VoiceParams) -> Result<VoiceId, PlayError> {
        self.plays += 1;
        let voice = VoiceId::from_play(self.plays);
        if params.group == VoiceGroup::Effects {
            self.effects += 1;
            self.last_effect = Some(voice);
        }
        Ok(voice)
    }

    fn stop_voice(&mut self, voice: VoiceId) -> Result<(), PlayError> {
        self.last_stopped = Some(voice);
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
    session_with_mods(map, track, offsets, mods(vec![slam_osu::mods::Mod::NoFail]))
}

fn session_with_mods(map: &Beatmap, track: &Track, offsets: Offsets, mods: ModSet) -> PlaySession {
    let (sounds, _) = hitsounds(map, &["normal-hitnormal.wav", "soft-hitnormal.wav"]);
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
fn the_snapshot_carries_every_judgement_that_shows_a_popup() {
    let map = beatmap();
    let track = Track::default();
    let end = end_ns(&map, session(&map, &track).start_time());
    let mut seen = 0;
    let mut shown = Vec::new();
    let (session, _, _) = run(&map, MS, end, |s| {
        shown.extend(s.judgements.since(seen).copied());
        seen = s.judgements.written();
    });
    let targets = slam_play::judgements::PopupTargets::new(&map, &ModSet::default());
    let expected: Vec<_> = session
        .log()
        .events()
        .iter()
        .filter_map(|j| targets.shown(j))
        .collect();
    assert!(!expected.is_empty());
    assert!(
        expected.len() < session.log().events().len(),
        "ticks show none"
    );
    assert_eq!(shown, expected);
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
    let mut view = view(&map);
    let mut textures = FrameTextures::default();
    // A continuous trail: the most parts.
    let piece = Some(SkinSprite::new(WHITE, 1.0, f32::MAX));
    textures.cursor = CursorTextures {
        cursor: piece,
        middle: piece,
        trail: piece,
        config: CursorSkinConfig {
            disjoint: false,
            ..CursorSkinConfig::default()
        },
    };
    textures.follow_points = AnimationSprites::new(piece, SIXTY_FRAME_TIME);
    textures.meter = Some(slam_play::hit_error::MeterTextures {
        white: WHITE,
        disc: WHITE,
        gradient: WHITE,
        triangle: WHITE,
    });
    textures.disc = Some(WHITE);
    let mut batch = SpriteBatch::with_capacity(
        4096 + MAX_CURSOR_SPRITES
            + slam_play::resume::MAX_TARGET_SPRITES
            + slam_play::hit_error::MAX_HIT_ERROR_SPRITES,
    );
    let mut bodies = SliderBodies::new();
    let mut drawn = 0;
    let mut bodies_drawn = 0;
    let mut follow_points_drawn = 0;
    // The HUD's glyphs are only requested here: without a device they stay pending.
    let registry = Arc::new(FontRegistry::with_builtin());
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
    let mut hud = HudView::new(registry, &labels());
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
            hud.update(
                1280.0,
                720.0,
                &snapshot.hud,
                snapshot.offset_panel.offset_ms,
            );
            hud.push_shapes(&mut batch, &snapshot, now, white);
            view.push_hit_error(
                &mut batch,
                1280.0,
                720.0,
                &snapshot,
                now + MS / 2,
                &textures,
            );
            pending += hud
                .push_text(&mut text, &snapshot, now, &mut glyphs)
                .pending;
            view.push_cursor(&mut batch, &layout(), &snapshot, now + MS / 2, &textures);
            drawn += snapshot.circles.as_slice().len();
            follow_points_drawn += view
                .follow_points
                .alive(snapshot.time_at(now + MS / 2))
                .count();
            now += 4 * MS;
        }
    });
    assert_eq!(snapshot.state, PlayState::Results);
    assert!(drawn > 0);
    assert!(bodies_drawn > 0);
    assert!(follow_points_drawn > 0);
    assert!(pending > 0);
    assert!(snapshot.hud.score > 0);
    assert_eq!(hud.texts()[0], format!("{:06}", snapshot.hud.score));
}

/// The HUD's texts in Russian: the longest.
fn labels() -> HudLabels {
    HudLabels {
        skip: "Пропустить".to_owned(),
        offset_zero: format!("{OFFSET_PLACEHOLDER} мс"),
        offset_earlier: format!("{OFFSET_PLACEHOLDER} мс (объекты появляются раньше)"),
        offset_later: format!("{OFFSET_PLACEHOLDER} мс (объекты появляются позже)"),
        menu: MenuLabels {
            paused: "пауза".to_owned(),
            failed: "провал".to_owned(),
            continue_button: "Продолжить".to_owned(),
            retry_button: "Заново".to_owned(),
            quit_button: "Выйти".to_owned(),
            retry_count: format!("Перезапусков: {OFFSET_PLACEHOLDER}"),
            song_progress: format!("Прогресс песни: {OFFSET_PLACEHOLDER}"),
            accuracy: format!("Точность: {OFFSET_PLACEHOLDER}"),
        },
        fps: FpsLabels {
            frame_time: format!("{OFFSET_PLACEHOLDER} мс"),
            fps: format!("{OFFSET_PLACEHOLDER} fps"),
        },
    }
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
        spinners: slam_play::spinners::HitSpinners {
            visuals: slam_play::spinners::SpinnerVisuals::new(map),
            style: slam_play::spinners::SpinnerStyle::new(mods, &chain),
        },
        follow_points: slam_play::follow_points::FollowPoints::new(map),
        judgements: slam_play::judgements::JudgementView::new(
            map,
            CircleSettings::default(),
            None,
            &chain,
        ),
        hit_error: slam_play::hit_error::HitErrorMeter::new(
            slam_play::hit_error::HitErrorStyle::Bar,
            &windows,
            1.0,
        ),
        hit_error_hiding: slam_play::hit_error::SpinnerHiding::new(map),
        cursor: CursorView::default(),
        resume: slam_play::resume::ResumeView::default(),
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
const F10: u16 = 67;
const MINUS: u16 = 45;
const EQUALS: u16 = 46;
const ENTER: u16 = 40;
const SPACE: u16 = 44;
const DOWN: u16 = 81;
const UP: u16 = 82;

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
            GameplaySounds::new(map, sounds, SoundSettings::for_mods(&mods)).with_fail_sound(Some(
                Arc::new(Sample::from_interleaved(vec![0.1; 2 * 10], RATE, 2).unwrap()),
            )),
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

    /// Moves the mouse to the middle of button `index` of menu `kind`.
    fn menu_mouse(&self, kind: MenuKind, index: usize) -> InputEvent {
        let [width, height] = layout().window_size();
        let ([x, y], [w, h]) = MenuLayout::new(width, height, kind).button(index);
        press(
            self.now,
            InputKind::MouseMove {
                x: x + w / 2.0,
                y: y + h / 2.0,
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
fn while_resuming_the_overlay_cursor_follows_the_window_and_gameplay_cursor_hides() {
    const CURSOR: TextureId = TextureId(1);
    const DISC: TextureId = TextureId(2);
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    let mut view = view(&map);
    let mut textures = FrameTextures::default();
    let cursor = AtlasRegion {
        texture: CURSOR,
        ..WHITE
    };
    textures.cursor.cursor = Some(SkinSprite::new(cursor, 1.0, f32::MAX));
    textures.disc = Some(AtlasRegion {
        texture: DISC,
        ..WHITE
    });
    let mut batch = SpriteBatch::new();
    // The textures of a frame's sprites, in order.
    fn drawn(
        view: &mut PlayView,
        textures: &FrameTextures,
        d: &Driver,
        batch: &mut SpriteBatch,
    ) -> Vec<TextureId> {
        batch.clear();
        view.push_cursor(batch, &layout(), &d.snapshot, d.now, textures);
        batch.batches().iter().map(|b| b.texture).collect()
    }

    let at = Vec2::new(300.0, 300.0);
    let moved = d.mouse(at);
    d.tick(&[moved]);
    d.run_to(1500.0);
    assert_eq!(drawn(&mut view, &textures, &d, &mut batch), [CURSOR]);
    d.tap(ESCAPE);
    // Paused: gameplay's cursor stays where it was.
    let away = d.mouse(Vec2::new(100.0, 100.0));
    d.tick(&[away]);
    assert_eq!(drawn(&mut view, &textures, &d, &mut batch), [CURSOR]);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Resuming);
    // The target under the overlay's cursor, which is where the window's is.
    assert_eq!(drawn(&mut view, &textures, &d, &mut batch), [DISC, CURSOR]);
    assert_eq!(d.snapshot.resume_cursor, Vec2::new(100.0, 100.0));
    let window = layout().to_window(Vec2::new(100.0, 100.0));
    let cursor_quad = &batch.vertices()[batch.len() * 4 - 4..];
    let centre_x = cursor_quad.iter().map(|v| v.position[0]).sum::<f32>() / 4.0;
    assert!(
        (centre_x - window.x).abs() < 1e-3,
        "{centre_x} {}",
        window.x
    );

    // The overlay's cursor and trail draw without allocating, gameplay's hidden.
    let mut frames =
        SpriteBatch::with_capacity(MAX_CURSOR_SPRITES + slam_play::resume::MAX_TARGET_SPRITES);
    for step in 0..8 {
        let moved = d.mouse(Vec2::new(100.0 + 20.0 * step as f32, 100.0));
        d.tick(&[moved]);
        let (snapshot, now) = (&d.snapshot, d.now);
        assert_no_alloc(|| {
            frames.clear();
            view.push_cursor(&mut frames, &layout(), snapshot, now, &textures);
        });
    }
    assert_eq!(d.state(), PlayState::Resuming);

    let on = d.mouse(at);
    d.tick(&[on]);
    let click = d.key(Z, true);
    d.tick(&[click]);
    assert_eq!(d.state(), PlayState::Playing);
    d.run(1);
    assert_eq!(drawn(&mut view, &textures, &d, &mut batch), [CURSOR]);
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
    // The track slows down to a stop: t − t²/5000 ms of gameplay in t ms.
    let slowed = 2_490.0 - 2_490.0 * 2_490.0 / 5_000.0;
    assert!(
        (d.snapshot.time - time - slowed).abs() < 5.0,
        "{} after {time}",
        d.snapshot.time
    );
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
    assert_eq!(
        results.maximum_statistics,
        *d.session.score().maximum_statistics()
    );
    assert!(results.maximum_statistics.max_combo > 0);
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
fn the_snapshot_carries_every_judgement_the_hit_error_meter_shows() {
    let map = beatmap();
    let track = Track::default();
    let end = end_ns(&map, session(&map, &track).start_time());
    let mut seen = 0;
    let mut shown = Vec::new();
    let (session, _, _) = run(&map, MS, end, |s| {
        shown.extend(s.hit_errors.since(seen).copied());
        seen = s.hit_errors.written();
    });
    let targets = slam_play::hit_error::HitErrorTargets::new(&map);
    let expected: Vec<_> = session
        .log()
        .events()
        .iter()
        .filter_map(|j| targets.error(j))
        .collect();
    assert_eq!(shown, expected);
}

#[test]
fn hits_reach_the_meter_and_a_quick_retry_starts_it_over() {
    let map = beatmap();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    let circle = &map.hit_objects[0];
    let at = d.mouse(circle.stacked_position());
    d.tick(&[at]);
    d.run_to(circle.start_time - 10.0);
    d.tap(Z);
    let errors: Vec<_> = d.snapshot.hit_errors.since(0).copied().collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].result, HitResult::Great);
    assert!((errors[0].offset - (errors[0].time - circle.start_time)).abs() < 1e-9);
    // About 10 ms early: pressed between two ticks.
    assert!((-12.0..=-8.0).contains(&errors[0].offset), "{errors:?}");
    // A slider's head shows too, its tick and tail do not.
    let slider = &map.hit_objects[1];
    let at = d.mouse(slider.stacked_position());
    d.tick(&[at]);
    d.run_to(slider.start_time + 20.0);
    d.hold(None, Z, 1);
    d.run_to(slider.end_time() + 100.0);
    let errors: Vec<_> = d.snapshot.hit_errors.since(0).copied().collect();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!((18.0..=22.0).contains(&errors[1].offset), "{errors:?}");

    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    assert_eq!(d.snapshot.hit_errors.since(0).count(), 0);
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
    let maxima = d.session.results().unwrap().maximum_statistics;
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    assert_eq!(d.state(), PlayState::Playing);
    assert!(d.session.results().is_none());

    d.run_until(12_000, |s| s.state == PlayState::Results);
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 2);
    assert_eq!(d.state(), PlayState::Playing);
    assert!(d.session.log().events().is_empty());
    // The next play passes again, against the same maxima.
    d.run_until(12_000, |s| s.state == PlayState::Passed);
    assert_eq!(d.session.results().unwrap().maximum_statistics, maxima);
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

fn filter(kind: FilterKind, cutoff_hz: f32, duration_ms: f64) -> TrackAdjustment {
    TrackAdjustment::Filter {
        kind,
        cutoff_hz,
        duration_ms,
    }
}

fn frequency(frequency: f64, duration_ms: f64) -> TrackAdjustment {
    TrackAdjustment::Frequency {
        frequency,
        duration_ms,
    }
}

/// What the track is given when the fail animation ends.
const FAILED: [TrackAdjustment; 4] = [
    TrackAdjustment::Frequency {
        frequency: 0.0,
        duration_ms: 0.0,
    },
    TrackAdjustment::Volume(1.0),
    TrackAdjustment::Filter {
        kind: FilterKind::LowPass,
        cutoff_hz: 22_049.0,
        duration_ms: 0.0,
    },
    TrackAdjustment::Filter {
        kind: FilterKind::HighPass,
        cutoff_hz: 1.0,
        duration_ms: 0.0,
    },
];

#[test]
fn failing_slows_the_track_to_a_stop_with_the_fail_sound_and_filters() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    // A new play makes the track sound as it is: the last one may have failed.
    assert_eq!(d.track.adjustments, slam_play::fail::neutral_track());
    d.run_until(10_000, |s| s.state == PlayState::Failing);
    let fail = d.snapshot.fail.expect("failing");
    // The driver moves on a millisecond after each tick.
    assert_eq!(
        (fail.time, fail.at_ns, fail.cut),
        (d.snapshot.time, d.now - MS, false)
    );
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (Start)
    assert_eq!(
        d.track.adjustments[4..],
        [
            frequency(0.0, 2_500.0),
            TrackAdjustment::Volume(0.5),
            filter(FilterKind::LowPass, 300.0, 2_500.0),
            filter(FilterKind::HighPass, 300.0, 0.0),
        ]
    );
    assert_eq!(d.sink.effects, 1);
    let fail_voice = d.sink.last_effect;
    // The track only ever slows down (lazer's `TestSceneFailAnimation`).
    let mut speed = f64::INFINITY;
    while d.state() == PlayState::Failing {
        let frame = d.track.frame.get();
        let now = d.track.music(frame).unwrap().anchor.speed_at(frame as f64);
        assert!(now <= speed, "{now} after {speed}");
        speed = now;
        d.run(1);
    }
    assert!(speed < 0.01, "{speed}");
    assert_eq!(d.state(), PlayState::Failed);
    assert_eq!(d.track.adjustments[8..], FAILED);
    assert_eq!(d.snapshot.fail, Some(fail));
    // The fail sound plays on under the menu; a restart stops it and the track sounds as it
    // is again.
    assert_ne!(d.sink.last_stopped, fail_voice);
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    assert_eq!(d.sink.last_stopped, fail_voice);
    assert_eq!(d.track.adjustments[12..], slam_play::fail::neutral_track());
    assert_eq!(d.snapshot.fail, None);
    d.run(100);
    assert_eq!(d.track.adjustments.len(), 16);
}

#[test]
fn back_cuts_the_fail_animation_short_and_stops_the_track_at_once() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_until(10_000, |s| s.state == PlayState::Failing);
    d.run(500);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Failed);
    assert!(d.snapshot.fail.unwrap().cut);
    assert_eq!(d.track.adjustments[8..], FAILED);
    // Quitting stops the fail sound and resets the track.
    let fail_voice = d.sink.last_effect;
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Exited);
    assert_eq!(d.sink.last_stopped, fail_voice);
    assert_eq!(d.track.adjustments[12..], slam_play::fail::neutral_track());
}

#[test]
fn a_refused_track_adjustment_is_sent_again() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_until(10_000, |s| s.state == PlayState::Failing);
    d.track.refuse_adjust = true;
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Failed);
    assert_eq!(d.track.adjustments.len(), 8);
    let errors = d.session.clock_errors();
    d.run(1);
    assert_eq!(d.track.adjustments[8..], FAILED);
    assert_eq!(d.session.clock_errors(), errors);
}

#[test]
fn failed_objects_fall_and_fade_out() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    let mut view = view(&map);
    let mut textures = FrameTextures::default();
    let piece = Some(SkinSprite::new(WHITE, 1.0, f32::MAX));
    textures.circles.circle = piece;
    textures.circles.approach = piece;
    let mut bodies = SliderBodies::new();
    // The frame drawn with the snapshot's fail, and without it.
    let mut draw = |d: &Driver| {
        let mut frame = |snapshot: &PlaySnapshot| {
            let mut batch = SpriteBatch::new();
            view.push(
                &mut batch,
                &mut bodies,
                &layout(),
                snapshot,
                d.now,
                &textures,
            );
            batch.vertices().to_vec()
        };
        let failing = frame(&d.snapshot);
        let mut plain = d.snapshot.clone();
        plain.fail = None;
        (failing, frame(&plain))
    };
    let mean_y = |v: &[slam_render::sprite::SpriteVertex]| {
        v.iter().map(|v| v.position[1]).sum::<f32>() / v.len() as f32
    };
    d.run_until(10_000, |s| s.state == PlayState::Failing);
    let (failing, plain) = draw(&d);
    assert!(!plain.is_empty());
    // A millisecond on: the fall starts from where the objects are.
    for (f, p) in failing.iter().zip(&plain) {
        assert!((f.position[0] - p.position[0]).abs() < 0.5);
        assert!((f.position[1] - p.position[1]).abs() < 0.5);
        assert!((f.colour[3] - p.colour[3]).abs() < 0.01);
    }
    // Every frame with objects later on: they are lower and fainter.
    let mut checked = 0;
    for _ in 0..40 {
        d.run(50);
        let (failing, plain) = draw(&d);
        assert_eq!(failing.len(), plain.len());
        if plain.is_empty() {
            continue;
        }
        checked += 1;
        assert!(mean_y(&failing) > mean_y(&plain));
        for (f, p) in failing.iter().zip(&plain) {
            assert!(f.colour[3] < p.colour[3] || p.colour[3] == 0.0);
            assert_eq!(f.colour[..3], p.colour[..3]);
        }
    }
    assert!(checked > 0);
    // Back: the animation is at its end, the objects are gone.
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Failed);
    let (failing, _) = draw(&d);
    assert!(failing.iter().all(|v| v.colour[3] == 0.0));
}

#[test]
fn pausing_failing_and_ticks_after_a_restart_do_not_allocate() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    let moved = d.mouse(Vec2::new(300.0, 300.0));
    let ticks = |d: &mut Driver| {
        d.run_to(1500.0);
        d.tap(MINUS);
        d.tap(EQUALS);
        d.tap(ESCAPE);
        d.run(50);
        d.tap(DOWN);
        d.tap(UP);
        let over = d.menu_mouse(MenuKind::Pause, 1);
        let away = d.mouse(Vec2::new(300.0, 300.0));
        d.tick(&[over, away]);
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
    let mut hud = HudView::new(registry, &labels());
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
    let offsets = [0.0, -1.0, 12.5, 50.0, -50.0];
    assert_no_alloc(|| {
        for (index, values) in values.iter().cycle().take(16).enumerate() {
            let height = [720.0, 1080.0, 1.0, 0.0][index % 4];
            let offset = offsets[index % offsets.len()];
            hud.update(1280.0, height, values, offset);
            snapshot.hud = *values;
            snapshot.offset_panel = OffsetPanel::new(0.0);
            snapshot.offset_panel.changed(offset, 0);
            batch.clear();
            text.clear();
            hud.push_shapes(&mut batch, &snapshot, 100 * MS, WHITE);
            hud.push_text(&mut text, &snapshot, 100 * MS, &mut glyphs);
        }
    });
    // Health bar (two), skip button and offset panel; the resume target goes with the
    // cursor.
    assert_eq!(batch.len(), 4);
    hud.update(1280.0, 720.0, &HudValues::default(), -0.0001);
    assert_eq!(hud.offset_text(), "0 мс");
    hud.update(1280.0, 720.0, &HudValues::default(), 12.5);
    assert_eq!(hud.offset_text(), "12.5 мс (объекты появляются раньше)");
    hud.update(1280.0, 720.0, &HudValues::default(), -3.0);
    assert_eq!(hud.offset_text(), "-3 мс (объекты появляются позже)");
}

#[test]
fn fps_counter_frames_do_not_allocate() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
    let mut view = FpsView::new(registry, &labels().fps);
    assert!(view.request_glyphs(&mut glyphs) > 0);
    let mut counter = FpsCounter::new();
    let mut text = SpriteBatch::with_capacity(256);
    let mut batch = SpriteBatch::with_capacity(16);
    assert_no_alloc(|| {
        let mut now = 0;
        for frame in 0..4000_u64 {
            // Steady, then spikes and a rate change, at several window sizes.
            let step = if frame % 700 == 699 { 40 * MS } else { MS };
            now += step;
            let aim = if frame < 2000 { 1000.0 } else { 144.0 };
            // Update stalls too: two-digit and grouped frame times, and a suspension.
            let update = match frame % 900 {
                300 => 30 * MS,
                600 => 1234 * MS,
                899 => 15_000 * MS,
                _ => 1000 * 1000 + frame * 1000,
            };
            counter.frame(now, update, aim, 1000.0);
            let height = [720.0, 1080.0, 1.0, 0.0][(frame / 500) as usize % 4];
            view.update(1280.0, height, &counter);
            batch.clear();
            text.clear();
            view.push_shapes(&mut batch, WHITE);
            view.push_text(&mut text, &mut glyphs);
        }
    });
    assert_eq!(batch.len(), 1);
}

#[test]
fn menu_frames_do_not_allocate() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
    let mut view = MenuView::new(registry, &labels().menu);
    assert!(view.request_glyphs(&mut glyphs) > 0);
    let mut text = SpriteBatch::with_capacity(256);
    let mut batch = SpriteBatch::with_capacity(16);
    let mut menus = Vec::new();
    for (kind, retries, progress, accuracy) in [
        (MenuKind::Pause, 0, 0, 1.0),
        (MenuKind::Fail, 12_345, 100, 0.987_65),
        (MenuKind::Pause, u32::MAX, 57, f64::NAN),
    ] {
        let mut menu = GameplayMenu::default();
        let info = MenuInfo {
            retries,
            progress,
            accuracy,
        };
        menu.show(kind, info, 0, [1280.0, 720.0]);
        menus.push(menu.snapshot());
    }
    assert_no_alloc(|| {
        for (index, menu) in menus.iter().cycle().take(12).enumerate() {
            let height = [720.0, 1080.0, 1.0, 0.0][index % 4];
            view.update(1280.0, height, menu);
            batch.clear();
            text.clear();
            view.push_shapes(&mut batch, menu, 100 * MS, WHITE);
            view.push_text(&mut text, menu, 100 * MS, &mut glyphs);
        }
    });
    // The background and a rectangle per button.
    view.update(1280.0, 720.0, &menus[0]);
    batch.clear();
    view.push_shapes(&mut batch, &menus[0], 300 * MS, WHITE);
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

#[test]
fn f10_toggles_the_mouse_buttons_for_gameplay_only() {
    let map = beatmap();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    d.run_to(1500.0);
    let button = |d: &Driver, button, down| press(d.now, InputKind::MouseButton { button, down });

    // In one tick: the press before F10 reaches gameplay, the one after does not.
    let events = [
        button(&d, MouseButton::Left, true),
        d.key(F10, true),
        d.key(F10, false),
        button(&d, MouseButton::Right, true),
    ];
    d.tick(&events);
    assert!(d.session.input().mouse_buttons_disabled());
    d.run(1);
    let held = d.session.input().held();
    assert!(
        held.contains(Actions::LEFT) && !held.contains(Actions::RIGHT),
        "{held:?}"
    );
    // The button pressed before still releases.
    let release = [button(&d, MouseButton::Left, false)];
    d.tick(&release);
    d.run(1);
    assert_eq!(d.session.input().held(), Actions::NONE);

    // Kept over a restart, as a setting.
    d.hold(None, GRAVE, 250);
    assert!(d.session.input().mouse_buttons_disabled());

    // Global bindings still see the buttons: the middle one pauses.
    d.run_until(5000, |s| s.state == PlayState::Playing && s.time >= 1500.0);
    let click = [
        button(&d, MouseButton::Middle, true),
        button(&d, MouseButton::Middle, false),
    ];
    d.tick(&click);
    assert_eq!(d.state(), PlayState::Paused);
    // F10 works while paused too.
    d.tap(F10);
    assert!(!d.session.input().mouse_buttons_disabled());
}

#[test]
fn the_snapshot_carries_the_cursor_path_and_its_expansion() {
    let map = beatmap();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    d.run_to(1500.0);
    assert_eq!(d.snapshot.cursor_path.written(), 0);
    let moves = [
        d.mouse(Vec2::new(10.0, 20.0)),
        d.mouse(Vec2::new(30.0, 40.0)),
    ];
    d.tick(&moves);
    let path: Vec<Vec2> = d.snapshot.cursor_path.since(0).collect();
    assert_eq!(path.len(), 2);
    assert!((path[1].x - 30.0).abs() < 1e-3 && (path[1].y - 40.0).abs() < 1e-3);

    let down = d.key(Z, true);
    d.tick(&[down]);
    let pressed_at = d.snapshot.cursor_expand.expanded_at;
    assert!(pressed_at.is_finite());
    assert_eq!(d.snapshot.cursor_expand.contracted_at, f64::NEG_INFINITY);
    d.run(5);
    let up = d.key(Z, false);
    d.tick(&[up]);
    assert!(d.snapshot.cursor_expand.contracted_at > pressed_at);

    // A new play has a new cursor: never expanded.
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    assert_eq!(d.snapshot.cursor_expand, ExpandTimes::default());
}

#[test]
fn a_button_released_while_paused_contracts_the_cursor_where_gameplay_stopped() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_to(1500.0);
    let down = d.key(Z, true);
    d.tick(&[down]);
    d.run(5);
    d.tap(ESCAPE);
    let paused_at = d.snapshot.time;
    let up = d.key(Z, false);
    d.tick(&[up]);
    assert_eq!(d.snapshot.cursor_expand.contracted_at, f64::NEG_INFINITY);
    // The cursor never moved: gameplay resumes at once and syncs the release.
    d.tap(ESCAPE);
    d.run(2);
    assert_eq!(d.snapshot.cursor_expand.contracted_at, paused_at);
}

#[test]
fn the_os_cursor_hides_while_the_gameplay_cursor_is_the_one_to_follow() {
    for state in [
        PlayState::Waiting,
        PlayState::Playing,
        PlayState::Resuming,
        PlayState::Failing,
        PlayState::Passed,
    ] {
        assert!(state.hides_os_cursor(), "{state:?}");
    }
    for state in [
        PlayState::Paused,
        PlayState::Failed,
        PlayState::Results,
        PlayState::Exited,
    ] {
        assert!(!state.hides_os_cursor(), "{state:?}");
    }
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_to(1500.0);
    assert!(d.state().hides_os_cursor());
    d.tap(ESCAPE);
    assert!(!d.state().hides_os_cursor());
}

#[test]
fn offset_hotkeys_change_the_beatmap_offset_and_time_at_once() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_to(1500.0);
    assert_eq!(d.snapshot.offset_panel.alpha(d.now), 0.0);
    let before = d.snapshot.time;
    d.run(1);
    let step = d.snapshot.time - before;
    let before = d.snapshot.time;
    d.tap(MINUS);
    assert_eq!(d.session.beatmap_offset(), -1.0);
    assert_eq!(d.snapshot.offset_panel.offset_ms, -1.0);
    // Time never goes back: it stands for the millisecond the offset took.
    d.run(1);
    let moved = d.snapshot.time - before;
    assert!(
        (moved - (2.0 * step - 1.0)).abs() < 0.1,
        "{moved} after steps of {step}"
    );
    d.tap(EQUALS);
    d.tap(EQUALS);
    assert_eq!(d.session.beatmap_offset(), 1.0);
    // The panel shows, then goes 500 ms after the last change.
    d.run(250);
    assert!(d.snapshot.offset_panel.alpha(d.now) > 0.9);
    d.run(1000);
    assert_eq!(d.snapshot.offset_panel.alpha(d.now), 0.0);
}

#[test]
fn the_offset_stays_within_fifty_ms() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    for _ in 0..55 {
        d.tap(EQUALS);
    }
    assert_eq!(d.session.beatmap_offset(), 50.0);
    for _ in 0..105 {
        d.tap(MINUS);
    }
    assert_eq!(d.session.beatmap_offset(), -50.0);
    assert_eq!(d.snapshot.offset_panel.offset_ms, -50.0);
}

#[test]
fn the_offset_is_fixed_while_paused_resuming_or_turned_off() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    let moved = d.mouse(Vec2::new(300.0, 300.0));
    d.tick(&[moved]);
    d.run_to(1500.0);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Paused);
    d.tap(MINUS);
    assert_eq!(d.session.beatmap_offset(), 0.0);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Resuming);
    d.tap(MINUS);
    assert_eq!(d.session.beatmap_offset(), 0.0);
    assert_eq!(d.snapshot.offset_panel.alpha(d.now), 0.0);

    let mut d = Driver::new(&map, ModSet::default());
    d.session.set_offset_adjustable(false);
    d.tap(MINUS);
    assert_eq!(d.session.beatmap_offset(), 0.0);
}

/// The test map with a circle at 20 s and a break before it, effective from 7 s to
/// 18.675 s.
fn long_break() -> Beatmap {
    let mut map =
        Beatmap::decode(format!("{}256,192,20000,1,0\n", common::MAP).as_bytes()).unwrap();
    map.breaks.push(slam_osu::breaks::Break {
        start: 7000.0,
        end: 19000.0,
    });
    map
}

#[test]
fn the_offset_changes_only_in_the_first_ten_seconds_or_in_breaks() {
    let map = long_break();
    let start = slam_osu::lead_in::gameplay_start_time(&map);
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    d.run_to(start + 10_000.0);
    d.tap(MINUS);
    assert_eq!(d.session.beatmap_offset(), -1.0);
    // Past ten seconds, in the break.
    d.run_to(start + 10_100.0);
    assert!(d.snapshot.is_break_time);
    d.tap(MINUS);
    assert_eq!(d.session.beatmap_offset(), -2.0);
    // After the break.
    d.run_to(19_000.0);
    assert!(!d.snapshot.is_break_time);
    d.tap(MINUS);
    assert_eq!(d.session.beatmap_offset(), -2.0);
}

#[test]
fn the_offset_outlives_a_quick_retry_and_its_panel_does_not() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_to(1500.0);
    d.tap(MINUS);
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    assert_eq!(d.session.beatmap_offset(), -1.0);
    assert_eq!(d.snapshot.offset_panel.offset_ms, -1.0);
    assert_eq!(d.snapshot.offset_panel.alpha(d.now), 0.0);
}

#[test]
fn offset_presses_in_one_tick_apply_in_order_and_time_never_goes_back() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_to(1500.0);
    let presses = [
        d.key(Z, true),
        d.key(MINUS, true),
        d.key(MINUS, true),
        d.key(EQUALS, true),
        d.key(Z, false),
    ];
    let mut last = d.snapshot.time;
    d.tick(&presses);
    assert_eq!(d.session.beatmap_offset(), -1.0);
    for _ in 0..5 {
        assert!(d.snapshot.time >= last);
        last = d.snapshot.time;
        d.run(1);
    }
    // Z went to gameplay around the presses: pressed, then released.
    assert!(!d.session.input().held().contains(Actions::LEFT));
}

#[test]
fn the_pause_menu_shows_what_the_play_is_at_and_continues() {
    let map = beatmap();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    assert!(!d.snapshot.menu.visible);
    d.run_to(1500.0);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Paused);
    let menu = d.snapshot.menu;
    assert!(menu.visible);
    assert_eq!(menu.kind, MenuKind::Pause);
    assert_eq!(menu.info.retries, 0);
    let first = map.hit_objects[0].start_time;
    let last = map
        .hit_objects
        .iter()
        .map(|o| o.end_time())
        .fold(f64::NEG_INFINITY, f64::max);
    assert_eq!(
        menu.info.progress,
        song_progress(d.snapshot.time, first, last)
    );
    assert!(menu.info.progress > 0);
    assert_eq!(menu.info.accuracy, d.snapshot.hud.accuracy);

    // Enter with nothing selected does nothing; down selects continue.
    d.tap(ENTER);
    assert_eq!(d.state(), PlayState::Paused);
    d.tap(DOWN);
    d.tap(ENTER);
    // The cursor never moved: gameplay goes on at once.
    assert_eq!(d.state(), PlayState::Playing);
    assert!(!d.snapshot.menu.visible);

    // The middle mouse button continues too (after the pause cooldown).
    d.run(1_000);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Paused);
    let middle = press(
        d.now,
        InputKind::MouseButton {
            button: MouseButton::Middle,
            down: true,
        },
    );
    d.tick(&[middle]);
    assert_eq!(d.state(), PlayState::Playing);
}

#[test]
fn the_pause_menu_retries_without_skipping_and_quits() {
    let map = delayed(29000.0);
    let mut d = Driver::new(&map, ModSet::default());
    d.run(100);
    d.tap(ESCAPE);
    assert_eq!(d.state(), PlayState::Paused);
    // Up starts from the bottom: quit, then retry.
    d.tap(UP);
    d.tap(UP);
    let commands = d.track.commands.len();
    d.tap(SPACE);
    assert_eq!(d.state(), PlayState::Playing);
    assert_eq!(d.snapshot.restarts, 1);
    assert!(!d.snapshot.menu.visible);
    d.run(10);
    // Unlike a quick retry, the menu's retry does not skip the intro.
    assert!(!d.track.commands[commands..].contains(&Command::Seek(27000 * 48)));
    assert!(d.snapshot.time < 1_000.0, "{}", d.snapshot.time);

    d.tap(ESCAPE);
    assert_eq!(d.snapshot.menu.info.retries, 1);
    d.tap(UP);
    d.tap(ENTER);
    assert_eq!(d.state(), PlayState::Exited);
}

#[test]
fn space_in_the_pause_menu_does_not_skip_the_intro() {
    let map = delayed(29000.0);
    let mut d = Driver::new(&map, ModSet::default());
    d.run(100);
    assert!(d.snapshot.skip_available);
    d.tap(ESCAPE);
    d.tap(DOWN);
    let commands = d.track.commands.len();
    d.tap(SPACE);
    assert_eq!(d.state(), PlayState::Playing);
    d.run(10);
    assert!(!d.track.commands[commands..].contains(&Command::Seek(27000 * 48)));
    // Space while playing still skips.
    d.tap(SPACE);
    d.run(5);
    assert!(d.track.commands.contains(&Command::Seek(27000 * 48)));
}

#[test]
fn the_fail_menu_retries_or_quits_with_the_mouse() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_until(10_000, |s| s.state == PlayState::Failing);
    assert!(!d.snapshot.menu.visible, "not during the fail animation");
    d.run_until(3_000, |s| s.state == PlayState::Failed);
    assert!(d.snapshot.menu.visible);
    assert_eq!(d.snapshot.menu.kind, MenuKind::Fail);

    // A click on retry: down and up over the button.
    let over = d.menu_mouse(MenuKind::Fail, 0);
    d.tick(&[over]);
    let left = |d: &Driver, down| {
        press(
            d.now,
            InputKind::MouseButton {
                button: MouseButton::Left,
                down,
            },
        )
    };
    let down = left(&d, true);
    d.tick(&[down]);
    assert_eq!(d.state(), PlayState::Failed);
    let up = left(&d, false);
    d.tick(&[up]);
    assert_eq!(d.state(), PlayState::Playing);
    assert_eq!(d.snapshot.restarts, 1);
    assert!(!d.snapshot.menu.visible);

    // Failed again: a click on quit.
    d.run_until(10_000, |s| s.state == PlayState::Failed);
    assert_eq!(d.snapshot.menu.info.retries, 1);
    let over = d.menu_mouse(MenuKind::Fail, 1);
    let (down, up) = (left(&d, true), left(&d, false));
    d.tick(&[over, down, up]);
    assert_eq!(d.state(), PlayState::Exited);
}

#[test]
fn continue_and_the_resume_click_in_one_tick_go_on_playing() {
    let map = beatmap();
    let mut d = Driver::new(&map, mods(vec![slam_osu::mods::Mod::NoFail]));
    // The cursor stands on continue's place when gameplay pauses.
    let over = d.menu_mouse(MenuKind::Pause, 0);
    d.tick(&[over]);
    d.run_to(1500.0);
    d.tap(ESCAPE);
    d.tap(ENTER);
    assert_eq!(
        d.state(),
        PlayState::Paused,
        "hovering alone selects nothing"
    );
    let mut nudge = d.menu_mouse(MenuKind::Pause, 0);
    if let InputKind::MouseMove { x, .. } = &mut nudge.kind {
        *x += 1.0;
    }
    let left = |d: &Driver, down| {
        press(
            d.now,
            InputKind::MouseButton {
                button: MouseButton::Left,
                down,
            },
        )
    };
    let (down, up) = (left(&d, true), left(&d, false));
    // Continue, then the resume target (where gameplay's cursor stopped) is clicked.
    let click = d.key(Z, true);
    d.tick(&[nudge, down, up, click]);
    assert_eq!(d.state(), PlayState::Playing);
}

#[test]
fn quick_retry_and_quick_exit_take_the_menu_away() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_to(500.0);
    d.tap(ESCAPE);
    assert!(d.snapshot.menu.visible);
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    assert!(!d.snapshot.menu.visible);

    d.run(10);
    d.tap(ESCAPE);
    assert!(d.snapshot.menu.visible);
    d.hold(Some(LCTRL), GRAVE, 200);
    assert_eq!(d.state(), PlayState::Exited);
    assert!(!d.snapshot.menu.visible);
    // Keys after quitting press nothing.
    d.tap(DOWN);
    d.tap(ENTER);
    assert_eq!(d.state(), PlayState::Exited);
}

#[test]
fn the_middle_button_quits_the_fail_menu() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    d.run_until(10_000, |s| s.state == PlayState::Failed);
    let middle = press(
        d.now,
        InputKind::MouseButton {
            button: MouseButton::Middle,
            down: true,
        },
    );
    d.tick(&[middle]);
    assert_eq!(d.state(), PlayState::Exited);
}

#[test]
fn the_menu_is_hit_in_the_units_of_the_input() {
    let map = beatmap();
    let mut d = Driver::new(&map, ModSet::default());
    // Input in points on a window twice the default size.
    d.session.set_layout(PlayfieldLayout::new(1280.0, 960.0));
    d.run_until(10_000, |s| s.state == PlayState::Failed);
    let ([x, y], [w, h]) = MenuLayout::new(1280.0, 960.0, MenuKind::Fail).button(1);
    let over = press(
        d.now,
        InputKind::MouseMove {
            x: x + w / 2.0,
            y: y + h / 2.0,
        },
    );
    let left = |down| {
        press(
            d.now,
            InputKind::MouseButton {
                button: MouseButton::Left,
                down,
            },
        )
    };
    let (down, up) = (left(true), left(false));
    d.tick(&[over, down, up]);
    assert_eq!(d.state(), PlayState::Exited);
}

fn rate_mod(acronym: &str) -> ModSet {
    mods(vec![slam_osu::mods::Mod::from_acronym(acronym)])
}

/// Ticks until the track plays, then checks that game time runs `rate` times as fast as the
/// output from the start time on.
fn assert_runs_at(d: &mut Driver, rate: f64) {
    d.run(2_000);
    let real_ms = (d.now - MS - T0) as f64 / MS as f64;
    let expected = d.session.start_time() + rate * real_ms;
    assert!(
        (d.snapshot.time - expected).abs() < 0.5,
        "{} for {expected}",
        d.snapshot.time
    );
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModRateAdjust.cs (ApplyToTrack)
#[test]
fn rate_mods_set_the_track_tempo_or_frequency_and_the_clock_rate() {
    let map = beatmap();
    // Mod, tempo, frequency, rate: Double Time and Half Time keep the pitch, Nightcore and
    // Daycore take their speed as frequency.
    for (acronym, tempo, freq, rate) in [
        ("DT", 1.5, 1.0, 1.5_f64),
        ("HT", 0.75, 1.0, 0.75),
        ("NC", 1.0, 1.5, 1.5),
        ("DC", 1.0, 0.75, 0.75),
    ] {
        let mut d = Driver::new(&map, rate_mod(acronym));
        assert_eq!(d.track.rates, [tempo], "{acronym}");
        assert_eq!(d.track.adjustments[0], frequency(freq, 0.0), "{acronym}");
        // The lead-in runs at the clock's rate: the track starts when game time reaches 0.
        let frame = (1000.0 / rate * 48.0).round() as u64;
        assert_eq!(
            d.track.commands,
            [Command::Pause, Command::Seek(0), Command::PlayAt(frame)],
            "{acronym}"
        );
        assert_runs_at(&mut d, rate);
    }
}

#[test]
fn a_restart_keeps_the_rate() {
    let map = beatmap();
    let mut d = Driver::new(&map, rate_mod("DT"));
    d.run_to(1_500.0);
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    d.run(5);
    // The tempo stays with the track; the clock keeps its rate.
    assert_eq!(d.track.rates, [1.5]);
    let since_start = d.snapshot.time - d.session.start_time();
    assert!((0.0..10.0).contains(&since_start), "{since_start}");
    let time = d.snapshot.time;
    d.run(100);
    assert!(
        (d.snapshot.time - time - 150.0).abs() < 0.5,
        "{}",
        d.snapshot.time
    );
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (Start)
#[test]
fn failing_with_nightcore_slows_its_frequency_to_a_stop_and_a_restart_brings_it_back() {
    let map = beatmap();
    let mut d = Driver::new(&map, rate_mod("NC"));
    d.run_until(10_000, |s| s.state == PlayState::Failing);
    // The fall from the mods' frequency: lazer multiplies 1 → 0 by it.
    assert_eq!(d.track.adjustments[4], frequency(0.0, 2_500.0));
    let frame = d.track.frame.get();
    let speed = d.track.music(frame).unwrap().anchor.speed_at(frame as f64);
    assert!(speed > 1.4 && speed <= 1.5, "{speed}");
    d.run_until(10_000, |s| s.state == PlayState::Failed);
    d.hold(None, GRAVE, 200);
    assert_eq!(d.snapshot.restarts, 1);
    assert_eq!(d.track.adjustments[12], frequency(1.5, 0.0));
    assert_eq!(d.track.rates, [1.0]);
}

#[test]
fn a_refused_rate_is_set_again_before_the_start() {
    let map = beatmap();
    let track = Track {
        refuse_rate: true,
        ..Track::default()
    };
    let session = session_with_mods(&map, &track, Offsets::default(), rate_mod("DT"));
    let mut states = Vec::new();
    let (session, track, _) = run_with(&map, session, track, MS, T0 + 100 * MS, |s| {
        states.push(s.state);
    });
    assert_eq!(session.clock_errors(), 1);
    assert_eq!(states[11], PlayState::Waiting);
    assert_eq!(states[12], PlayState::Playing);
    // Nothing went to the track on the refused tick: the retry 1 ms later seeks and starts
    // at the new rate.
    assert_eq!(track.rates, [1.5]);
    assert_eq!(
        track.commands,
        [
            Command::Pause,
            Command::Seek(0),
            Command::PlayAt(48 + 32_000)
        ]
    );
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/FramedBeatmapClock.cs (TotalAppliedOffset)
#[test]
fn the_global_offset_scales_with_the_rate() {
    let map = beatmap();
    let offsets = Offsets {
        global_ms: 20.0,
        beatmap_ms: 0.0,
    };
    let track = Track::default();
    let session = session_with_mods(&map, &track, offsets, rate_mod("DT"));
    let mut states = Vec::new();
    let (_, track, _) = run_with(&map, session, track, MS, T0 + 100 * MS, |s| {
        states.push((s.state, s.time));
    });
    assert_eq!(states[11].0, PlayState::Playing);
    // Track time is game time less 20 × 1.5 ms: the track starts 1030 ms of track time
    // after the start, 686.7 ms of real time.
    let frame = (1030.0 / 1.5 * 48.0_f64).round() as u64;
    assert_eq!(track.commands.last(), Some(&Command::PlayAt(frame)));
    assert!(
        (states[11].1 - (-1000.0 + 1.5)).abs() < 0.1,
        "{}",
        states[11].1
    );
}

#[test]
fn the_hit_error_meter_hides_during_a_spinner_and_keeps_its_lines() {
    // The shared map's spinner runs from 3200 to 4700.
    let map = beatmap();
    let mut view = view(&map);
    let textures = FrameTextures {
        meter: Some(slam_play::hit_error::MeterTextures {
            white: WHITE,
            disc: WHITE,
            gradient: WHITE,
            triangle: WHITE,
        }),
        ..FrameTextures::default()
    };
    let mut snapshot = PlaySnapshot::default();
    snapshot.hit_errors.push(slam_play::hit_error::HitError {
        time: 3000.0,
        offset: 10.0,
        result: HitResult::Great,
    });
    let mut batch = SpriteBatch::with_capacity(slam_play::hit_error::MAX_HIT_ERROR_SPRITES);
    let mut draw = |time: f64| {
        snapshot.time = time;
        batch.clear();
        view.push_hit_error(&mut batch, 1280.0, 720.0, &snapshot, 0, &textures);
        let alpha = batch
            .vertices()
            .iter()
            .map(|v| v.colour[3])
            .fold(0.0_f32, f32::max);
        (batch.len(), alpha)
    };
    let (before, before_alpha) = draw(3100.0);
    assert!(before > 0);
    assert_eq!(draw(3500.0).0, 0, "hidden during the spinner");
    let (fading, fading_alpha) = draw(4700.0 + 100.0);
    assert_eq!(fading, before, "the same sprites, the line included");
    assert!((fading_alpha - before_alpha / 2.0).abs() < 1e-3);
    assert_eq!(draw(5000.0), (before, before_alpha));
}
