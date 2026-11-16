//! Input the game makes for the automation mods (ADR-0153): Autoplay plays the frames of
//! [`slam_osu::auto`], Autopilot moves the cursor along them while the player presses, Relax
//! presses while the player moves the cursor.
//!
//! Allocates only when made, off the hot paths; the steps do not allocate.

use slam_osu::replay::{
    ActionEvent, Actions, MAX_STEP_EVENTS, ReplayCursor, ReplayInput, StepInput,
};
use slam_osu::rules::OsuRules;
use slam_osu::{Beatmap, ModSet, Vec2};

use crate::cursor::CursorPath;
use crate::input::MAX_LIVE_STEP_EVENTS;

/// Which automation mod drives the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoMode {
    /// The frames press and move: the player only watches.
    Autoplay,
    /// The frames move the cursor; the player presses.
    Autopilot,
    /// The player moves the cursor; the game presses.
    Relax,
}

/// Relax's button state (lazer's `OsuModRelax` fields).
#[derive(Debug, Clone, Copy, Default)]
struct RelaxState {
    is_down: bool,
    last_change_time: f64,
    /// Lazer's `wasLeft`: the next press uses the left button when set; it starts unset, so
    /// the first press is the right one.
    was_left: bool,
    held: Actions,
}

/// The game's input for a play with an automation mod.
#[derive(Debug, Clone)]
pub struct AutoInput {
    mode: AutoMode,
    frames: ReplayInput,
    cursor: ReplayCursor,
    /// The events of the last step: Autoplay's at most [`MAX_STEP_EVENTS`], Autopilot's the
    /// player's.
    events: [ActionEvent; MAX_LIVE_STEP_EVENTS],
    position: Vec2,
    path: CursorPath,
    relax: RelaxState,
}

impl AutoInput {
    /// The input of `mods`' automation mod on `beatmap` (with `mods` applied), if they have
    /// one. Allocates the frames.
    pub fn for_play(beatmap: &Beatmap, mods: &ModSet) -> Option<AutoInput> {
        let mode = if mods.get("AT").is_some() {
            AutoMode::Autoplay
        } else if mods.get("AP").is_some() {
            AutoMode::Autopilot
        } else if mods.get("RX").is_some() {
            AutoMode::Relax
        } else {
            return None;
        };
        let frames = match mode {
            AutoMode::Relax => ReplayInput::new([]),
            _ => ReplayInput::new(slam_osu::auto::generate(beatmap, mods)),
        };
        let position = frames.frames().first().map_or(Vec2::ZERO, |f| f.position);
        let no_event = ActionEvent {
            time: 0.0,
            position,
            action: Actions::NONE,
            pressed: false,
        };
        Some(AutoInput {
            mode,
            frames,
            cursor: ReplayCursor::new(),
            events: [no_event; MAX_LIVE_STEP_EVENTS],
            position,
            path: CursorPath::default(),
            relax: RelaxState::default(),
        })
    }

    pub fn mode(&self) -> AutoMode {
        self.mode
    }

    /// Whether the game moves the cursor: Autoplay and Autopilot.
    pub fn moves_cursor(&self) -> bool {
        self.mode != AutoMode::Relax
    }

    /// Where the frames put the cursor at the last step, in osu!pixels.
    pub fn position(&self) -> Vec2 {
        self.position
    }

    /// The cursor positions the frames took, newest last.
    pub fn path(&self) -> &CursorPath {
        &self.path
    }

    /// Starts over, for a restart or a seek back.
    pub fn reset(&mut self) {
        self.cursor = ReplayCursor::new();
        self.path = CursorPath::default();
        self.relax = RelaxState::default();
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/FrameStabilityContainer.cs (applyFrameStability)
    /// Autoplay: the next step towards `time`, one frame at a time as lazer's frame-stable
    /// replay playback goes, or `None` once the frames are at `time`. Does not allocate.
    pub fn next_step(&mut self, time: f64) -> Option<StepInput<'_>> {
        // Also stops on a NaN time.
        if self.cursor.current_time().partial_cmp(&time) != Some(std::cmp::Ordering::Less) {
            return None;
        }
        let at = self.cursor.set_frame_from_time(&self.frames, time);
        let state = self.cursor.collect(&self.frames);
        self.moved_to(state.position);
        let buffer: &mut [ActionEvent; MAX_STEP_EVENTS] = (&mut self.events[..MAX_STEP_EVENTS])
            .try_into()
            // The assertion below the type: the buffer is at least that long.
            .expect("MAX_LIVE_STEP_EVENTS is at least MAX_STEP_EVENTS");
        Some(StepInput {
            time: at,
            position: state.position,
            held: state.held,
            events: state.events(at, buffer),
        })
    }

    /// Autopilot: `live` (the player's step) with the cursor where the frames are at its
    /// time. Does not allocate.
    pub fn pilot(&mut self, live: &StepInput<'_>) -> StepInput<'_> {
        // The frames up to the step, without their presses.
        while self.cursor.current_time() < live.time {
            self.cursor.set_frame_from_time(&self.frames, live.time);
        }
        let position = self.cursor.collect(&self.frames).position;
        self.moved_to(position);
        let len = live.events.len().min(MAX_LIVE_STEP_EVENTS);
        for (slot, event) in self.events.iter_mut().zip(&live.events[..len]) {
            *slot = ActionEvent { position, ..*event };
        }
        StepInput {
            time: live.time,
            position,
            held: live.held,
            events: &self.events[..len],
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModRelax.cs (Update, changeState, PressHandler)
    /// Relax: `live` (the player's step) with the presses the game makes by what `rules`
    /// want at its time and cursor; the player's own presses do not count. Does not
    /// allocate.
    pub fn relax(&mut self, live: &StepInput<'_>, rules: &OsuRules) -> StepInput<'_> {
        let time = live.time;
        let wants = rules.relax_wants(time, live.position);
        let before = self.relax.held;
        if wants.hit {
            self.change_relax(false, time);
            self.change_relax(true, time);
        }
        if wants.hold {
            self.change_relax(true, time);
        } else if self.relax.is_down
            && time - self.relax.last_change_time > slam_osu::auto::KEY_UP_DELAY
        {
            self.change_relax(false, time);
        }
        let held = self.relax.held;
        let mut len = 0;
        // Releases before presses, as the input manager delivers them.
        for (actions, pressed) in [
            (before.difference(held), false),
            (held.difference(before), true),
        ] {
            for action in Actions::SINGLE {
                if actions.contains(action) && len < self.events.len() {
                    self.events[len] = ActionEvent {
                        time,
                        position: live.position,
                        action,
                        pressed,
                    };
                    len += 1;
                }
            }
        }
        self.position = live.position;
        StepInput {
            time,
            position: live.position,
            held,
            events: &self.events[..len],
        }
    }

    fn change_relax(&mut self, down: bool, time: f64) {
        let relax = &mut self.relax;
        if relax.is_down == down {
            return;
        }
        relax.is_down = down;
        relax.last_change_time = time;
        // A press holds just its button; a release lets go of everything.
        relax.held = if down {
            let button = if relax.was_left {
                Actions::LEFT
            } else {
                Actions::RIGHT
            };
            relax.was_left = !relax.was_left;
            button
        } else {
            Actions::NONE
        };
    }

    fn moved_to(&mut self, position: Vec2) {
        if position != self.position || self.path.written() == 0 {
            self.path.push(position);
        }
        self.position = position;
    }
}

const _: () = assert!(MAX_LIVE_STEP_EVENTS >= MAX_STEP_EVENTS);

#[cfg(test)]
mod tests {
    use slam_osu::judgement::JudgementLog;
    use slam_osu::replay::Rules as _;

    use super::*;

    const MAP: &str = "osu file format v14

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:8
ApproachRate:9
SliderMultiplier:1.4
SliderTickRate:1

[TimingPoints]
0,300,4,2,0,100,1,0

[HitObjects]
100,100,1000,1,0
300,300,1400,1,0
";

    fn live(time: f64, position: Vec2) -> StepInput<'static> {
        StepInput {
            time,
            position,
            held: Actions::NONE,
            events: &[],
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModRelax.cs (wasLeft)
    #[test]
    fn relax_presses_right_first_then_alternates_and_lets_go_after_the_key_up_delay() {
        let beatmap = Beatmap::decode(MAP.as_bytes()).unwrap();
        let mods = ModSet::new(vec![slam_osu::Mod::Relax]).unwrap();
        let mut auto = AutoInput::for_play(&beatmap, &mods).unwrap();
        assert_eq!(auto.mode(), AutoMode::Relax);
        assert!(!auto.moves_cursor());
        let mut rules = OsuRules::new(&beatmap, &mods);
        let mut log = JudgementLog::with_capacity(rules.max_judgements());
        let first = beatmap.hit_objects[0].stacked_position();
        let second = beatmap.hit_objects[1].stacked_position();
        let mut step = |auto: &mut AutoInput, time: f64, at: Vec2| {
            let step = auto.relax(&live(time, at), &rules);
            let (held, events) = (step.held, step.events.to_vec());
            rules.update(&step, &mut log);
            (held, events)
        };
        // Too early: nothing.
        let (held, events) = step(&mut auto, 900.0, first);
        assert!(held.is_empty() && events.is_empty());
        // 12 ms before the circle: the right button.
        let (held, events) = step(&mut auto, 988.0, first);
        assert_eq!(held, Actions::RIGHT);
        assert_eq!(events.len(), 1);
        assert!(events[0].pressed && events[0].action == Actions::RIGHT);
        // Held for the key-up delay, then let go.
        assert_eq!(step(&mut auto, 1030.0, first).0, Actions::RIGHT);
        let (held, events) = step(&mut auto, 1039.0, first);
        assert!(held.is_empty());
        assert!(!events[0].pressed);
        // The next circle: the left button.
        assert_eq!(step(&mut auto, 1390.0, second).0, Actions::LEFT);
        assert!(log.events().iter().all(|j| j.result.is_hit()));
        assert_eq!(log.events().len(), 2);
    }
}
