//! The icons of the settings sections' tabs (ADR-0165), drawn as shapes: SLAM has no icon
//! font. Each icon is a static table of rounded rectangles in a unit square, so drawing one
//! allocates nothing.

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::shape::{Shape, ShapeBatch};

use crate::menu::Rect;

/// One part of an icon: a rounded rectangle in unit-square coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Part {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// Corner radius, a share of the icon's side (a large one makes a circle or a pill).
    r: f32,
    /// Outline width, a share of the icon's side; 0 fills.
    o: f32,
    /// Skew in degrees, the way the tops lean: strokes at an angle.
    skew: f32,
}

const fn fill(x: f32, y: f32, w: f32, h: f32, r: f32) -> Part {
    Part {
        x,
        y,
        w,
        h,
        r,
        o: 0.0,
        skew: 0.0,
    }
}

const fn ring(x: f32, y: f32, w: f32, h: f32, r: f32) -> Part {
    Part {
        x,
        y,
        w,
        h,
        r,
        o: STROKE,
        skew: 0.0,
    }
}

const fn leaning(x: f32, y: f32, w: f32, h: f32, skew: f32) -> Part {
    Part {
        x,
        y,
        w,
        h,
        r: 0.04,
        o: 0.0,
        skew,
    }
}

const STROKE: f32 = 0.11;

/// Speaker with sound waves.
const AUDIO: &[Part] = &[
    fill(0.08, 0.36, 0.2, 0.28, 0.04),
    fill(0.22, 0.2, 0.2, 0.6, 0.05),
    fill(0.58, 0.4, 0.09, 0.2, 0.05),
    fill(0.73, 0.28, 0.09, 0.44, 0.05),
    fill(0.88, 0.14, 0.09, 0.72, 0.05),
];
/// Clock.
const OFFSET: &[Part] = &[
    ring(0.04, 0.04, 0.92, 0.92, 0.5),
    fill(0.45, 0.22, 0.1, 0.32, 0.05),
    fill(0.45, 0.45, 0.28, 0.1, 0.05),
];
/// Keycap.
const KEYS: &[Part] = &[
    ring(0.04, 0.14, 0.92, 0.72, 0.18),
    fill(0.2, 0.34, 0.12, 0.12, 0.03),
    fill(0.44, 0.34, 0.12, 0.12, 0.03),
    fill(0.68, 0.34, 0.12, 0.12, 0.03),
    fill(0.24, 0.58, 0.52, 0.1, 0.04),
];
/// Monitor on a stand.
const LAYOUT: &[Part] = &[
    ring(0.04, 0.1, 0.92, 0.6, 0.1),
    fill(0.43, 0.7, 0.14, 0.12, 0.0),
    fill(0.24, 0.82, 0.52, 0.09, 0.04),
];
/// Palette with paint dots.
const SKIN: &[Part] = &[
    ring(0.04, 0.08, 0.92, 0.84, 0.42),
    fill(0.22, 0.34, 0.16, 0.16, 0.5),
    fill(0.46, 0.22, 0.16, 0.16, 0.5),
    fill(0.66, 0.42, 0.16, 0.16, 0.5),
    fill(0.34, 0.6, 0.16, 0.16, 0.5),
];
/// Bars rising, like a refresh-rate graph.
const FRAME: &[Part] = &[
    fill(0.06, 0.62, 0.17, 0.3, 0.04),
    fill(0.29, 0.44, 0.17, 0.48, 0.04),
    fill(0.52, 0.26, 0.17, 0.66, 0.04),
    fill(0.75, 0.06, 0.17, 0.86, 0.04),
];
/// A circle to hit: rings round a dot.
const GAMEPLAY: &[Part] = &[
    ring(0.04, 0.04, 0.92, 0.92, 0.5),
    ring(0.24, 0.24, 0.52, 0.52, 0.5),
    fill(0.42, 0.42, 0.16, 0.16, 0.5),
];
/// Books on a shelf, one leaning.
const LIBRARY: &[Part] = &[
    fill(0.06, 0.18, 0.2, 0.7, 0.04),
    fill(0.3, 0.1, 0.2, 0.78, 0.04),
    leaning(0.58, 0.14, 0.2, 0.74, 14.0),
    fill(0.02, 0.9, 0.96, 0.08, 0.03),
];
/// Speech bubble with its tail.
const FEEDBACK: &[Part] = &[
    ring(0.04, 0.1, 0.92, 0.62, 0.2),
    leaning(0.2, 0.68, 0.16, 0.24, -30.0),
    fill(0.24, 0.32, 0.12, 0.12, 0.5),
    fill(0.44, 0.32, 0.12, 0.12, 0.5),
    fill(0.64, 0.32, 0.12, 0.12, 0.5),
];

/// Number of icons: one per section.
pub const ICONS: usize = 9;

/// The parts of the icon of section `section`; the last one's for any further section.
pub fn icon(section: usize) -> &'static [Part] {
    const ALL: [&[Part]; ICONS] = [
        AUDIO, OFFSET, KEYS, LAYOUT, SKIN, FRAME, GAMEPLAY, LIBRARY, FEEDBACK,
    ];
    ALL[section.min(ICONS - 1)]
}

/// Writes `parts` centred in `rect`, `share` of its side big, in `colour` (already faded).
pub fn push_icon(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    rect: &Rect,
    share: f32,
    colour: Color,
    parts: &[Part],
) {
    let side = rect.width.min(rect.height) * share;
    let x0 = rect.x + (rect.width - side) / 2.0;
    let y0 = rect.y + (rect.height - side) / 2.0;
    for part in parts {
        let size = [part.w * side, part.h * side];
        shapes.push_shape(&Shape {
            radius: (part.r * side).min(size[0].min(size[1]) / 2.0),
            outline: part.o * side,
            skew: part.skew.to_radians(),
            ..Shape::new(
                white.texture,
                white.uv,
                [x0 + part.x * side, y0 + part.y * side],
                size,
                colour,
            )
        });
    }
}
