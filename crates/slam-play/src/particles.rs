//! Cursor particles of legacy skins (`star2`): stars that spew from the cursor during breaks,
//! towards the side of the buttons held, and in kiai while a kiai slider or spinner is
//! tracked.
//!
//! As in lazer, the particles live on the draw side and run on the game clock of the frame:
//! the update side only tells whether gameplay is in a break, whether a kiai object is
//! tracked, and which buttons are held. Positions are in osu!pixels (the playfield's space,
//! where lazer's spewer lives); the cursor's velocity is measured in window pixels per ms
//! and added as is, as lazer adds its screen-space velocity to local units. Particles are
//! cosmetic: the random numbers come from a fixed-seed generator, not lazer's unseeded one.
//!
//! Not done: lazer measures the velocity on every mouse move (adding the frame's elapsed
//! time each time); here it is measured once a frame from the drawn cursor. A spewer turning
//! on spawns in the same frame; lazer's, reset after the frame's mouse move, a frame later.
//! Scale and angle are interpolated in `f32`, lazer's `Interpolation.Lerp` in `f64`.

use slam_osu::Vec2;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};

use crate::circles::SkinSprite;
use crate::cursor::STABLE_MAGIC_SCALE_FACTOR;
use crate::playfield::PlayfieldLayout;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorParticles.cs
const PARTICLE_DURATION_MIN: f32 = 300.0;
const PARTICLE_DURATION_MAX: f32 = 1000.0;
const PARTICLE_GRAVITY: f32 = 240.0;
const MAX_VELOCITY_FRAME_LENGTH: f64 = 15.0;
/// The break spewer's particles per second.
const BREAK_PER_SECOND: usize = 20;
/// The kiai spewer's particles per second.
const KIAI_PER_SECOND: usize = 60;
/// Particles alive at once: lazer's `perSecond * ceil(maxDuration / 1000)`.
const BREAK_PARTICLES: usize = BREAK_PER_SECOND;
const KIAI_PARTICLES: usize = KIAI_PER_SECOND;

/// The most sprites [`CursorParticles::push`] writes.
pub const MAX_PARTICLE_SPRITES: usize = BREAK_PARTICLES + KIAI_PARTICLES;

/// `StarBreakAdditive` when the skin sets none.
pub const DEFAULT_STAR_COLOUR: Color = Color {
    r: 1.0,
    g: 182.0 / 255.0,
    b: 193.0 / 255.0,
    a: 1.0,
};

/// Where the break spewer throws its particles, by the buttons held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Direction {
    #[default]
    None,
    Left,
    Right,
    Omni,
}

impl Direction {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorParticles.cs (handleInput)
    fn of(left: bool, right: bool) -> Direction {
        match (left, right) {
            (true, true) => Direction::Omni,
            (true, false) => Direction::Left,
            (false, true) => Direction::Right,
            (false, false) => Direction::None,
        }
    }
}

/// Lazer's `FallingParticle`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Particle {
    start_time: f32,
    start_position: Vec2,
    velocity: Vec2,
    /// 0 for a slot never used.
    duration: f32,
    start_angle: f32,
    end_angle: f32,
    end_scale: f32,
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/ParticleSpewer.cs (FallingParticle)
impl Particle {
    fn progress(&self, since: f32) -> f32 {
        (since / self.duration).clamp(0.0, 1.0)
    }

    fn alpha(&self, since: f32) -> f32 {
        1.0 - self.progress(since)
    }

    fn scale(&self, since: f32) -> f32 {
        1.0 + (self.end_scale - 1.0) * self.progress(since)
    }

    fn angle(&self, since: f32) -> f32 {
        self.start_angle + (self.end_angle - self.start_angle) * self.progress(since)
    }

    fn position(&self, since: f32) -> Vec2 {
        let gravity =
            PARTICLE_GRAVITY * self.duration / PARTICLE_DURATION_MAX * self.progress(since);
        // `(Velocity + currentGravity) * timeSinceStart / maxDuration`, in that order.
        Vec2::new(
            self.start_position.x + self.velocity.x * since / PARTICLE_DURATION_MAX,
            self.start_position.y + (self.velocity.y + gravity) * since / PARTICLE_DURATION_MAX,
        )
    }
}

/// A fixed-seed xorshift: particles only need to look random.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rng(u64);

impl Rng {
    /// In `0..1`, as `RNG.NextSingle()`.
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }

    /// In `min..max`, as `RNG.NextSingle(min, max)`.
    fn between(&mut self, min: f32, max: f32) -> f32 {
        min + self.next() * (max - min)
    }
}

/// Lazer's `ParticleSpewer` with `LegacyCursorParticleSpewer`, holding `N` particles.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Spewer<const N: usize> {
    particles: [Particle; N],
    current: usize,
    last_added: Option<f64>,
    time_between: f64,
    active: bool,
    direction: Direction,
    /// The cursor in window pixels, once it moved since the spewer turned on.
    cursor: Option<Vec2>,
    velocity: Vec2,
    velocity_frame_length: f64,
    total_difference: Vec2,
}

impl<const N: usize> Spewer<N> {
    fn new(per_second: usize) -> Self {
        Spewer {
            particles: [Particle::default(); N],
            current: 0,
            last_added: None,
            time_between: f64::from(1000.0f32 / per_second as f32),
            active: false,
            direction: Direction::None,
            cursor: None,
            velocity: Vec2::ZERO,
            velocity_frame_length: 0.0,
            total_difference: Vec2::ZERO,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/ParticleSpewer.cs (LoadComplete)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorParticles.cs (resetVelocityCalculation)
    fn set_active(&mut self, active: bool) {
        if active == self.active {
            return;
        }
        self.active = active;
        self.cursor = None;
        self.total_difference = Vec2::ZERO;
        self.velocity_frame_length = 0.0;
        if active {
            self.last_added = None;
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorParticles.cs (OnMouseMove)
    fn cursor_moved(&mut self, screen: Vec2, elapsed: f64) {
        let Some(previous) = self.cursor else {
            self.cursor = Some(screen);
            return;
        };
        self.total_difference = Vec2::new(
            self.total_difference.x + (screen.x - previous.x),
            self.total_difference.y + (screen.y - previous.y),
        );
        self.cursor = Some(screen);
        self.velocity_frame_length += elapsed.abs();
        if self.velocity_frame_length > MAX_VELOCITY_FRAME_LENGTH {
            let length = self.velocity_frame_length as f32;
            self.velocity = Vec2::new(
                self.total_difference.x / length,
                self.total_difference.y / length,
            );
            self.total_difference = Vec2::ZERO;
            self.velocity_frame_length = 0.0;
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/ParticleSpewer.cs (Update)
    fn update(&mut self, time: f64, at: Vec2, rng: &mut Rng) {
        if !self.active || self.cursor.is_none() {
            return;
        }
        let Some(last) = self.last_added else {
            self.last_added = Some(time);
            self.spawn(time, at, rng);
            return;
        };
        let elapsed = time - last;
        // Not too many particles after a long time.
        if elapsed.abs() > f64::from(PARTICLE_DURATION_MAX) {
            self.last_added = Some(time);
            self.spawn(time, at, rng);
            return;
        }
        let mut added = last;
        let mut i = 0;
        while f64::from(i) < elapsed / self.time_between {
            added += self.time_between;
            self.spawn(added, at, rng);
            i += 1;
        }
        self.last_added = Some(added);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorParticles.cs (CreateParticle, getVelocity)
    fn spawn(&mut self, start_time: f64, at: Vec2, rng: &mut Rng) {
        let duration = rng.between(PARTICLE_DURATION_MIN, PARTICLE_DURATION_MAX);
        let start_angle = rng.next() * 4.0 - 2.0;
        let end_angle = rng.between(-2.0, 2.0);
        let end_scale = rng.next() * 2.0;
        let base = match self.direction {
            Direction::Left => Vec2::new(rng.between(-460.0, 0.0), rng.between(-40.0, 40.0)),
            Direction::Right => Vec2::new(rng.between(0.0, 460.0), rng.between(-40.0, 40.0)),
            Direction::Omni => Vec2::new(rng.between(-460.0, 460.0), rng.between(-160.0, 160.0)),
            Direction::None => Vec2::ZERO,
        };
        self.particles[self.current] = Particle {
            start_time: start_time as f32,
            start_position: at,
            velocity: Vec2::new(
                base.x + self.velocity.x * 40.0,
                base.y + self.velocity.y * 40.0,
            ),
            duration,
            start_angle,
            end_angle,
            end_scale,
        };
        self.current = (self.current + 1) % N;
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/ParticleSpewer.cs (ParticleSpewerDrawNode.Blit)
    fn push(&self, batch: &mut SpriteBatch, layout: &PlayfieldLayout, time: f64, star: &Star) {
        let now = time as f32;
        for p in &self.particles {
            if p.duration == 0.0 {
                continue;
            }
            let since = now - p.start_time;
            // Particles from the future, after a seek back.
            if since < 0.0 {
                continue;
            }
            let alpha = p.alpha(since);
            if alpha <= 0.0 {
                continue;
            }
            let scale = p.scale(since) * layout.scale();
            let size = [star.display.x * scale, star.display.y * scale];
            let at = layout.to_window(p.position(since));
            let mut sprite = Sprite::new(star.sprite.texture, star.sprite.uv, [at.x, at.y], size);
            sprite.origin = [size[0] / 2.0, size[1] / 2.0];
            // Lazer turns the quad counter-clockwise on screen; sprites turn clockwise.
            sprite.rotation = -p.angle(since);
            sprite.colour = Color {
                a: star.colour.a * alpha,
                ..star.colour
            };
            sprite.blend = Blend::Additive;
            batch.push(&sprite);
        }
    }
}

/// What the particles draw with.
#[derive(Debug, Clone, Copy)]
pub struct Star<'a> {
    pub sprite: &'a SkinSprite,
    /// The texture's display size in osu!pixels (`ScaleAdjust` times 1.6).
    pub display: Vec2,
    /// `StarBreakAdditive`.
    pub colour: Color,
}

impl<'a> Star<'a> {
    pub fn new(sprite: &'a SkinSprite, colour: Color) -> Self {
        Star {
            sprite,
            display: Vec2::new(
                sprite.size.x / STABLE_MAGIC_SCALE_FACTOR,
                sprite.size.y / STABLE_MAGIC_SCALE_FACTOR,
            ),
            colour,
        }
    }
}

/// One frame's inputs to [`CursorParticles::update`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleFrame {
    /// Game time of the frame.
    pub time: f64,
    /// The cursor as drawn, in osu!pixels; `None` before it first moved.
    pub cursor: Option<Vec2>,
    /// Gameplay is in a break (lazer's `IsBreakTime`).
    pub break_time: bool,
    /// Kiai, with a kiai slider or spinner tracked.
    pub kiai_tracking: bool,
    /// The left and right buttons held.
    pub left: bool,
    pub right: bool,
    /// Times the play was restarted: a new play has no particles.
    pub restarts: u32,
}

/// Lazer's `LegacyCursorParticles`: the break and kiai spewers.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorParticles {
    break_spewer: Spewer<BREAK_PARTICLES>,
    kiai_spewer: Spewer<KIAI_PARTICLES>,
    rng: Rng,
    previous_time: Option<f64>,
    restarts: u32,
}

impl Default for CursorParticles {
    fn default() -> Self {
        CursorParticles {
            break_spewer: Spewer::new(BREAK_PER_SECOND),
            kiai_spewer: Spewer::new(KIAI_PER_SECOND),
            rng: Rng(0x9E37_79B9_7F4A_7C15),
            previous_time: None,
            restarts: 0,
        }
    }
}

impl CursorParticles {
    /// Moves on to `frame`: spewers turn on and off, take the cursor's motion and spawn.
    /// Allocation-free.
    pub fn update(&mut self, frame: &ParticleFrame, layout: &PlayfieldLayout) {
        if frame.restarts != self.restarts {
            *self = CursorParticles {
                restarts: frame.restarts,
                ..CursorParticles::default()
            };
        }
        let elapsed = self.previous_time.map_or(0.0, |t| frame.time - t);
        self.previous_time = Some(frame.time);
        self.break_spewer.set_active(frame.break_time);
        self.break_spewer.direction = Direction::of(frame.left, frame.right);
        self.kiai_spewer.set_active(frame.kiai_tracking);
        let Some(cursor) = frame.cursor else {
            return;
        };
        // `IRequireHighFrequencyMousePosition`: a move every frame.
        let screen = layout.to_window(cursor);
        self.break_spewer.cursor_moved(screen, elapsed);
        self.kiai_spewer.cursor_moved(screen, elapsed);
        self.break_spewer.update(frame.time, cursor, &mut self.rng);
        self.kiai_spewer.update(frame.time, cursor, &mut self.rng);
    }

    /// Writes the particles alive at game `time`: at most [`MAX_PARTICLE_SPRITES`].
    pub fn push(&self, batch: &mut SpriteBatch, layout: &PlayfieldLayout, time: f64, star: &Star) {
        self.break_spewer.push(batch, layout, time, star);
        self.kiai_spewer.push(batch, layout, time, star);
    }

    /// Particles of both spewers ever spawned and not yet faded at `time`, for tests.
    pub fn alive(&self, time: f64) -> usize {
        let now = time as f32;
        let alive = |p: &Particle| {
            let since = now - p.start_time;
            p.duration > 0.0 && since >= 0.0 && p.alpha(since) > 0.0
        };
        self.break_spewer
            .particles
            .iter()
            .filter(|p| alive(p))
            .count()
            + self
                .kiai_spewer
                .particles
                .iter()
                .filter(|p| alive(p))
                .count()
    }
}

#[cfg(test)]
mod tests;
