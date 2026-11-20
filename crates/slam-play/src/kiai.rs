//! The kiai flash of legacy hit circles (`LegacyKiaiFlashingDrawable`): on every beat in kiai
//! time the circle's texture flashes additively in a brightened combo colour and fades out
//! over the beat.
//!
//! Lazer fires the flash from a `BeatSyncedContainer` when the beat changes and runs it as a
//! transform started at the beat. Seen at a high frame rate this is a pure function of the
//! game time: the alpha comes from the latest beat at or before the time. The draw side keeps
//! its own copy of the timing and effect points (it does not call `slam-osu`), made at load.

use slam_osu::control_points::{ControlPoints, TimingPoint};
use slam_render::Color;

use crate::circles::Easing;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyKiaiFlashingDrawable.cs (flash_opacity)
/// The alpha a flash starts at.
const FLASH_OPACITY: f32 = 0.3;

/// Shortest fade of a flash, ms; it also stops this much before the next beat.
const FADE_MARGIN: f64 = 80.0;

/// A timing point as the beat clock reads it. `OmitFirstBarLine` only shifts lazer's beat
/// index, not when beats fall.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Timing {
    time: f64,
    beat_length: f64,
}

impl From<TimingPoint> for Timing {
    fn from(point: TimingPoint) -> Self {
        Timing {
            time: point.time,
            beat_length: point.beat_length,
        }
    }
}

/// How far after a beat kiai is looked up: lazer fires the beat on the first frame after it
/// and reads the effect point at that frame's time. A green line placed on a beat (rounded to
/// whole milliseconds by the editor) counts for that beat.
const KIAI_LOOKUP_DELAY: f64 = 1.0;

/// Most beats looked back through for a flash still fading; beyond, there is none.
const MAX_STEPS: usize = 4096;

/// How long the flash of a beat of `beat_length` ms fades.
fn fade_length(beat_length: f64) -> f64 {
    (beat_length - FADE_MARGIN).max(FADE_MARGIN)
}

/// The beats of a beatmap and where kiai time is, for the flash.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KiaiFlash {
    /// Timing points in time order; empty: lazer's default point (1000 ms beats from 0).
    timing: Vec<Timing>,
    /// Effect points in time order: their time and whether kiai is on.
    effects: Vec<(f64, bool)>,
    /// The longest fade of any timing point: how far back a flash can still show.
    max_fade: f64,
}

impl KiaiFlash {
    /// The flash of a beatmap with `points`. Allocates: call it at load.
    pub fn new(points: &ControlPoints) -> Self {
        let timing: Vec<Timing> = points
            .timing_points()
            .iter()
            .map(|&p| Timing::from(p))
            .collect();
        let max_fade = if timing.is_empty() {
            fade_length(TimingPoint::DEFAULT.beat_length)
        } else {
            timing
                .iter()
                .map(|t| fade_length(t.beat_length))
                .fold(FADE_MARGIN, f64::max)
        };
        KiaiFlash {
            timing,
            effects: points
                .effect_points()
                .iter()
                .map(|p| (p.time, p.kiai))
                .collect(),
            max_fade,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/ControlPointInfo.cs (TimingPointAt)
    /// The index of the timing point active at `time` (the first one before it starts), or
    /// `None` without timing points.
    fn timing_index(&self, time: f64) -> Option<usize> {
        if self.timing.is_empty() {
            return None;
        }
        Some(
            self.timing
                .partition_point(|p| p.time <= time)
                .saturating_sub(1),
        )
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/ControlPointInfo.cs (EffectPointAt)
    /// Whether the beat at `beat` is in kiai, as the frame just after it sees.
    fn kiai_after(&self, beat: f64) -> bool {
        let time = beat + KIAI_LOOKUP_DELAY;
        let active = self.effects.partition_point(|&(t, _)| t <= time);
        active.checked_sub(1).is_some_and(|i| self.effects[i].1)
    }

    /// The flash's alpha at game `time`, 0..=0.3: the circle texture is drawn additively at
    /// this alpha (times the circle's own) in [`kiai_colour`].
    ///
    /// A beat out of kiai leaves the running fade alone, so the flash comes from the latest
    /// beat in kiai at or before `time`, looked for back through the beats (across timing
    /// points: a new point's first beat is at its time) as far as a fade can reach.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Containers/BeatSyncedContainer.cs (Update)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyKiaiFlashingDrawable.cs (OnNewBeat)
    pub fn alpha(&self, time: f64) -> f32 {
        if self.effects.is_empty() {
            return 0.0;
        }
        let mut index = self.timing_index(time);
        let mut timing = index.map_or(Timing::from(TimingPoint::DEFAULT), |i| self.timing[i]);
        // The latest beat on the point's grid at or before `time`.
        let mut beat =
            timing.time + ((time - timing.time) / timing.beat_length).floor() * timing.beat_length;
        if beat > time {
            beat -= timing.beat_length;
        }
        for _ in 0..MAX_STEPS {
            let elapsed = time - beat;
            if elapsed >= self.max_fade {
                return 0.0;
            }
            if self.kiai_after(beat) {
                let fade = fade_length(timing.beat_length);
                if elapsed >= fade {
                    return 0.0;
                }
                // `FadeTo(flash_opacity).Then().FadeOut(fade, Easing.OutSine)`.
                let t = Easing::OutSine.apply(elapsed / fade);
                return FLASH_OPACITY * (1.0 - t as f32);
            }
            let previous = beat - timing.beat_length;
            match index {
                // The first beat of a point: the one before is the last of the previous point.
                Some(i) if i > 0 && previous < timing.time => {
                    let old = self.timing[i - 1];
                    let mut b = old.time
                        + ((timing.time - old.time) / old.beat_length).floor() * old.beat_length;
                    if b >= timing.time {
                        b -= old.beat_length;
                    }
                    index = Some(i - 1);
                    timing = old;
                    beat = b;
                }
                _ => beat = previous,
            }
        }
        0.0
    }
}

/// The colour a circle of `colour` flashes in: each channel raised by
/// `max(25, 300 − r − g − b)` (in 0..=255 units), up to 255.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyMainCirclePiece.cs (LoadComplete)
pub fn kiai_colour(colour: Color) -> Color {
    let byte = |c: f32| (c * 255.0) as u8;
    let (r, g, b) = (byte(colour.r), byte(colour.g), byte(colour.b));
    let add = (300 - i32::from(r) - i32::from(g) - i32::from(b)).max(25);
    let raise = |c: u8| (i32::from(c) + add).min(255) as f32 / 255.0;
    Color {
        r: raise(r),
        g: raise(g),
        b: raise(b),
        a: 1.0,
    }
}

#[cfg(test)]
mod tests;
