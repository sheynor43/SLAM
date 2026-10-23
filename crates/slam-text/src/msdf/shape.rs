// Copyright (c) 2014 - 2025 Viktor Chlumsky (msdfgen, MIT licence; see THIRD_PARTY_NOTICES).
// Ported from msdfgen v1.13: core/Shape.cpp (incl. orientContours), core/Contour.cpp, core/convergent-curve-ordering.cpp

//! Shapes made of closed contours.

use super::scanline::{Intersection, Scanline};
use super::segment::{Curve, EdgeSegment};
use super::vector::{Point2, Vector2, cross, dot, mix_f, sign};

/// Threshold of the dot product of adjacent edge directions to be considered convergent.
const CORNER_DOT_EPSILON: f64 = 0.000001;
/// Moves control points slightly more than necessary to account for floating-point errors.
#[allow(clippy::excessive_precision)] // msdfgen's literal
const DECONVERGE_OVERSHOOT: f64 = 1.11111111111111111;

/// A closed sequence of edges.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contour {
    pub edges: Vec<EdgeSegment>,
}

fn shoelace(a: Point2, b: Point2) -> f64 {
    (b.x - a.x) * (a.y + b.y)
}

impl Contour {
    /// Orientation of the contour: 1, -1 or 0 when degenerate.
    pub fn winding(&self) -> i32 {
        let edges = &self.edges;
        if edges.is_empty() {
            return 0;
        }
        let mut total = 0.0;
        if edges.len() == 1 {
            let a = edges[0].point(0.0);
            let b = edges[0].point(1.0 / 3.0);
            let c = edges[0].point(2.0 / 3.0);
            total += shoelace(a, b);
            total += shoelace(b, c);
            total += shoelace(c, a);
        } else if edges.len() == 2 {
            let a = edges[0].point(0.0);
            let b = edges[0].point(0.5);
            let c = edges[1].point(0.0);
            let d = edges[1].point(0.5);
            total += shoelace(a, b);
            total += shoelace(b, c);
            total += shoelace(c, d);
            total += shoelace(d, a);
        } else {
            let mut prev = edges[edges.len() - 1].point(0.0);
            for edge in edges {
                let cur = edge.point(0.0);
                total += shoelace(prev, cur);
                prev = cur;
            }
        }
        sign(total)
    }

    /// Reverses the direction of the contour.
    pub fn reverse(&mut self) {
        self.edges.reverse();
        for edge in &mut self.edges {
            edge.reverse();
        }
    }
}

/// A vector shape. Y points up.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub contours: Vec<Contour>,
}

impl Shape {
    /// Prepares the geometry for distance field generation: splits
    /// single-edge contours and pushes apart convergent edges.
    pub fn normalize(&mut self) {
        for contour in &mut self.contours {
            if contour.edges.len() == 1 {
                let parts = contour.edges[0].split_in_thirds();
                contour.edges.clear();
                contour.edges.extend_from_slice(&parts);
            } else if !contour.edges.is_empty() {
                // Push apart convergent edge segments
                let n = contour.edges.len();
                let mut prev = n - 1;
                for cur in 0..n {
                    let prev_dir = contour.edges[prev].direction(1.0).normalize(false);
                    let cur_dir = contour.edges[cur].direction(0.0).normalize(false);
                    if dot(prev_dir, cur_dir) < CORNER_DOT_EPSILON - 1.0 {
                        let factor = DECONVERGE_OVERSHOOT
                            * (1.0 - (CORNER_DOT_EPSILON - 1.0) * (CORNER_DOT_EPSILON - 1.0))
                                .sqrt()
                            / (CORNER_DOT_EPSILON - 1.0);
                        let mut axis = factor * (cur_dir - prev_dir).normalize(false);
                        if convergent_curve_ordering(&contour.edges[prev], &contour.edges[cur]) < 0
                        {
                            axis = -axis;
                        }
                        deconverge_edge(&mut contour.edges[prev], 1, axis.orthogonal(true));
                        deconverge_edge(&mut contour.edges[cur], 0, axis.orthogonal(false));
                    }
                    prev = cur;
                }
            }
        }
    }

    /// Reverses contours whose orientation is opposite to the one implied by
    /// their position: an even-odd scanline through each contour decides
    /// whether it is an outer contour or a hole.
    pub fn orient_contours(&mut self) {
        struct Hit {
            x: f64,
            direction: i32,
            contour: usize,
        }

        // an irrational number to minimize chance of intersecting a corner or other point of interest
        let ratio = 0.5 * (5.0f64.sqrt() - 1.0);
        let mut orientations = vec![0i32; self.contours.len()];
        let mut hits: Vec<Hit> = Vec::new();
        for i in 0..self.contours.len() {
            let edges = &self.contours[i].edges;
            if orientations[i] != 0 || edges.is_empty() {
                continue;
            }
            // Find an Y that crosses the contour
            let y0 = edges[0].point(0.0).y;
            let mut y1 = y0;
            for edge in edges {
                if y0 != y1 {
                    break;
                }
                y1 = edge.point(1.0).y;
            }
            for edge in edges {
                if y0 != y1 {
                    break;
                }
                y1 = edge.point(ratio).y; // in case all endpoints are in a horizontal line
            }
            let y = mix_f(y0, y1, ratio);
            // Scanline through whole shape at Y
            let mut x = [0.0; 3];
            let mut dy = [0; 3];
            for (j, contour) in self.contours.iter().enumerate() {
                for edge in &contour.edges {
                    let n = edge.scanline_intersections(&mut x, &mut dy, y);
                    for k in 0..n {
                        hits.push(Hit {
                            x: x[k],
                            direction: dy[k],
                            contour: j,
                        });
                    }
                }
            }
            if !hits.is_empty() {
                // `qsort` is not stable; a stable sort keeps the result deterministic
                // and only reorders hits at equal X, which are disqualified below.
                hits.sort_by(|a, b| a.x.total_cmp(&b.x));
                // Disqualify multiple intersections
                for j in 1..hits.len() {
                    if hits[j].x == hits[j - 1].x {
                        hits[j].direction = 0;
                        hits[j - 1].direction = 0;
                    }
                }
                // Inspect scanline and deduce orientations of intersected contours
                for (j, hit) in hits.iter().enumerate() {
                    if hit.direction != 0 {
                        orientations[hit.contour] +=
                            2 * (((j & 1) as i32) ^ i32::from(hit.direction > 0)) - 1;
                    }
                }
                hits.clear();
            }
        }
        // Reverse contours that have the opposite orientation
        for (contour, orientation) in self.contours.iter_mut().zip(orientations) {
            if orientation < 0 {
                contour.reverse();
            }
        }
    }

    /// Bounding box `[x_min, y_min, x_max, y_max]`, or `None` without edges.
    pub fn bounds(&self) -> Option<[f64; 4]> {
        const LARGE_VALUE: f64 = 1e240;
        let mut b = [LARGE_VALUE, LARGE_VALUE, -LARGE_VALUE, -LARGE_VALUE];
        let mut any = false;
        for edge in self.contours.iter().flat_map(|c| &c.edges) {
            edge.bound(&mut b);
            any = true;
        }
        any.then_some(b)
    }

    /// Fills `line` with the intersections of the shape with the horizontal line at `y`.
    pub fn scanline(&self, line: &mut Scanline, y: f64) {
        let intersections = line.intersections_mut();
        intersections.clear();
        let mut x = [0.0; 3];
        let mut dy = [0; 3];
        for edge in self.contours.iter().flat_map(|c| &c.edges) {
            let n = edge.scanline_intersections(&mut x, &mut dy, y);
            for i in 0..n {
                intersections.push(Intersection {
                    x: x[i],
                    direction: dy[i],
                });
            }
        }
        line.preprocess();
    }

    pub fn edge_count(&self) -> usize {
        self.contours.iter().map(|c| c.edges.len()).sum()
    }
}

fn deconverge_edge(edge: &mut EdgeSegment, param: i32, vector: Vector2) {
    if let Curve::Quadratic(_) = edge.curve {
        *edge = edge.to_cubic();
    }
    if let Curve::Cubic(p) = &mut edge.curve {
        match param {
            0 => p[1] += (p[1] - p[0]).length() * vector,
            1 => p[2] += (p[2] - p[3]).length() * vector,
            _ => {}
        }
    }
}

fn simplify_degenerate_curve(cp: &mut [Point2], order: &mut usize) {
    if *order == 3 && (cp[1] == cp[0] || cp[1] == cp[3]) && (cp[2] == cp[0] || cp[2] == cp[3]) {
        cp[1] = cp[3];
        *order = 1;
    }
    if *order == 2 && (cp[1] == cp[0] || cp[1] == cp[2]) {
        cp[1] = cp[2];
        *order = 1;
    }
    if *order == 1 && cp[0] == cp[1] {
        *order = 0;
    }
}

/// Which side the curve before a sharp convergent corner lies on relative
/// to the curve after it, judged by the limit of their cross product.
fn convergent_curve_ordering(a: &EdgeSegment, b: &EdgeSegment) -> i32 {
    let a_points = a.control_points();
    let b_points = b.control_points();
    let mut a_order = a_points.len() - 1;
    let mut b_order = b_points.len() - 1;
    let mut a_cp = [Point2::ZERO; 4];
    let mut b_cp = [Point2::ZERO; 4];
    a_cp[..=a_order].copy_from_slice(a_points);
    b_cp[..=b_order].copy_from_slice(b_points);
    if a_cp[a_order] != b_cp[0] {
        return 0;
    }
    simplify_degenerate_curve(&mut a_cp, &mut a_order);
    simplify_degenerate_curve(&mut b_cp, &mut b_order);
    // Control points before the corner, nearest first: before[i] = *(corner - 1 - i).
    let mut before = [Point2::ZERO; 3];
    for (i, p) in before.iter_mut().take(a_order).enumerate() {
        *p = a_cp[a_order - 1 - i];
    }
    ordering_at_corner(b_cp[0], &before, &b_cp[1..], a_order, b_order)
}

/// `convergentCurveOrdering(corner, controlPointsBefore, controlPointsAfter)`;
/// `before[i]` is `corner[-1 - i]` and `after[i]` is `corner[1 + i]`.
fn ordering_at_corner(
    corner: Point2,
    before: &[Point2],
    after: &[Point2],
    n_before: usize,
    n_after: usize,
) -> i32 {
    if !(n_before > 0 && n_after > 0) {
        return 0;
    }
    let (mut a2, mut a3, mut b2, mut b3) =
        (Vector2::ZERO, Vector2::ZERO, Vector2::ZERO, Vector2::ZERO);
    let mut a1 = before[0] - corner;
    let mut b1 = after[0] - corner;
    if n_before >= 2 {
        a2 = before[1] - before[0] - a1;
    }
    if n_after >= 2 {
        b2 = after[1] - after[0] - b1;
    }
    if n_before >= 3 {
        a3 = before[2] - before[1] - (before[1] - before[0]) - a2;
        a2 *= 3.0;
    }
    if n_after >= 3 {
        b3 = after[2] - after[1] - (after[1] - after[0]) - b2;
        b2 *= 3.0;
    }
    a1 *= n_before as f64;
    b1 *= n_after as f64;
    // Non-degenerate case
    if a1.is_nonzero() && b1.is_nonzero() {
        let as_ = a1.length();
        let bs = b1.length();
        // Third derivative
        let d = as_ * cross(a1, b2) + bs * cross(a2, b1);
        if d != 0.0 {
            return sign(d);
        }
        // Fourth derivative
        let d = as_ * as_ * cross(a1, b3) + as_ * bs * cross(a2, b2) + bs * bs * cross(a3, b1);
        if d != 0.0 {
            return sign(d);
        }
        // Fifth derivative
        let d = as_ * cross(a2, b3) + bs * cross(a3, b2);
        if d != 0.0 {
            return sign(d);
        }
        // Sixth derivative
        return sign(cross(a3, b3));
    }
    // Degenerate curve after corner (control point after corner equals corner)
    let mut s = 1;
    if a1.is_nonzero() {
        // !b1: swap aN <-> bN and handle in the branch below
        b1 = a1;
        std::mem::swap(&mut a2, &mut b2);
        std::mem::swap(&mut a3, &mut b3);
        s = -1; // make sure to also flip output
    }
    // Degenerate curve before corner (control point before corner equals corner)
    if b1.is_nonzero() {
        // Two-and-a-half-th derivative
        let d = cross(a3, b1);
        if d != 0.0 {
            return s * sign(d);
        }
        // Third derivative
        let d = cross(a2, b2);
        if d != 0.0 {
            return s * sign(d);
        }
        // Three-and-a-half-th derivative
        let d = cross(a3, b2);
        if d != 0.0 {
            return s * sign(d);
        }
        // Fourth derivative
        let d = cross(a2, b3);
        if d != 0.0 {
            return s * sign(d);
        }
        // Four-and-a-half-th derivative
        return s * sign(cross(a3, b3));
    }
    // Degenerate curves on both sides of the corner
    let d = a2.length().sqrt() * cross(a2, b3) + b2.length().sqrt() * cross(a3, b2);
    if d != 0.0 {
        return sign(d);
    }
    sign(cross(a3, b3))
}
