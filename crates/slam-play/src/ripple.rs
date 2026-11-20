//! Cursor ripples: lazer's `CursorRippleVisualiser` (ADR-0159).
//!
//! Update side: every press of a gameplay action goes into a ring in the snapshot. Draw
//! side: each ripple is a function of its age alone, no state.

use slam_osu::Vec2;
use slam_osu::replay::ActionEvent;
use slam_render::atlas::AtlasRegion;
use slam_render::shape::{Shape, ShapeBatch};
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};

use crate::circles::{Easing, OBJECT_SIZE, SkinSprite};
use crate::playfield::PlayfieldLayout;
use crate::ring::EventRing;

/// Presses a snapshot keeps; more than live at once (700 ms each).
pub const RIPPLE_CAPACITY: usize = 32;

/// Sprites (or shapes) a frame of ripples writes at most.
pub const MAX_RIPPLE_SPRITES: usize = RIPPLE_CAPACITY;

/// A press: where and when a ripple starts.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Ripple {
    /// Game time of the press.
    pub time: f64,
    /// Cursor position at the press, in osu!pixels.
    pub position: Vec2,
}

/// The presses of the current attempt.
pub type RippleRing = EventRing<Ripple, RIPPLE_CAPACITY>;

/// Records the presses of a step's events: every action, smoke too.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/CursorRippleVisualiser.cs (OnPressed)
pub fn record(ring: &mut RippleRing, events: &[ActionEvent]) {
    for event in events.iter().filter(|e| e.pressed) {
        ring.push(Ripple {
            time: event.time,
            position: event.position,
        });
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/CursorRippleVisualiser.cs (PrepareForUse)
const DURATION: f64 = 700.0;
const START_SCALE: f64 = 0.1;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/CursorRippleVisualiser.cs (DefaultCursorRipple)
const DEFAULT_RING_THICKNESS: f32 = 3.0;
const DEFAULT_RING_ALPHA: f32 = 0.1;

/// A ripple's scale and alpha at game time `time`; `None` before its press and once gone.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/CursorRippleVisualiser.cs (PrepareForUse)
pub fn pose(ripple: &Ripple, time: f64) -> Option<(f32, f32)> {
    let age = time - ripple.time;
    if !(0.0..DURATION).contains(&age) {
        return None;
    }
    let progress = age / DURATION;
    let scale = START_SCALE + (1.0 - START_SCALE) * Easing::Out.apply(progress);
    let alpha = 1.0 - progress;
    Some((scale as f32, alpha as f32))
}

/// Writes the ripples alive at game time `time` with the skin's `cursor-ripple`, sized by
/// the cursor's scale. Allocation-free once `batch` has room for [`MAX_RIPPLE_SPRITES`].
pub fn push_sprites(
    batch: &mut SpriteBatch,
    layout: &PlayfieldLayout,
    ring: &RippleRing,
    time: f64,
    cursor_scale: f32,
    texture: &SkinSprite,
) {
    for ripple in ring.since(0) {
        let Some((scale, alpha)) = pose(ripple, time) else {
            continue;
        };
        let size = texture.size * (scale * cursor_scale * layout.scale());
        let at = layout.to_window(ripple.position);
        batch.push(&Sprite {
            position: [at.x, at.y],
            size: [size.x, size.y],
            origin: [size.x / 2.0, size.y / 2.0],
            rotation: 0.0,
            colour: Color {
                a: alpha,
                ..Color::WHITE
            },
            texture: texture.texture,
            uv: texture.uv,
            blend: Blend::Additive,
        });
    }
}

/// Writes the ripples alive at game time `time` as lazer's default, a ring 3 thick of an
/// object's size at alpha 0.1, for skins without `cursor-ripple`. Allocation-free once
/// `shapes` has room for [`MAX_RIPPLE_SPRITES`].
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/CursorRippleVisualiser.cs (DefaultCursorRipple)
pub fn push_rings(
    shapes: &mut ShapeBatch,
    layout: &PlayfieldLayout,
    ring: &RippleRing,
    time: f64,
    cursor_scale: f32,
    white: AtlasRegion,
) {
    for ripple in ring.since(0) {
        let Some((scale, alpha)) = pose(ripple, time) else {
            continue;
        };
        let factor = scale * cursor_scale * layout.scale();
        let size = OBJECT_SIZE * factor;
        let at = layout.to_window(ripple.position);
        let mut shape = Shape::new(
            white.texture,
            white.uv,
            [at.x - size / 2.0, at.y - size / 2.0],
            [size, size],
            Color {
                a: alpha * DEFAULT_RING_ALPHA,
                ..Color::WHITE
            },
        );
        shape.radius = size / 2.0;
        shape.outline = DEFAULT_RING_THICKNESS * factor;
        shape.blend = Blend::Additive;
        shapes.push_shape(&shape);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slam_osu::replay::Actions;

    #[test]
    fn every_press_ripples() {
        let mut ring = RippleRing::default();
        let event = |action, pressed| ActionEvent {
            time: 5.0,
            position: Vec2::new(3.0, 4.0),
            action,
            pressed,
        };
        record(
            &mut ring,
            &[
                event(Actions::LEFT, true),
                event(Actions::LEFT, false),
                event(Actions::SMOKE, true),
            ],
        );
        assert_eq!(ring.since(0).count(), 2);
    }

    #[test]
    fn a_ripple_grows_out_and_fades_over_700_ms() {
        let ripple = Ripple {
            time: 1000.0,
            position: Vec2::ZERO,
        };
        assert_eq!(pose(&ripple, 999.0), None);
        assert_eq!(pose(&ripple, 1000.0), Some((0.1, 1.0)));
        let (scale, alpha) = pose(&ripple, 1350.0).unwrap();
        // Out at half way: 0.75 of the growth.
        assert!((scale - (0.1 + 0.9 * 0.75)).abs() < 1e-6);
        assert!((alpha - 0.5).abs() < 1e-6);
        assert_eq!(pose(&ripple, 1700.0), None);
    }
}
