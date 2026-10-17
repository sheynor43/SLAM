//! The legacy hit error meter: lazer's `LegacyBarHitErrorMeter`, a port of stable's
//! `ScoreMeterError`. Lazer's legacy skins put it at the bottom centre of the HUD.
//!
//! It reads the same [`HitErrorRing`] as the bar meter ([`crate::hit_error`]) and, like it,
//! evaluates its ticks and arrow at the frame's game time from the judgement's time
//! (ADR-0043, ADR-0050).

use slam_osu::hit_windows::HitWindows;
use slam_osu::judgement::HitResult;

use slam_render::hal::Extent;
use slam_render::image::Image;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};

use crate::circles::{Easing, value_at};
use crate::hit_error::{HitErrorRing, MeterTextures, TARGET_SIZE, colour_for};

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkin.cs (STABLE_MAGIC_SCALE_FACTOR)
const STABLE_MAGIC_SCALE_FACTOR: f32 = 1.6;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/LegacyBarHitErrorMeter.cs
const BAR_HEIGHT: f32 = 3.0;
/// The meter's height: the background, the centre line and the ticks.
const METER_HEIGHT: f32 = BAR_HEIGHT * 4.0 * STABLE_MAGIC_SCALE_FACTOR;
const CENTRE_LINE_WIDTH: f32 = 1.5 * STABLE_MAGIC_SCALE_FACTOR;
const BACKGROUND_ALPHA: f32 = 0.6;
const TICK_WIDTH: f32 = 3.0;
const TICK_ALPHA: f32 = 0.4;
const TICK_FADE_OUT_DURATION: f64 = 10_000.0;
const ARROW_SIZE: [f32; 2] = [17.0 * 0.6, 8.0 * 0.6];
const ARROW_MOVE_DURATION: f64 = 800.0;

/// The most ticks alive at once; beyond it the oldest goes. Lazer's pool grows instead.
pub const MAX_TICKS: usize = 256;

/// The most sprites [`LegacyHitErrorView::push`] writes: the background, three colour bars,
/// the centre line, the ticks and the arrow.
pub const MAX_LEGACY_METER_SPRITES: usize = 1 + 3 + 1 + MAX_TICKS + 1;

/// Where an offset lands on the meter: -0.5 at the early end, 0.5 at the late end.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/LegacyBarHitErrorMeter.cs (getRelativeJudgementPosition)
pub fn relative_position(offset: f64, max_window: f64) -> f32 {
    ((offset / max_window) as f32 / 2.0).clamp(-0.5, 0.5)
}

/// A judgement's tick on the meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    /// Gameplay time it appeared at.
    pub time: f64,
    /// Across the meter, -0.5..=0.5.
    pub position: f32,
    pub result: HitResult,
}

impl Tick {
    const EMPTY: Tick = Tick {
        time: 0.0,
        position: 0.0,
        result: HitResult::None,
    };

    /// When it expires.
    pub fn end(&self) -> f64 {
        self.time + TICK_FADE_OUT_DURATION
    }

    /// Its alpha at game `time`: 0.4 at once, then a linear fade out over 10 s.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/LegacyBarHitErrorMeter.cs (JudgementLine.PrepareForUse)
    pub fn alpha(&self, time: f64) -> f32 {
        if time < self.time {
            return 0.0;
        }
        value_at(
            time.min(self.end()),
            TICK_ALPHA,
            0.0,
            self.time,
            self.end(),
            Easing::None,
        )
    }
}

/// The average arrow's move across the meter.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ArrowMove {
    from: f32,
    to: f32,
    start: f64,
}

impl ArrowMove {
    const CENTRE: ArrowMove = ArrowMove {
        from: 0.0,
        to: 0.0,
        start: f64::NEG_INFINITY,
    };

    fn at(&self, time: f64) -> f32 {
        if time >= self.start + ARROW_MOVE_DURATION {
            self.to
        } else if time <= self.start {
            self.from
        } else {
            value_at(
                time,
                self.from,
                self.to,
                self.start,
                self.start + ARROW_MOVE_DURATION,
                Easing::Out,
            )
        }
    }
}

/// The legacy meter of a play on the draw side.
#[derive(Debug, Clone)]
pub struct LegacyHitErrorView {
    /// The colour bars from the widest: Meh, Ok, Great with their windows.
    windows: [(HitResult, f64); 3],
    max_window: f64,
    /// Multiplier of the meter's size (ADR-0050).
    size: f32,
    /// Alive ticks, oldest first.
    ticks: [Tick; MAX_TICKS],
    len: usize,
    /// Judgements of the ring taken.
    seen: u64,
    restarts: u32,
    /// Ticks that went early: the ring overflowed between frames or too many were alive.
    dropped: u64,
    floating_error: f32,
    arrow: ArrowMove,
}

impl LegacyHitErrorView {
    /// The meter of a play with the hit `windows` of its beatmap (mods applied).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/LegacyBarHitErrorMeter.cs (load)
    pub fn new(windows: &HitWindows) -> Self {
        // `GetAllAvailableWindows` of the hits, in `HitResult` order: Meh, Ok, Great.
        let windows = [HitResult::Meh, HitResult::Ok, HitResult::Great]
            .map(|result| (result, windows.window_for(result)));
        let max_window = windows.iter().fold(1.0, |max: f64, w| max.max(w.1));
        LegacyHitErrorView {
            windows,
            max_window,
            size: 1.0,
            ticks: [Tick::EMPTY; MAX_TICKS],
            len: 0,
            seen: 0,
            restarts: 0,
            dropped: 0,
            floating_error: 0.0,
            arrow: ArrowMove::CENTRE,
        }
    }

    /// The meter `size` times as large (1 is lazer's).
    pub fn sized(self, size: f32) -> Self {
        debug_assert!(size.is_finite() && size > 0.0, "meter size {size}");
        LegacyHitErrorView { size, ..self }
    }

    /// Takes the new judgements of `ring` and drops the ticks expired by game `time`. A
    /// restart clears the meter: lazer builds a new HUD.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/LegacyBarHitErrorMeter.cs (OnNewJudgement)
    pub fn update(&mut self, ring: &HitErrorRing, restarts: u32, time: f64) {
        if restarts != self.restarts {
            self.restarts = restarts;
            self.len = 0;
            self.floating_error = 0.0;
            self.arrow = ArrowMove::CENTRE;
        }
        self.dropped += ring.skipped(self.seen);
        for error in ring.since(self.seen) {
            // Ticks and the arrow start at the judgement's time, not the frame's (ADR-0043).
            let now = error.time;
            self.expire(now);
            let position = relative_position(error.offset, self.max_window);
            if self.len == MAX_TICKS {
                self.ticks.copy_within(1..self.len, 0);
                self.len -= 1;
                self.dropped += 1;
            }
            self.ticks[self.len] = Tick {
                time: now,
                position,
                result: error.result,
            };
            self.len += 1;
            self.floating_error = self.floating_error * 0.8 + position * 0.2;
            self.arrow = ArrowMove {
                from: self.arrow.at(now),
                to: self.floating_error,
                start: now,
            };
        }
        self.seen = ring.written();
        self.expire(time);
    }

    /// Drops the ticks expired at `time`.
    fn expire(&mut self, time: f64) {
        let mut kept = 0;
        for i in 0..self.len {
            if time < self.ticks[i].end() {
                self.ticks[kept] = self.ticks[i];
                kept += 1;
            }
        }
        self.len = kept;
    }

    /// Lazer's `Clear` on a seek: the ticks go, the average and the arrow stay.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/LegacyBarHitErrorMeter.cs (Clear)
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// The alive ticks, oldest first.
    pub fn ticks(&self) -> &[Tick] {
        &self.ticks[..self.len]
    }

    /// Ticks that went before their end.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The moving average of the ticks' positions, -0.5..=0.5.
    pub fn floating_error(&self) -> f32 {
        self.floating_error
    }

    /// Where the average arrow is across the meter at game `time`, -0.5..=0.5.
    pub fn arrow_position(&self, time: f64) -> f32 {
        self.arrow.at(time)
    }

    /// Writes the meter at game `time` on a window of `width` × `height` pixels.
    /// Allocation-free once `batch` has room for [`MAX_LEGACY_METER_SPRITES`].
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/LegacyBarHitErrorMeter.cs (load)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs (MainHUDComponents)
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        width: f32,
        height: f32,
        time: f64,
        textures: &MeterTextures,
    ) {
        // Anchored and originated at the bottom centre of the HUD, scaled around that point.
        let scale = (width / TARGET_SIZE[0]).min(height / TARGET_SIZE[1]) * self.size;
        let meter_width = self.max_window as f32 * STABLE_MAGIC_SCALE_FACTOR;
        let centre = [width / 2.0, height - METER_HEIGHT / 2.0 * scale];
        let white = textures.white;
        let mut rect = |x: f32, size: [f32; 2], colour: Color, blend: Blend| {
            let size = [size[0] * scale, size[1] * scale];
            let mut sprite = Sprite::new(
                white.texture,
                white.uv,
                [
                    centre[0] + x * scale - size[0] / 2.0,
                    centre[1] - size[1] / 2.0,
                ],
                size,
            );
            sprite.colour = colour;
            sprite.blend = blend;
            batch.push(&sprite);
        };

        rect(
            0.0,
            [meter_width, METER_HEIGHT],
            Color {
                a: BACKGROUND_ALPHA,
                ..Color::BLACK
            },
            Blend::Alpha,
        );
        for &(result, window) in &self.windows {
            rect(
                0.0,
                [
                    window as f32 * STABLE_MAGIC_SCALE_FACTOR,
                    BAR_HEIGHT * STABLE_MAGIC_SCALE_FACTOR,
                ],
                colour_for(result),
                Blend::Alpha,
            );
        }
        rect(
            0.0,
            [CENTRE_LINE_WIDTH, METER_HEIGHT],
            Color::WHITE,
            Blend::Alpha,
        );
        for tick in self.ticks() {
            let alpha = tick.alpha(time);
            if alpha <= 0.0 {
                continue;
            }
            let colour = colour_for(tick.result);
            rect(
                tick.position * meter_width,
                [TICK_WIDTH, METER_HEIGHT],
                Color {
                    a: colour.a * alpha,
                    ..colour
                },
                Blend::Additive,
            );
        }
        // A triangle hanging from the top edge, pointing down: lazer's upward `Triangle`
        // originated at its bottom centre on the top edge and flipped vertically.
        let size = [ARROW_SIZE[0] * scale, ARROW_SIZE[1] * scale];
        let x = centre[0] + self.arrow.at(time) * meter_width * scale;
        let top = height - METER_HEIGHT * scale;
        let triangle = textures.triangle;
        let mut sprite = Sprite::new(
            triangle.texture,
            triangle.uv,
            [x - size[0] / 2.0, top],
            size,
        );
        sprite.blend = Blend::Alpha;
        batch.push(&sprite);
    }
}

/// Size of the triangle image: the arrow's proportions, large enough for smooth edges.
const TRIANGLE_SIZE: [u32; 2] = [68, 32];
/// Samples per pixel side when covering the triangle.
const TRIANGLE_SAMPLES: u32 = 4;

/// A white triangle on transparent, pointing down (its base along the top edge, its apex at
/// the bottom centre), anti-aliased by supersampling, for [`MeterTextures::triangle`].
pub fn triangle_image() -> Image {
    let [w, h] = TRIANGLE_SIZE;
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    let half = w as f32 / 2.0;
    for y in 0..h {
        for x in 0..w {
            let mut covered = 0;
            for sy in 0..TRIANGLE_SAMPLES {
                for sx in 0..TRIANGLE_SAMPLES {
                    let px = x as f32 + (sx as f32 + 0.5) / TRIANGLE_SAMPLES as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / TRIANGLE_SAMPLES as f32;
                    // The half width shrinks linearly from the base to the apex.
                    if (px - half).abs() <= half * (1.0 - py / h as f32) {
                        covered += 1;
                    }
                }
            }
            let coverage = covered as f32 / (TRIANGLE_SAMPLES * TRIANGLE_SAMPLES) as f32;
            pixels.extend_from_slice(&[
                u8::MAX,
                u8::MAX,
                u8::MAX,
                (coverage * 255.0).round() as u8,
            ]);
        }
    }
    Image {
        size: Extent::new(w, h),
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_half_the_share_of_the_widest_window_and_clamp() {
        assert_eq!(relative_position(0.0, 100.0), 0.0);
        assert_eq!(relative_position(-100.0, 100.0), -0.5);
        assert_eq!(relative_position(50.0, 100.0), 0.25);
        assert_eq!(relative_position(-400.0, 100.0), -0.5);
        assert_eq!(relative_position(400.0, 100.0), 0.5);
    }

    #[test]
    fn the_triangle_points_down() {
        let image = triangle_image();
        let [w, h] = TRIANGLE_SIZE;
        let alpha = |x: u32, y: u32| image.pixels[((y * w + x) * 4 + 3) as usize];
        assert_eq!(alpha(w / 2, 0), 255);
        assert_eq!(alpha(1, 0), 255);
        assert_eq!(alpha(1, h - 1), 0);
        assert!(alpha(w / 2, h - 1) > 0);
    }
}
