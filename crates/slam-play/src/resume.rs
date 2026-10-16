//! The resume overlay (lazer's `OsuResumeOverlay`), draw side: the click-to-resume target
//! where gameplay paused and a cursor of its own with its trail, following the window's
//! cursor while gameplay's stands still and is hidden.
//!
//! The target is not lazer's: lazer draws the skin's cursor tinted orange, here it is an
//! orange disc pulsing towards red, so that it does not look like a second cursor. Its
//! size, appearance and hover colour follow lazer.

use slam_osu::Vec2;
use slam_render::atlas::AtlasRegion;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};

use crate::circles::Easing;
use crate::cursor::{CursorFrame, CursorTextures, CursorView, ExpandTimes};
use crate::playfield::PlayfieldLayout;
use crate::session::{PlaySnapshot, PlayState, RESUME_CURSOR_SIZE, hovers};

/// Sprites the target writes: its glow and itself. The overlay's cursor writes
/// [`MAX_CURSOR_SPRITES`](crate::cursor::MAX_CURSOR_SPRITES) at most, while gameplay's
/// writes none.
pub const MAX_TARGET_SPRITES: usize = 2;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuResumeOverlay.cs (Appear)
/// The target appears four times its size and shrinks to it in a second.
const APPEAR_SCALE: f64 = 4.0;
const APPEAR_NS: f64 = 1_000_000_000.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuResumeOverlay.cs (updateColour)
/// Hovered, the target fades to white, and back when left.
const HOVER_FADE_NS: f64 = 400_000_000.0;

/// One pulse of the target, in nanoseconds.
const PULSE_NS: f64 = 1_200_000_000.0;
/// How much the target grows at the height of a pulse.
const PULSE_SCALE: f32 = 0.08;
/// The glow around the target, as a multiple of its size, and its opacity.
const GLOW_SIZE: f32 = 1.6;
const GLOW_ALPHA: f32 = 0.35;

/// lazer's `Color4.Orange`.
const ORANGE: Color = Color {
    r: 1.0,
    g: 0.647,
    b: 0.0,
    a: 1.0,
};
/// The colour the target pulses towards.
const RED: Color = Color {
    r: 1.0,
    g: 0.25,
    b: 0.1,
    a: 1.0,
};
/// How far towards [`RED`] a pulse goes.
const PULSE_RED: f32 = 0.45;

/// The draw side of the resume overlay.
#[derive(Debug, Clone, Default)]
pub struct ResumeView {
    cursor: CursorView,
    /// The resume shown ([`PlaySnapshot::resumes`]) and the engine time it was first drawn.
    shown: Option<(u32, u64)>,
    hover: HoverFade,
}

/// The target's fade to white and back.
#[derive(Debug, Clone, Copy, Default)]
struct HoverFade {
    hovered: bool,
    /// Engine time of the last change, and how white the target was then.
    changed_ns: u64,
    from: f32,
}

impl HoverFade {
    fn white(&self, now_ns: u64) -> f32 {
        let to = if self.hovered { 1.0 } else { 0.0 };
        let progress = (now_ns.saturating_sub(self.changed_ns) as f64 / HOVER_FADE_NS).min(1.0);
        self.from + (to - self.from) * Easing::OutQuint.apply(progress) as f32
    }

    fn set(&mut self, hovered: bool, now_ns: u64) {
        if hovered != self.hovered {
            self.from = self.white(now_ns);
            self.hovered = hovered;
            self.changed_ns = now_ns;
        }
    }
}

impl ResumeView {
    /// Writes the target and the overlay's cursor of `snapshot` drawn at engine time
    /// `now_ns`, while it is [`PlayState::Resuming`]; nothing otherwise. `disc` is a white
    /// disc on transparent. Allocation-free once `batch` has room for the cursor's
    /// [`MAX_CURSOR_SPRITES`](crate::cursor::MAX_CURSOR_SPRITES) and
    /// [`MAX_TARGET_SPRITES`].
    pub fn push(
        &mut self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        snapshot: &PlaySnapshot,
        now_ns: u64,
        disc: Option<AtlasRegion>,
        cursor: &CursorTextures,
    ) {
        if snapshot.state != PlayState::Resuming {
            return;
        }
        let shown_ns = match self.shown {
            Some((resumes, at)) if resumes == snapshot.resumes => at,
            _ => {
                self.shown = Some((snapshot.resumes, now_ns));
                // Lazer's `Appear` fades from the colour the target had: white, as a click
                // on it left it (and as it starts).
                self.hover = HoverFade {
                    hovered: hovers(snapshot.resume_target, snapshot.resume_cursor),
                    changed_ns: now_ns,
                    from: 1.0,
                };
                now_ns
            }
        };
        self.hover.set(
            hovers(snapshot.resume_target, snapshot.resume_cursor),
            now_ns,
        );
        if let Some(disc) = disc {
            let elapsed = now_ns.saturating_sub(shown_ns) as f64;
            let target = TargetPose::at(elapsed, self.hover.white(now_ns));
            target.push(batch, layout, snapshot.resume_target, disc);
        }

        let frame = CursorFrame {
            path: &snapshot.resume_cursor_path,
            position: snapshot.resume_cursor,
            expand: ExpandTimes::default(),
            // The overlay is outside gameplay's clock, which stands still: its cursor spins
            // on in real time.
            time: now_ns as f64 / 1e6,
            restarts: snapshot.resumes,
            now_ns,
            visible: true,
        };
        self.cursor.push(batch, layout, &frame, cursor);
    }
}

/// How the target looks at a moment.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TargetPose {
    /// Size relative to [`RESUME_CURSOR_SIZE`].
    scale: f32,
    colour: Color,
    /// The glow's opacity.
    glow: f32,
}

impl TargetPose {
    /// `elapsed` nanoseconds after it appeared, `white` (0..=1) of the way to white.
    fn at(elapsed: f64, white: f32) -> TargetPose {
        let appear = (elapsed / APPEAR_NS).min(1.0);
        let appear_scale = APPEAR_SCALE + (1.0 - APPEAR_SCALE) * Easing::OutQuint.apply(appear);
        // 0 at the start of a pulse, 1 at its height: a smooth cosine.
        let phase = (elapsed / PULSE_NS).fract();
        let pulse = (0.5 - 0.5 * (phase * std::f64::consts::TAU).cos()) as f32;
        let colour = mix(mix(ORANGE, RED, pulse * PULSE_RED), Color::WHITE, white);
        TargetPose {
            scale: appear_scale as f32 * (1.0 + PULSE_SCALE * pulse),
            colour,
            glow: GLOW_ALPHA * pulse,
        }
    }

    fn push(&self, batch: &mut SpriteBatch, layout: &PlayfieldLayout, at: Vec2, disc: AtlasRegion) {
        let centre = layout.to_window(at);
        let side = RESUME_CURSOR_SIZE * layout.scale() * self.scale;
        let mut circle = |side: f32, colour: Color| {
            let mut sprite = Sprite::new(
                disc.texture,
                disc.uv,
                [centre.x - side / 2.0, centre.y - side / 2.0],
                [side, side],
            );
            sprite.colour = colour;
            sprite.blend = Blend::Alpha;
            batch.push(&sprite);
        };
        circle(
            side * GLOW_SIZE,
            Color {
                a: self.glow,
                ..self.colour
            },
        );
        circle(side, self.colour);
    }
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: f64 = 1_000_000.0;

    #[test]
    fn the_target_appears_four_times_its_size_and_shrinks_in_a_second() {
        assert_eq!(TargetPose::at(0.0, 0.0).scale, 4.0);
        let halfway = TargetPose::at(500.0 * MS, 0.0).scale;
        assert!(halfway > 1.0 && halfway < 1.5, "{halfway}");
        // A whole pulse after appearing: settled, at the start of the next pulse.
        assert!((TargetPose::at(PULSE_NS, 0.0).scale - 1.0).abs() < 1e-4);
    }

    #[test]
    fn the_target_pulses_from_orange_towards_red_and_back() {
        let start = TargetPose::at(PULSE_NS * 2.0, 0.0);
        let height = TargetPose::at(PULSE_NS * 2.5, 0.0);
        assert!((start.colour.g - ORANGE.g).abs() < 1e-4);
        assert_eq!(start.glow, 0.0);
        let red = mix(ORANGE, RED, PULSE_RED);
        assert!((height.colour.g - red.g).abs() < 1e-4);
        assert!((height.colour.b - red.b).abs() < 1e-4);
        assert!((height.glow - GLOW_ALPHA).abs() < 1e-4);
        assert!((height.scale - (1.0 + PULSE_SCALE)).abs() < 1e-4);
    }

    #[test]
    fn hovering_fades_the_target_to_white_in_400_ms() {
        let mut hover = HoverFade::default();
        assert_eq!(hover.white(0), 0.0);
        hover.set(true, 0);
        assert!(hover.white(100 * MS as u64) > 0.5);
        assert_eq!(hover.white(400 * MS as u64), 1.0);
        assert_eq!(TargetPose::at(PULSE_NS * 2.5, 1.0).colour, Color::WHITE);
        // Left halfway, it fades back from where it was.
        hover.set(false, 400 * MS as u64);
        assert_eq!(hover.white(400 * MS as u64), 1.0);
        assert_eq!(hover.white(800 * MS as u64), 0.0);
    }
}
