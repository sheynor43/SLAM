//! Biquad filters on the music (ADR-0054): lazer's `AudioFilter`, a BASS BQF effect on
//! the track mixer.

use crate::sample::MAX_CHANNELS;

/// Highest low-pass cutoff, Hz: at or above it the low-pass is off.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Audio/Effects/AudioFilter.cs (MAX_LOWPASS_CUTOFF)
pub const MAX_LOW_PASS_CUTOFF: f32 = 22_049.0;

/// Lowest high-pass cutoff, Hz: at or below it the high-pass is off.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Audio/Effects/AudioFilter.cs (getInitialCutoff)
pub const MIN_HIGH_PASS_CUTOFF: f32 = 1.0;

/// Frames between coefficient updates while a cutoff moves.
pub(crate) const FILTER_CHUNK: usize = 256;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Audio/Effects/AudioFilter.cs (fQ)
const Q: f64 = 0.7;

/// Which filter of the track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    LowPass,
    HighPass,
}

/// A cutoff moving to `to` along OutCubic.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ramp {
    from: f32,
    to: f32,
    frames: u64,
    elapsed: u64,
}

impl Ramp {
    fn at(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            frames: 0,
            elapsed: 0,
        }
    }

    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/DefaultEasingFunction.cs (OutCubic)
    fn value(&self) -> f32 {
        if self.elapsed >= self.frames {
            return self.to;
        }
        let t = self.elapsed as f64 / self.frames as f64 - 1.0;
        let eased = t * t * t + 1.0;
        (f64::from(self.from) + (f64::from(self.to) - f64::from(self.from)) * eased) as f32
    }
}

/// One biquad, transposed direct form II, per channel.
#[derive(Debug, Clone)]
struct Biquad {
    kind: FilterKind,
    cutoff: Ramp,
    /// Cutoff the coefficients were computed for; `None` while off.
    applied: Option<f32>,
    b: [f64; 3],
    a: [f64; 2],
    z: [[f64; 2]; MAX_CHANNELS as usize],
}

impl Biquad {
    fn new(kind: FilterKind) -> Self {
        let off = match kind {
            FilterKind::LowPass => MAX_LOW_PASS_CUTOFF,
            FilterKind::HighPass => MIN_HIGH_PASS_CUTOFF,
        };
        Self {
            kind,
            cutoff: Ramp::at(off),
            applied: None,
            b: [1.0, 0.0, 0.0],
            a: [0.0, 0.0],
            z: [[0.0; 2]; MAX_CHANNELS as usize],
        }
    }

    fn is_off(&self, cutoff: f32) -> bool {
        match self.kind {
            FilterKind::LowPass => cutoff >= MAX_LOW_PASS_CUTOFF,
            FilterKind::HighPass => cutoff <= MIN_HIGH_PASS_CUTOFF,
        }
    }

    fn set(&mut self, cutoff: f32, frames: u64) {
        self.cutoff = Ramp {
            from: self.cutoff.value(),
            to: cutoff,
            frames,
            elapsed: 0,
        };
    }

    /// Whether the filter acts on the next chunk.
    fn is_active(&self) -> bool {
        !self.is_off(self.cutoff.value())
    }

    /// Takes the cutoff for the next chunk; returns whether the filter is on.
    fn update(&mut self, sample_rate: u32) -> bool {
        let cutoff = self.cutoff.value();
        if self.is_off(cutoff) {
            self.applied = None;
            return false;
        }
        if self.applied == Some(cutoff) {
            return true;
        }
        if self.applied.is_none() {
            // Attached anew: no ringing of the last time it was on.
            self.z = [[0.0; 2]; MAX_CHANNELS as usize];
        }
        self.applied = Some(cutoff);
        // RBJ cookbook, as BASS_FX's BQF; the cutoff stays below Nyquist.
        let rate = f64::from(sample_rate);
        let f0 = f64::from(cutoff).clamp(1.0, rate * 0.49);
        let w0 = std::f64::consts::TAU * f0 / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * Q);
        let a0 = 1.0 + alpha;
        let (b0, b1) = match self.kind {
            FilterKind::LowPass => ((1.0 - cos) / 2.0, 1.0 - cos),
            FilterKind::HighPass => ((1.0 + cos) / 2.0, -(1.0 + cos)),
        };
        self.b = [b0 / a0, b1 / a0, b0 / a0];
        self.a = [-2.0 * cos / a0, (1.0 - alpha) / a0];
        true
    }

    fn process(&mut self, data: &mut [f32], channels: usize) {
        let [b0, b1, b2] = self.b;
        let [a1, a2] = self.a;
        for frame in data.chunks_exact_mut(channels) {
            for (x, z) in frame.iter_mut().zip(&mut self.z) {
                let input = f64::from(*x);
                let y = b0 * input + z[0];
                z[0] = b1 * input - a1 * y + z[1];
                z[1] = b2 * input - a2 * y;
                // A decaying state turns subnormal on silence: slow on some CPUs.
                for v in z.iter_mut() {
                    if v.abs() < 1e-30 {
                        *v = 0.0;
                    }
                }
                *x = y as f32;
            }
        }
    }

    fn advance(&mut self, frames: u64) {
        self.cutoff.elapsed = self.cutoff.elapsed.saturating_add(frames);
    }
}

/// The track's low-pass and high-pass filters. Allocation-free.
#[derive(Debug, Clone)]
pub(crate) struct TrackFilters {
    low: Biquad,
    high: Biquad,
    sample_rate: u32,
}

impl TrackFilters {
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            low: Biquad::new(FilterKind::LowPass),
            high: Biquad::new(FilterKind::HighPass),
            sample_rate,
        }
    }

    /// Moves the cutoff of `kind` from its current value to `cutoff` over `frames`
    /// (at once for 0) along OutCubic.
    pub(crate) fn set(&mut self, kind: FilterKind, cutoff: f32, frames: u64) {
        match kind {
            FilterKind::LowPass => self.low.set(cutoff, frames),
            FilterKind::HighPass => self.high.set(cutoff, frames),
        }
    }

    /// Whether either filter acts on the next chunk.
    pub(crate) fn is_active(&self) -> bool {
        self.low.is_active() || self.high.is_active()
    }

    /// Filters one chunk (at most [`FILTER_CHUNK`] frames, interleaved) and moves the
    /// cutoffs on by its length.
    pub(crate) fn process(&mut self, data: &mut [f32], channels: usize) {
        if self.high.update(self.sample_rate) {
            self.high.process(data, channels);
        }
        if self.low.update(self.sample_rate) {
            self.low.process(data, channels);
        }
        self.advance((data.len() / channels.max(1)) as u64);
    }

    /// Moves the cutoffs on without filtering (no audio this buffer).
    pub(crate) fn advance(&mut self, frames: u64) {
        self.low.advance(frames);
        self.high.advance(frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f64, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin() as f32)
            .collect()
    }

    /// Mono RMS after filtering `hz` through `filters`, past the settling time.
    fn rms_through(filters: &mut TrackFilters, hz: f64) -> f64 {
        let rate = 48_000;
        let mut data = sine(rate, hz, rate as usize);
        for chunk in data.chunks_mut(FILTER_CHUNK) {
            filters.process(chunk, 1);
        }
        let tail = &data[rate as usize / 2..];
        (tail.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / tail.len() as f64).sqrt()
    }

    const SINE_RMS: f64 = std::f64::consts::FRAC_1_SQRT_2;

    #[test]
    fn filters_start_off_and_pass_everything() {
        let mut filters = TrackFilters::new(48_000);
        assert!(!filters.is_active());
        let mut data = sine(48_000, 1_000.0, 1_024);
        let before = data.clone();
        filters.process(&mut data, 1);
        assert_eq!(data, before);
    }

    #[test]
    fn low_pass_at_300_hz_cuts_highs_and_keeps_lows() {
        let mut filters = TrackFilters::new(48_000);
        filters.set(FilterKind::LowPass, 300.0, 0);
        assert!(filters.is_active());
        let low = rms_through(&mut filters.clone(), 50.0);
        let high = rms_through(&mut filters, 5_000.0);
        assert!((low / SINE_RMS - 1.0).abs() < 0.02, "{low}");
        // Second order: about −48 dB at four octaves above the cutoff.
        assert!(high / SINE_RMS < 0.01, "{high}");
    }

    #[test]
    fn high_pass_at_300_hz_cuts_lows_and_keeps_highs() {
        let mut filters = TrackFilters::new(48_000);
        filters.set(FilterKind::HighPass, 300.0, 0);
        let low = rms_through(&mut filters.clone(), 20.0);
        let high = rms_through(&mut filters, 5_000.0);
        assert!(low / SINE_RMS < 0.01, "{low}");
        assert!((high / SINE_RMS - 1.0).abs() < 0.02, "{high}");
    }

    #[test]
    fn cutoff_at_q_0_7_is_about_minus_3_db() {
        let mut filters = TrackFilters::new(48_000);
        filters.set(FilterKind::LowPass, 1_000.0, 0);
        let gain = rms_through(&mut filters, 1_000.0) / SINE_RMS;
        // Q 0.7 is just under Butterworth's 0.707: −3.1 dB.
        assert!((gain - 0.70).abs() < 0.01, "{gain}");
    }

    #[test]
    fn cutoff_moves_along_out_cubic_and_switches_off_at_the_ends() {
        let mut filters = TrackFilters::new(48_000);
        filters.set(FilterKind::LowPass, 300.0, 1_000);
        let at = |f: &TrackFilters| f.low.cutoff.value();
        assert_eq!(at(&filters), MAX_LOW_PASS_CUTOFF);
        assert!(
            !filters.is_active(),
            "starts off: the cutoff has not moved yet"
        );
        filters.advance(500);
        let expected = 22_049.0 + (300.0 - 22_049.0) * (1.0 - 0.5f64.powi(3));
        assert!((f64::from(at(&filters)) - expected).abs() < 0.01);
        assert!(filters.is_active());
        filters.advance(500);
        assert_eq!(at(&filters), 300.0);
        // Back to off.
        filters.set(FilterKind::LowPass, MAX_LOW_PASS_CUTOFF, 0);
        assert!(!filters.is_active());
    }

    #[test]
    fn switching_on_again_starts_from_rest() {
        let mut filters = TrackFilters::new(48_000);
        filters.set(FilterKind::HighPass, 300.0, 0);
        let mut loud = vec![1.0f32; 256];
        filters.process(&mut loud, 1);
        filters.set(FilterKind::HighPass, MIN_HIGH_PASS_CUTOFF, 0);
        filters.process(&mut vec![0.0; 256], 1);
        filters.set(FilterKind::HighPass, 300.0, 0);
        let mut silence = vec![0.0f32; 256];
        filters.process(&mut silence, 1);
        assert!(silence.iter().all(|&x| x == 0.0));
    }
}
