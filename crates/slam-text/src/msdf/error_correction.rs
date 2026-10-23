// Copyright (c) 2014 - 2025 Viktor Chlumsky (msdfgen, MIT licence; see THIRD_PARTY_NOTICES).
// Ported from msdfgen v1.13: core/MSDFErrorCorrection.cpp, core/msdf-error-correction.cpp

//! MSDF error correction: `EDGE_PRIORITY` mode with `DO_NOT_CHECK_DISTANCE`.
//!
//! Texels whose interpolation with a neighbour would produce a median outside
//! the expected range are flattened to their median. Texels at corners and
//! edges are protected and only flattened for inversion artifacts.

use super::generate::{MsdfField, Projection, Range};
use super::segment::{BLUE, GREEN, RED};
use super::shape::Shape;
use super::solver::solve_quadratic;
use super::vector::{Vector2, median};

/// `ErrorCorrectionConfig::defaultMinDeviationRatio`.
#[allow(clippy::excessive_precision)] // literal kept verbatim from msdfgen
pub const DEFAULT_MIN_DEVIATION_RATIO: f64 = 1.11111111111111111;

const ARTIFACT_T_EPSILON: f64 = 0.01;
const PROTECTION_RADIUS_TOLERANCE: f64 = 1.001;

const CLASSIFIER_FLAG_CANDIDATE: u8 = 0x01;
const CLASSIFIER_FLAG_ARTIFACT: u8 = 0x02;

/// Stencil flag: texel potentially causing interpolation errors.
const ERROR: u8 = 1;
/// Stencil flag: texel only flagged as an error for inversion artifacts.
const PROTECTED: u8 = 2;

/// `mix` of two floats with a double weight, rounded back to float.
fn mix32(a: f32, b: f32, weight: f64) -> f32 {
    ((1.0 - weight) * f64::from(a) + weight * f64::from(b)) as f32
}

/// `BaseArtifactClassifier`: recognizes artifacts from the field alone.
#[derive(Clone, Copy)]
struct ArtifactClassifier {
    span: f64,
    protected_flag: bool,
}

impl ArtifactClassifier {
    /// Whether the median `xm` interpolated at `xt` between `am` at `at` and
    /// `bm` at `bt` indicates an artifact.
    fn range_test(&self, at: f64, bt: f64, xt: f64, am: f32, bm: f32, xm: f32) -> u8 {
        // For protected texels, only consider inversion artifacts (interpolated median has different sign than boundaries). For the rest, it is sufficient that the interpolated median is outside its boundaries.
        if (am > 0.5 && bm > 0.5 && xm <= 0.5)
            || (am < 0.5 && bm < 0.5 && xm >= 0.5)
            || (!self.protected_flag && median(am, bm, xm) != xm)
        {
            let ax_span = (xt - at) * self.span;
            let bx_span = (bt - xt) * self.span;
            let (am, bm, xm) = (f64::from(am), f64::from(bm), f64::from(xm));
            // Check if the interpolated median's value is in the expected range based on its distance (span) from boundaries a, b.
            if !(xm >= am - ax_span
                && xm <= am + ax_span
                && xm >= bm - bx_span
                && xm <= bm + bx_span)
            {
                return CLASSIFIER_FLAG_CANDIDATE | CLASSIFIER_FLAG_ARTIFACT;
            }
            return CLASSIFIER_FLAG_CANDIDATE;
        }
        0
    }

    fn evaluate(flags: u8) -> bool {
        flags & CLASSIFIER_FLAG_ARTIFACT != 0
    }
}

/// Whether `channel` contributes to an edge between texels `a` and `b`.
fn edge_between_texels_channel(a: &[f32; 3], b: &[f32; 3], channel: usize) -> bool {
    // Find interpolation ratio t (0 < t < 1) where an edge is expected (mix(a[channel], b[channel], t) == 0.5).
    let t = (f64::from(a[channel]) - 0.5) / f64::from(a[channel] - b[channel]);
    if t > 0.0 && t < 1.0 {
        // Interpolate channel values at t.
        let c = [
            mix32(a[0], b[0], t),
            mix32(a[1], b[1], t),
            mix32(a[2], b[2], t),
        ];
        // This is only an edge if the zero-distance channel is the median.
        return median(c[0], c[1], c[2]) == c[channel];
    }
    false
}

/// Bit mask of the channels contributing to an edge between `a` and `b`.
fn edge_between_texels(a: &[f32; 3], b: &[f32; 3]) -> u8 {
    RED * u8::from(edge_between_texels_channel(a, b, 0))
        + GREEN * u8::from(edge_between_texels_channel(a, b, 1))
        + BLUE * u8::from(edge_between_texels_channel(a, b, 2))
}

/// Protects the texel if one of its non-median channels is in `mask`.
fn protect_extreme_channels(stencil: &mut u8, msd: &[f32; 3], m: f32, mask: u8) {
    if (mask & RED != 0 && msd[0] != m)
        || (mask & GREEN != 0 && msd[1] != m)
        || (mask & BLUE != 0 && msd[2] != m)
    {
        *stencil |= PROTECTED;
    }
}

/// Median of the linear interpolation of `a`, `b` at `t`.
fn interpolated_median(a: &[f32; 3], b: &[f32; 3], t: f64) -> f32 {
    median(
        mix32(a[0], b[0], t),
        mix32(a[1], b[1], t),
        mix32(a[2], b[2], t),
    )
}

/// Median of the bilinear interpolation with constant `a`, linear `l` and quadratic `q` terms at `t`.
fn interpolated_median_quadratic(a: &[f32; 3], l: &[f32; 3], q: &[f32; 3], t: f64) -> f32 {
    let at = |i: usize| t * (t * f64::from(q[i]) + f64::from(l[i])) + f64::from(a[i]);
    median(at(0), at(1), at(2)) as f32
}

/// Checks a linear interpolation artifact where two channels are equal (`dA`, `dB` are their differences).
fn has_linear_artifact_inner(
    classifier: ArtifactClassifier,
    am: f32,
    bm: f32,
    a: &[f32; 3],
    b: &[f32; 3],
    d_a: f32,
    d_b: f32,
) -> bool {
    // Find interpolation ratio t (0 < t < 1) where two color channels are equal (mix(dA, dB, t) == 0).
    let t = f64::from(d_a) / f64::from(d_a - d_b);
    if t > ARTIFACT_T_EPSILON && t < 1.0 - ARTIFACT_T_EPSILON {
        // Interpolate median at t and let the classifier decide if its value indicates an artifact.
        let xm = interpolated_median(a, b, t);
        return ArtifactClassifier::evaluate(classifier.range_test(0.0, 1.0, t, am, bm, xm));
    }
    false
}

/// Checks a bilinear interpolation artifact where two channels are equal.
#[allow(clippy::too_many_arguments)]
fn has_diagonal_artifact_inner(
    classifier: ArtifactClassifier,
    am: f32,
    dm: f32,
    a: &[f32; 3],
    l: &[f32; 3],
    q: &[f32; 3],
    d_a: f32,
    d_bc: f32,
    d_d: f32,
    t_ex0: f64,
    t_ex1: f64,
) -> bool {
    // Find interpolation ratios t (0 < t[i] < 1) where two color channels are equal.
    let mut t = [0.0; 3];
    let solutions = solve_quadratic(
        &mut t,
        f64::from(d_d - d_bc + d_a),
        f64::from(d_bc - d_a - d_a),
        f64::from(d_a),
    );
    for &ti in t.iter().take(solutions.max(0) as usize) {
        // Solutions t[i] == 0 and t[i] == 1 are singularities and occur very often because two channels are usually equal at texels.
        if ti > ARTIFACT_T_EPSILON && ti < 1.0 - ARTIFACT_T_EPSILON {
            // Interpolate median xm at t.
            let xm = interpolated_median_quadratic(a, l, q, ti);
            // Determine if xm deviates too much from medians of a, d.
            let mut range_flags = classifier.range_test(0.0, 1.0, ti, am, dm, xm);
            // Additionally, check xm against the interpolated medians at the local extremes tEx0, tEx1.
            for t_ex in [t_ex0, t_ex1] {
                if t_ex > 0.0 && t_ex < 1.0 {
                    let mut t_end = [0.0, 1.0];
                    let mut em = [am, dm];
                    let i = usize::from(t_ex > ti);
                    t_end[i] = t_ex;
                    em[i] = interpolated_median_quadratic(a, l, q, t_ex);
                    range_flags |= classifier.range_test(t_end[0], t_end[1], ti, em[0], em[1], xm);
                }
            }
            if ArtifactClassifier::evaluate(range_flags) {
                return true;
            }
        }
    }
    false
}

/// Checks a linear interpolation artifact between horizontally or vertically adjacent texels `a`, `b`.
fn has_linear_artifact(
    classifier: ArtifactClassifier,
    am: f32,
    a: &[f32; 3],
    b: &[f32; 3],
) -> bool {
    let bm = median(b[0], b[1], b[2]);
    // Out of the pair, only report artifacts for the texel further from the edge to minimize side effects.
    (am - 0.5).abs() >= (bm - 0.5).abs()
        && (
            // Check points where each pair of color channels meets.
            has_linear_artifact_inner(classifier, am, bm, a, b, a[1] - a[0], b[1] - b[0])
                || has_linear_artifact_inner(classifier, am, bm, a, b, a[2] - a[1], b[2] - b[1])
                || has_linear_artifact_inner(classifier, am, bm, a, b, a[0] - a[2], b[0] - b[2])
        )
}

/// Checks a bilinear interpolation artifact between diagonally adjacent texels `a`, `d` (`b`, `c` form the other diagonal).
fn has_diagonal_artifact(
    classifier: ArtifactClassifier,
    am: f32,
    a: &[f32; 3],
    b: &[f32; 3],
    c: &[f32; 3],
    d: &[f32; 3],
) -> bool {
    let dm = median(d[0], d[1], d[2]);
    // Out of the pair, only report artifacts for the texel further from the edge to minimize side effects.
    if (am - 0.5).abs() >= (dm - 0.5).abs() {
        let abc = [a[0] - b[0] - c[0], a[1] - b[1] - c[1], a[2] - b[2] - c[2]];
        // Compute the linear terms for bilinear interpolation.
        let l = [-a[0] - abc[0], -a[1] - abc[1], -a[2] - abc[2]];
        // Compute the quadratic terms for bilinear interpolation.
        let q = [d[0] + abc[0], d[1] + abc[1], d[2] + abc[2]];
        // Compute interpolation ratios tEx (0 < tEx[i] < 1) for the local extremes of each color channel (the derivative 2*q[i]*tEx[i]+l[i] == 0).
        let t_ex = [0, 1, 2].map(|i| -0.5 * f64::from(l[i]) / f64::from(q[i]));
        // Check points where each pair of color channels meets.
        return has_diagonal_artifact_inner(
            classifier,
            am,
            dm,
            a,
            &l,
            &q,
            a[1] - a[0],
            b[1] - b[0] + c[1] - c[0],
            d[1] - d[0],
            t_ex[0],
            t_ex[1],
        ) || has_diagonal_artifact_inner(
            classifier,
            am,
            dm,
            a,
            &l,
            &q,
            a[2] - a[1],
            b[2] - b[1] + c[2] - c[1],
            d[2] - d[1],
            t_ex[1],
            t_ex[2],
        ) || has_diagonal_artifact_inner(
            classifier,
            am,
            dm,
            a,
            &l,
            &q,
            a[0] - a[2],
            b[0] - b[2] + c[0] - c[2],
            d[0] - d[2],
            t_ex[2],
            t_ex[0],
        );
    }
    false
}

/// `MSDFErrorCorrection` over a stencil of the field's size.
struct ErrorCorrection<'a> {
    stencil: Vec<u8>,
    projection: &'a Projection,
    /// `distanceMapping(DistanceMapping::Delta(1))`.
    unit_delta: f64,
    min_deviation_ratio: f64,
}

impl<'a> ErrorCorrection<'a> {
    fn new(field: &MsdfField, projection: &'a Projection, range: Range) -> Self {
        Self {
            stencil: vec![0; field.width * field.height],
            projection,
            unit_delta: 1.0 / (range.upper - range.lower),
            min_deviation_ratio: DEFAULT_MIN_DEVIATION_RATIO,
        }
    }

    /// Length of the expected median delta between texels in direction `(dx, dy)`.
    fn delta_length(&self, dx: f64, dy: f64) -> f64 {
        self.projection
            .unproject_vector(Vector2::new(dx * self.unit_delta, dy * self.unit_delta))
            .length()
    }

    /// Flags the four texels around every corner as protected.
    fn protect_corners(&mut self, shape: &Shape, width: usize, height: usize) {
        let (w, h) = (width as i64, height as i64);
        for contour in &shape.contours {
            let Some(mut prev_edge) = contour.edges.last() else {
                continue;
            };
            for edge in &contour.edges {
                let common_color = prev_edge.color & edge.color;
                // If the color changes from prevEdge to edge, this is a corner.
                if common_color & common_color.wrapping_sub(1) == 0 {
                    // Find the four texels that envelop the corner and mark them as protected.
                    let p = self.projection.project(edge.point(0.0));
                    let l = (p.x - 0.5).floor() as i64;
                    let b = (p.y - 0.5).floor() as i64;
                    let r = l.saturating_add(1);
                    let t = b.saturating_add(1);
                    // Check that the positions are within bounds.
                    if l < w && b < h && r >= 0 && t >= 0 {
                        for (x, y) in [(l, b), (r, b), (l, t), (r, t)] {
                            if (0..w).contains(&x) && (0..h).contains(&y) {
                                self.stencil[(y * w + x) as usize] |= PROTECTED;
                            }
                        }
                    }
                }
                prev_edge = edge;
            }
        }
    }

    /// Flags texels contributing to edges as protected.
    fn protect_edges(&mut self, sdf: &MsdfField) {
        let (w, h) = (sdf.width, sdf.height);
        let px = &sdf.pixels;
        let m = |p: &[f32; 3]| median(p[0], p[1], p[2]);
        let near = |a: f32, b: f32, radius: f32| (a - 0.5).abs() + (b - 0.5).abs() < radius;
        let radius = |dx, dy| (PROTECTION_RADIUS_TOLERANCE * self.delta_length(dx, dy)) as f32;
        let (h_radius, v_radius, d_radius) = (radius(1.0, 0.0), radius(0.0, 1.0), radius(1.0, 1.0));
        let stencil = &mut self.stencil;
        let mut protect_pair = |i: usize, j: usize, mi: f32, mj: f32| {
            let mask = edge_between_texels(&px[i], &px[j]);
            protect_extreme_channels(&mut stencil[i], &px[i], mi, mask);
            protect_extreme_channels(&mut stencil[j], &px[j], mj, mask);
        };
        // Horizontal texel pairs
        for y in 0..h {
            for x in 0..w.saturating_sub(1) {
                let (i, j) = (y * w + x, y * w + x + 1);
                let (lm, rm) = (m(&px[i]), m(&px[j]));
                if near(lm, rm, h_radius) {
                    protect_pair(i, j, lm, rm);
                }
            }
        }
        // Vertical texel pairs
        for y in 0..h.saturating_sub(1) {
            for x in 0..w {
                let (i, j) = (y * w + x, (y + 1) * w + x);
                let (bm, tm) = (m(&px[i]), m(&px[j]));
                if near(bm, tm, v_radius) {
                    protect_pair(i, j, bm, tm);
                }
            }
        }
        // Diagonal texel pairs
        for y in 0..h.saturating_sub(1) {
            for x in 0..w.saturating_sub(1) {
                let (lb, rb) = (y * w + x, y * w + x + 1);
                let (lt, rt) = (lb + w, rb + w);
                let (mlb, mrb, mlt, mrt) = (m(&px[lb]), m(&px[rb]), m(&px[lt]), m(&px[rt]));
                if near(mlb, mrt, d_radius) {
                    protect_pair(lb, rt, mlb, mrt);
                }
                if near(mrb, mlt, d_radius) {
                    protect_pair(rb, lt, mrb, mlt);
                }
            }
        }
    }

    /// Flags texels expected to cause interpolation artifacts, from the field alone.
    fn find_errors(&mut self, sdf: &MsdfField) {
        let (w, h) = (sdf.width, sdf.height);
        let px = &sdf.pixels;
        // Compute the expected deltas between values of horizontally, vertically, and diagonally adjacent texels.
        let h_span = self.min_deviation_ratio * self.delta_length(1.0, 0.0);
        let v_span = self.min_deviation_ratio * self.delta_length(0.0, 1.0);
        let d_span = self.min_deviation_ratio * self.delta_length(1.0, 1.0);
        // Inspect all texels.
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let c = &px[i];
                let cm = median(c[0], c[1], c[2]);
                let protected_flag = self.stencil[i] & PROTECTED != 0;
                let hc = ArtifactClassifier {
                    span: h_span,
                    protected_flag,
                };
                let vc = ArtifactClassifier {
                    span: v_span,
                    protected_flag,
                };
                let dc = ArtifactClassifier {
                    span: d_span,
                    protected_flag,
                };
                let (left, right, bottom, top) = (x > 0, x < w - 1, y > 0, y < h - 1);
                // Mark current texel c with the error flag if an artifact occurs when it's interpolated with any of its 8 neighbors.
                let error = (left && has_linear_artifact(hc, cm, c, &px[i - 1]))
                    || (bottom && has_linear_artifact(vc, cm, c, &px[i - w]))
                    || (right && has_linear_artifact(hc, cm, c, &px[i + 1]))
                    || (top && has_linear_artifact(vc, cm, c, &px[i + w]))
                    || (left
                        && bottom
                        && has_diagonal_artifact(
                            dc,
                            cm,
                            c,
                            &px[i - 1],
                            &px[i - w],
                            &px[i - w - 1],
                        ))
                    || (right
                        && bottom
                        && has_diagonal_artifact(
                            dc,
                            cm,
                            c,
                            &px[i + 1],
                            &px[i - w],
                            &px[i - w + 1],
                        ))
                    || (left
                        && top
                        && has_diagonal_artifact(
                            dc,
                            cm,
                            c,
                            &px[i - 1],
                            &px[i + w],
                            &px[i + w - 1],
                        ))
                    || (right
                        && top
                        && has_diagonal_artifact(
                            dc,
                            cm,
                            c,
                            &px[i + 1],
                            &px[i + w],
                            &px[i + w + 1],
                        ));
                if error {
                    self.stencil[i] |= ERROR;
                }
            }
        }
    }

    /// Sets all channels of texels flagged as errors to their median.
    fn apply(&self, sdf: &mut MsdfField) {
        for (pixel, &mask) in sdf.pixels.iter_mut().zip(&self.stencil) {
            if mask & ERROR != 0 {
                let m = median(pixel[0], pixel[1], pixel[2]);
                *pixel = [m; 3];
            }
        }
    }
}

/// `msdfErrorCorrection` with `EDGE_PRIORITY`, `DO_NOT_CHECK_DISTANCE` and
/// the default deviation ratio. `shape` is the normalized, colored shape the
/// field was generated from.
pub fn msdf_error_correction(
    sdf: &mut MsdfField,
    shape: &Shape,
    projection: &Projection,
    range: Range,
) {
    let mut ec = ErrorCorrection::new(sdf, projection, range);
    ec.protect_corners(shape, sdf.width, sdf.height);
    ec.protect_edges(sdf);
    ec.find_errors(sdf);
    ec.apply(sdf);
}

#[cfg(test)]
pub(super) fn corrected_pixels(before: &MsdfField, after: &MsdfField) -> usize {
    before
        .pixels
        .iter()
        .zip(&after.pixels)
        .filter(|(a, b)| a != b)
        .count()
}
