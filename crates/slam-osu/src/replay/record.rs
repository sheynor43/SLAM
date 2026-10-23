//! Records the input of a live play as replay frames, like lazer's `ReplayRecorder`
//! (ADR-0029).

use super::input::{Actions, InputFrame};
use super::sim::StepInput;

/// The rate frames are recorded at while nothing important happens, in frames per second
/// of real time (lazer's `RecordFrameRate`).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/ReplayRecorder.cs (RecordFrameRate)
pub const RECORD_FRAME_RATE: f64 = 60.0;

/// Room for presses and releases per second of real time: 30 taps a second, beyond any
/// player.
const INPUT_FRAMES_PER_SECOND: f64 = 60.0;

/// Room for frames outside the estimate, such as after the last object.
const SPARE_FRAMES: usize = 16_384;

/// Records frames of a live play into a buffer allocated up front.
///
/// [`ReplayRecorder::record_step`] does not allocate: when the buffer is full, the recorder
/// stops and reports [`ReplayRecorder::overflowed`].
#[derive(Debug, Clone)]
pub struct ReplayRecorder {
    frames: Vec<InputFrame>,
    /// The minimum gameplay time between frames that are not important.
    interval: f64,
    /// Actions held after the previous step.
    held: Actions,
    overflowed: bool,
}

impl ReplayRecorder {
    /// A recorder with room for a play spanning `span` milliseconds of gameplay time at the
    /// gameplay clock's `rate`, with up to `judgements` judgements (each may record a frame).
    /// Call it off the hot paths.
    pub fn new(span: f64, rate: f64, judgements: usize) -> ReplayRecorder {
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/ReplayRecorder.cs (RecordFrame)
        let interval = 1000.0 / RECORD_FRAME_RATE * rate;
        let span = span.max(0.0);
        // Regular frames come at most once an interval of gameplay time; presses and
        // releases come in real time, which runs `1 / rate` times as long.
        let (regular, real_seconds) = if rate > 0.0 {
            (span / interval, span / 1000.0 / rate)
        } else {
            (0.0, 0.0)
        };
        let capacity = (regular + real_seconds * INPUT_FRAMES_PER_SECOND).ceil() as usize;
        let capacity = capacity
            .saturating_add(judgements)
            .saturating_add(SPARE_FRAMES);
        ReplayRecorder {
            frames: Vec::with_capacity(capacity),
            interval,
            held: Actions::NONE,
            overflowed: false,
        }
    }

    /// Records one gameplay step: a frame at each press and release, then a frame at the
    /// step's time. That one is important when `judged`: the step made a judgement (lazer
    /// records on every `NewResult`); otherwise it is dropped if the previous frame is closer
    /// than the recording interval.
    pub fn record_step(&mut self, step: &StepInput<'_>, judged: bool) {
        let mut held = self.held;
        for event in step.events {
            held = if event.pressed {
                held | event.action
            } else {
                held.difference(event.action)
            };
            self.record(
                InputFrame {
                    time: event.time,
                    position: event.position,
                    actions: held,
                },
                true,
            );
        }
        self.record(
            InputFrame {
                time: step.time,
                position: step.position,
                actions: step.held,
            },
            judged,
        );
        self.held = step.held;
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/ReplayRecorder.cs (RecordFrame)
    fn record(&mut self, frame: InputFrame, important: bool) {
        let last = self.frames.last().copied();
        if let Some(last) = last
            && !important
            && frame.time - last.time < self.interval
        {
            return;
        }
        // An equivalent last frame (same time, position and actions) is replaced.
        if last == Some(frame) {
            return;
        }
        if self.frames.len() == self.frames.capacity() {
            self.overflowed = true;
            return;
        }
        self.frames.push(frame);
    }

    /// The frames recorded, in order.
    pub fn frames(&self) -> &[InputFrame] {
        &self.frames
    }

    /// Whether frames were dropped because the buffer was full: the recording is incomplete.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Forgets the frames recorded (a restart), keeping the buffer.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.held = Actions::NONE;
        self.overflowed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::input::ActionEvent;
    use slam_formats::osu::Vec2;

    fn step(time: f64, x: f32, held: Actions, events: &[ActionEvent]) -> StepInput<'_> {
        StepInput {
            time,
            position: Vec2::new(x, 0.0),
            held,
            events,
        }
    }

    fn event(time: f64, x: f32, action: Actions, pressed: bool) -> ActionEvent {
        ActionEvent {
            time,
            position: Vec2::new(x, 0.0),
            action,
            pressed,
        }
    }

    fn times(r: &ReplayRecorder) -> Vec<f64> {
        r.frames().iter().map(|f| f.time).collect()
    }

    #[test]
    fn regular_frames_are_limited_to_the_record_rate() {
        let mut r = ReplayRecorder::new(1000.0, 1.0, 0);
        for ms in 0..=100 {
            r.record_step(&step(ms as f64, ms as f32, Actions::NONE, &[]), false);
        }
        // 1000 / 60 = 16.67 ms: a frame at 0, then the first step at least that far after
        // the previous frame.
        assert_eq!(times(&r), [0.0, 17.0, 34.0, 51.0, 68.0, 85.0]);
    }

    #[test]
    fn the_interval_scales_with_the_rate() {
        let mut r = ReplayRecorder::new(1000.0, 1.5, 0);
        for ms in 0..=60 {
            r.record_step(&step(ms as f64, ms as f32, Actions::NONE, &[]), false);
        }
        assert_eq!(times(&r), [0.0, 25.0, 50.0]);
    }

    #[test]
    fn presses_releases_and_judgements_are_always_recorded() {
        let mut r = ReplayRecorder::new(1000.0, 1.0, 0);
        r.record_step(&step(0.0, 0.0, Actions::NONE, &[]), false);
        let press = [event(1.5, 1.0, Actions::LEFT, true)];
        r.record_step(&step(2.0, 2.0, Actions::LEFT, &press), false);
        r.record_step(&step(3.0, 3.0, Actions::LEFT, &[]), true);
        r.record_step(&step(4.0, 4.0, Actions::LEFT, &[]), false);
        let release = [event(4.25, 4.0, Actions::LEFT, false)];
        r.record_step(&step(5.0, 5.0, Actions::NONE, &release), false);

        let frames: Vec<(f64, f32, Actions)> = r
            .frames()
            .iter()
            .map(|f| (f.time, f.position.x, f.actions))
            .collect();
        assert_eq!(
            frames,
            [
                (0.0, 0.0, Actions::NONE),
                (1.5, 1.0, Actions::LEFT),
                (3.0, 3.0, Actions::LEFT),
                (4.25, 4.0, Actions::NONE),
            ]
        );
    }

    #[test]
    fn each_event_records_the_actions_held_after_it() {
        let mut r = ReplayRecorder::new(1000.0, 1.0, 0);
        let events = [
            event(1.0, 0.0, Actions::LEFT, true),
            event(1.0, 0.0, Actions::RIGHT, true),
            event(2.0, 0.0, Actions::LEFT, false),
        ];
        r.record_step(&step(3.0, 0.0, Actions::RIGHT, &events), false);
        let actions: Vec<Actions> = r.frames().iter().map(|f| f.actions).collect();
        assert_eq!(
            actions,
            [
                Actions::LEFT,
                Actions::LEFT | Actions::RIGHT,
                Actions::RIGHT
            ]
        );
    }

    #[test]
    fn an_equivalent_frame_is_not_repeated() {
        let mut r = ReplayRecorder::new(1000.0, 1.0, 0);
        r.record_step(&step(0.0, 0.0, Actions::NONE, &[]), true);
        r.record_step(&step(0.0, 0.0, Actions::NONE, &[]), true);
        assert_eq!(times(&r), [0.0]);
        // Same time, another position: a new frame.
        r.record_step(&step(0.0, 1.0, Actions::NONE, &[]), true);
        assert_eq!(times(&r), [0.0, 0.0]);
    }

    #[test]
    fn room_grows_with_slower_rates_and_judgements() {
        let room = |rate: f64, judgements: usize| {
            ReplayRecorder::new(600_000.0, rate, judgements)
                .frames
                .capacity()
        };
        // Ten minutes: 36 000 regular frames and room for 36 000 input frames.
        assert!(room(1.0, 0) >= 72_000);
        // At half speed both last twice as long in real time.
        assert!(room(0.5, 0) >= 144_000);
        assert!(room(1.0, 5_000) >= room(1.0, 0) + 5_000);
    }

    #[test]
    fn a_full_buffer_stops_recording_without_growing() {
        let mut r = ReplayRecorder::new(0.0, 1.0, 0);
        let capacity = r.frames.capacity();
        for ms in 0..capacity + 10 {
            r.record_step(&step(ms as f64, 0.0, Actions::NONE, &[]), true);
        }
        assert_eq!(r.frames().len(), capacity);
        assert_eq!(r.frames.capacity(), capacity);
        assert!(r.overflowed());
        r.clear();
        assert!(r.frames().is_empty());
        assert!(!r.overflowed());
    }
}
