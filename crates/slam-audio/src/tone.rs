//! Test tone for the audio settings test button and backend checks.

use crate::hal::{AudioCallback, CallbackInfo};

/// Sine wave written to every channel. The phase is derived from the stream frame index,
/// so dropped or resized buffers never cause a phase jump.
#[derive(Debug, Clone)]
pub struct SineTone {
    frequency: f64,
    amplitude: f32,
}

impl SineTone {
    /// `amplitude` is linear, 0..=1.
    pub fn new(frequency: f64, amplitude: f32) -> Self {
        Self {
            frequency,
            amplitude: amplitude.clamp(0.0, 1.0),
        }
    }
}

impl AudioCallback for SineTone {
    fn process(&mut self, out: &mut [f32], info: &CallbackInfo) {
        let channels = usize::from(info.channels.max(1));
        let rate = f64::from(info.sample_rate.max(1));
        // Cycles per frame. `f64` keeps the phase exact to well under a sample for
        // days of playback.
        let step = self.frequency / rate;
        for (i, frame) in out.chunks_exact_mut(channels).enumerate() {
            let n = info.frame + i as u64;
            let phase = (n as f64 * step).fract();
            let sample = (phase * std::f64::consts::TAU).sin() as f32 * self.amplitude;
            frame.fill(sample);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(frame: u64) -> CallbackInfo {
        CallbackInfo {
            frame,
            timestamp_ns: 0,
            latency_ns: 0,
            sample_rate: 48_000,
            channels: 2,
        }
    }

    #[test]
    fn quarter_period_peaks() {
        // 12 kHz at 48 kHz: 4 frames per period.
        let mut tone = SineTone::new(12_000.0, 0.5);
        let mut out = [9.0f32; 8];
        tone.process(&mut out, &info(0));
        let expected = [0.0, 0.0, 0.5, 0.5, 0.0, 0.0, -0.5, -0.5];
        for (got, want) in out.iter().zip(expected) {
            assert!((got - want).abs() < 1e-6, "{out:?}");
        }
    }

    #[test]
    fn split_buffers_match_one_buffer() {
        let mut tone = SineTone::new(440.0, 1.0);
        let mut whole = [0.0f32; 512];
        tone.process(&mut whole, &info(1_000));
        let mut split = [0.0f32; 512];
        let (a, b) = split.split_at_mut(2 * 100);
        tone.process(a, &info(1_000));
        tone.process(b, &info(1_100));
        assert_eq!(whole, split);
    }
}
