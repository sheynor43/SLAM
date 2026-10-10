//! The headless replay simulator: step times (ADR-0028), input delivery and determinism.

mod common;

use common::{PressRules, beatmap, noisy_frames, replay};
use slam_formats::osr::GeneratedFrame;
use slam_formats::osu::Vec2;
use slam_osu::judgement::{HitResult, JudgementLog};
use slam_osu::replay::{
    ActionEvent, Actions, InputFrame, NoRules, ReplayInput, Rules, SIXTY_FRAME_TIME, Simulator,
    StepInput, gameplay_start_time,
};

/// An owned copy of one [`StepInput`].
#[derive(Debug, Clone, PartialEq)]
struct Step {
    time: f64,
    position: Vec2,
    held: Actions,
    events: Vec<ActionEvent>,
}

impl Step {
    /// The actions pressed (`true`) or released (`false`) at this step.
    fn changed(&self, pressed: bool) -> Actions {
        self.events
            .iter()
            .filter(|e| e.pressed == pressed)
            .fold(Actions::NONE, |a, e| a | e.action)
    }
}

/// Records every step and finishes once `end` is reached.
struct Recorder {
    steps: Vec<Step>,
    end: f64,
}

impl Rules for Recorder {
    fn max_judgements(&self) -> usize {
        0
    }

    fn update(&mut self, step: &StepInput<'_>, _log: &mut JudgementLog) {
        for e in step.events {
            assert_eq!(e.time, step.time);
            assert_eq!(e.position, step.position);
        }
        self.steps.push(Step {
            time: step.time,
            position: step.position,
            held: step.held,
            events: step.events.to_vec(),
        });
    }

    fn is_complete(&self) -> bool {
        self.steps.last().is_some_and(|s| s.time >= self.end)
    }
}

fn frame(time: f64, x: f32, actions: Actions) -> InputFrame {
    InputFrame {
        time,
        position: Vec2::new(x, 0.0),
        actions,
    }
}

fn record(input: ReplayInput, end: f64) -> Vec<Step> {
    let map = beatmap();
    let mut sim = Simulator::new(
        &map,
        input,
        Recorder {
            steps: Vec::new(),
            end,
        },
    );
    sim.run();
    assert!(sim.is_finished());
    assert!(!sim.step());
    sim.rules().steps.clone()
}

#[test]
fn gameplay_starts_two_seconds_before_first_object() {
    // Preempt at AR 9 is 600 ms, less than 2000.
    assert_eq!(gameplay_start_time(&beatmap()), 1000.0);
}

#[test]
fn steps_visit_every_frame_and_cap_at_sixty_fps() {
    let s = SIXTY_FRAME_TIME;
    let input = ReplayInput::new([
        frame(0.0, 0.0, Actions::NONE),
        frame(500.0, 1.0, Actions::NONE),
        frame(1010.0, 2.0, Actions::NONE),
        frame(1020.0, 3.0, Actions::NONE),
        frame(1060.0, 4.0, Actions::NONE),
        frame(1060.0, 5.0, Actions::NONE),
        frame(1100.0, 6.0, Actions::NONE),
    ]);
    let steps = record(input, 1150.0);
    let times: Vec<f64> = steps.iter().map(|s| s.time).collect();
    assert_eq!(
        times,
        [
            // First step: the first frame.
            0.0,
            // Before gameplay start the clock jumps to it, stopping at each frame on the way.
            500.0,
            1000.0,
            // From gameplay start on, at most one sixty-fps frame per step, never past a frame.
            1010.0,
            1020.0,
            1020.0 + s,
            1020.0 + s + s,
            1060.0,
            // Equal frame times still get their own step.
            1060.0,
            1060.0 + s,
            1060.0 + s + s,
            1100.0,
            // After the last frame time keeps going until the rules are done.
            1100.0 + s,
            1100.0 + s + s,
            1100.0 + s + s + s,
        ]
    );
    // The second frame with time 1060 is the current one at its step.
    assert_eq!(steps[8].position, Vec2::new(5.0, 0.0));
}

#[test]
fn input_is_interpolated_and_presses_are_reported_once() {
    let l = Actions::LEFT;
    let r = Actions::RIGHT;
    let input = ReplayInput::new([
        frame(1000.0, 0.0, Actions::NONE),
        frame(1050.0, 100.0, l),
        frame(1060.0, 100.0, l | r),
        frame(1070.0, 100.0, r),
        frame(1080.0, 100.0, Actions::NONE),
    ]);
    let steps = record(input, 1080.0);
    let got: Vec<_> = steps
        .iter()
        .map(|s| (s.time, s.position.x, s.changed(true), s.changed(false)))
        .collect();
    let s = SIXTY_FRAME_TIME;
    let n = Actions::NONE;
    let x = |t: f64| ((t - 1000.0) as f32 / 50.0) * 100.0;
    assert_eq!(
        got,
        [
            (1000.0, 0.0, n, n),
            (1000.0 + s, x(1000.0 + s), n, n),
            (1000.0 + s + s, x(1000.0 + s + s), n, n),
            // 1000 + 3 * (1000 / 60) rounds to exactly 1050: the frame's own step.
            (1050.0, 100.0, l, n),
            (1060.0, 100.0, r, n),
            (1070.0, 100.0, n, l),
            (1080.0, 100.0, n, r),
        ]
    );
}

#[test]
fn replay_frames_are_shifted_for_old_beatmaps() {
    let frames = [
        GeneratedFrame {
            time: 0.0,
            x: 1.0,
            y: 2.0,
            buttons: 0,
        },
        GeneratedFrame {
            time: 100.0,
            x: 3.0,
            y: 4.0,
            buttons: 5,
        },
    ];
    let v14 = ReplayInput::from_replay(&replay(&frames, 14), 14);
    assert_eq!(
        v14.frames(),
        [
            InputFrame {
                time: 0.0,
                position: Vec2::new(1.0, 2.0),
                actions: Actions::NONE,
            },
            InputFrame {
                time: 100.0,
                position: Vec2::new(3.0, 4.0),
                actions: Actions::LEFT,
            },
        ]
    );
    // Lazer adds 24 ms on decode for format versions below 5; the encoder removed it.
    let v4 = ReplayInput::from_replay(&replay(&frames, 4), 4);
    assert_eq!(v4.frames()[1].time, 100.0);
    let v4_on_v14 = ReplayInput::from_replay(&replay(&frames, 4), 14);
    assert_eq!(v4_on_v14.frames()[1].time, 76.0);
}

#[test]
fn rules_judgements_build_statistics() {
    let map = beatmap();
    let l = Actions::LEFT;
    let input = ReplayInput::new([
        InputFrame {
            time: 0.0,
            position: Vec2::new(0.0, 0.0),
            actions: Actions::NONE,
        },
        // Hits the first object in its centre.
        InputFrame {
            time: 3010.0,
            position: Vec2::new(100.0, 100.0),
            actions: l,
        },
        InputFrame {
            time: 3020.0,
            position: Vec2::new(100.0, 100.0),
            actions: Actions::NONE,
        },
        // Hits the second one far from it.
        InputFrame {
            time: 3290.0,
            position: Vec2::new(500.0, 380.0),
            actions: Actions::RIGHT,
        },
        // Nothing for the third one.
        InputFrame {
            time: 3800.0,
            position: Vec2::new(0.0, 0.0),
            actions: Actions::NONE,
        },
    ]);
    let mut sim = Simulator::new(&map, input, PressRules::new(&map));
    sim.run();
    let log = sim.log();
    let results: Vec<(u32, f64, HitResult)> = log
        .events()
        .iter()
        .map(|j| (j.object.index, j.time, j.result))
        .collect();
    assert_eq!(results[0], (0, 3010.0, HitResult::Great));
    assert_eq!(results[1], (1, 3290.0, HitResult::Meh));
    assert_eq!(results[2].0, 2);
    assert_eq!(results[2].2, HitResult::Miss);
    assert!(results[2].1 > 3700.0 && results[2].1 <= 3700.0 + SIXTY_FRAME_TIME);
    let stats = log.statistics();
    assert_eq!(stats.count(HitResult::Great), 1);
    assert_eq!(stats.count(HitResult::Meh), 1);
    assert_eq!(stats.count(HitResult::Miss), 1);
    assert_eq!(stats.max_combo, 2);
    assert_eq!(stats.combo, 0);
}

#[test]
fn no_rules_finish_after_last_object() {
    let map = beatmap();
    let input = ReplayInput::new([frame(0.0, 0.0, Actions::NONE)]);
    let mut sim = Simulator::new(&map, input, NoRules::new(&map));
    sim.run();
    assert!(sim.time() >= 3600.0 && sim.time() < 3600.0 + SIXTY_FRAME_TIME);
    assert!(sim.log().events().is_empty());
}

#[test]
fn empty_replay_starts_at_gameplay_start() {
    let map = beatmap();
    let mut sim = Simulator::new(&map, ReplayInput::default(), NoRules::new(&map));
    assert!(sim.step());
    assert_eq!(sim.time(), 1000.0);
    assert!(sim.step());
    assert_eq!(sim.time(), 1000.0 + SIXTY_FRAME_TIME);
    sim.run();
    assert!(sim.is_finished());
}

#[test]
fn two_runs_are_identical() {
    let map = beatmap();
    for seed in [1, 2, 3, 42] {
        let osr = replay(&noisy_frames(seed), map.format_version);
        let run = || {
            let input = ReplayInput::from_replay(&osr, map.format_version);
            let mut sim = Simulator::new(&map, input, PressRules::new(&map));
            sim.run();
            let events: Vec<(u32, u64, HitResult)> = sim
                .log()
                .events()
                .iter()
                .map(|j| (j.object.index, j.time.to_bits(), j.result))
                .collect();
            (
                events,
                *sim.log().statistics(),
                sim.steps(),
                sim.time().to_bits(),
            )
        };
        let first = run();
        assert_eq!(first.0.len(), 3, "seed {seed}: every object is judged");
        assert_eq!(first, run(), "seed {seed}");
    }
}

#[test]
fn frame_on_gameplay_start_is_not_doubled() {
    let s = SIXTY_FRAME_TIME;
    let input = ReplayInput::new([
        frame(0.0, 0.0, Actions::NONE),
        frame(1000.0, 1.0, Actions::NONE),
        frame(1100.0, 2.0, Actions::NONE),
    ]);
    let times: Vec<f64> = record(input, 1000.0 + s + s)
        .iter()
        .map(|s| s.time)
        .collect();
    assert_eq!(times, [0.0, 1000.0, 1000.0 + s, 1000.0 + s + s]);
}

#[test]
fn simulation_ends_when_rules_finish_even_with_frames_left() {
    let input = ReplayInput::new([
        frame(0.0, 0.0, Actions::NONE),
        frame(1000.0, 0.0, Actions::NONE),
        // A corrupt replay can place a frame absurdly late; it must not be played out.
        frame(1e12, 0.0, Actions::NONE),
    ]);
    let steps = record(input, 1050.0);
    let last = steps.last().unwrap().time;
    assert!((1050.0..1050.0 + SIXTY_FRAME_TIME).contains(&last));
}

#[test]
fn empty_replay_on_empty_beatmap_finishes() {
    let map = slam_osu::Beatmap::from_file(
        slam_formats::osu::decode_str("osu file format v14\n\n[HitObjects]\n")
            .unwrap()
            .beatmap,
    )
    .unwrap();
    assert_eq!(gameplay_start_time(&map), 0.0);
    let mut sim = Simulator::new(&map, ReplayInput::default(), NoRules::new(&map));
    sim.run();
    assert!(sim.is_finished());
    assert_eq!(sim.steps(), 1);
    assert_eq!(sim.time(), 0.0);
}

#[test]
fn both_buttons_in_one_frame_are_separate_presses() {
    // Two objects 300 ms apart: with a 100 ms window only the first can be hit at 3000, so
    // the second press of the same step finds no object in range and is ignored.
    let map = beatmap();
    let both = Actions::LEFT | Actions::RIGHT;
    let input = ReplayInput::new([
        InputFrame {
            time: 0.0,
            position: Vec2::ZERO,
            actions: Actions::NONE,
        },
        InputFrame {
            time: 3000.0,
            position: Vec2::new(100.0, 100.0),
            actions: both,
        },
        InputFrame {
            time: 3010.0,
            position: Vec2::new(100.0, 100.0),
            actions: Actions::NONE,
        },
    ]);
    let mut sim = Simulator::new(&map, input, PressRules::new(&map));
    sim.run();
    let events = sim.log().events();
    assert_eq!(events[0].object.index, 0);
    assert_eq!(events[0].result, HitResult::Great);
    assert_eq!(events[1].result, HitResult::Miss);
    assert!(events[1].time > 3400.0);
}
