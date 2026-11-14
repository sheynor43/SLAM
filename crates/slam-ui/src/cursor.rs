//! SLAM's own cursor (ADR-0124, ADR-0128), drawn by code from shapes in the manner of
//! osu!lazer's. The menus get a pointer with lazer's `MenuCursor` motion: it shrinks while a
//! button is held, glows in the accent, and turns towards the drag of the right button.
//! Gameplay gets lazer's default `OsuCursor`: a ring around an accent disc that expands on a
//! click, in place of the skin's textures.

use slam_render::Blend;
use slam_render::atlas::AtlasRegion;
use slam_render::hal::Color;
use slam_render::shape::{Shape, ShapeBatch};
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

// The pointer in units of its height, tip at the origin, y down, before the tilt: two
// rounded strokes that meet at the tip and open downwards, and a body between them that
// stops short of the bottom, which leaves the notch of an arrowhead.
const STROKE_WIDTH: f32 = 0.24;
const STROKE_LENGTH: f32 = 1.0;
/// Radians each stroke is turned from vertical.
const STROKE_TURN: f32 = 0.25;
const BODY_HALF_WIDTH: f32 = 0.2;
const BODY_TOP: f32 = 0.15;
const BODY_BOTTOM: f32 = 0.72;
/// The glow's size and width, as parts of the height.
const GLOW_SIZE: f32 = 0.45;

/// The height of the pointer as drawn at `size`: for placing it by its hot spot.
pub const fn arrow_height_units() -> f32 {
    STROKE_LENGTH * 1.08
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
}

impl CursorMotion {
    /// At rest: full size, no glow, not turned.
    pub const REST: CursorMotion = CursorMotion {
        scale: 1.0,
        glow: 0.0,
        rotation: 0.0,
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
    right: bool,
    last_ns: u64,
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
            right: false,
            last_ns: 0,
        }
    }
}

impl MenuCursorMotion {
    /// Takes the state of one frame at `now_ns`: the pointer in pixels (`None` when it is
    /// outside the window or has not moved yet), whether any mouse button and whether the
    /// right one is held, and `ui` pixels per reference pixel for the drag's distances.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Cursor/MenuCursorContainer.cs (Update, OnMouseMove, OnMouseDown, OnMouseUp)
    pub fn update(
        &mut self,
        theme: &Theme,
        ui: f32,
        pointer: Option<[f32; 2]>,
        held: bool,
        right: bool,
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
        // Already turning: the centre stays.
        if right
            && !self.right
            && self.drag != DragState::Rotating
            && let Some(at) = position
        {
            self.drag = DragState::DragStarted;
            self.origin = at;
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
        self.right = right;
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

    /// How the cursor looks at `now_ns`: at rest without effects (ADR-0123).
    pub fn sample(&self, theme: &Theme, now_ns: u64) -> CursorMotion {
        if !theme.effects {
            return CursorMotion::REST;
        }
        CursorMotion {
            scale: self.scale.at(now_ns),
            glow: self.glow.at(now_ns).clamp(0.0, 1.0),
            rotation: self.rotation.at(now_ns),
        }
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
/// has it. `white` is an opaque white image. Allocation-free once the batch has room for
/// [`MAX_CURSOR_SHAPES`].
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
    let first = shapes.len();
    let outline = (theme.cursor_outline * height).max(1.0);
    let radius = STROKE_WIDTH * 0.5 * height;
    // The strokes turn about the centre of their rounded ends.
    let hinge = [tip[0], tip[1] + radius];
    for (colour, grow) in [
        (theme.cursor_outline_colour, outline),
        (theme.cursor_fill, 0.0),
    ] {
        for turn in [-STROKE_TURN, STROKE_TURN] {
            let at = shapes.len();
            let mut stroke = Shape::new(
                white.texture,
                white.uv,
                [tip[0] - radius - grow, tip[1] - grow],
                [
                    STROKE_WIDTH * height + 2.0 * grow,
                    STROKE_LENGTH * height + 2.0 * grow,
                ],
                colour,
            );
            stroke.radius = radius + grow;
            shapes.push_shape(&stroke);
            shapes.transform_from(at, &Affine::about(hinge, 1.0, turn, [0.0; 2]), [1.0; 4]);
        }
        let mut body = Shape::new(
            white.texture,
            white.uv,
            [
                tip[0] - BODY_HALF_WIDTH * height - grow,
                tip[1] + BODY_TOP * height - grow,
            ],
            [
                2.0 * BODY_HALF_WIDTH * height + 2.0 * grow,
                (BODY_BOTTOM - BODY_TOP) * height + 2.0 * grow,
            ],
            colour,
        );
        body.radius = 0.08 * height + grow;
        shapes.push_shape(&body);
    }
    let glow = motion.glow * theme.glow_alpha;
    if theme.effects && glow > 0.002 {
        // Lazer's additive layer fades in over the pointer while a button is held.
        let size = GLOW_SIZE * height;
        let mut shape = Shape::new(
            white.texture,
            white.uv,
            [tip[0] - size * 0.5, tip[1] + 0.5 * height - size * 0.5],
            [size, size],
            Color {
                a: glow,
                ..theme.accent
            },
        );
        shape.radius = size * 0.5;
        shape.glow = size;
        shape.blend = Blend::Additive;
        shapes.push_shape(&shape);
    }
    shapes.scale_from(first, tip, motion.scale);
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
    fn the_pointer_is_drawn_within_its_budget_dark_outline_below_white_fill() {
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
        assert!(!shapes.is_empty());
        assert!(shapes.len() <= MAX_CURSOR_SHAPES, "{}", shapes.len());
        let first = shapes.vertices()[0].colour;
        let last = shapes.vertices().last().unwrap().colour;
        let c = theme.cursor_outline_colour;
        assert_eq!(first[..3], [c.r, c.g, c.b]);
        let f = theme.cursor_fill;
        assert_eq!(last[..3], [f.r, f.g, f.b]);
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
        m.update(&theme, 1.0, Some([10.0, 10.0]), false, false, 0);
        assert_eq!(sample(&m, &theme, 0), CursorMotion::REST);
        m.update(&theme, 1.0, Some([10.0, 10.0]), true, false, 100 * MS);
        assert!(m.held());
        let mid = sample(&m, &theme, 150);
        assert!(mid.scale < 1.0 && mid.scale > theme.cursor_press_scale);
        assert!(mid.glow > 0.0 && mid.glow < 1.0);
        let done = sample(&m, &theme, 100 + 900);
        assert!((done.scale - theme.cursor_press_scale).abs() < 1e-3);
        assert!((done.glow - 1.0).abs() < 1e-3);
        m.update(&theme, 1.0, Some([10.0, 10.0]), false, false, 2_000 * MS);
        let back = sample(&m, &theme, 2_000 + 600);
        assert!((back.scale - 1.0).abs() < 1e-3);
        assert!(back.glow.abs() < 1e-3);
        // The elastic return overshoots 1 on its way.
        let peak = (2_001..2_500)
            .map(|ms| sample(&m, &theme, ms).scale)
            .fold(0.0, f32::max);
        assert!(peak > 1.0, "{peak}");
    }

    #[test]
    fn without_effects_the_cursor_stays_at_rest() {
        let theme = Theme::default().with_effects(false);
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([0.0; 2]), true, true, 0);
        assert_eq!(sample(&m, &theme, 1_000), CursorMotion::REST);
    }

    fn drag(m: &mut MenuCursorMotion, theme: &Theme, to: [f32; 2], ms: u64) {
        m.update(theme, 1.0, Some(to), true, true, ms * MS);
    }

    #[test]
    fn a_right_drag_turns_towards_the_drag_after_a_minimum_distance() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([500.0, 500.0]), false, false, 0);
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
    fn the_left_button_alone_does_not_turn_the_cursor() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([0.0, 0.0]), true, false, 0);
        m.update(&theme, 1.0, Some([0.0, 400.0]), true, false, 10 * MS);
        assert!(!m.dragging());
        assert_eq!(sample(&m, &theme, 1_000).rotation, 0.0);
    }

    #[test]
    fn the_turn_takes_the_shorter_way_round() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([500.0, 500.0]), false, false, 0);
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
        m.update(&theme, 1.0, Some([500.0, 500.0]), false, false, 0);
        drag(&mut m, &theme, [500.0, 500.0], 10);
        drag(&mut m, &theme, [800.0, 500.0], 20);
        assert!(sample(&m, &theme, 1_000).rotation.abs() > 10.0);
        // The right button goes while the left stays: still dragging.
        m.update(&theme, 1.0, Some([800.0, 500.0]), true, false, 1_100 * MS);
        assert!(m.dragging());
        m.update(&theme, 1.0, Some([800.0, 500.0]), false, false, 1_200 * MS);
        assert!(!m.dragging());
        assert!(sample(&m, &theme, 1_200 + 1_000).rotation.abs() < 0.1);
    }

    #[test]
    fn the_drag_centre_follows_a_far_pointer() {
        let theme = Theme::default();
        let mut m = MenuCursorMotion::default();
        m.update(&theme, 1.0, Some([0.0, 0.0]), false, false, 0);
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
