//! Replay frames as gameplay input and the cursor that walks them like lazer's
//! `FramedReplayInputHandler`.

use slam_formats::osr::{self, TimedFrame};
use slam_formats::osu::Vec2;

/// A set of osu! actions held at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Actions(u8);

impl Actions {
    /// No action.
    pub const NONE: Actions = Actions(0);
    /// Left button (`OsuAction.LeftButton`).
    pub const LEFT: Actions = Actions(1);
    /// Right button (`OsuAction.RightButton`).
    pub const RIGHT: Actions = Actions(2);
    /// Smoke (`OsuAction.Smoke`): recorded, never judged.
    pub const SMOKE: Actions = Actions(4);
    /// The single actions in lazer's `OsuAction` order, which is the order presses and
    /// releases are delivered in.
    pub const SINGLE: [Actions; 3] = [Actions::LEFT, Actions::RIGHT, Actions::SMOKE];

    /// Converts legacy replay button bits (`ReplayButtonState`).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuReplayFrame.cs (FromLegacy)
    pub fn from_legacy(buttons: i32) -> Actions {
        // ReplayButtonState: Left1 = 1, Right1 = 2, Left2 = 4, Right2 = 8, Smoke = 16.
        let mut a = Actions::NONE;
        if buttons & (1 | 4) != 0 {
            a = a | Actions::LEFT;
        }
        if buttons & (2 | 8) != 0 {
            a = a | Actions::RIGHT;
        }
        if buttons & 16 != 0 {
            a = a | Actions::SMOKE;
        }
        a
    }

    /// Whether no action is held.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every action of `other` is held.
    pub fn contains(self, other: Actions) -> bool {
        self.0 & other.0 == other.0
    }

    /// The actions held here but not in `other`.
    pub fn difference(self, other: Actions) -> Actions {
        Actions(self.0 & !other.0)
    }
}

impl std::ops::BitOr for Actions {
    type Output = Actions;

    fn bitor(self, rhs: Actions) -> Actions {
        Actions(self.0 | rhs.0)
    }
}

/// A replay frame ready for gameplay (`OsuReplayFrame`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputFrame {
    /// Gameplay time in milliseconds.
    pub time: f64,
    /// Cursor position in playfield coordinates.
    pub position: Vec2,
    /// Actions held from this frame on.
    pub actions: Actions,
}

impl From<TimedFrame> for InputFrame {
    fn from(f: TimedFrame) -> InputFrame {
        InputFrame {
            time: f.time as f64,
            position: Vec2::new(f.x, f.y),
            actions: Actions::from_legacy(f.buttons),
        }
    }
}

/// The frames of a replay, sorted by time.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReplayInput {
    frames: Vec<InputFrame>,
}

impl ReplayInput {
    /// Builds the input from frames with absolute times, such as the output of
    /// [`osr::lazer_timeline`]. Frames are sorted by time, keeping the order of equal times, as
    /// lazer does when it starts playback. Frames with a non-finite time are dropped: a
    /// decoded `.osr` never has them, and they would stall playback.
    pub fn new(frames: impl IntoIterator<Item = InputFrame>) -> ReplayInput {
        let mut frames: Vec<InputFrame> =
            frames.into_iter().filter(|f| f.time.is_finite()).collect();
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Replays/FramedReplayInputHandler.cs (constructor, OrderBy)
        frames.sort_by(|a, b| a.time.total_cmp(&b.time));
        ReplayInput { frames }
    }

    /// The input of a decoded `.osr` played on a beatmap of the given format version
    /// (versions below 5 shift all frames by 24 ms).
    pub fn from_replay(replay: &osr::Replay, beatmap_format_version: i32) -> ReplayInput {
        let timeline = osr::lazer_timeline(replay.frames(), beatmap_format_version);
        ReplayInput::new(timeline.into_iter().map(InputFrame::from))
    }

    /// The frames, sorted by time.
    pub fn frames(&self) -> &[InputFrame] {
        &self.frames
    }
}

/// A press or release of one action.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActionEvent {
    /// Gameplay time of the event, in milliseconds.
    pub time: f64,
    /// Cursor position at the event.
    pub position: Vec2,
    /// The action: one of [`Actions::SINGLE`].
    pub action: Actions,
    /// `true` for a press, `false` for a release.
    pub pressed: bool,
}

/// The most events one replay step can produce: every action released and pressed.
pub const MAX_STEP_EVENTS: usize = 2 * Actions::SINGLE.len();

/// The input state at one replay step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputState {
    /// Cursor position, interpolated between frames.
    pub position: Vec2,
    /// Actions held.
    pub held: Actions,
    /// Actions released at this step. Releases are applied before presses.
    pub released: Actions,
    /// Actions pressed at this step.
    pub pressed: Actions,
}

impl InputState {
    /// The presses and releases of this step as events at `time`, in the order lazer's
    /// `RulesetInputManager` triggers them: all releases, then all presses, each in
    /// [`Actions::SINGLE`] order.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/RulesetInputManager.cs (HandleInputStateChange)
    pub fn events<'a>(
        &self,
        time: f64,
        buf: &'a mut [ActionEvent; MAX_STEP_EVENTS],
    ) -> &'a [ActionEvent] {
        let mut len = 0;
        for (set, pressed) in [(self.released, false), (self.pressed, true)] {
            for action in Actions::SINGLE {
                if set.contains(action) {
                    buf[len] = ActionEvent {
                        time,
                        position: self.position,
                        action,
                        pressed,
                    };
                    len += 1;
                }
            }
        }
        &buf[..len]
    }
}

/// Walks the frames of a [`ReplayInput`] the way lazer's `FramedReplayInputHandler` does.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayCursor {
    /// Index of the current frame, -1 before the first frame.
    index: isize,
    current_time: f64,
    last_held: Actions,
}

impl Default for ReplayCursor {
    fn default() -> Self {
        ReplayCursor::new()
    }
}

impl ReplayCursor {
    /// A cursor before the first frame.
    pub fn new() -> ReplayCursor {
        ReplayCursor {
            index: -1,
            current_time: f64::NEG_INFINITY,
            last_held: Actions::NONE,
        }
    }

    /// The time of the last [`ReplayCursor::set_frame_from_time`].
    pub fn current_time(&self) -> f64 {
        self.current_time
    }

    /// Index of the current frame, `None` before the first frame.
    pub fn frame_index(&self) -> Option<usize> {
        usize::try_from(self.index).ok()
    }

    /// Whether the current frame is the last one (or there are no frames).
    pub fn at_last_frame(&self, input: &ReplayInput) -> bool {
        self.index + 1 >= input.frames.len() as isize
    }

    fn frame_time(input: &ReplayInput, index: isize) -> f64 {
        if index < 0 {
            return f64::NEG_INFINITY;
        }
        match input.frames.get(index as usize) {
            Some(f) => f.time,
            None => f64::INFINITY,
        }
    }

    /// Moves to the frame for `time` and returns the time to use: never past the next frame,
    /// so every frame gets its own step. All frames are known up front and frame-accurate
    /// playback is off, so a time is always returned.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Replays/FramedReplayInputHandler.cs (SetFrameFromTime)
    pub fn set_frame_from_time(&mut self, input: &ReplayInput, mut time: f64) -> f64 {
        if input.frames.is_empty() {
            self.current_time = time;
            return time;
        }

        let mut frame_start = Self::frame_time(input, self.index);
        let mut frame_end = Self::frame_time(input, self.index + 1);

        if frame_end <= time {
            time = frame_end;
            self.index += 1;
        } else if time < frame_start && self.current_time == frame_start {
            self.index -= 1;
        }

        frame_start = Self::frame_time(input, self.index);
        frame_end = Self::frame_time(input, self.index + 1);

        // Math.Clamp; frames are sorted, so frame_start <= frame_end.
        self.current_time = time.max(frame_start).min(frame_end);
        self.current_time
    }

    /// The input at the current time: the interpolated position and the actions of the current
    /// frame, with the presses and releases relative to the previous call.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuFramedReplayInputHandler.cs (CollectReplayInputs)
    pub fn collect(&mut self, input: &ReplayInput) -> InputState {
        let (position, held) = match input.frames.len() {
            0 => (Vec2::ZERO, Actions::NONE),
            n => {
                let start = &input.frames[self.index.max(0) as usize];
                let end = &input.frames[((self.index + 1) as usize).min(n - 1)];
                let position = interpolate(
                    self.current_time,
                    start.position,
                    end.position,
                    start.time,
                    end.time,
                );
                let held = match self.frame_index() {
                    Some(i) => input.frames[i].actions,
                    None => Actions::NONE,
                };
                (position, held)
            }
        };

        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Input/Handlers/ReplayInputHandler.cs (ReplayState.Apply)
        // The action lists are always in the order Left, Right, Smoke, so lazer's list
        // differences are set differences.
        let released = self.last_held.difference(held);
        let pressed = held.difference(self.last_held);
        self.last_held = held;

        InputState {
            position,
            held,
            released,
            pressed,
        }
    }
}

/// `Interpolation.ValueAt` for `Vector2` with linear easing.
// Ported from osu-framework 2026.921.1: osu.Framework/Utils/Interpolation.cs (ValueAt<TEasing>, Vector2)
fn interpolate(time: f64, val1: Vec2, val2: Vec2, start_time: f64, end_time: f64) -> Vec2 {
    let current = (time - start_time) as f32;
    let duration = (end_time - start_time) as f32;

    if duration == 0.0 || current == 0.0 {
        return val1;
    }

    let t = current / duration;
    val1 + t * (val2 - val1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(time: f64, x: f32, y: f32, actions: Actions) -> InputFrame {
        InputFrame {
            time,
            position: Vec2::new(x, y),
            actions,
        }
    }

    #[test]
    fn legacy_buttons_map_to_actions() {
        assert_eq!(Actions::from_legacy(0), Actions::NONE);
        assert_eq!(Actions::from_legacy(1), Actions::LEFT);
        assert_eq!(Actions::from_legacy(4), Actions::LEFT);
        assert_eq!(Actions::from_legacy(5), Actions::LEFT);
        assert_eq!(Actions::from_legacy(2), Actions::RIGHT);
        assert_eq!(Actions::from_legacy(8), Actions::RIGHT);
        assert_eq!(Actions::from_legacy(15), Actions::LEFT | Actions::RIGHT);
        assert_eq!(Actions::from_legacy(16), Actions::SMOKE);
        assert_eq!(
            Actions::from_legacy(-1),
            Actions::LEFT | Actions::RIGHT | Actions::SMOKE
        );
    }

    #[test]
    fn frames_are_sorted_stably() {
        let input = ReplayInput::new([
            frame(10.0, 1.0, 0.0, Actions::NONE),
            frame(5.0, 2.0, 0.0, Actions::NONE),
            frame(10.0, 3.0, 0.0, Actions::NONE),
        ]);
        let xs: Vec<f32> = input.frames().iter().map(|f| f.position.x).collect();
        assert_eq!(xs, [2.0, 1.0, 3.0]);
    }

    #[test]
    fn interpolation_edges() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(10.0, 20.0);
        assert_eq!(interpolate(5.0, a, b, 0.0, 10.0), Vec2::new(5.0, 10.0));
        // Zero duration and zero elapsed time both give the first value.
        assert_eq!(interpolate(5.0, a, b, 5.0, 5.0), a);
        assert_eq!(interpolate(0.0, a, b, 0.0, 10.0), a);
        // Arithmetic is in f32: t = 1/3 rounded to f32.
        let p = interpolate(1.0, a, Vec2::new(3.0, 3.0), 0.0, 3.0);
        assert_eq!(p.x, (1.0f32 / 3.0) * 3.0);
    }

    #[test]
    fn cursor_never_skips_a_frame() {
        let input = ReplayInput::new([
            frame(0.0, 0.0, 0.0, Actions::NONE),
            frame(10.0, 0.0, 0.0, Actions::NONE),
            frame(10.0, 0.0, 0.0, Actions::NONE),
            frame(30.0, 0.0, 0.0, Actions::NONE),
        ]);
        let mut c = ReplayCursor::new();
        assert_eq!(c.set_frame_from_time(&input, f64::INFINITY), 0.0);
        assert_eq!(c.set_frame_from_time(&input, 100.0), 10.0);
        assert_eq!(c.frame_index(), Some(1));
        // A frame with the same time still gets its own step.
        assert_eq!(c.set_frame_from_time(&input, 100.0), 10.0);
        assert_eq!(c.frame_index(), Some(2));
        assert_eq!(c.set_frame_from_time(&input, 20.0), 20.0);
        assert_eq!(c.frame_index(), Some(2));
        assert!(!c.at_last_frame(&input));
        assert_eq!(c.set_frame_from_time(&input, 100.0), 30.0);
        assert!(c.at_last_frame(&input));
        assert_eq!(c.set_frame_from_time(&input, 100.0), 100.0);
    }

    #[test]
    fn cursor_before_first_frame_holds_nothing() {
        let input = ReplayInput::new([frame(50.0, 7.0, 8.0, Actions::LEFT)]);
        let mut c = ReplayCursor::new();
        assert_eq!(c.set_frame_from_time(&input, 10.0), 10.0);
        assert_eq!(c.frame_index(), None);
        let s = c.collect(&input);
        assert_eq!(s.held, Actions::NONE);
        assert_eq!(s.position, Vec2::new(7.0, 8.0));
    }

    #[test]
    fn presses_and_releases_follow_frames() {
        let l = Actions::LEFT;
        let r = Actions::RIGHT;
        let input = ReplayInput::new([
            frame(0.0, 0.0, 0.0, Actions::NONE),
            frame(1.0, 0.0, 0.0, l),
            frame(2.0, 0.0, 0.0, l | r),
            frame(3.0, 0.0, 0.0, r),
            frame(4.0, 0.0, 0.0, l),
            frame(5.0, 0.0, 0.0, Actions::NONE),
        ]);
        let mut c = ReplayCursor::new();
        let mut got = Vec::new();
        // Each proposed time is past the next frame (lazer only proposes +inf on the first step).
        for _ in 0..6 {
            c.set_frame_from_time(&input, 1e9);
            let s = c.collect(&input);
            got.push((s.held, s.released, s.pressed));
        }
        let n = Actions::NONE;
        assert_eq!(
            got,
            [
                (n, n, n),
                (l, n, l),
                (l | r, n, r),
                (r, l, n),
                (l, r, l),
                (n, l, n),
            ]
        );
        // Holding the same actions produces no events.
        c.set_frame_from_time(&input, 1e9);
        let s = c.collect(&input);
        assert_eq!((s.released, s.pressed), (n, n));
    }

    #[test]
    fn non_finite_frames_are_dropped() {
        let input = ReplayInput::new([
            frame(f64::NAN, 0.0, 0.0, Actions::NONE),
            frame(1.0, 0.0, 0.0, Actions::NONE),
            frame(f64::INFINITY, 0.0, 0.0, Actions::NONE),
            frame(f64::NEG_INFINITY, 0.0, 0.0, Actions::NONE),
        ]);
        assert_eq!(input.frames().len(), 1);
    }

    #[test]
    fn events_are_releases_then_presses_in_action_order() {
        let state = InputState {
            position: Vec2::new(1.0, 2.0),
            held: Actions::LEFT | Actions::SMOKE,
            released: Actions::RIGHT,
            pressed: Actions::LEFT | Actions::SMOKE,
        };
        let mut buf = [ActionEvent {
            time: 0.0,
            position: Vec2::ZERO,
            action: Actions::NONE,
            pressed: false,
        }; MAX_STEP_EVENTS];
        let got: Vec<(Actions, bool)> = state
            .events(5.0, &mut buf)
            .iter()
            .map(|e| {
                assert_eq!((e.time, e.position), (5.0, Vec2::new(1.0, 2.0)));
                (e.action, e.pressed)
            })
            .collect();
        assert_eq!(
            got,
            [
                (Actions::RIGHT, false),
                (Actions::LEFT, true),
                (Actions::SMOKE, true)
            ]
        );
    }

    #[test]
    fn position_is_interpolated_between_frames() {
        let input = ReplayInput::new([
            frame(0.0, 0.0, 0.0, Actions::NONE),
            frame(20.0, 100.0, 50.0, Actions::NONE),
        ]);
        let mut c = ReplayCursor::new();
        c.set_frame_from_time(&input, f64::INFINITY);
        c.set_frame_from_time(&input, 5.0);
        assert_eq!(c.collect(&input).position, Vec2::new(25.0, 12.5));
        // Past the last frame the position stays at it.
        c.set_frame_from_time(&input, 100.0);
        c.set_frame_from_time(&input, 120.0);
        assert_eq!(c.collect(&input).position, Vec2::new(100.0, 50.0));
    }
}
