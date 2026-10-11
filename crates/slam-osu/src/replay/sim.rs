//! Headless replay simulation: steps time like lazer's `FrameStabilityContainer` playing a
//! replay while catching up with its clock (ADR-0028), feeds the input to the rules and
//! records their judgements.

use crate::beatmap::Beatmap;
use crate::health::HealthProcessor;
use crate::judgement::JudgementLog;
use crate::mods::ModSet;
use crate::scoring::ScoreProcessor;

use slam_formats::osu::Vec2;

use super::input::{ActionEvent, Actions, MAX_STEP_EVENTS, ReplayCursor, ReplayInput};

/// The longest step: one frame at 60 fps.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/FrameStabilityContainer.cs (applyFrameStability)
pub const SIXTY_FRAME_TIME: f64 = 1000.0 / 60.0;

/// The input of one update of the rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepInput<'a> {
    /// Gameplay time of the update, in milliseconds.
    pub time: f64,
    /// Cursor position at `time`.
    pub position: Vec2,
    /// Actions held at `time`, after all events.
    pub held: Actions,
    /// Presses and releases since the previous update, in order, with times not after `time`.
    /// A replay step delivers them all at the step's time; live input carries each event's
    /// own timestamp.
    pub events: &'a [ActionEvent],
}

/// The object rules, driven by the simulator (and later by the engine's update loop).
///
/// [`Rules::update`] is called once per update. The rules first handle `events` one by one,
/// each seeing the objects as the previous one left them (lazer triggers every action
/// separately), then update their objects to `time`, as lazer processes input before updating
/// the playfield.
pub trait Rules {
    /// The most judgements the rules can produce for the whole play; the log is allocated with
    /// this capacity.
    fn max_judgements(&self) -> usize;

    /// Processes one update. Must not allocate, and must not record more judgements than
    /// [`Rules::max_judgements`] over the play.
    fn update(&mut self, step: &StepInput<'_>, log: &mut JudgementLog);

    /// Whether every object is judged.
    fn is_complete(&self) -> bool;
}

/// Rules without object rules: judge nothing and finish once the last object has ended.
#[derive(Debug, Clone, PartialEq)]
pub struct NoRules {
    end_time: f64,
    time: f64,
}

impl NoRules {
    /// Rules for this beatmap.
    pub fn new(beatmap: &Beatmap) -> NoRules {
        let end_time = beatmap
            .hit_objects
            .iter()
            .map(|o| o.end_time())
            .fold(f64::NEG_INFINITY, f64::max);
        NoRules {
            end_time,
            time: f64::NEG_INFINITY,
        }
    }
}

impl Rules for NoRules {
    fn max_judgements(&self) -> usize {
        0
    }

    fn update(&mut self, step: &StepInput<'_>, _log: &mut JudgementLog) {
        self.time = step.time;
    }

    fn is_complete(&self) -> bool {
        self.time >= self.end_time
    }
}

/// The time from which gameplay steps are limited to [`SIXTY_FRAME_TIME`]: the first object's
/// start minus `max(2000, preempt)`, or 0 for an empty beatmap.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/DrawableOsuRuleset.cs (GameplayStartTime)
pub fn gameplay_start_time(beatmap: &Beatmap) -> f64 {
    match beatmap.hit_objects.first() {
        Some(first) => first.start_time - first.defaults.time_preempt.max(2000.0),
        None => 0.0,
    }
}

/// The health of a simulated play and the mods that may prevent failing.
#[derive(Debug, Clone)]
struct Health {
    processor: HealthProcessor,
    mods: ModSet,
}

/// Runs a replay through the rules.
///
/// Everything is allocated by [`Simulator::new`]; [`Simulator::step`] does not allocate as long
/// as the rules do not.
#[derive(Debug, Clone)]
pub struct Simulator<R> {
    input: ReplayInput,
    cursor: ReplayCursor,
    gameplay_start_time: f64,
    rules: R,
    log: JudgementLog,
    health: Option<Health>,
    score: Option<ScoreProcessor>,
    /// Judgements of the log already given to the health and score processors.
    applied: usize,
    events: [ActionEvent; MAX_STEP_EVENTS],
    started: bool,
    finished: bool,
    steps: u64,
}

impl<R: Rules> Simulator<R> {
    /// A simulator before its first step.
    pub fn new(beatmap: &Beatmap, input: ReplayInput, rules: R) -> Simulator<R> {
        Simulator {
            gameplay_start_time: gameplay_start_time(beatmap),
            log: JudgementLog::with_capacity(rules.max_judgements()),
            input,
            cursor: ReplayCursor::new(),
            rules,
            health: None,
            score: None,
            applied: 0,
            events: [ActionEvent {
                time: 0.0,
                position: Vec2::ZERO,
                action: Actions::NONE,
                pressed: false,
            }; MAX_STEP_EVENTS],
            started: false,
            finished: false,
            steps: 0,
        }
    }

    /// Tracks health with `processor` from the first step on; `mods` decide whether the player
    /// fails, starting with no Easy lives used ([`ModSet::reset_play_state`]).
    ///
    /// The play is simulated as lazer's `Player` plays it, not as its `ReplayPlayer` replays
    /// it: replaying, lazer does not let a play fail before its last frame. A failed play
    /// finishes at the failing step; lazer concludes it a scheduled frame later, which depends
    /// on real frames.
    pub fn with_health(mut self, processor: HealthProcessor, mut mods: ModSet) -> Simulator<R> {
        debug_assert_eq!(self.steps, 0, "health tracked from the first step");
        mods.reset_play_state();
        self.health = Some(Health { processor, mods });
        self
    }

    /// Scores the play with `processor` from the first step on. With health tracked, the
    /// health processor takes each judgement first, and judgements after a fail score
    /// nothing; a failed play is marked failed (rank F).
    pub fn with_scoring(mut self, processor: ScoreProcessor) -> Simulator<R> {
        debug_assert_eq!(self.steps, 0, "score tracked from the first step");
        self.score = Some(processor);
        self
    }

    /// Advances by one step. Returns `false`, doing nothing, once the simulation is finished:
    /// every object is judged (lazer's `ScoreProcessor.HasCompleted`), whether or not frames
    /// remain, or the player has failed.
    pub fn step(&mut self) -> bool {
        if self.finished {
            return false;
        }

        let current = self.cursor.current_time();

        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/FrameStabilityContainer.cs (applyFrameStability)
        // The reference clock is infinitely far ahead (ADR-0028), so outside the first step and
        // the pre-gameplay jump the proposed time is always one sixty-fps frame ahead.
        let proposed = if !self.started {
            self.started = true;
            if self.input.frames().is_empty() {
                // Lazer's clock would follow the reference clock; with no input there is
                // nothing to seek to, so start at the gameplay start.
                self.gameplay_start_time
            } else {
                f64::INFINITY
            }
        } else if current < self.gameplay_start_time {
            self.gameplay_start_time
        } else {
            current + SIXTY_FRAME_TIME
        };

        let time = self.cursor.set_frame_from_time(&self.input, proposed);
        let state = self.cursor.collect(&self.input);
        let step = StepInput {
            time,
            position: state.position,
            held: state.held,
            events: state.events(time, &mut self.events),
        };
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (LoadComponents)
        // The health processor is a frame-stable component ahead of the playfield: it drains
        // first, then takes the step's judgements as they come.
        if let Some(health) = &mut self.health {
            health.processor.update(time);
        }
        self.rules.update(&step, &mut self.log);
        self.steps += 1;

        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (LoadComponents)
        // Each judgement goes to the health processor, then to the score processor.
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HealthProcessor.cs (ApplyResultInternal)
        // A judgement is `FailedAtJudgement` if the player failed before it.
        for judgement in self.log.events_since(self.applied) {
            let mut failed_before = false;
            if let Some(health) = &mut self.health {
                failed_before = health.processor.has_failed();
                health.processor.apply(judgement, &mut health.mods);
            }
            if let Some(score) = &mut self.score {
                score.apply(judgement, failed_before);
            }
        }
        self.applied = self.log.events().len();
        let failed = self
            .health
            .as_ref()
            .is_some_and(|h| h.processor.has_failed());
        if failed && let Some(score) = &mut self.score {
            score.fail();
        }

        self.finished = failed || self.rules.is_complete();
        true
    }

    /// Steps until the simulation is finished.
    pub fn run(&mut self) {
        while self.step() {}
    }

    /// Whether the simulation is finished.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The time of the last step, `-inf` before the first.
    pub fn time(&self) -> f64 {
        self.cursor.current_time()
    }

    /// The number of steps taken.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// The judgements and statistics so far.
    pub fn log(&self) -> &JudgementLog {
        &self.log
    }

    /// The health, if tracked (see [`Simulator::with_health`]).
    pub fn health(&self) -> Option<&HealthProcessor> {
        self.health.as_ref().map(|h| &h.processor)
    }

    /// The score, if tracked (see [`Simulator::with_scoring`]).
    pub fn score(&self) -> Option<&ScoreProcessor> {
        self.score.as_ref()
    }

    /// The rules.
    pub fn rules(&self) -> &R {
        &self.rules
    }

    /// The replay input.
    pub fn input(&self) -> &ReplayInput {
        &self.input
    }
}
