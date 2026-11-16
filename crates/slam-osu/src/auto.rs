//! The frames of Autoplay (ADR-0153): lazer's `OsuAutoGenerator` for the presses, sliders and
//! spinners, with the cursor moving between objects as stable's does (McOsu's
//! `updateAutoCursorPos`): over the whole gap from the end of one object to the start of the
//! next, eased once unless the objects form a stream.
//!
//! The frames are deterministic: one playable beatmap and one mod set give the same frames.

use slam_formats::osu::Vec2;

use crate::beatmap::Beatmap;
use crate::math;
use crate::mods::ModSet;
use crate::objects::{OsuHitObject, OsuHitObjectKind};
use crate::replay::{Actions, InputFrame};

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Replays/AutoGenerator.cs (KEY_UP_DELAY)
/// How long a key stays down after an object ends, in milliseconds.
pub const KEY_UP_DELAY: f64 = 50.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGenerator.cs (MIN_FRAME_SEPARATION_FOR_ALTERNATING)
/// Objects closer than this (in real time) are pressed with alternating buttons.
pub const MIN_FRAME_SEPARATION_FOR_ALTERNATING: f64 = 266.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGeneratorBase.cs (SPIN_RADIUS, SPINNER_CENTRE)
/// The radius the cursor spins a spinner at.
pub const SPIN_RADIUS: f32 = 50.0;
const SPINNER_CENTRE: Vec2 = Vec2 { x: 256.0, y: 192.0 };
/// Where the cursor starts, below the playfield, and how long before the first object.
const START_POSITION: Vec2 = Vec2 { x: 256.0, y: 500.0 };
const START_LEAD: f64 = 1500.0;
// Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuBeatmapStandard.cpp (updateAutoCursorPos)
/// Objects further apart than this many circle diameters are not a stream: the movement
/// between them is eased (`m_fHitcircleDiameter*1.05f`).
const STREAM_DIAMETERS: f32 = 1.05;
/// How many times the movement is eased (`osu_auto_snapping_strength`).
const SNAPPING_STRENGTH: usize = 1;

/// A frame being built: lazer's `OsuReplayFrame`.
#[derive(Debug, Clone, Copy)]
struct Frame {
    time: f64,
    position: Vec2,
    actions: Actions,
}

/// The Autoplay frames of `beatmap` (playable, with `mods` applied) played with `mods`.
pub fn generate(beatmap: &Beatmap, mods: &ModSet) -> Vec<InputFrame> {
    let mut generator = Generator {
        frames: Vec::new(),
        rate: mods.rate().rate,
        button_index: 0,
        last_end: (0.0, START_POSITION),
    };
    generator.generate(&beatmap.hit_objects);
    generator
        .frames
        .into_iter()
        .map(|f| InputFrame {
            time: f.time,
            position: f.position,
            actions: f.actions,
        })
        .collect()
}

struct Generator {
    frames: Vec<Frame>,
    /// The speed of the rate mods: lazer's `IApplicableToRate` over the play.
    rate: f64,
    /// Even: the left button presses the next object; odd: the right one.
    button_index: i32,
    /// The time and cursor position the last object ended at, where the next movement
    /// starts.
    last_end: (f64, Vec2),
}

impl Generator {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGenerator.cs (Generate)
    fn generate(&mut self, objects: &[OsuHitObject]) {
        let Some(first) = objects.first() else {
            return;
        };
        self.button_index = 0;
        let start = first.start_time - START_LEAD;
        self.add(Frame {
            time: start,
            position: START_POSITION,
            actions: Actions::NONE,
        });
        self.last_end = (start, START_POSITION);
        for h in objects {
            self.add_hit_object(h);
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGeneratorBase.cs (ApplyModsToTimeDelta)
    fn time_delta(&self, start: f64, end: f64) -> f64 {
        (end - start) / self.rate
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGeneratorBase.cs (GetFrameDelay)
    fn frame_delay(&self) -> f64 {
        1000.0 / 60.0 * self.rate
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGeneratorBase.cs (FindInsertionIndex)
    /// The index after every frame at or before `time`.
    fn insertion_index(&self, time: f64) -> usize {
        self.frames.partition_point(|f| f.time <= time)
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGeneratorBase.cs (AddFrameToReplay)
    fn add(&mut self, frame: Frame) {
        let at = self.insertion_index(frame.time);
        self.frames.insert(at, frame);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGenerator.cs (addHitObjectReplay)
    fn add_hit_object(&mut self, h: &OsuHitObject) {
        let mut start_position = h.stacked_position();
        let mut spinner_direction = -1.0f32;
        if let OsuHitObjectKind::Spinner(spinner) = &h.kind {
            // Spinners with no spins required complete by themselves.
            if spinner.spins_required == 0 {
                return;
            }
            let previous = self.last_position();
            (start_position, spinner_direction) = spinner_start(previous);
        }
        self.move_to(h, start_position);
        self.add_click_frames(h, start_position, spinner_direction);
    }

    fn last_position(&self) -> Vec2 {
        self.frames.last().map_or(START_POSITION, |f| f.position)
    }

    /// Moves the cursor from where the last object ended to `target` by `h`'s start, as
    /// stable does, and picks the button as lazer's `moveToHitObject` does.
    fn move_to(&mut self, h: &OsuHitObject, target: Vec2) {
        // Lazer measures the button alternation from the frame its cursor waits at until it
        // could "see and react" to the object (ADR-0153).
        let reaction = 100.0 * self.rate;
        let wait_time = h.start_time - (h.defaults.time_preempt - reaction).max(0.0);
        let last_frame_time = self
            .frames
            .last()
            .map_or(h.start_time, |f| f.time.max(wait_time));
        // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuBeatmapStandard.cpp (updateAutoCursorPos)
        let (from_time, from) = self.last_end;
        if h.start_time > from_time {
            let span = h.start_time - from_time;
            let diameter = (h.defaults.radius() * 2.0) as f32;
            let snap = from.distance(target) > diameter * STREAM_DIAMETERS;
            let at = |time: f64| {
                let mut percent = ((time - from_time) / span).clamp(0.0, 1.0) as f32;
                if snap {
                    for _ in 0..SNAPPING_STRENGTH {
                        percent = (-percent) * (percent - 2.0);
                    }
                }
                from + (target - from) * percent
            };
            // A key-up frame inside the movement follows it.
            for frame in &mut self.frames {
                if frame.time > from_time && frame.time < h.start_time {
                    frame.position = at(frame.time);
                }
            }
            let delay = self.frame_delay();
            let mut time = from_time + delay;
            while time < h.start_time {
                // Lazer's `(int)time`.
                let frame_time = time.trunc();
                let held = self.insertion_index(frame_time).checked_sub(1);
                let actions = held.map_or(Actions::NONE, |i| self.frames[i].actions);
                self.add(Frame {
                    time: frame_time,
                    position: at(frame_time),
                    actions,
                });
                time += delay;
            }
        }
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGenerator.cs (moveToHitObject)
        // Start alternating once the time separation is too small (faster than ~225 BPM).
        let time_difference = self.time_delta(last_frame_time, h.start_time);
        if (0.0..MIN_FRAME_SEPARATION_FOR_ALTERNATING).contains(&time_difference) {
            self.button_index += 1;
        } else {
            self.button_index = 0;
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGenerator.cs (addHitObjectClickFrames)
    fn add_click_frames(&mut self, h: &OsuHitObject, start_position: Vec2, spinner_direction: f32) {
        let mut action = if self.button_index % 2 == 0 {
            Actions::LEFT
        } else {
            Actions::RIGHT
        };
        let mut start = Frame {
            time: h.start_time,
            position: start_position,
            actions: action,
        };
        let end_delay = if h.is_spinner() { 1.0 } else { 0.0 };
        let mut end = Frame {
            time: h.end_time() + KEY_UP_DELAY + end_delay,
            position: h.stacked_end_position(),
            actions: Actions::NONE,
        };
        // The frame before the press: if it holds a button, alternate and follow the newest
        // object from here on.
        if let Some(index) = self.insertion_index(start.time).checked_sub(1) {
            let previous_actions = self.frames[index].actions;
            if !previous_actions.is_empty() {
                if previous_actions.contains(action) {
                    action = if action == Actions::LEFT {
                        Actions::RIGHT
                    } else {
                        Actions::LEFT
                    };
                    start.actions = action;
                }
                let end_index = self.insertion_index(end.time);
                if index < self.frames.len() - 1 {
                    let to = end_index.max(index + 1);
                    self.frames.drain(index + 1..to);
                }
                let count = self.frames.len();
                for (j, frame) in self.frames.iter_mut().enumerate().skip(index + 1) {
                    // Frames that release a button keep releasing it.
                    if j < count - 1 || frame.actions == previous_actions {
                        frame.actions = action;
                    }
                }
            }
        }
        self.add(start);

        let delay = self.frame_delay();
        let mut end_position = h.stacked_end_position();
        match &h.kind {
            OsuHitObjectKind::Spinner(spinner) => {
                // 0.05 rad/ms, or ~477 RPM, as per stable.
                let spin_rpm: f32 = 0.05 / (2.0 * std::f32::consts::PI) * 60000.0;
                // .NET's `float.DegreesToRadians`: `(degrees * Pi) / 180`.
                let rads_per_millisecond =
                    (spin_rpm * 360.0 * std::f32::consts::PI) / 180.0 / 60000.0;
                let difference = start_position - SPINNER_CENTRE;
                let radius = difference.length();
                let mut angle = if radius == 0.0 {
                    0.0
                } else {
                    math::atan2f(difference.y, difference.x)
                };
                let mut previous = h.start_time;
                let mut next = h.start_time + delay;
                while next < spinner.end_time {
                    let t = self.time_delta(previous, next) * f64::from(spinner_direction);
                    angle += t as f32 * rads_per_millisecond;
                    self.add(Frame {
                        time: next.trunc(),
                        position: SPINNER_CENTRE + circle_position(angle, SPIN_RADIUS),
                        actions: action,
                    });
                    previous = next;
                    next += delay;
                }
                let t = self.time_delta(previous, spinner.end_time) * f64::from(spinner_direction);
                angle += t as f32 * rads_per_millisecond;
                end_position = SPINNER_CENTRE + circle_position(angle, SPIN_RADIUS);
                self.add(Frame {
                    time: spinner.end_time,
                    position: end_position,
                    actions: action,
                });
                end.position = end_position;
            }
            OsuHitObjectKind::Slider(slider) => {
                let duration = slider.duration(h.start_time);
                let mut j = delay;
                while j < duration {
                    let position = h.stacked_position() + slider.curve_position_at(j / duration);
                    self.add(Frame {
                        time: h.start_time + j,
                        position,
                        actions: action,
                    });
                    j += delay;
                }
                self.add(Frame {
                    time: h.end_time(),
                    position: h.stacked_end_position(),
                    actions: action,
                });
            }
            OsuHitObjectKind::Circle => {}
        }
        // Let go only at the end of the frames so far: a later object still holds otherwise.
        if self.frames.last().is_some_and(|f| f.time <= end.time) {
            self.add(end);
        }
        // An object inside a longer one (a circle on a slider) leaves the cursor following
        // the longer one: the next movement starts at its end.
        if h.end_time() >= self.last_end.0 {
            self.last_end = (h.end_time(), end_position);
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGeneratorBase.cs (CirclePosition)
fn circle_position(angle: f32, radius: f32) -> Vec2 {
    let t = f64::from(angle);
    let radius = f64::from(radius);
    Vec2 {
        x: (math::cos(t) * radius) as f32,
        y: (math::sin(t) * radius) as f32,
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuAutoGenerator.cs (calcSpinnerStartPosAndDirection)
/// The point on the spin circle the cursor enters it at from `previous`, and the direction
/// it spins in.
fn spinner_start(previous: Vec2) -> (Vec2, f32) {
    let mut offset = SPINNER_CENTRE - previous;
    let distance = offset.length();
    if distance > SPIN_RADIUS {
        let to_tangent = (distance * distance - SPIN_RADIUS * SPIN_RADIUS).sqrt();
        // The angle between the centre offset and the tangent point offset.
        let angle = math::asinf(SPIN_RADIUS / distance);
        let direction = if angle > 0.0 { -1.0 } else { 1.0 };
        let (sin, cos) = (math::sinf(angle), math::cosf(angle));
        // Lazer rotates X first and uses the new X for Y; kept as it is.
        offset.x = offset.x * cos - offset.y * sin;
        offset.y = offset.x * sin + offset.y * cos;
        // osuTK's `Normalize`: times the inverse of the length.
        let length = offset.length();
        if length > 0.0 {
            offset = offset * (1.0 / length);
        }
        offset = offset * to_tangent;
        (previous + offset, direction)
    } else if distance > 0.0 {
        (SPINNER_CENTRE - offset * (SPIN_RADIUS / distance), 1.0)
    } else {
        (
            SPINNER_CENTRE
                + Vec2 {
                    x: 0.0,
                    y: -SPIN_RADIUS,
                },
            1.0,
        )
    }
}

#[cfg(test)]
mod tests;
