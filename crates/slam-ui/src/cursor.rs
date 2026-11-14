//! SLAM's own cursor (ADR-0124): a pointer arrow drawn by code from shapes, outlined in the
//! colour scheme's accent, with a small squeeze while a mouse button is held. Used by the
//! menus and, in place of the skin's textures, by gameplay.

use slam_render::atlas::AtlasRegion;
use slam_render::hal::Color;
use slam_render::shape::{Shape, ShapeBatch};
use slam_render::sprite::Sprite;

use crate::anim::{ease_out_cubic, progress};
use crate::theme::Theme;

/// Which cursor is drawn (ADR-0124).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorStyle {
    /// SLAM's own, drawn by code.
    Slam,
    /// The skin's `cursor` and `cursormiddle`.
    Skin,
    /// The operating system's: nothing is drawn.
    System,
}

impl CursorStyle {
    pub const ALL: [CursorStyle; 3] = [CursorStyle::Slam, CursorStyle::Skin, CursorStyle::System];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            CursorStyle::Slam => "slam",
            CursorStyle::Skin => "skin",
            CursorStyle::System => "system",
        }
    }

    /// The style called `name`.
    pub fn from_name(name: &str) -> Option<CursorStyle> {
        CursorStyle::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// Shapes and sprites one SLAM cursor writes at most.
pub const MAX_CURSOR_SHAPES: usize = 40;

/// Slices the arrow's head is cut into.
const SLICES: usize = 14;

// The arrow in units of its height, tip at the origin, y down: a head with a vertical left
// edge, a 45 degree upper edge and a notch, and a stem leaning to the right.
const HEAD_HEIGHT: f32 = 0.82;
const HEAD_WIDTH: f32 = 0.58;
const STEM_CENTRE: [f32; 2] = [0.385, 0.84];
const STEM_LENGTH: f32 = 0.46;
const STEM_WIDTH: f32 = 0.19;
/// Radians the stem is turned from vertical, towards the right at the bottom.
const STEM_TURN: f32 = -0.44;
/// Height of the whole arrow.
const ARROW_HEIGHT: f32 = 1.08;

/// How far right the head reaches at `y` (in the arrow's units); 0 outside it.
fn head_right(y: f32) -> f32 {
    if !(0.0..=HEAD_HEIGHT).contains(&y) {
        0.0
    } else if y <= HEAD_WIDTH {
        y
    } else {
        HEAD_WIDTH * (HEAD_HEIGHT - y) / (HEAD_HEIGHT - HEAD_WIDTH)
    }
}

/// Follows the mouse buttons for the squeeze: eases towards 1 while one is held and back to
/// 0 after.
#[derive(Clone, Copy, Debug, Default)]
pub struct PressSqueeze {
    pressed: bool,
    changed_ns: u64,
    from: f32,
}

impl PressSqueeze {
    /// How pressed the cursor looks at `now_ns`, 0 to 1.
    pub fn amount(&self, theme: &Theme, now_ns: u64) -> f32 {
        let to = if self.pressed { 1.0 } else { 0.0 };
        let ms = if self.pressed {
            theme.cursor_squeeze_in_ms
        } else {
            theme.cursor_squeeze_out_ms
        };
        let t = progress(self.changed_ns, now_ns, (ms.max(0.0) * 1e6) as u64);
        self.from + (to - self.from) * ease_out_cubic(t)
    }

    /// Takes whether a button is held at `now_ns`; returns the new state when it changed.
    pub fn update(&mut self, theme: &Theme, pressed: bool, now_ns: u64) -> Option<bool> {
        if pressed == self.pressed {
            return None;
        }
        self.from = self.amount(theme, now_ns);
        self.pressed = pressed;
        self.changed_ns = now_ns;
        Some(pressed)
    }

    /// Whether a button is held.
    pub fn pressed(&self) -> bool {
        self.pressed
    }
}

/// The squeeze to draw: none without effects (ADR-0123).
pub fn squeeze_for(theme: &Theme, press: &PressSqueeze, now_ns: u64) -> f32 {
    if theme.effects {
        press.amount(theme, now_ns) * theme.cursor_squeeze
    } else {
        0.0
    }
}

/// Writes the cursor with its tip at `tip`, `height` pixels high, shrunk by the part
/// `squeeze` about the tip. `white` is an opaque white image. Allocation-free once the batch
/// has room for [`MAX_CURSOR_SHAPES`].
pub fn push_slam_cursor(
    shapes: &mut ShapeBatch,
    theme: &Theme,
    white: AtlasRegion,
    tip: [f32; 2],
    height: f32,
    squeeze: f32,
) {
    let size = height * (1.0 - squeeze.clamp(0.0, 0.9));
    if size.is_nan() || size <= 0.0 {
        return;
    }
    let outline = (theme.cursor_outline * size / theme.cursor_size.max(1.0)).max(1.0);
    // Underneath the outline's colour, grown by its width; on top the fill.
    for (colour, grow) in [(theme.accent, outline), (theme.cursor_fill, 0.0)] {
        push_arrow(shapes, white, tip, size, colour, grow);
    }
}

fn push_arrow(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    tip: [f32; 2],
    size: f32,
    colour: Color,
    grow: f32,
) {
    let slice = HEAD_HEIGHT / SLICES as f32;
    for i in 0..SLICES {
        let top = i as f32 * slice;
        // The widest point of the slice, so that the slices cover the head.
        let right = head_right(top + slice).max(head_right(top));
        let right = if i == 0 {
            head_right(top + slice)
        } else {
            right
        };
        let x0 = tip[0] - grow;
        let y0 = tip[1] + top * size - grow;
        let mut shape = Shape::new(
            white.texture,
            white.uv,
            [x0, y0],
            [right * size + 2.0 * grow, slice * size + 2.0 * grow + 0.5],
            colour,
        );
        shape.radius = grow.min(slice * size * 0.5);
        shapes.push_shape(&shape);
    }
    let (w, l) = (
        STEM_WIDTH * size + 2.0 * grow,
        STEM_LENGTH * size + 2.0 * grow,
    );
    shapes.push(&Sprite {
        position: [
            tip[0] + STEM_CENTRE[0] * size,
            tip[1] + STEM_CENTRE[1] * size,
        ],
        size: [w, l],
        origin: [w * 0.5, l * 0.5],
        rotation: STEM_TURN,
        colour,
        texture: white.texture,
        uv: white.uv,
        blend: slam_render::Blend::Alpha,
    });
}

/// The height of the arrow as drawn at `size`: for placing it by its hot spot.
pub const fn arrow_height_units() -> f32 {
    ARROW_HEIGHT
}

#[cfg(test)]
mod tests {
    use super::*;
    use slam_render::Origin;
    use slam_render::atlas::UvRect;
    use slam_render::hal::Extent;
    use slam_render::textures::TextureId;

    fn white() -> AtlasRegion {
        AtlasRegion {
            texture: TextureId(1),
            origin: Origin { x: 0, y: 0 },
            size: Extent::new(1, 1),
            uv: UvRect::FULL,
        }
    }

    #[test]
    fn style_names_round_trip() {
        for style in CursorStyle::ALL {
            assert_eq!(CursorStyle::from_name(style.name()), Some(style));
        }
        assert_eq!(CursorStyle::from_name("big"), None);
    }

    #[test]
    fn the_cursor_is_drawn_within_its_budget_and_does_not_grow_the_batch_later() {
        let theme = Theme::default();
        let mut shapes = ShapeBatch::with_capacity(MAX_CURSOR_SHAPES);
        push_slam_cursor(&mut shapes, &theme, white(), [100.0, 100.0], 28.0, 0.0);
        assert!(!shapes.is_empty());
        assert!(shapes.len() <= MAX_CURSOR_SHAPES, "{}", shapes.len());
        // Outline in the accent underneath, fill on top.
        let first = shapes.vertices()[0].colour;
        let last = shapes.vertices().last().unwrap().colour;
        assert_eq!(first[..3], [theme.accent.r, theme.accent.g, theme.accent.b]);
        assert_eq!(
            last[..3],
            [
                theme.cursor_fill.r,
                theme.cursor_fill.g,
                theme.cursor_fill.b
            ]
        );
    }

    #[test]
    fn the_cursor_stays_within_its_height_below_the_tip() {
        let theme = Theme::default();
        let mut shapes = ShapeBatch::with_capacity(MAX_CURSOR_SHAPES);
        push_slam_cursor(&mut shapes, &theme, white(), [50.0, 50.0], 40.0, 0.0);
        let bottom = shapes
            .vertices()
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        assert!(bottom <= 50.0 + 40.0 * ARROW_HEIGHT * 1.1 + 8.0, "{bottom}");
        assert!(
            shapes
                .vertices()
                .iter()
                .all(|v| v.position[0] >= 50.0 - 8.0)
        );
    }

    #[test]
    fn a_press_squeezes_towards_the_tip_and_eases_back() {
        let theme = Theme::default();
        let mut press = PressSqueeze::default();
        assert_eq!(squeeze_for(&theme, &press, 0), 0.0);
        assert_eq!(press.update(&theme, true, 1_000), Some(true));
        assert_eq!(press.update(&theme, true, 2_000), None);
        let done = 1_000 + (theme.cursor_squeeze_in_ms * 1e6) as u64 + 1;
        let full = squeeze_for(&theme, &press, done);
        assert!((full - theme.cursor_squeeze).abs() < 1e-5);
        let mid = squeeze_for(&theme, &press, 1_000 + 10_000_000);
        assert!(mid > 0.0 && mid < full);
        press.update(&theme, false, done);
        let later = done + (theme.cursor_squeeze_out_ms * 1e6) as u64 + 1;
        assert!(squeeze_for(&theme, &press, later) < 1e-5);

        let mut small = ShapeBatch::with_capacity(MAX_CURSOR_SHAPES);
        let mut big = ShapeBatch::with_capacity(MAX_CURSOR_SHAPES);
        push_slam_cursor(&mut big, &theme, white(), [0.0, 0.0], 40.0, 0.0);
        push_slam_cursor(&mut small, &theme, white(), [0.0, 0.0], 40.0, full);
        let extent = |b: &ShapeBatch| {
            b.vertices()
                .iter()
                .map(|v| v.position[1])
                .fold(f32::MIN, f32::max)
        };
        assert!(extent(&small) < extent(&big));
    }

    #[test]
    fn without_effects_there_is_no_squeeze() {
        let theme = Theme::default().with_effects(false);
        let mut press = PressSqueeze::default();
        press.update(&theme, true, 0);
        assert_eq!(squeeze_for(&theme, &press, 1_000_000_000), 0.0);
    }
}
