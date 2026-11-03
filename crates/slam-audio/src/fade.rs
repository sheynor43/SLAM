//! The track's timed volume (ADR-0090): a gain that waits, then moves to a target along
//! an easing, frame by frame. Lazer's `VolumeTo(value, duration, easing)` on a track.

/// How a [`TrackAdjustment::Fade`](crate::TrackAdjustment::Fade) moves, osu-framework's
/// curves of the same names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Easing {
    /// Linear (osu-framework's `Easing.None`).
    #[default]
    None,
    /// Quadratic, slowing down (`Easing.Out`, `OutQuad`).
    Out,
    /// Quintic, slowing down.
    OutQuint,
    /// Cubic, speeding up.
    InCubic,
}

impl Easing {
    /// The eased progress at `time` in 0..=1.
    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/DefaultEasingFunction.cs
    pub fn apply(self, time: f64) -> f64 {
        match self {
            Easing::None => time,
            Easing::Out => time * (2.0 - time),
            Easing::OutQuint => {
                let t = time - 1.0;
                t * t * t * t * t + 1.0
            }
            Easing::InCubic => time * time * time,
        }
    }
}

/// A gain at `from` that holds for `delay` frames, then moves to `to` over `frames`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Fade {
    from: f32,
    to: f32,
    delay: u64,
    frames: u64,
    elapsed: u64,
    easing: Easing,
}

impl Default for Fade {
    fn default() -> Self {
        Self::at(1.0)
    }
}

impl Fade {
    pub(crate) fn at(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            delay: 0,
            frames: 0,
            elapsed: 0,
            easing: Easing::None,
        }
    }

    /// Moves from the current gain to `to` after `delay` frames, over `frames` (at once
    /// for 0).
    pub(crate) fn start(&mut self, to: f32, delay: u64, frames: u64, easing: Easing) {
        *self = Self {
            from: self.value(),
            to,
            delay,
            frames,
            elapsed: 0,
            easing,
        };
    }

    /// The gain of the next frame.
    pub(crate) fn value(&self) -> f32 {
        if self.elapsed < self.delay {
            return self.from;
        }
        let moved = self.elapsed - self.delay;
        if moved >= self.frames {
            return self.to;
        }
        let eased = self.easing.apply(moved as f64 / self.frames as f64);
        (f64::from(self.from) + (f64::from(self.to) - f64::from(self.from)) * eased) as f32
    }

    /// The gain of the next frame; moves on by one frame.
    pub(crate) fn next(&mut self) -> f32 {
        let value = self.value();
        if self.elapsed < self.delay.saturating_add(self.frames) {
            self.elapsed += 1;
        }
        value
    }

    /// Whether every frame from now on has gain 1: the track needs no scaling.
    pub(crate) fn is_unity(&self) -> bool {
        self.to == 1.0 && self.elapsed >= self.delay.saturating_add(self.frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curves_match_osu_framework() {
        for easing in [Easing::None, Easing::Out, Easing::OutQuint, Easing::InCubic] {
            assert_eq!(easing.apply(0.0), 0.0, "{easing:?}");
            assert_eq!(easing.apply(1.0), 1.0, "{easing:?}");
        }
        assert_eq!(Easing::Out.apply(0.5), 0.75);
        assert_eq!(Easing::InCubic.apply(0.5), 0.125);
        assert_eq!(Easing::OutQuint.apply(0.5), 1.0 - 0.5f64.powi(5));
    }

    #[test]
    fn a_fade_waits_then_moves_and_holds() {
        let mut fade = Fade::at(0.0);
        assert!(!fade.is_unity());
        fade.start(1.0, 2, 4, Easing::None);
        let gains: Vec<f32> = (0..8).map(|_| fade.next()).collect();
        assert_eq!(gains, [0.0, 0.0, 0.0, 0.25, 0.5, 0.75, 1.0, 1.0]);
        assert!(fade.is_unity());
    }

    #[test]
    fn a_new_fade_starts_from_where_the_old_one_is() {
        let mut fade = Fade::default();
        assert!(fade.is_unity());
        fade.start(0.0, 0, 4, Easing::None);
        fade.next();
        fade.next();
        fade.start(1.0, 0, 0, Easing::None);
        assert_eq!(fade.next(), 1.0);
        fade.start(0.0, 0, 2, Easing::None);
        assert_eq!(fade.next(), 1.0);
        assert_eq!(fade.next(), 0.5);
        assert_eq!(fade.next(), 0.0);
        assert!(!fade.is_unity());
    }
}
