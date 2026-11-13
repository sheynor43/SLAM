//! SLAM's own interface sounds (ADR-0120), synthesized by code: soft sine and triangle
//! tones with a fast attack and an exponential decay, a little filtered noise on the
//! clicks. Rendered once, off the hot paths, into short stereo samples at the mixer's
//! rate. They repeat a lot, so they stay quiet (peak 0.25) and gentle.

use std::f32::consts::TAU;
use std::sync::Arc;

use slam_audio::Sample;

/// Interleaved channels of the samples (the mixer's).
const CHANNELS: usize = 2;
/// Loudest value of every sound, about -12 dBFS.
const PEAK: f32 = 0.25;
/// Attack time of every tone, in seconds.
const ATTACK_S: f32 = 0.002;
/// The last part of every sound fades to exactly zero, in seconds.
const FADE_OUT_S: f32 = 0.006;
/// Seed of the noise, fixed so renders are identical.
const NOISE_SEED: u32 = 0x5EED_0120;

/// Interface events that make a sound (ADR-0120).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuSound {
    /// The pointer moves onto a button or row.
    Hover,
    /// A button is pressed.
    Click,
    /// A step deeper (Play).
    Enter,
    /// A step back.
    Back,
    /// A switch turns on.
    ToggleOn,
    /// A switch turns off.
    ToggleOff,
    /// A slider moves by a step.
    SliderStep,
}

impl MenuSound {
    /// Every sound, in the order of [`render_all`].
    pub const ALL: [MenuSound; 7] = [
        MenuSound::Hover,
        MenuSound::Click,
        MenuSound::Enter,
        MenuSound::Back,
        MenuSound::ToggleOn,
        MenuSound::ToggleOff,
        MenuSound::SliderStep,
    ];

    /// The skin sample names standing for it, in lookup order (ADR-0120).
    pub fn skin_names(self) -> &'static [&'static str] {
        match self {
            MenuSound::Hover | MenuSound::SliderStep => &["click-short"],
            MenuSound::Click => &["click-short-confirm", "menuclick"],
            MenuSound::Enter => &["menuhit"],
            MenuSound::Back => &["menuback"],
            MenuSound::ToggleOn => &["check-on"],
            MenuSound::ToggleOff => &["check-off"],
        }
    }
}

/// SLAM's own sound for `sound` at `rate` Hz: interleaved stereo. Allocates: call it off
/// the hot paths.
pub fn render(sound: MenuSound, rate: u32) -> Vec<f32> {
    let rate = rate.max(1) as f32;
    let (length_s, mut voice): (f32, Box<dyn FnMut(f32) -> f32>) = match sound {
        MenuSound::Hover => (0.03, click(2_200.0, 0.006, 0.5)),
        MenuSound::SliderStep => (0.025, click(1_700.0, 0.005, 0.4)),
        MenuSound::Click => (0.06, click(1_300.0, 0.012, 1.0)),
        MenuSound::Enter => (0.32, enter(rate)),
        MenuSound::Back => (0.15, glide(520.0, 330.0, 0.15, 0.05)),
        MenuSound::ToggleOn => (0.09, blip([660.0, 990.0])),
        MenuSound::ToggleOff => (0.09, blip([880.0, 550.0])),
    };
    let frames = (length_s * rate) as usize;
    let fade = ((FADE_OUT_S * rate) as usize).clamp(1, frames.max(1));
    let mut mono: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f32 / rate;
            let tail = frames - 1 - i;
            let fade = if tail < fade {
                tail as f32 / fade as f32
            } else {
                1.0
            };
            voice(t) * fade
        })
        .collect();
    // Remove any DC offset, then normalise to PEAK; the ends stay at zero.
    if !mono.is_empty() {
        let mean = mono.iter().sum::<f32>() / mono.len() as f32;
        let last = mono.len() - 1;
        for (i, v) in mono.iter_mut().enumerate() {
            if i != 0 && i != last {
                *v -= mean;
            }
        }
    }
    let peak = mono.iter().fold(0.0_f32, |p, v| p.max(v.abs()));
    let gain = if peak > 0.0 { PEAK / peak } else { 0.0 };
    let mut data = Vec::with_capacity(frames * CHANNELS);
    for v in mono {
        let v = v * gain;
        data.extend([v, v]);
    }
    data
}

/// Every sound rendered into a [`Sample`], in [`MenuSound::ALL`] order.
pub fn render_all(rate: u32) -> [Arc<Sample>; 7] {
    MenuSound::ALL.map(|sound| {
        let data = render(sound, rate);
        Arc::new(
            Sample::from_interleaved(data, rate, CHANNELS as u16)
                .expect("a rendered menu sound is a valid sample"),
        )
    })
}

/// White noise in -1 ..= 1 (xorshift32).
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

/// Attack then exponential decay with time constant `decay_s`.
fn envelope(t: f32, decay_s: f32) -> f32 {
    (t / ATTACK_S).min(1.0) * (-t / decay_s).exp()
}

/// A triangle wave of phase `p` (cycles), in -1 ..= 1.
fn triangle(p: f32) -> f32 {
    4.0 * (p.fract() - 0.5).abs() - 1.0
}

/// A short tick: a sine with a touch of triangle at `freq` Hz plus a puff of noise
/// (differenced, so bright and DC-free), decaying in `decay_s`; `weight` scales the noise.
fn click(freq: f32, decay_s: f32, weight: f32) -> Box<dyn FnMut(f32) -> f32> {
    let mut noise = Noise::new(NOISE_SEED);
    let mut prev = 0.0_f32;
    Box::new(move |t| {
        let white = noise.next();
        let hiss = white - prev;
        prev = white;
        let tone = 0.8 * (TAU * freq * t).sin() + 0.2 * triangle(freq * t);
        envelope(t, decay_s) * tone + 0.12 * weight * envelope(t, decay_s * 0.4) * hiss
    })
}

/// A tone gliding from `from` to `to` Hz over `length_s`, decaying in `decay_s`.
fn glide(from: f32, to: f32, length_s: f32, decay_s: f32) -> Box<dyn FnMut(f32) -> f32> {
    let mut phase = 0.0_f32;
    let mut last_t = 0.0_f32;
    Box::new(move |t| {
        let p = (t / length_s).clamp(0.0, 1.0);
        let freq = from + (to - from) * p;
        phase = (phase + freq * (t - last_t)).fract();
        last_t = t;
        envelope(t, decay_s) * (0.85 * (TAU * phase).sin() + 0.15 * triangle(phase))
    })
}

/// A two-note blip: the first note, then the second from the middle on.
fn blip(notes: [f32; 2]) -> Box<dyn FnMut(f32) -> f32> {
    const SECOND_AT_S: f32 = 0.042;
    Box::new(move |t| {
        let first = envelope(t, 0.02) * (TAU * notes[0] * t).sin();
        let u = t - SECOND_AT_S;
        let second = if u > 0.0 {
            envelope(u, 0.03) * (TAU * notes[1] * u).sin()
        } else {
            0.0
        };
        first * 0.8 + second
    })
}

/// The step deeper: a gentle rising tone with a soft swell of low-passed noise.
fn enter(rate: f32) -> Box<dyn FnMut(f32) -> f32> {
    const LENGTH_S: f32 = 0.32;
    let mut noise = Noise::new(NOISE_SEED);
    let mut low = 0.0_f32;
    let mut phase = 0.0_f32;
    let mut last_t = 0.0_f32;
    Box::new(move |t| {
        let p = (t / LENGTH_S).clamp(0.0, 1.0);
        let freq = 330.0 * (1.5_f32).powf(p);
        phase = (phase + freq * (t - last_t)).fract();
        last_t = t;
        let tone = (0.85 * (TAU * phase).sin() + 0.15 * triangle(phase)) * envelope(t, 0.12);
        let k = 1.0 - (-TAU * 1_800.0 / rate).exp();
        low += k * (noise.next() - low);
        // The swell peaks around a third of the way and dies away.
        let swell = (p / 0.35).min(1.0) * (-(t - 0.1).max(0.0) / 0.08).exp();
        tone + 0.9 * swell * low
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn seconds(sound: MenuSound, rate: u32) -> f32 {
        (render(sound, rate).len() / CHANNELS) as f32 / rate as f32
    }

    #[test]
    fn every_sound_is_non_empty_stereo_finite_and_quiet() {
        for rate in [44_100, 48_000] {
            for sound in MenuSound::ALL {
                let data = render(sound, rate);
                assert!(!data.is_empty(), "{sound:?}");
                assert_eq!(data.len() % CHANNELS, 0);
                assert!(data.iter().all(|v| v.is_finite()), "{sound:?}");
                let peak = data.iter().fold(0.0_f32, |p, v| p.max(v.abs()));
                assert!(peak <= 0.3 && peak > 0.05, "{sound:?} {peak}");
                // Both ears carry the same signal.
                assert!(data.chunks(2).all(|f| f[0] == f[1]));
            }
        }
    }

    #[test]
    fn durations_are_in_range() {
        let ranges = [
            (MenuSound::Hover, 0.025, 0.040),
            (MenuSound::SliderStep, 0.025, 0.040),
            (MenuSound::Click, 0.05, 0.07),
            (MenuSound::Enter, 0.25, 0.40),
            (MenuSound::Back, 0.13, 0.17),
            (MenuSound::ToggleOn, 0.08, 0.10),
            (MenuSound::ToggleOff, 0.08, 0.10),
        ];
        for rate in [44_100, 48_000] {
            for (sound, min, max) in ranges {
                let s = seconds(sound, rate);
                assert!(s >= min - 0.001 && s <= max + 0.001, "{sound:?} {s}");
            }
        }
    }

    #[test]
    fn starts_and_ends_at_zero() {
        for sound in MenuSound::ALL {
            let data = render(sound, RATE);
            assert!(data[0].abs() < 1e-3 && data[1].abs() < 1e-3, "{sound:?}");
            let n = data.len();
            assert!(
                data[n - 2].abs() < 1e-3 && data[n - 1].abs() < 1e-3,
                "{sound:?}"
            );
        }
    }

    #[test]
    fn has_no_dc_offset() {
        for sound in MenuSound::ALL {
            let data = render(sound, RATE);
            let mean = data.iter().sum::<f32>() / data.len() as f32;
            assert!(mean.abs() < 0.01, "{sound:?} {mean}");
        }
    }

    #[test]
    fn is_deterministic() {
        for sound in MenuSound::ALL {
            assert_eq!(render(sound, RATE), render(sound, RATE), "{sound:?}");
        }
    }

    #[test]
    fn enter_is_longest_and_hover_shorter_than_click() {
        let enter = seconds(MenuSound::Enter, RATE);
        for sound in MenuSound::ALL {
            assert!(seconds(sound, RATE) <= enter, "{sound:?}");
        }
        assert!(seconds(MenuSound::Hover, RATE) < seconds(MenuSound::Click, RATE));
    }

    #[test]
    fn skin_names_follow_the_adr() {
        assert_eq!(MenuSound::Hover.skin_names(), ["click-short"]);
        assert_eq!(
            MenuSound::Click.skin_names(),
            ["click-short-confirm", "menuclick"]
        );
        assert_eq!(MenuSound::Enter.skin_names(), ["menuhit"]);
        assert_eq!(MenuSound::Back.skin_names(), ["menuback"]);
        assert_eq!(MenuSound::ToggleOn.skin_names(), ["check-on"]);
        assert_eq!(MenuSound::ToggleOff.skin_names(), ["check-off"]);
        assert_eq!(MenuSound::SliderStep.skin_names(), ["click-short"]);
    }

    #[test]
    fn render_all_builds_samples_in_order() {
        let all = render_all(RATE);
        for (sound, sample) in MenuSound::ALL.into_iter().zip(&all) {
            assert_eq!(sample.data(), render(sound, RATE).as_slice());
            assert_eq!(sample.channels(), 2);
            assert_eq!(sample.sample_rate(), RATE);
        }
    }
}
