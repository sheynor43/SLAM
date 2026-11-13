//! The startup intro's sound (ADR-0114), synthesized from the intro's cues in a cinematic
//! style: a low drone swelling under the letters, an FM blip with a sub thump each time a
//! letter lights up, a riser of detuned saws and noise up to a breath of silence, then the
//! impact — a saturated kick, a sub boom, a filtered saw chord ("braam"), a noise crash
//! and a high shimmer — all in a reverb, glued by soft saturation. Rendered once, off
//! the hot paths, into one stereo sample at the mixer's rate.

use std::f32::consts::TAU;

use slam_ui::intro::{CueKind, END_S, IMPACT_S, cues};

/// Interleaved channels of the sample (the mixer's).
const CHANNELS: usize = 2;
/// How long the impact rings after the intro ends, in seconds.
const TAIL_S: f32 = 2.2;
/// The last part of the sample fades out, so the reverb ends in silence.
const FADE_OUT_S: f32 = 0.4;
/// Loudest value of the result.
const PEAK: f32 = 0.89;
/// Drive of the master saturation.
const DRIVE: f32 = 1.6;

/// Notes of the blips, one per letter in the order they appear (A minor pentatonic).
const BLIP_NOTES: [f32; 5] = [880.0, 1046.5, 1318.5, 1568.0, 1760.0];
const BLIP_S: f32 = 0.16;
const BLIP_GAIN: f32 = 0.16;
/// The thump under a letter's first blip.
const THUMP_GAIN: f32 = 0.35;

/// The drone: A1 and its fifth, swelling from the start up to the impact.
const DRONE: [f32; 2] = [55.0, 82.41];
const DRONE_GAIN: f32 = 0.1;

/// The riser glides from `RISER_FROM` to `RISER_TO` Hz; it stops this long before the
/// impact, a breath of silence that makes the impact land.
const RISER_FROM: f32 = 110.0;
const RISER_TO: f32 = 1_760.0;
const RISER_DETUNE: [f32; 5] = [0.985, 0.993, 1.0, 1.007, 1.015];
const RISER_GAIN: f32 = 0.1;
const RISER_NOISE_GAIN: f32 = 0.2;
const BREATH_S: f32 = 0.04;

/// The kick falls from `KICK_FROM` to `KICK_TO` Hz.
const KICK_FROM: f32 = 160.0;
const KICK_TO: f32 = 38.0;
const KICK_GAIN: f32 = 0.9;
const SUB: f32 = 41.2;
const SUB_GAIN: f32 = 0.45;
/// The braam's chord (A minor, low), each note a stack of detuned saws.
const BRAAM: [f32; 5] = [55.0, 82.41, 110.0, 130.81, 164.81];
const BRAAM_DETUNE: [f32; 3] = [0.992, 1.0, 1.008];
const BRAAM_GAIN: f32 = 0.05;
const CRASH_GAIN: f32 = 0.22;
/// The shimmer high above (A minor, two octaves up), detuned between the ears.
const SHIMMER: [f32; 4] = [440.0, 659.26, 880.0, 1318.5];
const SHIMMER_DETUNE: f32 = 1.004;
const SHIMMER_GAIN: f32 = 0.03;

/// The reverb: comb delays (seconds) of the left ear (the right's are a little longer),
/// their feedback, then two allpass delays, and how much of it is mixed in.
const COMBS: [f32; 4] = [0.0297, 0.0371, 0.0411, 0.0437];
const COMB_FEEDBACK: f32 = 0.84;
const ALLPASSES: [f32; 2] = [0.005, 0.0017];
const ALLPASS_GAIN: f32 = 0.7;
const RIGHT_SPREAD: f32 = 1.07;
const WET: f32 = 0.32;

/// The intro's sound for a `letters`-letter title at `sample_rate` Hz: interleaved
/// stereo, as long as the intro plus the impact's tail. Allocates: call it off the hot
/// paths.
pub fn render(letters: usize, sample_rate: u32) -> Vec<f32> {
    let rate = sample_rate.max(1) as f32;
    let frames = ((END_S + TAIL_S) * rate) as usize;
    let mut left = vec![0.0_f32; frames];
    let mut right = vec![0.0_f32; frames];
    let mut list = Vec::new();
    cues(letters, &mut list);
    let first = list.first().map_or(0.0, |cue| cue.at_s);
    {
        let mut out = Out::new(&mut left, &mut right, rate, first);
        drone(&mut out, IMPACT_S - first);
    }
    for cue in &list {
        let mut out = Out::new(&mut left, &mut right, rate, cue.at_s);
        match cue.kind {
            CueKind::Blip { rank, step } => blip(&mut out, letters, rank, step),
            CueKind::Sweep { length_s } => riser(&mut out, length_s - BREATH_S, cue.at_s),
            CueKind::Impact => impact(&mut out),
        }
    }
    reverb(&mut left, &mut right, rate);
    master(&mut left, &mut right, rate)
}

/// Where a cue's sound goes.
struct Out<'a> {
    left: &'a mut [f32],
    right: &'a mut [f32],
    rate: f32,
    /// First frame of the cue.
    start: usize,
}

impl<'a> Out<'a> {
    fn new(left: &'a mut [f32], right: &'a mut [f32], rate: f32, at_s: f32) -> Out<'a> {
        Out {
            left,
            right,
            rate,
            start: (at_s.max(0.0) * rate) as usize,
        }
    }

    /// Adds `f(t)` for `length_s` seconds from the cue, `t` in seconds since it,
    /// panned by `balance` (−1 left ..= 1 right, equal power).
    fn add(&mut self, length_s: f32, balance: f32, mut f: impl FnMut(f32) -> f32) {
        let angle = (balance.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        let (gl, gr) = (angle.cos(), angle.sin());
        self.add_stereo(length_s, |t| {
            let v = f(t);
            [v * gl, v * gr]
        });
    }

    /// Like [`Out::add`] with a separate signal per ear.
    fn add_stereo(&mut self, length_s: f32, mut f: impl FnMut(f32) -> [f32; 2]) {
        let frames = (length_s.max(0.0) * self.rate) as usize;
        let end = (self.start + frames).min(self.left.len());
        for frame in self.start..end {
            let [l, r] = f((frame - self.start) as f32 / self.rate);
            self.left[frame] += l;
            self.right[frame] += r;
        }
    }
}

/// A saw oscillator in −1 ..= 1, its phase kept between calls.
#[derive(Clone, Copy, Default)]
struct Saw(f32);

impl Saw {
    fn next(&mut self, freq: f32, rate: f32) -> f32 {
        self.0 = (self.0 + freq / rate).fract();
        2.0 * self.0 - 1.0
    }
}

/// A one-pole low-pass filter.
#[derive(Clone, Copy, Default)]
struct LowPass(f32);

impl LowPass {
    /// Filters `x` with cutoff `freq` Hz.
    fn next(&mut self, x: f32, freq: f32, rate: f32) -> f32 {
        let k = 1.0 - (-TAU * freq / rate).exp();
        self.0 += k * (x - self.0);
        self.0
    }
}

/// The low drone under the letters: a fifth of filtered saws over a sine, swelling up to
/// the impact and cut there, under the breath.
fn drone(out: &mut Out<'_>, length_s: f32) {
    let rate = out.rate;
    let mut saws = [[Saw::default(); 2]; DRONE.len()];
    let mut filters = [LowPass::default(); 2];
    let audible = length_s - BREATH_S;
    out.add_stereo(audible, |t| {
        let p = (t / audible).clamp(0.0, 1.0);
        let swell = p * p * (3.0 - 2.0 * p);
        let cutoff = 180.0 + 900.0 * swell;
        let mut ears = [0.0; 2];
        for (ear, detune) in [0.997_f32, 1.003].into_iter().enumerate() {
            let saw: f32 = DRONE
                .iter()
                .zip(&mut saws)
                .map(|(f, s)| s[ear].next(f * detune, rate))
                .sum();
            let sine = (TAU * DRONE[0] * t).sin();
            ears[ear] =
                DRONE_GAIN * swell * (filters[ear].next(saw, cutoff, rate) * 0.6 + sine * 0.8);
        }
        ears
    });
}

/// A letter lights up: a bright FM ping with a resonant ring, and on its first switch on a
/// sub thump; letters spread across the stereo field in their order.
fn blip(out: &mut Out<'_>, letters: usize, rank: u32, step: u32) {
    let note = BLIP_NOTES[rank as usize % BLIP_NOTES.len()];
    let first = step == 0;
    let freq = note * if first { 1.0 } else { 0.5 };
    let gain = BLIP_GAIN * if first { 1.0 } else { 0.5 };
    let balance = if letters > 1 {
        (rank as f32 / (letters - 1) as f32) * 1.2 - 0.6
    } else {
        0.0
    };
    out.add(BLIP_S, balance, |t| {
        let envelope = (t / 0.002).min(1.0) * (-t / 0.035).exp();
        let index = 4.0 * (-t / 0.015).exp();
        let modulator = (TAU * freq * 1.5 * t).sin() * index;
        let ring = (TAU * freq * 2.0 * t).sin() * 0.3 * (-t / 0.06).exp();
        gain * (envelope * (TAU * freq * t + modulator).sin() + ring)
    });
    if first {
        let rate = out.rate;
        let mut phase = 0.0_f32;
        out.add(0.25, 0.0, |t| {
            let f = 55.0 + 90.0 * (-t / 0.03).exp();
            phase = (phase + f / rate).fract();
            THUMP_GAIN * (t / 0.002).min(1.0) * (-t / 0.08).exp() * (TAU * phase).sin()
        });
    }
}

/// The riser up to the breath before the impact: detuned saws gliding up exponentially
/// through an opening filter, with noise growing brighter and louder.
fn riser(out: &mut Out<'_>, length_s: f32, seed_s: f32) {
    let ratio = RISER_TO / RISER_FROM;
    let rate = out.rate;
    let mut saws = [[Saw::default(); RISER_DETUNE.len()]; 2];
    let mut filters = [LowPass::default(); 2];
    let mut noise = Noise::new(seed_s.to_bits());
    let mut low = [LowPass::default(); 2];
    out.add_stereo(length_s, |t| {
        let p = (t / length_s).clamp(0.0, 1.0);
        let freq = RISER_FROM * ratio.powf(p);
        let swell = p * p * p;
        let mut ears = [0.0; 2];
        for ear in 0..2 {
            let saw: f32 = RISER_DETUNE
                .iter()
                .zip(&mut saws[ear])
                .map(|(d, s)| s.next(freq * d * if ear == 0 { 1.0 } else { 1.002 }, rate))
                .sum();
            let tone = filters[ear].next(saw, freq * 4.0, rate);
            // The noise, high-passed less and less: brighter as it rises.
            let white = noise.next();
            let hiss = white - low[ear].next(white, 4_000.0 * (1.0 - p) + 200.0, rate);
            ears[ear] = swell * (RISER_GAIN * tone + RISER_NOISE_GAIN * hiss);
        }
        ears
    });
}

/// The impact.
fn impact(out: &mut Out<'_>) {
    let rate = out.rate;
    let length = TAIL_S + END_S - IMPACT_S;
    // A kick falling in pitch, driven into saturation, and a sub under it.
    let mut phase = 0.0_f32;
    out.add(length, 0.0, |t| {
        let freq = KICK_TO + (KICK_FROM - KICK_TO) * (-t / 0.05).exp();
        phase = (phase + freq / rate).fract();
        let body = (t / 0.002).min(1.0) * (-t / 0.6).exp();
        let kick = (2.5 * (TAU * phase).sin()).tanh();
        let sub = (TAU * SUB * t).sin() * (t / 0.02).min(1.0) * (-t / 1.4).exp();
        KICK_GAIN * body * kick + SUB_GAIN * sub
    });
    // The braam: a low saw chord through a filter that opens at once and closes slowly.
    let mut saws = [[[Saw::default(); BRAAM_DETUNE.len()]; BRAAM.len()]; 2];
    let mut filters = [[LowPass::default(); 2]; 2];
    out.add_stereo(length, |t| {
        let envelope = (t / 0.01).min(1.0) * (-t / 1.3).exp();
        let cutoff = 150.0 + 2_400.0 * (-t / 0.35).exp();
        let mut ears = [0.0; 2];
        for ear in 0..2 {
            let mut sum = 0.0;
            for (note, stack) in BRAAM.iter().zip(&mut saws[ear]) {
                for (detune, saw) in BRAAM_DETUNE.iter().zip(stack) {
                    let spread = if ear == 0 { 1.0 } else { 1.003 };
                    sum += saw.next(note * detune * spread, rate);
                }
            }
            // Two poles for a darker slope.
            let once = filters[ear][0].next(sum, cutoff, rate);
            let twice = filters[ear][1].next(once, cutoff, rate);
            ears[ear] = BRAAM_GAIN * envelope * (1.5 * twice).tanh() / 1.5 * 4.0;
        }
        ears
    });
    // A crash of noise, bright and fading.
    let mut noise = Noise::new(0x1A_4D);
    let mut low = [LowPass::default(); 2];
    out.add_stereo(0.9, |t| {
        let envelope = (t / 0.001).min(1.0) * (-t / 0.22).exp();
        let mut ears = [0.0; 2];
        for (ear, ear_low) in low.iter_mut().enumerate() {
            let white = noise.next();
            let bright = white - ear_low.next(white, 2_500.0, rate);
            ears[ear] = CRASH_GAIN * envelope * bright;
        }
        ears
    });
    // The shimmer high above, slowly in and long.
    out.add_stereo(length, |t| {
        let envelope = (t / 0.25).min(1.0) * (-t / 1.1).exp();
        let mut ears = [0.0; 2];
        for (ear, detune) in [1.0, SHIMMER_DETUNE].into_iter().enumerate() {
            ears[ear] = SHIMMER
                .iter()
                .map(|f| (TAU * f * detune * t).sin())
                .sum::<f32>()
                * SHIMMER_GAIN
                * envelope;
        }
        ears
    });
}

/// A Schroeder reverb per ear (parallel combs, then allpasses), mixed in by [`WET`].
fn reverb(left: &mut [f32], right: &mut [f32], rate: f32) {
    for (ear, spread) in [(left, 1.0), (right, RIGHT_SPREAD)] {
        let dry = ear.to_vec();
        let mut wet = vec![0.0_f32; dry.len()];
        for delay_s in COMBS {
            let delay = ((delay_s * spread * rate) as usize).max(1);
            let mut line = vec![0.0_f32; delay];
            for (i, (w, x)) in wet.iter_mut().zip(&dry).enumerate() {
                let slot = &mut line[i % delay];
                let out = *slot;
                *slot = x + out * COMB_FEEDBACK;
                *w += out / COMBS.len() as f32;
            }
        }
        for delay_s in ALLPASSES {
            let delay = ((delay_s * spread * rate) as usize).max(1);
            let mut line = vec![0.0_f32; delay];
            for (i, w) in wet.iter_mut().enumerate() {
                let slot = &mut line[i % delay];
                let delayed = *slot;
                let v = *w + delayed * ALLPASS_GAIN;
                *slot = v;
                *w = delayed - v * ALLPASS_GAIN;
            }
        }
        for (e, w) in ear.iter_mut().zip(&wet) {
            *e += w * WET;
        }
    }
}

/// Normalises the mix, saturates it softly and fades its end out; interleaved, at most
/// [`PEAK`].
fn master(left: &mut [f32], right: &mut [f32], rate: f32) -> Vec<f32> {
    let peak = left
        .iter()
        .chain(right.iter())
        .fold(0.0_f32, |peak, v| peak.max(v.abs()));
    let gain = if peak > 0.0 { 1.0 / peak } else { 0.0 };
    let norm = DRIVE.tanh();
    let fade_from = left.len().saturating_sub((FADE_OUT_S * rate) as usize);
    // Down to exactly 0 on the last frame.
    let fade_len = left.len().saturating_sub(fade_from + 1).max(1) as f32;
    let mut data = Vec::with_capacity(left.len() * CHANNELS);
    for (i, (l, r)) in left.iter().zip(right.iter()).enumerate() {
        let fade = if i >= fade_from {
            (1.0 - (i - fade_from) as f32 / fade_len).max(0.0)
        } else {
            1.0
        };
        let shape = |v: f32| (DRIVE * v * gain).tanh() / norm * PEAK * fade;
        data.push(shape(*l));
        data.push(shape(*r));
    }
    data
}

/// White noise in −1 ..= 1 (xorshift32).
struct Noise(u32);

impl Noise {
    fn new(seed: u32) -> Noise {
        Noise(seed | 1)
    }

    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / ((1 << 23) as f32) - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slam_ui::intro::GATHER_S;

    const RATE: u32 = 48_000;

    fn rms(data: &[f32], from_s: f32, to_s: f32) -> f32 {
        let (from, to) = (
            (from_s * RATE as f32) as usize * CHANNELS,
            (to_s * RATE as f32) as usize * CHANNELS,
        );
        let part = &data[from..to.min(data.len())];
        (part.iter().map(|v| v * v).sum::<f32>() / part.len() as f32).sqrt()
    }

    #[test]
    fn sound_is_finite_bounded_stereo_and_as_long_as_intro_and_tail() {
        let data = render(4, RATE);
        assert_eq!(
            data.len(),
            ((END_S + TAIL_S) * RATE as f32) as usize * CHANNELS
        );
        assert!(data.iter().all(|v| v.is_finite()));
        let peak = data.iter().fold(0.0_f32, |p, v| p.max(v.abs()));
        assert!(peak <= PEAK + 1e-6, "{peak}");
        assert!(peak > 0.5, "{peak}");
        assert!(slam_audio::Sample::from_interleaved(data, RATE, 2).is_ok());
    }

    #[test]
    fn sound_follows_the_cues() {
        let data = render(4, RATE);
        let mut list = Vec::new();
        cues(4, &mut list);
        let first = list[0].at_s;
        // Silent until the first blip, then sounding on it.
        assert_eq!(rms(&data, 0.0, first - 0.001), 0.0);
        assert!(rms(&data, first, first + 0.03) > 0.01);
        // The riser grows towards the impact, a breath before it, and the impact is the
        // loudest moment.
        let early = rms(&data, GATHER_S + 0.02, GATHER_S + 0.12);
        let late = rms(&data, IMPACT_S - 0.14, IMPACT_S - 0.05);
        assert!(late > early * 2.0, "{early} {late}");
        let hit = rms(&data, IMPACT_S, IMPACT_S + 0.1);
        assert!(hit > late, "{late} {hit}");
        // The tail dies out to silence.
        let end = END_S + TAIL_S;
        assert!(rms(&data, end - 0.1, end) < hit * 0.05);
        assert_eq!(data[data.len() - 1], 0.0);
    }

    #[test]
    fn sound_is_the_same_every_time_at_any_rate() {
        assert_eq!(render(4, RATE), render(4, RATE));
        let other = render(4, 44_100);
        assert_eq!(
            other.len(),
            ((END_S + TAIL_S) * 44_100.0) as usize * CHANNELS
        );
        assert!(other.iter().all(|v| v.is_finite()));
        // Any title length works.
        assert!(render(1, RATE).iter().all(|v| v.is_finite()));
        assert!(render(12, RATE).iter().all(|v| v.is_finite()));
    }
}
