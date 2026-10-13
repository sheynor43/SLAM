//! Gameplay sounds of a scripted play: the mixer commands each update queues.

mod common;

use std::sync::Arc;

use common::skin::hitsounds;
use slam_audio::{PlayError, Sample, VoiceId, VoiceParams};
use slam_osu::judgement::HitResult;
use slam_osu::replay::{Actions, InputFrame, ReplayInput, Rules, Simulator};
use slam_osu::rules::OsuRules;
use slam_osu::{Beatmap, Mod, ModSet, Vec2};
use slam_play::gameplay_sounds::{GameplaySounds, SoundSettings, VoiceSink, sample_balance};
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

/// A circle at x 64; a slider from x 64 to x 448 (one span of 1000 ms, a tick in the middle);
/// a circle at x 448 that is never hit, so that the play goes on after the slider.
const MAP: &str = "osu file format v14

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:5
ApproachRate:9
SliderMultiplier:1.92
SliderTickRate:1

[TimingPoints]
0,500,4,1,0,100,1,0

[HitObjects]
64,192,1000,1,0
64,192,2000,2,0,L|448:192,1,384
448,192,4000,1,0
";

const SAMPLES: [&str; 3] = [
    "normal-hitnormal.wav",
    "normal-sliderslide.wav",
    "normal-slidertick.wav",
];

const SLIDER_START: f64 = 2000.0;
const SLIDER_END: f64 = 3000.0;

/// One command a [`Recorder`] received, at the time of the update that queued it.
#[derive(Debug, Clone, PartialEq)]
enum Command {
    Play {
        time: f64,
        sample: &'static str,
        voice: u64,
        balance: f32,
        looping: bool,
    },
    Stop {
        time: f64,
        voice: u64,
    },
    Balance {
        time: f64,
        voice: u64,
        balance: f32,
    },
    /// A `spinnerspin` start, with the gain and rate other samples always have at 1.
    Spin {
        time: f64,
        voice: u64,
        gain: f32,
        rate: f64,
    },
    Gain {
        time: f64,
        voice: u64,
        gain: f32,
        ramp_ms: f64,
    },
    Rate {
        time: f64,
        voice: u64,
        rate: f64,
    },
}

/// Records commands with the sample names they play.
struct Recorder {
    samples: Vec<(&'static str, Arc<Sample>)>,
    time: f64,
    plays: u64,
    commands: Vec<Command>,
    /// Times at which every command fails with a full queue, `start..end`.
    failing: Vec<(f64, f64)>,
}

impl Recorder {
    fn new(samples: Vec<(&'static str, Arc<Sample>)>, capacity: usize) -> Recorder {
        Recorder {
            samples,
            time: f64::NEG_INFINITY,
            plays: 0,
            commands: Vec::with_capacity(capacity),
            failing: Vec::new(),
        }
    }

    fn check(&self) -> Result<(), PlayError> {
        if self
            .failing
            .iter()
            .any(|&(start, end)| (start..end).contains(&self.time))
        {
            Err(PlayError::QueueFull)
        } else {
            Ok(())
        }
    }
}

impl VoiceSink for Recorder {
    fn play_voice(
        &mut self,
        sample: &Arc<Sample>,
        params: VoiceParams,
    ) -> Result<VoiceId, PlayError> {
        self.check()?;
        let name = self
            .samples
            .iter()
            .find(|(_, s)| Arc::ptr_eq(s, sample))
            .map(|&(name, _)| name)
            .expect("a known sample");
        let voice = self.plays;
        self.plays += 1;
        if name.ends_with("spinnerspin.wav") {
            assert!(params.looping);
            assert_eq!(params.balance, 0.0);
            self.commands.push(Command::Spin {
                time: self.time,
                voice,
                gain: params.gain,
                rate: params.rate,
            });
            return Ok(VoiceId::from_play(voice));
        }
        assert_eq!(params.gain, 1.0);
        assert_eq!(params.rate, 1.0);
        self.commands.push(Command::Play {
            time: self.time,
            sample: name,
            voice,
            balance: params.balance,
            looping: params.looping,
        });
        Ok(VoiceId::from_play(voice))
    }

    fn stop_voice(&mut self, voice: VoiceId) -> Result<(), PlayError> {
        self.check()?;
        self.commands.push(Command::Stop {
            time: self.time,
            voice: voice.play().unwrap(),
        });
        Ok(())
    }

    fn set_voice_balance(&mut self, voice: VoiceId, balance: f32) -> Result<(), PlayError> {
        self.check()?;
        self.commands.push(Command::Balance {
            time: self.time,
            voice: voice.play().unwrap(),
            balance,
        });
        Ok(())
    }

    fn set_voice_gain(&mut self, voice: VoiceId, gain: f32, ramp_ms: f64) -> Result<(), PlayError> {
        self.check()?;
        self.commands.push(Command::Gain {
            time: self.time,
            voice: voice.play().unwrap(),
            gain,
            ramp_ms,
        });
        Ok(())
    }

    fn set_voice_rate(&mut self, voice: VoiceId, rate: f64) -> Result<(), PlayError> {
        self.check()?;
        self.commands.push(Command::Rate {
            time: self.time,
            voice: voice.play().unwrap(),
            rate,
        });
        Ok(())
    }
}

fn beatmap() -> Beatmap {
    Beatmap::decode(MAP.as_bytes()).unwrap()
}

/// The ball's x at `time`.
fn ball_x(time: f64) -> f32 {
    let progress = ((time - SLIDER_START) / (SLIDER_END - SLIDER_START)).clamp(0.0, 1.0);
    64.0 + 384.0 * progress as f32
}

/// Frames every millisecond to 4300 ms, following the objects, with `held(time)` held.
fn frames(held: impl Fn(f64) -> Actions) -> ReplayInput {
    let frames: Vec<InputFrame> = (0..=4300)
        .map(|t| {
            let time = f64::from(t);
            InputFrame {
                time,
                position: Vec2::new(ball_x(time), 192.0),
                actions: held(time),
            }
        })
        .collect();
    ReplayInput::new(frames)
}

/// Hits the first circle, holds the slider from its start to `release`.
fn holding_slider_until(release: f64) -> impl Fn(f64) -> Actions {
    move |time| {
        if (1000.0..1050.0).contains(&time) || (SLIDER_START..release).contains(&time) {
            Actions::LEFT
        } else {
            Actions::NONE
        }
    }
}

/// A judgement result by object and nested index.
type Judged = (u32, Option<u32>, HitResult);

/// The commands of a whole play, and its judgement results.
fn run(input: ReplayInput, mods: &ModSet) -> (Vec<Command>, Vec<Judged>) {
    let (commands, results, failed) = run_failing(input, mods, &[]);
    assert_eq!(failed, 0);
    (commands, results)
}

/// [`run`] with every command failing in the `failing` time ranges; also returns how many
/// commands failed.
fn run_failing(
    input: ReplayInput,
    mods: &ModSet,
    failing: &[(f64, f64)],
) -> (Vec<Command>, Vec<Judged>, u64) {
    let map = beatmap();
    let (sounds, samples) = hitsounds(&map, &SAMPLES);
    let mut sounds = GameplaySounds::new(&map, sounds, SoundSettings::for_mods(mods));
    let mut recorder = Recorder::new(samples, 0);
    recorder.failing = failing.to_vec();
    let mut sim = Simulator::new(&map, input, OsuRules::new(&map, mods));
    while sim.step() {
        recorder.time = sim.time();
        sounds.update(sim.time(), sim.log(), sim.rules(), &mut recorder);
    }
    assert!(sim.is_finished());
    assert!(sounds.stop_all(&mut recorder));
    let results = sim
        .log()
        .events()
        .iter()
        .map(|j| (j.object.index, j.object.nested, j.result))
        .collect();
    (recorder.commands, results, sounds.failed_commands())
}

fn play(time: f64, sample: &'static str, voice: u64, balance: f32) -> Command {
    Command::Play {
        time,
        sample,
        voice,
        balance,
        looping: false,
    }
}

fn slide(time: f64, voice: u64, balance: f32) -> Command {
    Command::Play {
        time,
        sample: "normal-sliderslide.wav",
        voice,
        balance,
        looping: true,
    }
}

/// The commands other than balance changes.
fn without_balance(commands: &[Command]) -> Vec<Command> {
    commands
        .iter()
        .filter(|c| !matches!(c, Command::Balance { .. }))
        .cloned()
        .collect()
}

/// The balance changes of `voice`, in order.
fn balances(commands: &[Command], voice: u64) -> Vec<(f64, f32)> {
    commands
        .iter()
        .filter_map(|c| match *c {
            Command::Balance {
                time,
                voice: v,
                balance,
            } if v == voice => Some((time, balance)),
            _ => None,
        })
        .collect()
}

/// Balance at x with lazer's default level.
fn at(x: f32) -> f32 {
    sample_balance(0.2, x / 512.0)
}

fn classic() -> ModSet {
    ModSet::new(vec![Mod::from_acronym("CL")]).unwrap()
}

#[test]
fn hits_play_in_their_update_and_the_slider_slides_with_its_ball() {
    let (commands, _) = run(
        frames(holding_slider_until(SLIDER_END + 100.0)),
        &ModSet::default(),
    );
    assert_eq!(at(64.0), -0.15);
    assert_eq!(
        without_balance(&commands),
        [
            play(1000.0, "normal-hitnormal.wav", 0, at(64.0)),
            // The head, then the sliding sample from the ball's place after the previous
            // update.
            play(SLIDER_START, "normal-hitnormal.wav", 1, at(64.0)),
            slide(SLIDER_START, 2, at(64.0)),
            play(2500.0, "normal-slidertick.wav", 3, at(256.0)),
            // The tail samples, from the middle of the slider's body.
            play(SLIDER_END, "normal-hitnormal.wav", 4, at(256.0)),
            // Tracking ends after the end time once the slider is judged; the slider drawable
            // sees it an update later.
            Command::Stop {
                time: SLIDER_END + 2.0,
                voice: 2,
            },
        ]
    );

    // The balance follows the ball one update late, changing only when it rounds differently.
    let changes = balances(&commands, 2);
    assert_eq!(changes.len(), 30, "{changes:?}");
    let mut previous = at(64.0);
    for &(time, balance) in &changes {
        assert_eq!(balance, at(ball_x(time - 1.0)), "at {time}");
        assert!(balance > previous);
        previous = balance;
    }
    assert_eq!(previous, at(448.0));
}

#[test]
fn misses_are_silent() {
    let (commands, results) = run(frames(|_| Actions::NONE), &ModSet::default());
    assert!(results.iter().all(|&(_, _, r)| !r.is_hit()), "{results:?}");
    assert_eq!(commands, []);
}

#[test]
fn the_tail_sample_needs_the_tail_unless_classic() {
    // Released before the tail is judged: the slider is hit, its tail missed.
    let input = frames(holding_slider_until(2900.0));

    let (commands, results) = run(input.clone(), &ModSet::default());
    assert!(
        results.contains(&(1, None, HitResult::IgnoreHit)),
        "{results:?}"
    );
    let plays: Vec<_> = without_balance(&commands);
    assert_eq!(
        plays.last(),
        Some(&Command::Stop {
            time: 2901.0,
            voice: 2
        })
    );

    let (commands, results) = run(input, &classic());
    assert!(
        results
            .iter()
            .any(|&(i, n, r)| i == 1 && n.is_none() && r.is_hit()),
        "{results:?}"
    );
    assert_eq!(
        without_balance(&commands).last(),
        Some(&play(SLIDER_END, "normal-hitnormal.wav", 4, at(256.0)))
    );
}

#[test]
fn losing_tracking_stops_the_sliding_sample_until_it_is_regained() {
    let held = |time: f64| {
        if (1000.0..1050.0).contains(&time)
            || (SLIDER_START..2300.0).contains(&time)
            || (2400.0..SLIDER_END + 100.0).contains(&time)
        {
            Actions::LEFT
        } else {
            Actions::NONE
        }
    };
    let (commands, _) = run(frames(held), &ModSet::default());
    let sliding: Vec<_> = without_balance(&commands)
        .into_iter()
        .filter(|c| match c {
            Command::Play { sample, .. } => *sample == "normal-sliderslide.wav",
            Command::Stop { .. } => true,
            _ => false,
        })
        .collect();
    assert_eq!(
        sliding,
        [
            slide(SLIDER_START, 2, at(64.0)),
            // The drawable sees tracking change an update late, except on a head press.
            Command::Stop {
                time: 2301.0,
                voice: 2
            },
            slide(2401.0, 3, at(ball_x(2400.0))),
            Command::Stop {
                time: SLIDER_END + 2.0,
                voice: 3
            },
        ]
    );
    // No balance changes while stopped.
    assert!(
        balances(&commands, 2)
            .iter()
            .all(|&(time, _)| time < 2301.0)
    );
}

#[test]
fn an_early_head_hit_waits_for_the_start_time_to_slide() {
    let held = |time: f64| {
        if (1000.0..1050.0).contains(&time) || (1970.0..SLIDER_END + 100.0).contains(&time) {
            Actions::LEFT
        } else {
            Actions::NONE
        }
    };
    let (commands, _) = run(frames(held), &ModSet::default());
    let plays = without_balance(&commands);
    assert_eq!(plays[1], play(1970.0, "normal-hitnormal.wav", 1, at(64.0)));
    assert_eq!(plays[2], slide(SLIDER_START, 2, at(64.0)));
}

#[test]
fn failed_commands_are_dropped_except_looping_starts_and_stops() {
    // Commands fail around the head and the release of the key at 2600.
    let failing = [(SLIDER_START, 2003.0), (2601.0, 2605.0)];
    let (commands, _, failed) = run_failing(
        frames(holding_slider_until(2600.0)),
        &ModSet::default(),
        &failing,
    );
    assert_eq!(
        without_balance(&commands),
        [
            play(1000.0, "normal-hitnormal.wav", 0, at(64.0)),
            // The head sample is dropped; the sliding sample starts once commands go through.
            slide(2003.0, 1, at(ball_x(2002.0))),
            play(2500.0, "normal-slidertick.wav", 2, at(256.0)),
            // The stop is tried again until it goes through.
            Command::Stop {
                time: 2605.0,
                voice: 1
            },
        ]
    );
    // The head, three sliding starts, one stop (retries do not count).
    assert_eq!(failed, 5);
}

#[test]
fn stop_all_reports_stops_still_owed() {
    let map = beatmap();
    let (sounds, samples) = hitsounds(&map, &SAMPLES);
    let mut sounds = GameplaySounds::new(&map, sounds, SoundSettings::for_mods(&ModSet::default()));
    let mut recorder = Recorder::new(samples, 0);
    let input = frames(holding_slider_until(SLIDER_END + 100.0));
    let mut sim = Simulator::new(&map, input, OsuRules::new(&map, &ModSet::default()));
    // Gameplay ends mid-slide with the queue full.
    while sim.time() < 2500.0 && sim.step() {
        recorder.time = sim.time();
        sounds.update(sim.time(), sim.log(), sim.rules(), &mut recorder);
    }
    recorder.failing = vec![(2500.0, 2501.0)];
    assert!(!sounds.stop_all(&mut recorder));
    recorder.time = 2501.0;
    assert!(sounds.stop_all(&mut recorder));
    assert_eq!(
        recorder.commands.last(),
        Some(&Command::Stop {
            time: 2501.0,
            voice: 2
        })
    );
}

#[test]
fn updates_do_not_allocate() {
    let map = common::beatmap();
    // The scripted map's timing point uses the soft bank.
    let names = [
        "soft-hitnormal.wav",
        "soft-sliderslide.wav",
        "soft-slidertick.wav",
        "soft-spinnerbonus.wav",
        "soft-spinnerspin.wav",
    ];
    let (sounds, samples) = hitsounds(&map, &names);
    let mut sounds = GameplaySounds::new(&map, sounds, SoundSettings::for_mods(&ModSet::default()));
    let mut recorder = Recorder::new(samples, 100_000);
    let play = common::play(&map, 3);
    let mut sim = Simulator::new(&map, play.frames, OsuRules::new(&map, &ModSet::default()));
    assert_no_alloc(|| {
        while sim.step() {
            recorder.time = sim.time();
            sounds.update(sim.time(), sim.log(), sim.rules(), &mut recorder);
        }
        assert!(sounds.stop_all(&mut recorder));
    });
    assert!(sim.rules().is_complete());
    assert!(
        recorder
            .commands
            .iter()
            .any(|c| matches!(c, Command::Play { looping: true, .. }))
    );
    assert!(
        recorder
            .commands
            .iter()
            .any(|c| matches!(c, Command::Spin { .. }))
    );
    assert!(recorder.commands.len() < recorder.commands.capacity());
}

// ---- Spinners ----

/// A spinner at the centre from 1000 to 3000 ms (five spins required, twelve ticks: the last
/// five bonus ticks), then a circle that is never hit, so that the play goes on.
const SPINNER_MAP: &str = "osu file format v14

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:5
ApproachRate:9
SliderMultiplier:1.92
SliderTickRate:1

[TimingPoints]
0,500,4,1,0,100,1,0

[HitObjects]
256,192,1000,12,0,3000
256,192,4000,1,0
";

const SPINNER_SAMPLES: [&str; 3] = [
    "normal-hitnormal.wav",
    "normal-spinnerspin.wav",
    "normal-spinnerbonus.wav",
];

const SPINNER_START: f64 = 1000.0;
const SPINNER_END: f64 = 3000.0;

/// Frames every millisecond to 4300 ms: the cursor circles the spinner 50 px out at
/// `degrees_per_ms` with the left button held from the spinner's start to `stop`, and rests
/// after.
fn spinning_until(stop: f64, degrees_per_ms: f32) -> ReplayInput {
    let frames: Vec<InputFrame> = (0..=4300)
        .map(|t| {
            let time = f64::from(t);
            let spun: f32 =
                (time.clamp(SPINNER_START, stop) - SPINNER_START) as f32 * degrees_per_ms;
            let angle = spun.to_radians();
            InputFrame {
                time,
                position: Vec2::new(256.0 + 50.0 * angle.sin(), 192.0 - 50.0 * angle.cos()),
                actions: if (SPINNER_START..stop).contains(&time) {
                    Actions::LEFT
                } else {
                    Actions::NONE
                },
            }
        })
        .collect();
    ReplayInput::new(frames)
}

/// The commands of a play of the spinner map with `settings`, updating through `pause_at` and
/// then pausing (`stop_all`) before the next update; every command fails in `failing`.
fn run_spinner(
    input: ReplayInput,
    settings: SoundSettings,
    mods: &ModSet,
    pause_at: Option<f64>,
    failing: &[(f64, f64)],
) -> (Vec<Command>, u64) {
    let map = Beatmap::decode(SPINNER_MAP.as_bytes()).unwrap();
    let (sounds, samples) = hitsounds(&map, &SPINNER_SAMPLES);
    let mut sounds = GameplaySounds::new(&map, sounds, settings);
    let mut recorder = Recorder::new(samples, 0);
    recorder.failing = failing.to_vec();
    let mut sim = Simulator::new(&map, input, OsuRules::new(&map, mods));
    while sim.step() {
        recorder.time = sim.time();
        sounds.update(sim.time(), sim.log(), sim.rules(), &mut recorder);
        if pause_at == Some(sim.time()) {
            assert!(sounds.stop_all(&mut recorder));
        }
    }
    assert!(sounds.stop_all(&mut recorder));
    (recorder.commands, sounds.failed_commands())
}

fn spinner_run(input: ReplayInput) -> Vec<Command> {
    let (commands, failed) = run_spinner(
        input,
        SoundSettings::for_mods(&ModSet::default()),
        &ModSet::default(),
        None,
        &[],
    );
    assert_eq!(failed, 0);
    commands
}

/// The commands other than rate changes.
fn without_rates(commands: &[Command]) -> Vec<Command> {
    commands
        .iter()
        .filter(|c| !matches!(c, Command::Rate { .. }))
        .cloned()
        .collect()
}

/// The rates `voice` was set to, in order.
fn rates(commands: &[Command], voice: u64) -> Vec<f64> {
    commands
        .iter()
        .filter_map(|c| match *c {
            Command::Rate { voice: v, rate, .. } if v == voice => Some(rate),
            _ => None,
        })
        .collect()
}

/// The time of the first command matching `f`.
fn time_of(commands: &[Command], f: impl Fn(&Command) -> bool) -> f64 {
    match *commands.iter().find(|&c| f(c)).expect("a matching command") {
        Command::Play { time, .. }
        | Command::Stop { time, .. }
        | Command::Balance { time, .. }
        | Command::Spin { time, .. }
        | Command::Gain { time, .. }
        | Command::Rate { time, .. } => time,
    }
}

const BASE_RATE: f64 = (20_000.0f32 / 44_100.0) as f64;
const MAX_RATE: f64 = (100_000.0f32 / 44_100.0) as f64;

#[test]
fn spinning_fades_in_rises_in_pitch_and_stops_after_the_judgement() {
    // Fifteen and a half spins in 1.55 s (3.6°/ms), then the cursor rests: the twelve ticks
    // take the first twelve spins (the last five play `spinnerbonus`), the other three play
    // `spinnerbonus-max`, which falls back to `spinnerbonus`.
    let commands = spinner_run(spinning_until(2550.0, 3.6));
    let spin_start = time_of(&commands, |c| matches!(c, Command::Spin { .. }));
    assert!(
        (SPINNER_START..SPINNER_START + 10.0).contains(&spin_start),
        "{spin_start}"
    );
    let stopped_spinning = time_of(
        &commands,
        |c| matches!(c, Command::Gain { gain, .. } if *gain == 0.0),
    );
    // At 3.6°/ms the drawn rotation lags about 356° behind; once the cursor rests, the lag
    // shrinks by 1% per millisecond and falls below 10° about 355 ms later.
    assert!(
        (2890.0..2920.0).contains(&stopped_spinning),
        "{stopped_spinning}"
    );

    // The rotation tracker reports spinning once the drawn rotation lags 10° behind, by
    // when the spinner has turned a little: the frequency is a little above its base.
    let Some(&Command::Spin {
        rate: start_rate, ..
    }) = commands.iter().find(|c| matches!(c, Command::Spin { .. }))
    else {
        unreachable!()
    };
    assert!(
        (BASE_RATE..BASE_RATE + 0.01).contains(&start_rate),
        "{start_rate}"
    );
    let mut expected = vec![
        // Silent, at the frequency of the progress before this update, then faded in.
        Command::Spin {
            time: spin_start,
            voice: 0,
            gain: 0.0,
            rate: start_rate,
        },
        Command::Gain {
            time: spin_start,
            voice: 0,
            gain: 1.0,
            ramp_ms: 300.0,
        },
    ];
    let others: Vec<Command> = commands
        .iter()
        .filter(|c| matches!(c, Command::Play { .. } | Command::Stop { .. }))
        .cloned()
        .collect();
    // Five bonus ticks, then three bonus-max spins, each stopping the one before, all at the
    // centre; the spinner's own sample at its judgement; `spinnerspin` stopped 240 ms after.
    let shape: Vec<(&str, f32)> = others
        .iter()
        .map(|c| match *c {
            Command::Play {
                sample, balance, ..
            } => (sample, balance),
            _ => ("stop", 0.0),
        })
        .collect();
    let b = ("normal-spinnerbonus.wav", 0.0);
    let stop = ("stop", 0.0);
    assert_eq!(
        shape,
        [
            b,
            b,
            b,
            b,
            b,
            b,
            stop,
            b,
            stop,
            b,
            ("normal-hitnormal.wav", 0.0),
            stop
        ]
    );
    for k in [6, 8] {
        let (
            Command::Play { voice, .. },
            Command::Stop {
                time,
                voice: stopped,
            },
            Command::Play { time: next, .. },
        ) = (&others[k - 1], &others[k], &others[k + 1])
        else {
            unreachable!()
        };
        assert_eq!((stopped, time), (voice, next));
    }
    assert!(matches!(
        others[10],
        Command::Play {
            time: SPINNER_END,
            ..
        }
    ));
    assert_eq!(
        others[11],
        Command::Stop {
            time: SPINNER_END + 240.0,
            voice: 0
        }
    );

    expected.extend([
        Command::Gain {
            time: stopped_spinning,
            voice: 0,
            gain: 0.0,
            ramp_ms: 240.0,
        },
        // The judgement triggers the spinning state again: the fade-out restarts.
        Command::Gain {
            time: SPINNER_END,
            voice: 0,
            gain: 0.0,
            ramp_ms: 240.0,
        },
    ]);
    let fades: Vec<Command> = commands
        .iter()
        .filter(|c| matches!(c, Command::Spin { .. } | Command::Gain { .. }))
        .cloned()
        .collect();
    assert_eq!(fades, expected);

    // The pitch rises with the progress, a step behind it, to its limit at two required
    // spins' worth of progress past none (ten spins).
    let r = rates(&commands, 0);
    assert!(r.windows(2).all(|w| w[0] < w[1]), "{r:?}");
    assert!(r[0] > BASE_RATE);
    assert_eq!(*r.last().unwrap(), MAX_RATE);
}

#[test]
fn slow_spinning_plays_no_spinning_sample() {
    // 0.03°/ms: the tracker never reports spinning.
    let commands = spinner_run(spinning_until(SPINNER_END, 0.03));
    assert!(!commands.iter().any(|c| matches!(
        c,
        Command::Spin { .. } | Command::Gain { .. } | Command::Rate { .. }
    )));
}

#[test]
fn spinning_fades_in_real_time_and_keeps_its_pitch_without_modulation() {
    let mods = ModSet::new(vec![Mod::from_acronym("DT")]).unwrap();
    let settings = SoundSettings {
        spinner_frequency_modulate: false,
        ..SoundSettings::for_mods(&mods)
    };
    assert_eq!(settings.rate, 1.5);
    let (commands, failed) = run_spinner(spinning_until(2500.0, 3.6), settings, &mods, None, &[]);
    assert_eq!(failed, 0);
    let fades: Vec<(f32, f64)> = commands
        .iter()
        .filter_map(|c| match *c {
            Command::Spin { rate, gain, .. } => {
                assert_eq!(rate, 1.0);
                Some((gain, 0.0))
            }
            Command::Gain { gain, ramp_ms, .. } => Some((gain, ramp_ms)),
            _ => None,
        })
        .collect();
    assert_eq!(
        fades,
        [(0.0, 0.0), (1.0, 200.0), (0.0, 160.0), (0.0, 160.0)]
    );
    assert!(rates(&commands, 0).is_empty());
}

#[test]
fn a_paused_spinner_starts_its_spinning_sample_again() {
    let (commands, failed) = run_spinner(
        spinning_until(2500.0, 3.6),
        SoundSettings::for_mods(&ModSet::default()),
        &ModSet::default(),
        Some(1500.0),
        &[],
    );
    assert_eq!(failed, 0);
    let around: Vec<Command> = without_rates(&commands)
        .into_iter()
        .filter(|c| !matches!(c, Command::Play { .. }))
        .skip(2)
        .take(3)
        .collect();
    let restart = time_of(
        &commands,
        |c| matches!(c, Command::Spin { voice, .. } if *voice > 0),
    );
    assert_eq!(restart, 1501.0);
    let Command::Spin { voice, rate, .. } = around[1] else {
        panic!("{around:?}");
    };
    assert!(rate > BASE_RATE);
    assert_eq!(
        around,
        [
            Command::Stop {
                time: 1500.0,
                voice: 0
            },
            Command::Spin {
                time: 1501.0,
                voice,
                gain: 0.0,
                rate
            },
            Command::Gain {
                time: 1501.0,
                voice,
                gain: 1.0,
                ramp_ms: 300.0
            },
        ]
    );
}

#[test]
fn failed_spinning_commands_are_tried_again() {
    let (commands, failed) = run_spinner(
        spinning_until(2500.0, 3.6),
        SoundSettings::for_mods(&ModSet::default()),
        &ModSet::default(),
        None,
        // From the first update on, through the start of spinning; then around the stop.
        &[(0.0, 1020.0), (SPINNER_END + 240.0, SPINNER_END + 243.0)],
    );
    assert!(failed > 0);
    let spin = time_of(&commands, |c| matches!(c, Command::Spin { .. }));
    let fade_in = time_of(&commands, |c| matches!(c, Command::Gain { .. }));
    assert_eq!((spin, fade_in), (1020.0, 1020.0));
    let stop = time_of(&commands, |c| matches!(c, Command::Stop { voice: 0, .. }));
    assert_eq!(stop, SPINNER_END + 243.0);
}
