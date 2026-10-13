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
        assert_eq!(params.gain, 1.0);
        assert_eq!(params.rate, 1.0);
        let voice = self.plays;
        self.plays += 1;
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
            Command::Balance { .. } => false,
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
    assert!(recorder.commands.len() < recorder.commands.capacity());
}
