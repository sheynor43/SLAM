// Copyright (c) 2014 - 2025 Viktor Chlumsky (msdfgen, MIT licence; see THIRD_PARTY_NOTICES).
// Ported from msdfgen v1.13: ext/import-font.cpp (outline decomposition), main.cpp (pipeline)

//! Glyph outline to [`Shape`] and the field pipeline of ADR-0062.

use skrifa::outline::OutlinePen;

use super::coloring::edge_coloring_simple;
use super::error_correction::msdf_error_correction;
use super::generate::{MsdfField, Projection, Range, distance_sign_correction, generate_msdf};
use super::scanline::FillRule;
use super::segment::EdgeSegment;
use super::shape::{Contour, Shape};
use super::vector::{Point2, cross};

/// Corner angle threshold of the edge coloring, radians (msdfgen `DEFAULT_ANGLE_THRESHOLD`).
pub const ANGLE_THRESHOLD: f64 = 3.0;
/// Fixed edge coloring seed, so generation is deterministic.
pub const COLORING_SEED: u64 = 0;

/// Collects outline commands into a [`Shape`] in font units (y up), skipping
/// degenerate edges like msdfgen's FreeType importer (`ext/import-font.cpp`).
#[derive(Default)]
pub struct OutlineShapePen {
    shape: Shape,
    contour: Contour,
    start: Point2,
    position: Point2,
}

fn pt(x: f32, y: f32) -> Point2 {
    Point2::new(f64::from(x), f64::from(y))
}

impl OutlineShapePen {
    fn flush(&mut self) {
        let contour = std::mem::take(&mut self.contour);
        if !contour.edges.is_empty() {
            self.shape.contours.push(contour);
        }
    }

    /// The collected shape; an unclosed last contour is kept as is.
    pub fn finish(mut self) -> Shape {
        self.flush();
        self.shape
    }
}

impl OutlinePen for OutlineShapePen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.flush();
        self.start = pt(x, y);
        self.position = self.start;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.line_to_point(pt(x, y));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let endpoint = pt(x, y);
        if endpoint != self.position {
            self.contour.edges.push(EdgeSegment::quadratic(
                self.position,
                pt(cx0, cy0),
                endpoint,
            ));
            self.position = endpoint;
        }
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let endpoint = pt(x, y);
        let (c0, c1) = (pt(cx0, cy0), pt(cx1, cy1));
        if endpoint != self.position || cross(c0 - endpoint, c1 - endpoint) != 0.0 {
            self.contour
                .edges
                .push(EdgeSegment::cubic(self.position, c0, c1, endpoint));
            self.position = endpoint;
        }
    }

    fn close(&mut self) {
        // FreeType closes every contour with a line back to its start.
        self.line_to_point(self.start);
        self.flush();
    }
}

impl OutlineShapePen {
    fn line_to_point(&mut self, endpoint: Point2) {
        if endpoint != self.position {
            self.contour
                .edges
                .push(EdgeSegment::linear(self.position, endpoint));
            self.position = endpoint;
        }
    }
}

/// The msdfgen utility pipeline of ADR-0062: normalize, simple edge coloring,
/// MSDF with overlap support, scanline sign correction with the non-zero rule,
/// then error correction.
pub fn shape_msdf(
    shape: Shape,
    width: usize,
    height: usize,
    projection: &Projection,
    range: Range,
) -> MsdfField {
    let shape = colored_shape(shape);
    let mut field = uncorrected_msdf(&shape, width, height, projection, range);
    msdf_error_correction(&mut field, &shape, projection, range);
    field
}

/// Normalizes `shape` and applies the simple edge coloring.
pub fn colored_shape(mut shape: Shape) -> Shape {
    shape.normalize();
    edge_coloring_simple(&mut shape, ANGLE_THRESHOLD, COLORING_SEED);
    shape
}

/// The field of a [`colored_shape`] with sign correction, before error correction.
pub fn uncorrected_msdf(
    shape: &Shape,
    width: usize,
    height: usize,
    projection: &Projection,
    range: Range,
) -> MsdfField {
    let mut field = MsdfField::new(width, height);
    generate_msdf(&mut field, shape, projection, range);
    distance_sign_correction(
        &mut field,
        shape,
        projection,
        range.zero_value(),
        FillRule::NonZero,
    );
    field
}
