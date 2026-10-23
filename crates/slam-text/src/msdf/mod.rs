//! MSDF bitmap of one glyph (ADR-0037).
//!
//! Pure and thread-agnostic: atlas code calls [`generate_glyph`] on worker
//! threads and uploads the result.

use fdsm::bezier::scanline::FillRule;
use fdsm::bezier::{Point, Segment};
use fdsm::correct_error::{ErrorCorrectionConfig, correct_error_msdf};
use fdsm::generate::generate_msdf;
use fdsm::render::correct_sign_msdf;
use fdsm::shape::{Contour, Shape};
use fdsm::transform::Transform;
use image::Rgb32FImage;
use nalgebra::{Affine2, Matrix3};
use skrifa::instance::{Location, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider, Tag};
use slam_render::Extent;
use slam_render::image::Image;

use crate::{FontId, FontRegistry, PositionedGlyph};

// Own MSDF generator ported from msdfgen (ADR-0062). Not yet used by
// `generate_glyph`; it switches over together with error correction.
#[allow(dead_code)]
mod coloring;
#[allow(dead_code)]
mod generate;
#[allow(dead_code)]
mod outline;
#[allow(dead_code)]
mod scanline;
#[allow(dead_code)]
mod segment;
#[allow(dead_code)]
mod selector;
#[allow(dead_code)]
mod shape;
#[allow(dead_code)]
mod solver;
#[cfg(test)]
mod tests;
#[allow(dead_code)]
mod vector;

/// Atlas pixels per em: glyph bitmaps are generated at this size; the renderer
/// scales by `font_size / EM_PX`.
pub const EM_PX: f32 = 32.0;
/// Total distance range in atlas pixels: the field covers
/// `[-DISTANCE_RANGE / 2, DISTANCE_RANGE / 2]` around the outline.
pub const DISTANCE_RANGE: f64 = 4.0;
/// Empty pixels around the outline on each side (`DISTANCE_RANGE / 2`).
pub const PADDING: u32 = 2;
/// Longest accepted bitmap side in pixels.
pub const MAX_GLYPH_SIDE: u32 = 512;

/// Corner angle threshold of the edge coloring (sine of the angle).
const SIN_ALPHA: f64 = 0.03;
/// Fixed edge coloring seed, so generation is deterministic.
const COLORING_SEED: u64 = 0;
const WGHT: Tag = Tag::new(b"wght");

/// One glyph of one face at one weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub font: FontId,
    /// Effective weight; used as the `wght` coordinate of variable faces.
    pub weight: u16,
    pub glyph_id: u16,
}

impl GlyphKey {
    /// Key of a laid-out glyph.
    pub fn of(glyph: &PositionedGlyph) -> Self {
        Self {
            font: glyph.font,
            weight: glyph.weight,
            glyph_id: glyph.glyph_id,
        }
    }
}

/// A generated glyph bitmap.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBitmap {
    /// RGBA8 (alpha 255), tight rows, top row first. Inside the glyph the
    /// median of RGB is above 127, outside below 128.
    pub image: Image,
    /// Top-left corner of the bitmap relative to the pen position on the
    /// baseline, in atlas pixels (`EM_PX` scale), y down. Integer values.
    pub offset: [f32; 2],
}

/// Why a glyph bitmap could not be generated.
#[derive(Debug, thiserror::Error)]
pub enum GlyphError {
    #[error("unknown font")]
    UnknownFont,
    #[error("font data cannot be parsed")]
    InvalidFont,
    #[error("glyph outline cannot be read")]
    Outline,
    #[error("glyph bitmap {width}x{height} exceeds {MAX_GLYPH_SIDE} px")]
    TooLarge { width: u32, height: u32 },
}

/// Collects outline commands into fdsm contours (font units, y up).
#[derive(Default)]
struct ShapePen {
    shape: Shape<Contour>,
    contour: Contour,
    start: Point,
    current: Point,
}

fn pt(x: f32, y: f32) -> Point {
    Point::new(f64::from(x), f64::from(y))
}

impl ShapePen {
    fn flush(&mut self) {
        let contour = std::mem::take(&mut self.contour);
        if !contour.segments.is_empty() {
            self.shape.contours.push(contour);
        }
    }
}

impl OutlinePen for ShapePen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.flush();
        self.start = pt(x, y);
        self.current = self.start;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = pt(x, y);
        if p != self.current {
            self.contour.segments.push(Segment::line(self.current, p));
            self.current = p;
        }
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let p = pt(x, y);
        self.contour
            .segments
            .push(Segment::quad(self.current, pt(cx0, cy0), p));
        self.current = p;
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let p = pt(x, y);
        self.contour
            .segments
            .push(Segment::cubic(self.current, pt(cx0, cy0), pt(cx1, cy1), p));
        self.current = p;
    }

    fn close(&mut self) {
        if self.current != self.start {
            self.contour
                .segments
                .push(Segment::line(self.current, self.start));
            self.current = self.start;
        }
        self.flush();
    }
}

/// Bounds of all control points: `[xmin, ymin, xmax, ymax]`.
fn control_bounds(shape: &Shape<Contour>) -> Option<[f64; 4]> {
    let mut b: Option<[f64; 4]> = None;
    for seg in shape.contours.iter().flat_map(|c| &c.segments) {
        for i in 0..seg.order() as usize + 1 {
            let p = seg.control_point(i);
            let b = b.get_or_insert([p.x, p.y, p.x, p.y]);
            b[0] = b[0].min(p.x);
            b[1] = b[1].min(p.y);
            b[2] = b[2].max(p.x);
            b[3] = b[3].max(p.y);
        }
    }
    b
}

/// Generates the MSDF bitmap of `key`, or `None` for an empty outline (space).
///
/// Deterministic: the same registry contents and key give identical pixels.
pub fn generate_glyph(
    registry: &FontRegistry,
    key: GlyphKey,
) -> Result<Option<GlyphBitmap>, GlyphError> {
    let (data, index) = registry
        .face_data(key.font)
        .ok_or(GlyphError::UnknownFont)?;
    let face = FontRef::from_index(data, index).map_err(|_| GlyphError::InvalidFont)?;
    let location: Location = if registry.is_weight_variable(key.font) {
        face.axes().location([(WGHT, f32::from(key.weight))])
    } else {
        Location::default()
    };

    let outlines = face.outline_glyphs();
    let glyph = outlines
        .get(GlyphId::new(u32::from(key.glyph_id)))
        .ok_or(GlyphError::Outline)?;
    let mut pen = ShapePen::default();
    glyph
        .draw(
            DrawSettings::unhinted(Size::unscaled(), &location),
            &mut pen,
        )
        .map_err(|_| GlyphError::Outline)?;
    pen.flush();
    let mut shape = pen.shape;

    let Some([xmin, ymin, xmax, ymax]) = control_bounds(&shape) else {
        return Ok(None);
    };
    let upem = face
        .metrics(Size::unscaled(), &location)
        .units_per_em
        .max(1);
    let s = f64::from(EM_PX) / f64::from(upem);
    let pad = f64::from(PADDING);
    let x0 = (xmin * s).floor() - pad;
    let y_top = (ymax * s).ceil() + pad;
    let width = ((xmax * s).ceil() - (xmin * s).floor() + 2.0 * pad) as u32;
    let height = ((ymax * s).ceil() - (ymin * s).floor() + 2.0 * pad) as u32;
    if width > MAX_GLYPH_SIDE || height > MAX_GLYPH_SIDE {
        return Err(GlyphError::TooLarge { width, height });
    }

    // Font units (y up) to bitmap pixels (y down): a reflection, so not a similarity.
    #[rustfmt::skip]
    let m = Matrix3::new(
        s, 0.0, -x0,
        0.0, -s, y_top,
        0.0, 0.0, 1.0,
    );
    shape.transform(&Affine2::from_matrix_unchecked(m));

    let colored = Shape::edge_coloring_simple(shape, SIN_ALPHA, COLORING_SEED);
    let prepared = colored.prepare();
    let mut field = Rgb32FImage::new(width, height);
    generate_msdf(&prepared, DISTANCE_RANGE, &mut field);
    // Error correction needs the unsigned-corrected field and runs before sign correction.
    correct_error_msdf(
        &mut field,
        &colored,
        &prepared,
        DISTANCE_RANGE,
        &ErrorCorrectionConfig::default(),
    );
    correct_sign_msdf(&mut field, &prepared, FillRule::Nonzero);

    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for px in field.pixels() {
        let [r, g, b] = px.0.map(to_u8);
        pixels.extend_from_slice(&[r, g, b, 255]);
    }
    Ok(Some(GlyphBitmap {
        image: Image {
            size: Extent { width, height },
            pixels,
        },
        offset: [x0 as f32, -y_top as f32],
    }))
}

fn to_u8(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}
