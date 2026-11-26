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

/// One line weight for every icon, a share of the icon's side.
const STROKE: f32 = 0.1;

/// Speaker: a box and a stepped cone, two sound waves as pills.
const AUDIO: &[Part] = &[
    fill(0.04, 0.37, 0.16, 0.26, 0.04),
    fill(0.18, 0.27, 0.12, 0.46, 0.03),
    fill(0.28, 0.16, 0.12, 0.68, 0.04),
    fill(0.56, 0.36, 0.09, 0.28, 0.5),
    fill(0.74, 0.22, 0.09, 0.56, 0.5),
    fill(0.91, 0.1, 0.07, 0.8, 0.5),
];
/// Clock: a ring with two hands.
const OFFSET: &[Part] = &[
    ring(0.04, 0.04, 0.92, 0.92, 0.5),
    fill(0.455, 0.2, 0.09, 0.34, 0.5),
    fill(0.455, 0.455, 0.27, 0.09, 0.5),
];
/// Keyboard: a case with two rows of keys and a space bar.
const KEYS: &[Part] = &[
    ring(0.03, 0.2, 0.94, 0.6, 0.12),
    fill(0.17, 0.33, 0.1, 0.1, 0.03),
    fill(0.35, 0.33, 0.1, 0.1, 0.03),
    fill(0.53, 0.33, 0.1, 0.1, 0.03),
    fill(0.71, 0.33, 0.1, 0.1, 0.03),
    fill(0.26, 0.5, 0.1, 0.1, 0.03),
    fill(0.44, 0.5, 0.1, 0.1, 0.03),
    fill(0.62, 0.5, 0.1, 0.1, 0.03),
];
/// Monitor on a neck and a base.
const LAYOUT: &[Part] = &[
    ring(0.04, 0.1, 0.92, 0.6, 0.1),
    fill(0.44, 0.7, 0.12, 0.14, 0.0),
    fill(0.24, 0.84, 0.52, 0.09, 0.5),
];
/// Palette: a round board with four paint dots.
const SKIN: &[Part] = &[
    ring(0.04, 0.06, 0.92, 0.88, 0.5),
    fill(0.2, 0.36, 0.14, 0.14, 0.5),
    fill(0.34, 0.2, 0.14, 0.14, 0.5),
    fill(0.54, 0.2, 0.14, 0.14, 0.5),
    fill(0.68, 0.38, 0.14, 0.14, 0.5),
];
/// Gauge: a dial with ticks, a hub and a needle.
const FRAME: &[Part] = &[
    ring(0.04, 0.06, 0.92, 0.92, 0.5),
    fill(0.17, 0.3, 0.08, 0.08, 0.5),
    fill(0.46, 0.17, 0.08, 0.08, 0.5),
    fill(0.75, 0.3, 0.08, 0.08, 0.5),
    leaning(0.5, 0.3, 0.09, 0.26, 40.0),
    fill(0.41, 0.47, 0.18, 0.18, 0.5),
];
/// A circle to hit: rings round a dot.
const GAMEPLAY: &[Part] = &[
    ring(0.04, 0.04, 0.92, 0.92, 0.5),
    ring(0.26, 0.26, 0.48, 0.48, 0.5),
    fill(0.43, 0.43, 0.14, 0.14, 0.5),
];
/// Folder: a tab on a solid body.
const LIBRARY: &[Part] = &[
    fill(0.05, 0.16, 0.36, 0.2, 0.06),
    fill(0.05, 0.28, 0.9, 0.58, 0.1),
];
/// Network: three nodes and the links between them.
pub(crate) const FEEDBACK: &[Part] = &[
    leaning(0.2, 0.26, 0.07, 0.5, -34.0),
    leaning(0.73, 0.26, 0.07, 0.5, 34.0),
    fill(0.2, 0.7, 0.6, 0.07, 0.5),
    fill(0.38, 0.04, 0.24, 0.24, 0.5),
    fill(0.04, 0.62, 0.24, 0.24, 0.5),
    fill(0.72, 0.62, 0.24, 0.24, 0.5),
];
/// Tablet with its active area and a leaning pen.
pub(crate) const TABLET: &[Part] = &[
    ring(0.04, 0.14, 0.7, 0.72, 0.1),
    fill(0.19, 0.3, 0.34, 0.4, 0.06),
    leaning(0.84, 0.16, 0.09, 0.66, 20.0),
];

/// Cog: a ring with four teeth round a dot (the toolbar's settings button, ADR-0168).
pub(crate) const GEAR: &[Part] = &[
    ring(0.24, 0.24, 0.52, 0.52, 0.5),
    fill(0.43, 0.43, 0.14, 0.14, 0.5),
    fill(0.42, 0.04, 0.16, 0.2, 0.04),
    fill(0.42, 0.76, 0.16, 0.2, 0.04),
    fill(0.04, 0.42, 0.2, 0.16, 0.04),
    fill(0.76, 0.42, 0.2, 0.16, 0.04),
];
/// House: a roof of two leaning strokes over a body with a door (the toolbar's home button).
pub(crate) const HOME: &[Part] = &[
    leaning(0.2, 0.08, 0.13, 0.4, 50.0),
    leaning(0.67, 0.08, 0.13, 0.4, -50.0),
    ring(0.2, 0.4, 0.6, 0.5, 0.06),
    fill(0.43, 0.62, 0.14, 0.28, 0.03),
];

/// Number of icons: one per section.
pub const ICONS: usize = 10;

/// The parts of the icon of section `section`; the last one's for any further section.
pub fn icon(section: usize) -> &'static [Part] {
    const ALL: [&[Part]; ICONS] = [
        AUDIO, OFFSET, KEYS, LAYOUT, SKIN, FRAME, GAMEPLAY, LIBRARY, TABLET, FEEDBACK,
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
