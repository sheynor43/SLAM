//! The beating logo (ADR-0186): how it moves on the music's beat and the pointer, its
//! spectrum visualiser, the triangles it throws out and the side flashes. Update-side state
//! advanced by [`LogoMotion::tick`]; [`LogoMotion::snapshot`] is what the draw side needs.
//! Neither allocates.

use crate::anim::{ease_out_elastic, ease_out_expo, ease_out_quad, ease_out_quint};
use crate::song_select::beat::{Beat, BeatTiming};

/// Bars of one visualiser round.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Menu/LogoVisualisation.cs
pub const BARS: usize = 200;
/// Rounds of bars around the logo.
pub const ROUNDS: usize = 5;
/// Bar length at amplitude 1, as a part of the logo's size (lazer: 600 on its 480).
pub const BAR_LENGTH: f32 = 600.0 / 480.0;
const INDEX_CHANGE: usize = 5;
const DECAY_PER_MS: f32 = 0.0024;
const UPDATE_MS: f64 = 50.0;
/// Bars below this are not drawn.
pub const DEAD_ZONE: f32 = 1.0 / 600.0;
/// The visualiser's alpha at rest; its bars are white at 0.2 of it.
const VISUALISER_ALPHA: f32 = 0.5;
pub const BAR_ALPHA: f32 = 0.2;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Menu/OsuLogo.cs
/// Beats come this early, so that the motion peaks on them.
pub const EARLY_MS: f64 = 60.0;
const HOVER_SCALE: f32 = 1.1;
const HOVER_MS: f64 = 500.0;
const PRESS_SCALE: f32 = 0.9;
const PRESS_MS: f64 = 1000.0;
const RELEASE_MS: f64 = 500.0;
const CLICK_FLASH: f32 = 0.4;
const CLICK_FLASH_MS: f64 = 1500.0;
const SCALE_ADJUST_CUTOFF: f32 = 0.4;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Menu/MenuSideFlashes.cs
const SIDE_DEAD_ZONE: f32 = 0.25;
const SIDE_ALPHA_MULT: f32 = (1.0 - SIDE_DEAD_ZONE) / 0.55;
const SIDE_KIAI_MULT: f32 = (1.0 - SIDE_DEAD_ZONE * 0.95) / 0.8;
const SIDE_FADE_IN_MS: f64 = 65.0;

/// Triangles thrown out at once, at most.
pub const PARTICLES: usize = 32;
/// How long a triangle flies, ms.
const PARTICLE_MS: f64 = 1400.0;

/// What the logo hears: where the music is and what it sounds like.
#[derive(Clone, Copy, Debug, Default)]
pub struct LogoAudio<'a> {
    /// Track ms of the music, `None` while nothing plays.
    pub track_ms: Option<f64>,
    pub timing: Option<&'a BeatTiming>,
    /// The music's spectrum now ([`slam_audio::SPECTRUM_BINS`] magnitudes), if known.
    pub spectrum: Option<&'a [f32]>,
}

/// One triangle the logo threw out, ready to draw.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Particle {
    /// Centre, as a part of the logo's size from its centre.
    pub offset: [f32; 2],
    /// Side, as a part of the logo's size.
    pub size: f32,
    pub rotation: f32,
    pub alpha: f32,
    /// Which of the theme's particle colours.
    pub colour: u8,
}

/// The logo as the draw side needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogoSnapshot {
    /// Product of the beat, amplitude, hover and press scales.
    pub scale: f32,
    /// The beat's ring: its scale (of the logo) and alpha.
    pub ripple_scale: f32,
    pub ripple_alpha: f32,
    /// White over the logo: click and kiai flashes.
    pub flash: f32,
    /// The visualiser's alpha and its bars (amplitudes) for one round.
    pub visualiser_alpha: f32,
    pub bars: [f32; BARS],
    pub particles: [Particle; PARTICLES],
    /// Alpha of the left and right side flashes.
    pub sides: [f32; 2],
    /// Whether the pointer is over the logo.
    pub hovered: bool,
}

impl Default for LogoSnapshot {
    fn default() -> Self {
        LogoSnapshot {
            scale: 1.0,
            ripple_scale: 1.0,
            ripple_alpha: 0.0,
            flash: 0.0,
            visualiser_alpha: VISUALISER_ALPHA,
            bars: [0.0; BARS],
            particles: [Particle::default(); PARTICLES],
            sides: [0.0; 2],
            hovered: false,
        }
    }
}

/// A value easing from `from` to `to` over `ms` from `at_ns`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tween {
    from: f32,
    to: f32,
    at_ns: u64,
    ms: f64,
    elastic: bool,
}

impl Tween {
    const fn rest(value: f32) -> Tween {
        Tween {
            from: value,
            to: value,
            at_ns: 0,
            ms: 0.0,
            elastic: false,
        }
    }

    fn value(&self, now_ns: u64) -> f32 {
        let t = elapsed_ms(self.at_ns, now_ns) / self.ms.max(f64::EPSILON);
        let t = t.clamp(0.0, 1.0) as f32;
        let eased = if self.elastic {
            ease_out_elastic(t)
        } else {
            ease_out_quad(t)
        };
        self.from + (self.to - self.from) * eased
    }

    /// Eases from where it is now to `to`.
    fn go(&mut self, to: f32, ms: f64, elastic: bool, now_ns: u64) {
        *self = Tween {
            from: self.value(now_ns),
            to,
            at_ns: now_ns,
            ms,
            elastic,
        };
    }
}

fn elapsed_ms(from_ns: u64, now_ns: u64) -> f64 {
    now_ns.saturating_sub(from_ns) as f64 / 1e6
}

/// A beat the logo reacted to.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hit {
    at_ns: u64,
    length: f64,
    /// lazer's `amplitudeAdjust`.
    strength: f32,
    kiai: bool,
    /// Kiai flashed the logo (only while an earlier flash is below 0.4).
    flash: bool,
    /// Which side flashes: bit 0 left, bit 1 right.
    sides: u8,
    side_alpha: f32,
}

/// A triangle in flight.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Flight {
    born_ns: u64,
    angle: f32,
    /// Distance travelled, as a part of the logo's size.
    reach: f32,
    size: f32,
    spin: f32,
    colour: u8,
}

/// The logo's update-side state.
#[derive(Clone, Debug, PartialEq)]
pub struct LogoMotion {
    /// Track ms of the last beat reacted to.
    last_beat: Option<f64>,
    hit: Option<Hit>,
    amplitude_scale: f32,
    hover: Tween,
    press: Tween,
    click_ns: Option<u64>,
    hovered: bool,
    bars: [f32; BARS],
    index_offset: usize,
    last_bars_ms: f64,
    last_tick_ns: Option<u64>,
    flights: [Flight; PARTICLES],
    next_flight: usize,
}

impl Default for LogoMotion {
    fn default() -> Self {
        LogoMotion {
            last_beat: None,
            hit: None,
            amplitude_scale: 1.0,
            hover: Tween::rest(1.0),
            press: Tween::rest(1.0),
            click_ns: None,
            hovered: false,
            bars: [0.0; BARS],
            index_offset: 0,
            last_bars_ms: f64::NEG_INFINITY,
            last_tick_ns: None,
            flights: [Flight::default(); PARTICLES],
            next_flight: 0,
        }
    }
}

/// A small, stable hash: the same beat throws the same triangles.
fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ x >> 33
}

/// A number in 0..1 from `seed`.
fn unit(seed: u64) -> f32 {
    (mix(seed) >> 40) as f32 / (1u64 << 24) as f32
}

impl LogoMotion {
    pub fn new() -> LogoMotion {
        LogoMotion::default()
    }

    /// Advances to `now_ns`: reacts to a new beat of `audio`, damps the scale to the
    /// loudness and moves the visualiser on. `effects` off leaves only the beat's scale.
    pub fn tick(&mut self, now_ns: u64, audio: &LogoAudio<'_>, effects: bool) {
        let dt = self
            .last_tick_ns
            .map_or(0.0, |last| elapsed_ms(last, now_ns));
        self.last_tick_ns = Some(now_ns);
        let spectrum = audio.spectrum.filter(|_| audio.track_ms.is_some());
        let maximum = spectrum.map_or(0.0, |s| s.iter().copied().fold(0.0, f32::max));
        if let Some((time, timing)) = audio.track_ms.zip(audio.timing)
            && let Some(beat) = timing.beat_at(time, EARLY_MS)
            && self.last_beat != Some(beat.start)
        {
            let first = self.last_beat.is_none();
            self.last_beat = Some(beat.start);
            // A beat already long gone when the music is joined does not fire.
            if !first || time + EARLY_MS - beat.start < EARLY_MS {
                self.on_beat(now_ns, &beat, maximum, effects);
            }
        }
        if audio.track_ms.is_none() {
            self.last_beat = None;
        }
        // Damped toward 1 - 0.04·max(0, amplitude - 0.4), lazer's Interpolation.Damp(…, 0.9).
        let target = 1.0 - (maximum - SCALE_ADJUST_CUTOFF).max(0.0) * 0.04;
        let keep = 0.9f64.powf(dt) as f32;
        self.amplitude_scale = target + (self.amplitude_scale - target) * keep;
        self.visualise(now_ns, audio, spectrum, dt, effects);
    }

    fn on_beat(&mut self, now_ns: u64, beat: &Beat, maximum: f32, effects: bool) {
        let strength = (0.4 + maximum).min(1.0);
        let flashing = self.flash_at(now_ns);
        let sides = if beat.kiai {
            if beat.index % 2 == 0 { 1 } else { 2 }
        } else if beat.index % i64::from(beat.meter) == 0 {
            3
        } else {
            0
        };
        let mult = if beat.kiai {
            SIDE_KIAI_MULT
        } else {
            SIDE_ALPHA_MULT
        };
        self.hit = Some(Hit {
            at_ns: now_ns,
            length: beat.length,
            strength,
            kiai: beat.kiai,
            flash: beat.kiai && flashing < 0.4,
            sides,
            side_alpha: (0.1 + (maximum - SIDE_DEAD_ZONE) / mult).clamp(0.1, 1.0),
        });
        if effects {
            let count = if beat.kiai { 4 } else { 2 };
            for i in 0..count {
                let seed = (beat.start.to_bits()) ^ (i as u64) << 56;
                self.flights[self.next_flight] = Flight {
                    born_ns: now_ns,
                    angle: unit(seed) * std::f32::consts::TAU,
                    reach: 0.25 + 0.35 * unit(seed ^ 1),
                    size: 0.05 + 0.08 * unit(seed ^ 2),
                    spin: (unit(seed ^ 3) - 0.5) * 2.0,
                    colour: (mix(seed ^ 4) & 1) as u8,
                };
                self.next_flight = (self.next_flight + 1) % PARTICLES;
            }
        }
    }

    /// lazer's visualiser: every 50 ms the bars jump up to the spectrum (rotated by 5
    /// bars each time, halved outside kiai); in between they decay.
    fn visualise(
        &mut self,
        now_ns: u64,
        audio: &LogoAudio<'_>,
        spectrum: Option<&[f32]>,
        dt: f64,
        effects: bool,
    ) {
        let now_ms = now_ns as f64 / 1e6;
        if !effects {
            self.bars = [0.0; BARS];
            return;
        }
        if now_ms - self.last_bars_ms >= UPDATE_MS {
            self.last_bars_ms = now_ms;
            if let Some(spectrum) = spectrum {
                let kiai = audio
                    .track_ms
                    .zip(audio.timing)
                    .and_then(|(t, timing)| timing.beat_at(t, 0.0))
                    .is_some_and(|b| b.kiai);
                let mult = if kiai { 1.0 } else { 0.5 };
                for (i, bar) in self.bars.iter_mut().enumerate() {
                    let at = (i + self.index_offset) % BARS;
                    let target = spectrum.get(at).copied().unwrap_or(0.0) * mult;
                    if target > *bar {
                        *bar = target;
                    }
                }
                self.index_offset = (self.index_offset + INDEX_CHANGE) % BARS;
            }
        }
        let decay = dt as f32 * DECAY_PER_MS;
        for bar in &mut self.bars {
            // 3% more, so that an almost gone bar goes faster.
            *bar = (*bar - decay * (*bar + 0.03)).max(0.0);
        }
    }

    /// The pointer came over the logo or left it.
    pub fn set_hovered(&mut self, hovered: bool, now_ns: u64) {
        if hovered != self.hovered {
            self.hovered = hovered;
            let to = if hovered { HOVER_SCALE } else { 1.0 };
            self.hover.go(to, HOVER_MS, true, now_ns);
        }
    }

    pub fn hovered(&self) -> bool {
        self.hovered
    }

    /// The logo was pressed: it gives under the pointer.
    pub fn press(&mut self, now_ns: u64) {
        self.press.go(PRESS_SCALE, PRESS_MS, false, now_ns);
    }

    /// The press ended: it springs back and flashes (a click).
    pub fn release(&mut self, now_ns: u64, clicked: bool) {
        self.press.go(1.0, RELEASE_MS, true, now_ns);
        if clicked {
            self.click_ns = Some(now_ns);
        }
    }

    /// A whole click at once (keyboard): a quick give and spring back, and the flash.
    pub fn click(&mut self, now_ns: u64) {
        self.press = Tween {
            from: PRESS_SCALE,
            to: 1.0,
            at_ns: now_ns,
            ms: RELEASE_MS,
            elastic: true,
        };
        self.click_ns = Some(now_ns);
    }

    fn flash_at(&self, now_ns: u64) -> f32 {
        let click = self.click_ns.map_or(0.0, |at| {
            let t = (elapsed_ms(at, now_ns) / CLICK_FLASH_MS) as f32;
            CLICK_FLASH * (1.0 - ease_out_expo(t))
        });
        let kiai = self.hit.filter(|h| h.flash).map_or(0.0, |h| {
            let ms = elapsed_ms(h.at_ns, now_ns);
            let peak = 0.2 * h.strength;
            if ms < EARLY_MS {
                peak * ease_out_quad((ms / EARLY_MS) as f32)
            } else {
                peak * (1.0 - ((ms - EARLY_MS) / h.length.max(1.0)).min(1.0) as f32)
            }
        });
        click.max(kiai)
    }

    /// The logo at `now_ns`.
    pub fn snapshot(&self, now_ns: u64) -> LogoSnapshot {
        let mut snapshot = LogoSnapshot {
            bars: self.bars,
            hovered: self.hovered,
            ..LogoSnapshot::default()
        };
        let mut beat_scale = 1.0;
        if let Some(hit) = self.hit {
            let ms = elapsed_ms(hit.at_ns, now_ns);
            // Down by 0.02·a in 60 ms (Out), back over two beats (OutQuint).
            let low = 1.0 - 0.02 * hit.strength;
            beat_scale = if ms < EARLY_MS {
                1.0 - (1.0 - low) * ease_out_quad((ms / EARLY_MS) as f32)
            } else {
                let t = ((ms - EARLY_MS) / (2.0 * hit.length).max(1.0)) as f32;
                low + (1.0 - low) * ease_out_quint(t)
            };
            let t = (ms / hit.length.max(1.0)) as f32;
            let eased = ease_out_quint(t);
            snapshot.ripple_scale = 1.0 + 0.04 * hit.strength * eased;
            snapshot.ripple_alpha = 0.15 * hit.strength * (1.0 - eased);
            if hit.flash {
                let boost = VISUALISER_ALPHA * 1.8 * hit.strength;
                snapshot.visualiser_alpha = if ms < EARLY_MS {
                    VISUALISER_ALPHA
                        + (boost - VISUALISER_ALPHA) * ease_out_quad((ms / EARLY_MS) as f32)
                } else {
                    let t = ((ms - EARLY_MS) / hit.length.max(1.0)).min(1.0) as f32;
                    boost + (VISUALISER_ALPHA - boost) * t
                };
            }
            if hit.sides != 0 {
                let alpha = if ms < SIDE_FADE_IN_MS {
                    hit.side_alpha * (ms / SIDE_FADE_IN_MS) as f32
                } else {
                    // FadeOut over the beat, Easing.In.
                    let t = ((ms - SIDE_FADE_IN_MS) / hit.length.max(1.0)).min(1.0) as f32;
                    hit.side_alpha * (1.0 - t * t)
                };
                for (side, value) in snapshot.sides.iter_mut().enumerate() {
                    if hit.sides >> side & 1 == 1 {
                        *value = alpha;
                    }
                }
            }
        }
        snapshot.flash = self.flash_at(now_ns);
        snapshot.scale =
            beat_scale * self.amplitude_scale * self.hover.value(now_ns) * self.press.value(now_ns);
        for (out, flight) in snapshot.particles.iter_mut().zip(&self.flights) {
            if flight.size == 0.0 {
                continue;
            }
            let t = (elapsed_ms(flight.born_ns, now_ns) / PARTICLE_MS) as f32;
            if t >= 1.0 {
                continue;
            }
            let distance = 0.5 + flight.reach * ease_out_expo(t);
            let (sin, cos) = flight.angle.sin_cos();
            *out = Particle {
                offset: [cos * distance, sin * distance],
                size: flight.size * (1.0 - 0.3 * t),
                rotation: flight.spin * t,
                alpha: (1.0 - t) * (t * 8.0).min(1.0),
                colour: flight.colour,
            };
        }
        snapshot
    }
}

#[cfg(test)]
mod tests;
