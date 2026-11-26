//! The beat of the music the song select plays, for the selection's glow (ADR-0167).

use crate::anim::ease_out_quad;

/// Timing points kept; later ones are dropped (a preview rarely crosses that many).
pub const MAX_BEAT_POINTS: usize = 32;
/// Kiai sections kept.
pub const MAX_KIAI: usize = 16;

/// Lazer's pulsing panel fades in this fast before the beat, and takes it as its start.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/Carousel/Panel.cs (PulsatingBox)
const RISE_MS: f64 = 40.0;
/// A beat shorter than this pulses every other beat (lazer doubles `length` while short).
const MIN_PULSE_MS: f64 = 250.0;
/// How strong a beat off the bar's first is, and the boost of kiai.
const OFF_BEAT: f32 = 0.6;
const KIAI: f32 = 1.0;

/// A red line: from `time` on, beats of `beat_length` ms, `meter` to the bar.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BeatPoint {
    pub time: f64,
    pub beat_length: f64,
    pub meter: u32,
}

/// The red lines and kiai sections of the selected difficulty, fixed-size so that it
/// travels by copy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatTiming {
    points: [BeatPoint; MAX_BEAT_POINTS],
    point_count: usize,
    /// Start and end of each kiai section, in ms.
    kiai: [(f64, f64); MAX_KIAI],
    kiai_count: usize,
}

impl Default for BeatTiming {
    fn default() -> Self {
        BeatTiming {
            points: [BeatPoint::default(); MAX_BEAT_POINTS],
            point_count: 0,
            kiai: [(0.0, 0.0); MAX_KIAI],
            kiai_count: 0,
        }
    }
}

impl BeatTiming {
    /// Adds a red line, in time order; past [`MAX_BEAT_POINTS`] or with a beat length
    /// that is not positive, it is skipped.
    pub fn push_point(&mut self, point: BeatPoint) {
        if self.point_count < MAX_BEAT_POINTS
            && point.beat_length.is_finite()
            && point.beat_length > 0.0
        {
            self.points[self.point_count] = point;
            self.point_count += 1;
        }
    }

    /// Adds a kiai section from `start` to `end` ms, in time order.
    pub fn push_kiai(&mut self, start: f64, end: f64) {
        if self.kiai_count < MAX_KIAI && end > start {
            self.kiai[self.kiai_count] = (start, end);
            self.kiai_count += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.point_count == 0
    }

    /// How much the glow flares at `time` ms of the track, 0 to 1: up within 40 ms of
    /// each beat, then easing out to 0 over the beat, like lazer's selected panel; the
    /// bar's first beat and kiai pulse fullest.
    pub fn pulse_at(&self, time: f64) -> f32 {
        if self.point_count == 0 || !time.is_finite() {
            return 0.0;
        }
        let points = &self.points[..self.point_count];
        // The red line in force, or the first one before it starts.
        let point = points
            .iter()
            .rev()
            .find(|p| p.time <= time + RISE_MS)
            .unwrap_or(&points[0]);
        let mut length = point.beat_length;
        let mut step = 1;
        while length < MIN_PULSE_MS {
            length *= 2.0;
            step *= 2;
        }
        // Beats count from the red line; the rise starts RISE_MS before each.
        let since = time + RISE_MS - point.time;
        let index = (since / point.beat_length).floor();
        let index = index - index.rem_euclid(f64::from(step));
        let beat_at = point.time + index * point.beat_length;
        let into = time + RISE_MS - beat_at;
        let shape = if into < RISE_MS {
            ease_out_quad((into / RISE_MS).clamp(0.0, 1.0) as f32)
        } else {
            1.0 - ease_out_quad(((into - RISE_MS) / length).clamp(0.0, 1.0) as f32)
        };
        let meter = point.meter.max(1);
        let downbeat = (index as i64).rem_euclid(i64::from(meter)) == 0;
        let kiai = self.kiai[..self.kiai_count]
            .iter()
            .any(|&(start, end)| (start..end).contains(&time));
        let strength = if kiai {
            KIAI
        } else if downbeat {
            1.0
        } else {
            OFF_BEAT
        };
        shape * strength
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(beat_length: f64) -> BeatTiming {
        let mut timing = BeatTiming::default();
        timing.push_point(BeatPoint {
            time: 1000.0,
            beat_length,
            meter: 4,
        });
        timing
    }

    #[test]
    fn flares_on_the_beat_and_fades_over_it() {
        let t = timing(500.0);
        assert_eq!(t.pulse_at(1000.0), 1.0, "the bar's first beat, at its peak");
        assert!(t.pulse_at(1000.0 - RISE_MS / 2.0) < 1.0, "rising before it");
        assert!(t.pulse_at(1250.0) < t.pulse_at(1100.0), "fading");
        let off = t.pulse_at(1500.0);
        assert!(
            (off - OFF_BEAT).abs() < 1e-6,
            "an off beat is weaker: {off}"
        );
        assert_eq!(t.pulse_at(3000.0), 1.0, "the next bar");
    }

    #[test]
    fn short_beats_pulse_every_other_one() {
        let t = timing(200.0);
        assert_eq!(t.pulse_at(1000.0), 1.0);
        assert!(t.pulse_at(1200.0) < 0.9, "the second beat does not flare");
        assert!(t.pulse_at(1400.0) > 0.5);
    }

    #[test]
    fn kiai_pulses_fully_and_no_timing_does_not() {
        let mut t = timing(500.0);
        t.push_kiai(1400.0, 2000.0);
        assert_eq!(t.pulse_at(1500.0), KIAI);
        assert_eq!(BeatTiming::default().pulse_at(1000.0), 0.0);
    }
}
