//! Live input and a replay of the same play give the same steps and the same judgements.
//!
//! The scripted play changes inputs on whole milliseconds and the engine ticks at the replay's
//! step times, so every event lands exactly on a step: this checks the bindings (several inputs
//! on one action against the union a replay frame records), the window-to-playfield mapping and
//! the path into the rules end to end. Event timestamps inside a step, clamping and deferral
//! are covered by the unit tests of `slam_play::input`; the game clock's rate by
//! `clock_time_applies_rate_and_offsets`. With DT the replay is already in track time, so the
//! DT runs check the rules on live steps, not the clock.

mod common;

use common::{Linear, beatmap, layout, ns, play};
use slam_osu::judgement::JudgementLog;
use slam_osu::replay::{ActionEvent, Actions, Rules, Simulator, StepInput};
use slam_osu::rules::OsuRules;
use slam_osu::{Mod, ModSet, Vec2};
use slam_play::{Bindings, LiveInput};

#[derive(Debug, Clone, PartialEq)]
struct Step {
    time: f64,
    position: Vec2,
    held: Actions,
    events: Vec<ActionEvent>,
}

impl Step {
    fn of(step: &StepInput<'_>) -> Step {
        Step {
            time: step.time,
            position: step.position,
            held: step.held,
            events: step.events.to_vec(),
        }
    }
}

/// Rules that record every step they are given.
struct Recorder {
    rules: OsuRules,
    steps: Vec<Step>,
}

impl Rules for Recorder {
    fn max_judgements(&self) -> usize {
        self.rules.max_judgements()
    }

    fn update(&mut self, step: &StepInput<'_>, log: &mut JudgementLog) {
        self.steps.push(Step::of(step));
        self.rules.update(step, log);
    }

    fn is_complete(&self) -> bool {
        self.rules.is_complete()
    }
}

#[test]
fn live_input_judges_like_its_replay() {
    let map = beatmap();
    for acronyms in [&[][..], &["CL"], &["DT"]] {
        let mods = ModSet::new(acronyms.iter().map(|a| Mod::from_acronym(a)).collect()).unwrap();
        for seed in [1, 2, 3, 5, 8] {
            let play = play(&map, seed);
            let recorder = Recorder {
                rules: OsuRules::new(&map, &mods),
                steps: Vec::new(),
            };
            let mut sim = Simulator::new(&map, play.frames, recorder);
            sim.run();
            let replay_steps = &sim.rules().steps;

            // The engine ticks at the replay's step times.
            let mut live = LiveInput::new(Bindings::default(), layout());
            let mut rules = OsuRules::new(&map, &mods);
            let mut log = JudgementLog::with_capacity(rules.max_judgements());
            let mut next = 0;
            for (i, expected) in replay_steps.iter().enumerate() {
                let now = ns(expected.time);
                let start = next;
                while next < play.events.len() && play.events[next].time_ns <= now {
                    next += 1;
                }
                let step = live.step(&play.events[start..next], now, &Linear);
                assert_eq!(
                    Step::of(&step),
                    *expected,
                    "{acronyms:?} seed {seed}, step {i}"
                );
                rules.update(&step, &mut log);
            }

            assert!(
                replay_steps.iter().any(|s| !s.events.is_empty()),
                "{acronyms:?} seed {seed}: no presses"
            );
            assert!(
                !sim.log().events().is_empty(),
                "{acronyms:?} seed {seed}: nothing judged"
            );
            assert_eq!(log.events(), sim.log().events(), "{acronyms:?} seed {seed}");
            assert_eq!(rules.is_complete(), sim.rules().is_complete());
        }
    }
}
