//! Objects shaking after a press blocked by the hit policy (note lock) or far too early:
//! lazer's `ShakeContainer` around a hit circle's pieces and approach circle, and around a
//! slider's body, tail, ticks and repeats. A slider shakes through its head, which shakes
//! itself and the slider at once, so one shake moves the whole slider but its ball and follow
//! circle. A circle's shake container is inside its scale container: it moves by the offset
//! times the object's scale, the slider's container by the offset itself. A judgement ends the
//! shake of the judged drawable at once: lazer's `UpdateState` rewinds its transforms.
//!
//! The update side starts a shake for each [`slam_osu::rules::Shake`] of a tick in
//! [`ObjectShakes`], which the snapshot carries; the draw side moves an object by
//! [`ObjectShakes::offset`] at the frame's game time.

use crate::circles::{Easing, value_at};

/// `ShakeDuration` of `DrawableHitCircle` and `DrawableSlider`: the length of one swing.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (load)
const SHAKE_DURATION: f64 = 30.0;
/// `DrawableExtensions.Shake`'s default magnitude, in osu!pixels.
const SHAKE_MAGNITUDE: f32 = 8.0;
/// How long a shake lasts: half a swing out, three swings, half a swing back.
pub const SHAKE_LENGTH: f64 = SHAKE_DURATION * 4.0;
/// The most objects shaking at once that [`ObjectShakes`] keeps; a new shake beyond them
/// replaces the one that started first.
pub const MAX_SHAKES: usize = 32;

/// One object's shake.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ObjectShake {
    /// Object index in the beatmap.
    index: u32,
    /// When the shake started.
    start: f64,
    /// The offset it starts from: where an earlier shake of the object was then.
    from: f32,
}

impl ObjectShake {
    /// The horizontal offset at `time`, in osu!pixels.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Extensions/DrawableExtensions.cs (Shake)
    fn offset(&self, time: f64) -> f32 {
        // `MoveToX` to each target in sequence, each from where the previous one ended.
        const SWINGS: [(f64, f32, Easing); 5] = [
            (SHAKE_DURATION / 2.0, SHAKE_MAGNITUDE, Easing::OutSine),
            (SHAKE_DURATION, -SHAKE_MAGNITUDE, Easing::InOutSine),
            (SHAKE_DURATION, SHAKE_MAGNITUDE, Easing::InOutSine),
            (SHAKE_DURATION, -SHAKE_MAGNITUDE, Easing::InOutSine),
            (SHAKE_DURATION / 2.0, 0.0, Easing::InSine),
        ];
        let (mut start, mut from) = (self.start, self.from);
        if time < start {
            return from;
        }
        for (duration, to, easing) in SWINGS {
            let end = start + duration;
            if time < end {
                return value_at(time, from, to, start, end, easing);
            }
            (start, from) = (end, to);
        }
        0.0
    }

    fn ended(&self, time: f64) -> bool {
        time >= self.start + SHAKE_LENGTH
    }
}

/// The objects shaking, at most [`MAX_SHAKES`]. Fixed size: copied into the snapshot without
/// allocating.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectShakes {
    shakes: [ObjectShake; MAX_SHAKES],
    len: usize,
}

impl Default for ObjectShakes {
    fn default() -> Self {
        ObjectShakes {
            shakes: [ObjectShake {
                index: 0,
                start: 0.0,
                from: 0.0,
            }; MAX_SHAKES],
            len: 0,
        }
    }
}

impl ObjectShakes {
    /// Starts a shake of object `index` at `time`. A shake of an object already shaking takes
    /// over from where it is then: osu-framework ends a property's running transform when a
    /// new one starts and reads the new one's start value after it.
    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/TargetGroupingTransformTracker.cs (AddTransform, UpdateTransforms)
    pub fn start(&mut self, index: u32, time: f64) {
        let shakes = &mut self.shakes[..self.len];
        if let Some(shake) = shakes.iter_mut().find(|s| s.index == index) {
            *shake = ObjectShake {
                index,
                start: time,
                from: shake.offset(time),
            };
            return;
        }
        let shake = ObjectShake {
            index,
            start: time,
            from: 0.0,
        };
        if let Some(ended) = shakes.iter_mut().find(|s| s.ended(time)) {
            *ended = shake;
        } else if self.len < MAX_SHAKES {
            self.shakes[self.len] = shake;
            self.len += 1;
        } else if let Some(first) = self
            .shakes
            .iter_mut()
            .min_by(|a, b| a.start.total_cmp(&b.start))
        {
            *first = shake;
        }
    }

    /// The horizontal offset of object `index` at `time`, in osu!pixels: 0 unless it shakes.
    pub fn offset(&self, index: u32, time: f64) -> f32 {
        self.shakes[..self.len]
            .iter()
            .find(|s| s.index == index)
            .map_or(0.0, |s| s.offset(time))
    }

    /// [`offset`](Self::offset) of a drawable judged at `judged`, if it is: none from then on.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Drawables/DrawableHitObject.cs (UpdateState, clearExistingStateTransforms)
    pub fn offset_until(&self, index: u32, time: f64, judged: Option<f64>) -> f32 {
        // A judged drawable cannot shake again, so its judgement is after any shake.
        if judged.is_some_and(|judged| time >= judged) {
            return 0.0;
        }
        self.offset(index, time)
    }

    /// Forgets every shake (a restart).
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shaking(start: f64) -> ObjectShakes {
        let mut shakes = ObjectShakes::default();
        shakes.start(3, start);
        shakes
    }

    #[test]
    fn a_shake_swings_through_its_targets_in_120_ms() {
        let s = shaking(1000.0);
        let at = |t| s.offset(3, t);
        assert_eq!(at(999.0), 0.0);
        assert_eq!(at(1000.0), 0.0);
        assert_eq!(at(1015.0), 8.0);
        assert_eq!(at(1045.0), -8.0);
        assert_eq!(at(1075.0), 8.0);
        assert_eq!(at(1105.0), -8.0);
        assert_eq!(at(1120.0), 0.0);
        assert_eq!(at(2000.0), 0.0);
        // Halfway through the swings: `Sin(π/4)`, then the `InOutSine` midpoint, then
        // `1 - Cos(π/4)` of the way back.
        let quarter = std::f64::consts::FRAC_PI_4;
        assert_eq!(at(1007.5), (quarter.sin() * 8.0) as f32);
        assert!(at(1030.0).abs() < 1e-6);
        let back = -8.0 + (1.0 - quarter.cos()) * 8.0;
        assert_eq!(at(1112.5), back as f32);
        // Other objects do not move.
        assert_eq!(s.offset(4, 1015.0), 0.0);
    }

    #[test]
    fn a_new_shake_of_a_shaking_object_starts_from_its_offset() {
        let mut s = shaking(1000.0);
        let from = s.offset(3, 1050.0);
        assert!(from < 0.0);
        s.start(3, 1050.0);
        assert_eq!(s.offset(3, 1050.0), from);
        assert_eq!(s.offset(3, 1065.0), 8.0);
        assert_eq!(s.offset(3, 1170.0), 0.0);
        assert_eq!(s.len, 1);
    }

    #[test]
    fn ended_shakes_make_room_and_the_first_one_gives_way_when_full() {
        let mut s = ObjectShakes::default();
        for i in 0..MAX_SHAKES as u32 {
            s.start(i, 1000.0 + f64::from(i));
        }
        assert_eq!(s.len, MAX_SHAKES);
        // All still shaking: object 0, the first, gives way.
        s.start(100, 1050.0);
        assert_eq!(s.offset(0, 1060.0), 0.0);
        assert_ne!(s.offset(100, 1060.0), 0.0);
        // Later, every shake has ended: a new one reuses a slot.
        s.start(200, 2000.0);
        assert_eq!(s.len, MAX_SHAKES);
        assert_eq!(s.offset(200, 2015.0), 8.0);
    }

    #[test]
    fn a_judgement_stops_the_shake() {
        let s = shaking(1000.0);
        assert_eq!(s.offset_until(3, 1015.0, None), 8.0);
        assert_eq!(s.offset_until(3, 1015.0, Some(1020.0)), 8.0);
        assert_eq!(s.offset_until(3, 1020.0, Some(1020.0)), 0.0);
        assert_eq!(s.offset_until(3, 1045.0, Some(1020.0)), 0.0);
    }

    #[test]
    fn clear_forgets_every_shake() {
        let mut s = shaking(1000.0);
        s.clear();
        assert_eq!(s.offset(3, 1015.0), 0.0);
    }
}
