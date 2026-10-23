//! MSDF bitmap of one glyph (ADR-0037).
//!
//! Pure and thread-agnostic: atlas code calls [`generate_glyph`] on worker
//! threads and uploads the result.

use skrifa::instance::{Location, Size};
use skrifa::outline::DrawSettings;
use skrifa::{FontRef, GlyphId, MetadataProvider, Tag};
use slam_render::Extent;
use slam_render::image::Image;

use crate::{FontId, FontRegistry, PositionedGlyph};

use self::generate::{Projection, Range};
use self::outline::{OutlineShapePen, shape_msdf};
use self::vector::Vector2;

// MSDF generator ported from msdfgen (ADR-0062) with contour union (ADR-0063).
mod coloring;
mod error_correction;
mod generate;
mod outline;
mod scanline;
mod segment;
mod selector;
mod shape;
mod solver;
#[cfg(test)]
mod tests;
mod union;
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
    let mut pen = OutlineShapePen::default();
    glyph
        .draw(
            DrawSettings::unhinted(Size::unscaled(), &location),
            &mut pen,
        )
        .map_err(|_| GlyphError::Outline)?;
    let shape = pen.finish();

    let Some([xmin, ymin, xmax, ymax]) = shape.bounds() else {
        return Ok(None);
    };
    let upem = face
        .metrics(Size::unscaled(), &location)
        .units_per_em
        .max(1);
    let s = f64::from(EM_PX) / f64::from(upem);
    let pad = f64::from(PADDING);
    let x0 = (xmin * s).floor() - pad;
    let y0 = (ymin * s).floor() - pad;
    let width = ((xmax * s).ceil() - x0 + pad) as u32;
    let height = ((ymax * s).ceil() - y0 + pad) as u32;
    if width > MAX_GLYPH_SIDE || height > MAX_GLYPH_SIDE {
        return Err(GlyphError::TooLarge { width, height });
    }

    // Font units (y up) to bitmap pixels; field row 0 is the bottom row.
    let projection = Projection {
        scale: Vector2::new(s, s),
        translate: Vector2::new(-x0 / s, -y0 / s),
    };
    let field = shape_msdf(
        shape,
        width as usize,
        height as usize,
        &projection,
        Range::symmetric(DISTANCE_RANGE / s),
    );

    let mut pixels = Vec::with_capacity(field.pixels.len() * 4);
    for row in field.pixels.chunks_exact(field.width.max(1)).rev() {
        for px in row {
            let [r, g, b] = px.map(to_u8);
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Ok(Some(GlyphBitmap {
        image: Image {
            size: Extent { width, height },
            pixels,
        },
        offset: [x0 as f32, -(y0 + f64::from(height)) as f32],
    }))
}

fn to_u8(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}
