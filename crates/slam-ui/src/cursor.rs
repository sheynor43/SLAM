//! SLAM's own cursor (ADR-0124, ADR-0128, ADR-0136), drawn by code from shapes in the manner
//! of osu!lazer's. The menus get a pointer with lazer's `MenuCursor` motion: it shrinks while
//! a button is held, glows in the accent, and turns towards a drag; and SLAM's extras, each
//! switched in the settings: it spins about its long axis while the pointer rests, leaves a
//! trail behind a moving pointer and sends a ring out at each press.
//! Gameplay gets lazer's default `OsuCursor`: a ring around an accent disc that expands on a
//! click, in place of the skin's textures.

use slam_render::Blend;
use slam_render::atlas::AtlasRegion;
use slam_render::hal::Color;
use slam_render::shape::{Gradient, Shape, ShapeBatch, ShapeKind};
use slam_render::sprite::Affine;

use crate::anim::{ease_out_elastic, ease_out_elastic_quarter, ease_out_quint, progress};
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

/// Shapes one SLAM cursor writes at most.
pub const MAX_CURSOR_SHAPES: usize = 16;

/// Positions of the trail [`CursorMotion`] carries.
pub const TRAIL_LENGTH: usize = 6;
/// Rings from presses [`CursorMotion`] carries.
pub const MAX_RIPPLES: usize = 3;

// The pointer in units of its height, tip at the origin, y down, before the tilt: an
// arrowhead (`ShapeKind::Arrow`) this wide, rounded by this much.
const ARROW_WIDTH: f32 = 0.74;
const ARROW_RADIUS: f32 = 0.07;

/// The height of the pointer as drawn at `size`: for placing it by its hot spot.
pub const fn arrow_height_units() -> f32 {
    1.0
}

/// How the menu cursor looks at a moment: what [`MenuCursorMotion::sample`] gives and
/// [`push_slam_cursor`] draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorMotion {
    /// Scale about the tip.
    pub scale: f32,
    /// The accent glow's opacity, 0 to 1.
    pub glow: f32,
    /// Degrees clockwise from the resting pose.
    pub rotation: f32,
    /// Radians turned about the pointer's long axis (ADR-0136): its width shows as the
    /// cosine, its back past a quarter turn.
    pub spin: f32,
    /// Where the pointer was, from the newest: offsets in pixels from where it is, and the
    /// opacity left, 0 to 1 (ADR-0136). Unused entries have opacity 0.
    pub trail: [([f32; 2], f32); TRAIL_LENGTH],
    /// Rings from presses: offsets in pixels from the pointer and progress, 0 to 1; unused
    /// entries have progress 1 or more (ADR-0136).
    pub ripples: [([f32; 2], f32); MAX_RIPPLES],
}

impl CursorMotion {
    /// At rest: full size, no glow, not turned, no trail or rings.
    pub const REST: CursorMotion = CursorMotion {
        scale: 1.0,
        glow: 0.0,
        rotation: 0.0,
        spin: 0.0,
        trail: [([0.0; 2], 0.0); TRAIL_LENGTH],
        ripples: [([0.0; 2], 1.0); MAX_RIPPLES],
    };
}

impl Default for CursorMotion {
    fn default() -> Self {
        CursorMotion::REST
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(clippy::enum_variant_names)]
enum Ease {
    OutQuint,
    OutElastic,
    OutElasticQuarter,
}

impl Ease {
    fn apply(self, t: f32) -> f32 {
        match self {
            Ease::OutQuint => ease_out_quint(t),
            Ease::OutElastic => ease_out_elastic(t),
            Ease::OutElasticQuarter => ease_out_elastic_quarter(t),
        }
    }
}

/// A value on its way from one number to another (a framework transform).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tween {
    from: f32,
    to: f32,
    start_ns: u64,
    duration_ns: u64,
    ease: Ease,
}

impl Tween {
    const fn fixed(value: f32) -> Self {
        Tween {
            from: value,
            to: value,
            start_ns: 0,
            duration_ns: 0,
            ease: Ease::OutQuint,
        }
    }

    fn at(&self, now_ns: u64) -> f32 {
        let t = progress(self.start_ns, now_ns, self.duration_ns);
        self.from + (self.to - self.from) * self.ease.apply(t)
    }

    /// When the value arrives.
    fn end_ns(&self) -> u64 {
        self.start_ns.saturating_add(self.duration_ns)
    }

    /// Starts towards `to` from `from` at `now_ns`.
    fn start(&mut self, from: f32, to: f32, now_ns: u64, ms: f32, ease: Ease) {
        *self = Tween {
            from,
            to,
            start_ns: now_ns,
            duration_ns: (ms.max(0.0) * 1e6) as u64,
            ease,
        };
    }

    /// Starts towards `to` from where the value is now.
    fn retarget(&mut self, to: f32, now_ns: u64, ms: f32, ease: Ease) {
        let from = self.at(now_ns);
        self.start(from, to, now_ns, ms, ease);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragState {
    NotDragging,
    DragStarted,
    Rotating,
}

/// The motion of the menu cursor: follows the mouse buttons and the pointer like lazer's
/// `MenuCursorContainer`. Plain data, advanced once per frame by [`update`](Self::update).
#[derive(Clone, Copy, Debug)]
pub struct MenuCursorMotion {
    scale: Tween,
    glow: Tween,
    rotation: Tween,
    drag: DragState,
    /// Where the drag turns about, in pixels.
    origin: [f32; 2],
    last_move: Option<[f32; 2]>,
    held: bool,
    last_ns: u64,
    /// When the pointer last moved, for the spin.
    moved_ns: u64,
    /// The spin's way back to face front after the pointer moves again.
    spin_back: Tween,
    /// Recent positions of the pointer and when it reached them, newest first.
    trail: [([f32; 2], u64); TRAIL_LENGTH],
    /// Presses: where and when, newest first.
    ripples: [([f32; 2], u64); MAX_RIPPLES],
}

impl Default for MenuCursorMotion {
    fn default() -> Self {
        MenuCursorMotion {
            scale: Tween::fixed(1.0),
            glow: Tween::fixed(0.0),
            rotation: Tween::fixed(0.0),
            drag: DragState::NotDragging,
            origin: [0.0; 2],
            last_move: None,
            held: false,
            last_ns: 0,
            moved_ns: 0,
            spin_back: Tween::fixed(0.0),
            trail: [([0.0; 2], 0); TRAIL_LENGTH],
            ripples: [([0.0; 2], 0); MAX_RIPPLES],
        }
    }
}

impl MenuCursorMotion {
    /// Takes the state of one frame at `now_ns`: the pointer in pixels (`None` when it is
    /// outside the window or has not moved yet), whether a mouse button is held, and `ui`
    /// pixels per reference pixel for the drag's distances.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Cursor/MenuCursorContainer.cs (Update, OnMouseMove, OnMouseDown, OnMouseUp)
    pub fn update(
        &mut self,
        theme: &Theme,
        ui: f32,
        pointer: Option<[f32; 2]>,
        held: bool,
        now_ns: u64,
    ) {
        let frame_ms = now_ns.saturating_sub(self.last_ns) as f64 / 1e6;
        self.last_ns = now_ns;
        let position = pointer.or(self.last_move);

        if held && !self.held {
            // The scale starts again from 1; the glow from nothing.
            self.scale.start(
                1.0,
                theme.cursor_press_scale,
                now_ns,
                theme.cursor_press_in_ms,
                Ease::OutQuint,
            );
            self.glow
                .start(0.0, 1.0, now_ns, theme.cursor_glow_in_ms, Ease::OutQuint);
        }
        if held
            && !self.held
            && let Some(at) = position
        {
            // A ring from the press (ADR-0136).
            self.ripples.copy_within(..MAX_RIPPLES - 1, 1);
            self.ripples[0] = (at, now_ns.max(1));
            // Any button turns the cursor, as in lazer; already turning, the centre stays.
            if theme.cursor_fx.rotate && self.drag != DragState::Rotating {
                self.drag = DragState::DragStarted;
                self.origin = at;
            }
        }

        // The centre of the turn floats after the pointer once it is far away. The
        // interpolation is lazer's, taken as written: a fraction of 0.04 over the frame's
        // milliseconds.
        if let (DragState::DragStarted | DragState::Rotating, Some(last)) =
            (self.drag, self.last_move)
            && distance(self.origin, last) > theme.cursor_drag_float * ui
            && frame_ms > 0.0
        {
            let t = (0.04 / frame_ms).clamp(0.0, 1.0) as f32;
            self.origin = [
                self.origin[0] + (last[0] - self.origin[0]) * t,
                self.origin[1] + (last[1] - self.origin[1]) * t,
            ];
        }

        if let Some(at) = pointer
            && self.last_move != Some(at)
        {
            if self.last_move.is_some() {
                self.moved(theme, ui, at, now_ns);
            }
            self.last_move = Some(at);
            self.drag_to(theme, ui, at, now_ns);
        }

        if !held && self.held {
            self.glow
                .start(1.0, 0.0, now_ns, theme.cursor_glow_out_ms, Ease::OutQuint);
            self.scale
                .retarget(1.0, now_ns, theme.cursor_press_out_ms, Ease::OutElastic);
            if self.drag != DragState::NotDragging {
                let turned = self.rotation.at(now_ns);
                let ms = theme.cursor_rotate_back_ms * (0.5 + (turned / 960.0).abs());
                self.rotation
                    .retarget(0.0, now_ns, ms, Ease::OutElasticQuarter);
                self.drag = DragState::NotDragging;
            }
        }
        self.held = held;
    }

    /// The pointer moved to `at`: the spin turns back to face front, and the trail takes
    /// the position once it is far enough from the last one kept.
    fn moved(&mut self, theme: &Theme, ui: f32, at: [f32; 2], now_ns: u64) {
        let spin = self.spin_at(theme, now_ns);
        if spin != 0.0 {
            // To the nearest whole turn ahead, so it keeps its direction.
            let turns = (spin / std::f32::consts::TAU).ceil() * std::f32::consts::TAU;
            self.spin_back.start(
                spin,
                turns,
                now_ns,
                theme.cursor_spin_back_ms,
                Ease::OutQuint,
            );
        }
        self.moved_ns = now_ns;
        let (last, at_ns) = self.trail[0];
        if at_ns == 0 || distance(last, at) >= theme.cursor_trail_spacing * ui {
            self.trail.copy_within(..TRAIL_LENGTH - 1, 1);
            self.trail[0] = (at, now_ns.max(1));
        }
    }

    /// Radians turned about the long axis at `now_ns`: on the way back after a move, or
    /// spinning up once the pointer has rested long enough.
    fn spin_at(&self, theme: &Theme, now_ns: u64) -> f32 {
        if !theme.cursor_fx.spin {
            return 0.0;
        }
        if now_ns < self.spin_back.end_ns() {
            return self.spin_back.at(now_ns);
        }
        let rested_ms = now_ns.saturating_sub(self.moved_ns) as f32 / 1e6;
        let spinning = rested_ms - theme.cursor_spin_delay_ms;
        if self.last_move.is_none() || spinning <= 0.0 || self.held {
            return 0.0;
        }
        // Easing in over the first turn, then steady; wrapped to keep precision.
        let period = theme.cursor_spin_period_ms.max(1.0);
        let ramp = (spinning / period).min(1.0);
        let turns = if spinning < period {
            0.5 * ramp * ramp
        } else {
            0.5 + (spinning - period) / period
        };
        (turns * std::f32::consts::TAU).rem_euclid(std::f32::consts::TAU * 64.0)
    }

    fn drag_to(&mut self, theme: &Theme, ui: f32, at: [f32; 2], now_ns: u64) {
        if self.drag == DragState::NotDragging {
            return;
        }
        let distance = distance(at, self.origin);
        // No turn until the pointer is a minimum distance away: it would only annoy.
        if self.drag == DragState::DragStarted && distance > theme.cursor_drag_start * ui {
            self.drag = DragState::Rotating;
        }
        // Not at distance zero: the angle would be NaN.
        if self.drag == DragState::Rotating && distance > 0.0 {
            let offset = [at[0] - self.origin[0], at[1] - self.origin[1]];
            let mut degrees =
                (-offset[0]).atan2(offset[1]).to_degrees() + theme.cursor_tilt_degrees;
            // Always the way of least turning.
            let current = self.rotation.at(now_ns);
            let mut diff = (degrees - current) % 360.0;
            if diff < -180.0 {
                diff += 360.0;
            }
            if diff > 180.0 {
                diff -= 360.0;
            }
            degrees = current + diff;
            self.rotation
                .retarget(degrees, now_ns, theme.cursor_rotate_ms, Ease::OutQuint);
        }
    }

    /// How the cursor looks at `now_ns`. Without effects (ADR-0123) it neither shrinks,
    /// glows nor turns; the extras follow their own switches (ADR-0136).
    pub fn sample(&self, theme: &Theme, now_ns: u64) -> CursorMotion {
        let mut motion = CursorMotion::REST;
        if theme.effects {
            motion.scale = self.scale.at(now_ns);
            motion.glow = self.glow.at(now_ns).clamp(0.0, 1.0);
            motion.rotation = self.rotation.at(now_ns);
        }
        motion.spin = self.spin_at(theme, now_ns);
        let here = self.last_move.unwrap_or([0.0; 2]);
        let offset = |at: [f32; 2]| [at[0] - here[0], at[1] - here[1]];
        if theme.cursor_fx.trail {
            let life = theme.cursor_trail_ms.max(1.0);
            // The newest position is where the pointer is: the trail starts behind it.
            for (slot, &(at, at_ns)) in motion.trail.iter_mut().zip(&self.trail[1..]) {
                if at_ns == 0 {
                    break;
                }
                let age = now_ns.saturating_sub(at_ns) as f32 / 1e6;
                *slot = (offset(at), (1.0 - age / life).max(0.0));
            }
        }
        if theme.cursor_fx.ripple {
            let life = theme.cursor_ripple_ms.max(1.0);
            for (slot, &(at, at_ns)) in motion.ripples.iter_mut().zip(&self.ripples) {
                if at_ns == 0 {
                    break;
                }
                let age = now_ns.saturating_sub(at_ns) as f32 / 1e6;
                *slot = (offset(at), age / life);
            }
        }
        motion
    }

    /// Whether a mouse button is held.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Whether a drag has begun (the cursor may be turning).
    pub fn dragging(&self) -> bool {
        self.drag != DragState::NotDragging
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Writes the menu pointer with its tip at `tip`, `height` pixels high at rest, as `motion`
/// has it: the rings of presses, the trail, the shadow, the arrowhead lit from above with a
/// dark outline, and the accent glow of a press (ADR-0136). `white` is an opaque white
/// image. Allocation-free once the batch has room for [`MAX_CURSOR_SHAPES`].
pub fn push_slam_cursor(
    shapes: &mut ShapeBatch,
    theme: &Theme,
    white: AtlasRegion,
    tip: [f32; 2],
    height: f32,
    motion: &CursorMotion,
) {
    if height.is_nan() || height <= 0.0 || motion.scale.is_nan() || motion.scale <= 0.0 {
        return;
    }
    // Rings from presses, under everything and untouched by the pointer's turn.
    for &(offset, progress) in &motion.ripples {
        if !(0.0..1.0).contains(&progress) {
            continue;
        }
        let eased = ease_out_quint(progress);
        let size = height * theme.cursor_ripple_size * (0.25 + 0.75 * eased);
        let mut ring = Shape::new(
            white.texture,
            white.uv,
            [
                tip[0] + offset[0] - size / 2.0,
                tip[1] + offset[1] - size / 2.0,
            ],
            [size, size],
            Color {
                a: theme.cursor_ripple_alpha * (1.0 - progress),
                ..theme.accent
            },
        );
        ring.radius = size / 2.0;
        ring.outline = (height * 0.06 * (1.0 - eased)).max(1.0);
        shapes.push_shape(&ring);
    }
    // The trail: fading copies of the pointer where it was, in the accent.
    for &(offset, alpha) in motion.trail.iter().rev() {
        if alpha <= 0.0 {
            continue;
        }
        let first = shapes.len();
        let at = [tip[0] + offset[0], tip[1] + offset[1]];
        let colour = Color {
            a: theme.cursor_trail_alpha * alpha,
            ..theme.accent
        };
        shapes.push_shape(&arrow(white, at, height, colour));
        place(
            shapes,
            first,
            theme,
            at,
            motion.scale * (0.7 + 0.3 * alpha),
            motion,
            0.0,
        );
    }
    let first = shapes.len();
    let body = arrow(white, tip, height, theme.cursor_fill);
    if theme.shadow > 0.0 {
        let drop = height * 0.06;
        shapes.push_shape(&Shape {
            position: [body.position[0], body.position[1] + drop],
            glow: height * 0.12 * (0.5 + 0.5 * theme.shadow),
            colour: Color {
                a: (theme.shadow_alpha * theme.shadow).min(0.95),
                ..theme.cursor_outline_colour
            },
            gradient: None,
            ..body
        });
    }
    let glow = motion.glow * theme.glow_alpha;
    if theme.effects && glow > 0.002 {
        // Lazer's additive layer lights the pointer while a button is held.
        shapes.push_shape(&Shape {
            glow: height * 0.3,
            colour: Color {
                a: glow,
                ..theme.accent
            },
            gradient: None,
            blend: Blend::Additive,
            ..body
        });
    }
    shapes.push_shape(&body);
    shapes.push_shape(&Shape {
        outline: (theme.cursor_outline * height).max(1.0),
        colour: theme.cursor_outline_colour,
        gradient: None,
        ..body
    });
    place(shapes, first, theme, tip, motion.scale, motion, motion.spin);
}

/// The pointer's arrowhead with its tip at `tip`, `height` pixels high, before the tilt,
/// lit from above: `colour` darkening a little towards the bottom.
fn arrow(white: AtlasRegion, tip: [f32; 2], height: f32, colour: Color) -> Shape {
    let width = ARROW_WIDTH * height;
    let shade = 0.82;
    Shape {
        kind: ShapeKind::Arrow,
        radius: ARROW_RADIUS * height,
        gradient: Some(Gradient::Vertical(Color {
            r: colour.r * shade,
            g: colour.g * shade,
            b: colour.b * shade,
            a: colour.a,
        })),
        ..Shape::new(
            white.texture,
            white.uv,
            [tip[0] - width / 2.0, tip[1]],
            [width, height],
            colour,
        )
    }
}

/// Scales the quads from `first` on by `scale` about `tip`, squeezes them about the
/// pointer's long axis by the spin (darkening its back) and turns them to the tilt plus
/// the motion's rotation.
fn place(
    shapes: &mut ShapeBatch,
    first: usize,
    theme: &Theme,
    tip: [f32; 2],
    scale: f32,
    motion: &CursorMotion,
    spin: f32,
) {
    shapes.scale_from(first, tip, scale);
    if spin != 0.0 {
        let width = spin.cos();
        // Never a line: a sliver stays at the quarter turns.
        let width = if width.abs() < 0.04 {
            0.04f32.copysign(width)
        } else {
            width
        };
        let shade = if width < 0.0 { 0.72 } else { 1.0 };
        let squeeze = Affine {
            a: width,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: tip[0] - width * tip[0],
            ty: 0.0,
        };
        shapes.transform_from(first, &squeeze, [shade, shade, shade, 1.0]);
    }
    // Lazer's pointer is drawn tilted to the left; the drag's angle is measured from there.
    let turn = (motion.rotation - theme.cursor_tilt_degrees).to_radians();
    shapes.transform_from(first, &Affine::about(tip, 1.0, turn, [0.0; 2]), [1.0; 4]);
}

/// Writes the gameplay cursor of lazer's default skin, `diameter` pixels across, centred on
/// `centre`: a ring around an accent disc that grows with `expand` (1 at rest, see
/// `ExpandTimes`), a light dot in the middle that stays. `white` is an opaque white image.
/// Allocation-free once the batch has room for [`MAX_CURSOR_SHAPES`].
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/OsuCursor.cs (DefaultCursor)
pub fn push_gameplay_cursor(
    shapes: &mut ShapeBatch,
    theme: &Theme,
    white: AtlasRegion,
    centre: [f32; 2],
    diameter: f32,
    expand: f32,
) {
    if diameter.is_nan() || diameter <= 0.0 || expand.is_nan() || expand <= 0.0 {
        return;
    }
    let size = diameter * expand;
    let disc = |size: f32, colour: Color| {
        let mut shape = Shape::new(
            white.texture,
            white.uv,
            [centre[0] - size * 0.5, centre[1] - size * 0.5],
            [size, size],
            colour,
        );
        shape.radius = size * 0.5;
        shape
    };
    let with_alpha = |colour: Color, a: f32| Color { a, ..colour };
    if theme.effects {
        let mut glow = disc(size, with_alpha(theme.accent, 0.5));
        glow.glow =
            theme.cursor_gameplay_glow * diameter / theme.cursor_gameplay_size.max(1.0) * expand;
        shapes.push_shape(&glow);
    }
    shapes.push_shape(&disc(
        size,
        with_alpha(theme.accent, theme.cursor_ring_fill_alpha),
    ));
    for (width, alpha) in [
        (theme.cursor_inner_ring, theme.cursor_inner_ring_alpha),
        (theme.cursor_ring, 1.0),
    ] {
        let mut ring = disc(size, with_alpha(Color::WHITE, alpha));
        ring.outline = size * width;
        shapes.push_shape(&ring);
    }
    shapes.push_shape(&disc(diameter * theme.cursor_dot, theme.cursor_fill));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::CursorFx;
    use slam_render::Origin;
    use slam_render::atlas::UvRect;
    use slam_render::hal::Extent;
    use slam_render::textures::TextureId;

    const MS: u64 = 1_000_000;

    fn white() -> AtlasRegion {
        AtlasRegion {
            texture: TextureId(1),
            origin: Origin { x: 0, y: 0 },
            size: Extent::new(1, 1),
            uv: UvRect::FULL,
        }
    }

    fn batch() -> ShapeBatch {
        ShapeBatch::with_capacity(MAX_CURSOR_SHAPES)
    }

    #[test]
    fn style_names_round_trip() {
        for style in CursorStyle::ALL {
            assert_eq!(CursorStyle::from_name(style.name()), Some(style));
        }
        assert_eq!(CursorStyle::from_name("big"), None);
    }

    #[test]
    fn the_pointer_is_a_shadowed_arrowhead_lit_from_above_with_a_dark_outline() {
        let theme = Theme::default();
        let mut shapes = batch();
        push_slam_cursor(
            &mut shapes,
            &theme,
            white(),
            [100.0, 100.0],
            30.0,
            &CursorMotion::REST,
        );
        // The shadow, the fill, the outline.
        assert_eq!(shapes.len(), 3);
        let v = shapes.vertices();
        let kind = |quad: usize| slam_render::shape::unpack_effects(v[quad * 4].params[2]).3;
        assert!((0..3).all(|q| kind(q) == ShapeKind::Arrow));
        assert!(v[0].params[1] < 0.0, "the shadow is a glow");
        // The corners lie a margin past the edges, so the gradient's colour there is close
        // to the fill's.
        let f = theme.cursor_fill;
        assert!((v[4].colour[1] - f.g).abs() < 0.05, "{:?}", v[4].colour);
        // Darker at the bottom.
        assert!(v[4 + 3].colour[0] < v[4].colour[0]);
        let c = theme.cursor_outline_colour;
        assert_eq!(v[8].colour[..3], [c.r, c.g, c.b]);
        assert!(v[8].params[1] > 0.0, "an outline");
        // No shadow when shadows are off.
        let mut flat = batch();
        push_slam_cursor(
            &mut flat,
            &theme.with_shadow(0.0),
            white(),
            [100.0, 100.0],
            30.0,
            &CursorMotion::REST,
        );
        assert_eq!(flat.len(), 2);
    }

    #[test]
    fn trail_and_rings_stay_within_the_budget() {
        let theme = Theme::default();
        let mut motion = CursorMotion {
            glow: 1.0,
            ..CursorMotion::REST
        };
        for (i, slot) in motion.trail.iter_mut().enumerate() {
            *slot = ([-(i as f32) * 8.0, 0.0], 0.9);
        }
        motion.ripples.fill(([0.0; 2], 0.5));
        let mut shapes = batch();
        push_slam_cursor(&mut shapes, &theme, white(), [100.0, 100.0], 30.0, &motion);
        assert_eq!(shapes.len(), MAX_RIPPLES + TRAIL_LENGTH + 4);
        assert!(shapes.len() <= MAX_CURSOR_SHAPES);
    }

    #[test]
    fn the_pointer_rests_tilted_to_the_left_with_its_tip_on_the_hot_spot() {
        let theme = Theme::default();
        let mut shapes = batch();
        push_slam_cursor(
            &mut shapes,
            &theme,
            white(),
            [200.0, 200.0],
            40.0,
            &CursorMotion::REST,
        );
        let xs = || shapes.vertices().iter().map(|v| v.position[0]);
        let ys = || shapes.vertices().iter().map(|v| v.position[1]);
        // The body leans right of the tip, below it; nothing reaches far above it.
        assert!(xs().fold(f32::MIN, f32::max) > 200.0 + 10.0);
        assert!(ys().fold(f32::MIN, f32::max) > 200.0 + 25.0);
        assert!(ys().fold(f32::MAX, f32::min) > 200.0 - 12.0);
    }

    #[test]
    fn a_glow_comes_with_the_press_only_with_effects() {
        let theme = Theme::default();
        let held = CursorMotion {
            glow: 1.0,
            ..CursorMotion::REST
        };
        let (mut rest, mut glow, mut flat) = (batch(), batch(), batch());
        push_slam_cursor(
            &mut rest,
            &theme,
            white(),
            [0.0; 2],
            30.0,
            &CursorMotion::REST,
        );
        push_slam_cursor(&mut glow, &theme, white(), [0.0; 2], 30.0, &held);
        assert_eq!(glow.len(), rest.len() + 1);
        let off = theme.with_effects(false);
        push_slam_cursor(&mut flat, &off, white(), [0.0; 2], 30.0, &held);
        assert_eq!(flat.len(), rest.len());
    }

    #[test]
    fn scale_shrinks_towards_the_tip() {
        let theme = Theme::default();
        let (mut big, mut small) = (batch(), batch());
        push_slam_cursor(
            &mut big,
            &theme,
            white(),
            [0.0; 2],
            40.0,
            &CursorMotion::REST,
        );
        let shrunk = CursorMotion {
            scale: 0.5,
            ..CursorMotion::REST
        };
        push_slam_cursor(&mut small, &theme, white(), [0.0; 2], 40.0, &shrunk);
        let reach = |b: &ShapeBatch| {
            b.vertices()
                .iter()
                .map(|v| v.position[0].hypot(v.position[1]))
                .fold(0.0, f32::max)
        };
        assert!((reach(&small) * 2.0 - reach(&big)).abs() < 1.0);
    }

    #[test]
    fn rotation_turns_the_pointer_about_its_tip() {
        let theme = Theme::default();
        let (mut rest, mut turned) = (batch(), batch());
        push_slam_cursor(
            &mut rest,
            &theme,
            white(),
            [0.0; 2],
            40.0,
            &CursorMotion::REST,
        );
        let motion = CursorMotion {
            rotation: 90.0,
            ..CursorMotion::REST
        };
        push_slam_cursor(&mut turned, &theme, white(), [0.0; 2], 40.0, &motion);
        // A quarter turn clockwise maps (x, y) to (-y, x) about the tip.
        let a = rest.vertices()[0].position;
        let b = turned.vertices()[0].position;
        assert!(
            (b[0] + a[1]).abs() < 1e-3 && (b[1] - a[0]).abs() < 1e-3,
            "{a:?} {b:?}"
        );
    }

    fn sample(m: &MenuCursorMotion, theme: &Theme, ms: u64) -> CursorMotion {
        m.sample(theme, ms * MS)
    }

    #[test]
    fn a_press_shrinks_to_90_percent_and_the_release_springs_back() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([10.0, 10.0]), false, 0);
        assert_eq!(sample(&m, &theme, 0), CursorMotion::REST);
        m.update(&theme, 1.0, Some([10.0, 10.0]), true, 100 * MS);
        assert!(m.held());
        let mid = sample(&m, &theme, 150);
        assert!(mid.scale < 1.0 && mid.scale > theme.cursor_press_scale);
        assert!(mid.glow > 0.0 && mid.glow < 1.0);
        let done = sample(&m, &theme, 100 + 900);
        assert!((done.scale - theme.cursor_press_scale).abs() < 1e-3);
        assert!((done.glow - 1.0).abs() < 1e-3);
        m.update(&theme, 1.0, Some([10.0, 10.0]), false, 2_000 * MS);
        let back = sample(&m, &theme, 2_000 + 600);
        assert!((back.scale - 1.0).abs() < 1e-3);
        assert!(back.glow.abs() < 1e-3);
        // The elastic return overshoots 1 on its way.
        let peak = (2_001..2_500)
            .map(|ms| sample(&m, &theme, ms).scale)
            .fold(0.0, f32::max);
        assert!(peak > 1.0, "{peak}");
    }

    /// The theme without any of the cursor's extras.
    fn plain(theme: Theme) -> Theme {
        theme.with_cursor_fx(CursorFx {
            rotate: false,
            spin: false,
            trail: false,
            ripple: false,
        })
    }

    #[test]
    fn without_effects_or_extras_the_cursor_stays_at_rest() {
        let theme = plain(Theme::default().with_effects(false));
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([0.0; 2]), true, 0);
        assert_eq!(sample(&m, &theme, 1_000), CursorMotion::REST);
    }

    #[test]
    fn a_press_sends_out_a_ring_that_grows_and_ends() {
        // The extras do not need the effects.
        let theme = Theme::default().with_effects(false);
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([50.0, 50.0]), false, 0);
        m.update(&theme, 1.0, Some([50.0, 50.0]), true, 100 * MS);
        let ring = sample(&m, &theme, 100 + theme.cursor_ripple_ms as u64 / 2).ripples[0];
        assert_eq!(ring.0, [0.0, 0.0]);
        assert!((ring.1 - 0.5).abs() < 0.01, "{ring:?}");
        // It stays where the press was while the pointer moves on.
        m.update(&theme, 1.0, Some([80.0, 50.0]), true, 150 * MS);
        assert_eq!(sample(&m, &theme, 150).ripples[0].0, [-30.0, 0.0]);
        assert!(sample(&m, &theme, 2_000).ripples[0].1 >= 1.0);
        let off = theme.with_cursor_fx(CursorFx {
            ripple: false,
            ..CursorFx::default()
        });
        assert!(sample(&m, &off, 200).ripples.iter().all(|r| r.1 >= 1.0));
    }

    #[test]
    fn a_moving_pointer_leaves_a_fading_trail_behind_it() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        for i in 0..5u64 {
            m.update(
                &theme,
                1.0,
                Some([i as f32 * 20.0, 0.0]),
                false,
                i * 10 * MS,
            );
        }
        let motion = sample(&m, &theme, 40);
        // Behind the pointer, the older the further and the fainter.
        let (first, second) = (motion.trail[0], motion.trail[1]);
        assert_eq!(first.0, [-20.0, 0.0]);
        assert_eq!(second.0, [-40.0, 0.0]);
        assert!(first.1 > second.1 && second.1 > 0.0);
        assert!(sample(&m, &theme, 1_000).trail.iter().all(|t| t.1 == 0.0));
        let off = theme.with_cursor_fx(CursorFx {
            trail: false,
            ..CursorFx::default()
        });
        assert!(sample(&m, &off, 40).trail.iter().all(|t| t.1 == 0.0));
    }

    #[test]
    fn a_resting_pointer_spins_and_faces_front_again_when_moved() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([0.0; 2]), false, 0);
        m.update(&theme, 1.0, Some([5.0, 0.0]), false, 10 * MS);
        let delay = theme.cursor_spin_delay_ms as u64;
        assert_eq!(sample(&m, &theme, 10 + delay - 50).spin, 0.0);
        let spun = sample(&m, &theme, 10 + delay + 2_000).spin;
        assert!(spun > 0.5, "{spun}");
        // Moving turns it on to face front.
        let at = 10 + delay + 2_000;
        m.update(&theme, 1.0, Some([9.0, 0.0]), false, at * MS);
        // Front again once the way back is over, until it has rested long enough again.
        let back = sample(&m, &theme, at + 600).spin;
        assert_eq!(back, 0.0);
        assert!(sample(&m, &theme, at + 100).spin >= spun);
        let off = theme.with_cursor_fx(CursorFx {
            spin: false,
            ..CursorFx::default()
        });
        assert_eq!(sample(&m, &off, at - 1).spin, 0.0);
    }

    #[test]
    fn a_spin_squeezes_the_pointer_about_its_axis_and_darkens_its_back() {
        let theme = Theme::default().with_shadow(0.0);
        let (mut front, mut back) = (batch(), batch());
        push_slam_cursor(
            &mut front,
            &theme,
            white(),
            [0.0; 2],
            40.0,
            &CursorMotion::REST,
        );
        let turned = CursorMotion {
            spin: std::f32::consts::PI,
            ..CursorMotion::REST
        };
        push_slam_cursor(&mut back, &theme, white(), [0.0; 2], 40.0, &turned);
        assert!(back.vertices()[0].colour[0] < front.vertices()[0].colour[0]);
        let width = |b: &ShapeBatch| {
            let xs = b.vertices().iter().map(|v| v.position[0]);
            xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min)
        };
        let half = CursorMotion {
            spin: 1.3,
            ..CursorMotion::REST
        };
        let mut squeezed = batch();
        push_slam_cursor(&mut squeezed, &theme, white(), [0.0; 2], 40.0, &half);
        assert!(width(&squeezed) < width(&front));
    }

    fn drag(m: &mut MenuCursorMotion, theme: &Theme, to: [f32; 2], ms: u64) {
        m.update(theme, 1.0, Some(to), true, ms * MS);
    }

    #[test]
    fn a_drag_turns_towards_the_drag_after_a_minimum_distance() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([500.0, 500.0]), false, 0);
        drag(&mut m, &theme, [500.0, 500.0], 10);
        assert!(m.dragging());
        // Short of the minimum: no turn.
        drag(&mut m, &theme, [500.0, 560.0], 20);
        assert_eq!(sample(&m, &theme, 1_000).rotation, 0.0);
        // Straight down, far: atan2(-0, d) is 0, plus the tilt.
        drag(&mut m, &theme, [500.0, 700.0], 30);
        let r = sample(&m, &theme, 1_000).rotation;
        assert!((r - theme.cursor_tilt_degrees).abs() < 0.5, "{r}");
        // Dragging to the right: atan2(-d, 0) = -90 degrees, plus the tilt.
        drag(&mut m, &theme, [800.0, 500.0], 2_000);
        let r = sample(&m, &theme, 4_000).rotation;
        assert!((r - (-90.0 + theme.cursor_tilt_degrees)).abs() < 1.0, "{r}");
    }

    #[test]
    fn any_button_turns_the_cursor_unless_the_turn_is_off() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([0.0, 0.0]), true, 0);
        m.update(&theme, 1.0, Some([0.0, 400.0]), true, 10 * MS);
        assert!(m.dragging());
        assert!(sample(&m, &theme, 1_000).rotation != 0.0);
        let off = theme.with_cursor_fx(CursorFx {
            rotate: false,
            ..CursorFx::default()
        });
        let mut m = MenuCursorMotion::default();
        m.update(&off, 1.0, Some([0.0, 0.0]), true, 0);
        m.update(&off, 1.0, Some([0.0, 400.0]), true, 10 * MS);
        assert!(!m.dragging());
        assert_eq!(sample(&m, &off, 1_000).rotation, 0.0);
    }

    #[test]
    fn the_turn_takes_the_shorter_way_round() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([500.0, 500.0]), false, 0);
        drag(&mut m, &theme, [500.0, 500.0], 10);
        // Up-left of the origin gives about 135 + 24; then just past it to the other side,
        // which is a small step in the other direction, not nearly a full turn.
        drag(&mut m, &theme, [400.0, 300.0], 20);
        let a = sample(&m, &theme, 1_000).rotation;
        drag(&mut m, &theme, [400.0, 700.0], 1_020);
        let b = sample(&m, &theme, 3_000).rotation;
        assert!((b - a).abs() < 360.0, "{a} -> {b}");
        let straight = (-(400.0f32 - 500.0)).atan2(700.0 - 500.0).to_degrees() + 24.3;
        assert!(
            ((b - straight) % 360.0).abs() < 1.0 || ((b - straight) % 360.0 - 360.0).abs() < 1.0
        );
    }

    #[test]
    fn releasing_every_button_returns_the_rotation_to_zero() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([500.0, 500.0]), false, 0);
        drag(&mut m, &theme, [500.0, 500.0], 10);
        drag(&mut m, &theme, [800.0, 500.0], 20);
        assert!(sample(&m, &theme, 1_000).rotation.abs() > 10.0);
        // A button stays held: still dragging.
        m.update(&theme, 1.0, Some([800.0, 500.0]), true, 1_100 * MS);
        assert!(m.dragging());
        m.update(&theme, 1.0, Some([800.0, 500.0]), false, 1_200 * MS);
        assert!(!m.dragging());
        assert!(sample(&m, &theme, 1_200 + 1_000).rotation.abs() < 0.1);
    }

    #[test]
    fn the_drag_centre_follows_a_far_pointer() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([0.0, 0.0]), false, 0);
        drag(&mut m, &theme, [0.0, 0.0], 10);
        drag(&mut m, &theme, [0.0, 500.0], 12);
        let before = m.origin;
        drag(&mut m, &theme, [0.0, 500.0], 14);
        assert!(m.origin[1] > before[1]);
    }

    #[test]
    fn the_gameplay_cursor_is_a_ring_around_an_accent_disc_with_a_light_dot() {
        let theme = Theme::default();
        let mut shapes = batch();
        push_gameplay_cursor(&mut shapes, &theme, white(), [100.0, 100.0], 28.0, 1.0);
        assert!(!shapes.is_empty() && shapes.len() <= MAX_CURSOR_SHAPES);
        let colours: Vec<[f32; 4]> = shapes.vertices().chunks(4).map(|q| q[0].colour).collect();
        let a = theme.accent;
        assert!(colours.iter().any(|c| c[..3] == [a.r, a.g, a.b]));
        assert!(colours.iter().any(|c| c[..3] == [1.0, 1.0, 1.0]));
        let f = theme.cursor_fill;
        assert_eq!(colours.last().unwrap()[..3], [f.r, f.g, f.b]);
    }

    #[test]
    fn the_gameplay_cursor_expands_but_its_dot_does_not() {
        let theme = Theme::default();
        let (mut rest, mut big) = (batch(), batch());
        push_gameplay_cursor(&mut rest, &theme, white(), [0.0; 2], 28.0, 1.0);
        push_gameplay_cursor(&mut big, &theme, white(), [0.0; 2], 28.0, 1.3);
        assert_eq!(rest.len(), big.len());
        let width = |b: &ShapeBatch, quad: usize| {
            let v = &b.vertices()[quad * 4..quad * 4 + 4];
            v[1].position[0] - v[0].position[0]
        };
        let last = rest.len() - 1;
        assert!(width(&big, last - 1) > width(&rest, last - 1) * 1.25);
        assert!((width(&big, last) - width(&rest, last)).abs() < 1e-4);
        let off = Theme::default().with_effects(false);
        let mut flat = batch();
        push_gameplay_cursor(&mut flat, &off, white(), [0.0; 2], 28.0, 1.0);
        assert_eq!(flat.len() + 1, rest.len());
    }
}
