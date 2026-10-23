// Copyright (c) 2014 - 2025 Viktor Chlumsky (msdfgen, MIT licence; see THIRD_PARTY_NOTICES).
// Ported from msdfgen v1.13: core/edge-segments.cpp, core/EdgeColor.h

//! Edge segments: lines, quadratic and cubic Bézier curves.

use super::solver::{solve_cubic, solve_quadratic};
use super::vector::{Point2, SignedDistance, Vector2, cross, dot, mix, non_zero_sign, sign};

/// Parameters of the iterative closest-point search on a cubic curve.
const CUBIC_SEARCH_STARTS: i32 = 4;
const CUBIC_SEARCH_STEPS: i32 = 4;

/// Colour channels an edge belongs to (bit mask, `EdgeColor` in msdfgen).
pub type EdgeColor = u8;
pub const RED: EdgeColor = 1;
pub const GREEN: EdgeColor = 2;
pub const YELLOW: EdgeColor = 3;
pub const BLUE: EdgeColor = 4;
pub const MAGENTA: EdgeColor = 5;
pub const CYAN: EdgeColor = 6;
pub const WHITE: EdgeColor = 7;

/// Control points of an edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Linear([Point2; 2]),
    Quadratic([Point2; 3]),
    Cubic([Point2; 4]),
}

/// An edge segment with its colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeSegment {
    pub curve: Curve,
    pub color: EdgeColor,
}

impl EdgeSegment {
    pub fn linear(p0: Point2, p1: Point2) -> Self {
        Self::with(Curve::Linear([p0, p1]))
    }

    /// A quadratic curve, or a line if the control points are collinear
    /// (`EdgeSegment::create`).
    pub fn quadratic(p0: Point2, p1: Point2, p2: Point2) -> Self {
        if cross(p1 - p0, p2 - p1) == 0.0 {
            return Self::linear(p0, p2);
        }
        Self::with(Curve::Quadratic([p0, p1, p2]))
    }

    /// A cubic curve, or a lower-order curve if it is degenerate
    /// (`EdgeSegment::create`).
    pub fn cubic(p0: Point2, p1: Point2, p2: Point2, p3: Point2) -> Self {
        let p12 = p2 - p1;
        if cross(p1 - p0, p12) == 0.0 && cross(p12, p3 - p2) == 0.0 {
            return Self::linear(p0, p3);
        }
        let p12 = 1.5 * p1 - 0.5 * p0;
        if p12 == 1.5 * p2 - 0.5 * p3 {
            return Self::with(Curve::Quadratic([p0, p12, p3]));
        }
        Self::with(Curve::Cubic([p0, p1, p2, p3]))
    }

    fn with(curve: Curve) -> Self {
        Self {
            curve,
            color: WHITE,
        }
    }

    fn colored(curve: Curve, color: EdgeColor) -> Self {
        Self { curve, color }
    }

    pub fn control_points(&self) -> &[Point2] {
        match &self.curve {
            Curve::Linear(p) => p,
            Curve::Quadratic(p) => p,
            Curve::Cubic(p) => p,
        }
    }

    /// Point at `param` in `[0, 1]`.
    pub fn point(&self, param: f64) -> Point2 {
        match self.curve {
            Curve::Linear(p) => mix(p[0], p[1], param),
            Curve::Quadratic(p) => mix(mix(p[0], p[1], param), mix(p[1], p[2], param), param),
            Curve::Cubic(p) => {
                let p12 = mix(p[1], p[2], param);
                mix(
                    mix(mix(p[0], p[1], param), p12, param),
                    mix(p12, mix(p[2], p[3], param), param),
                    param,
                )
            }
        }
    }

    /// Direction (first derivative) at `param`.
    pub fn direction(&self, param: f64) -> Vector2 {
        match self.curve {
            Curve::Linear(p) => p[1] - p[0],
            Curve::Quadratic(p) => {
                let tangent = mix(p[1] - p[0], p[2] - p[1], param);
                if !tangent.is_nonzero() {
                    return p[2] - p[0];
                }
                tangent
            }
            Curve::Cubic(p) => {
                let tangent = mix(
                    mix(p[1] - p[0], p[2] - p[1], param),
                    mix(p[2] - p[1], p[3] - p[2], param),
                    param,
                );
                if !tangent.is_nonzero() {
                    if param == 0.0 {
                        return p[2] - p[0];
                    }
                    if param == 1.0 {
                        return p[3] - p[1];
                    }
                }
                tangent
            }
        }
    }

    /// Minimum signed distance from `origin` to the edge and the parameter
    /// of the closest point (may lie outside `[0, 1]`).
    pub fn signed_distance(&self, origin: Point2) -> (SignedDistance, f64) {
        match self.curve {
            Curve::Linear(p) => linear_distance(p, origin),
            Curve::Quadratic(p) => self.quadratic_distance(p, origin),
            Curve::Cubic(p) => self.cubic_distance(p, origin),
        }
    }

    fn quadratic_distance(&self, p: [Point2; 3], origin: Point2) -> (SignedDistance, f64) {
        let qa = p[0] - origin;
        let ab = p[1] - p[0];
        let br = p[2] - p[1] - ab;
        let a = dot(br, br);
        let b = 3.0 * dot(ab, br);
        let c = 2.0 * dot(ab, ab) + dot(qa, br);
        let d = dot(qa, ab);
        let mut t = [0.0; 3];
        let solutions = solve_cubic(&mut t, a, b, c, d);

        let mut ep_dir = self.direction(0.0);
        let mut min_distance = non_zero_sign(cross(ep_dir, qa)) * qa.length(); // distance from A
        let mut param = -dot(qa, ep_dir) / dot(ep_dir, ep_dir);
        {
            let distance = (p[2] - origin).length(); // distance from B
            if distance < min_distance.abs() {
                ep_dir = self.direction(1.0);
                min_distance = non_zero_sign(cross(ep_dir, p[2] - origin)) * distance;
                param = dot(origin - p[1], ep_dir) / dot(ep_dir, ep_dir);
            }
        }
        for &ti in t.iter().take(solutions.max(0) as usize) {
            if ti > 0.0 && ti < 1.0 {
                let qe = qa + 2.0 * ti * ab + ti * ti * br;
                let distance = qe.length();
                if distance <= min_distance.abs() {
                    min_distance = non_zero_sign(cross(ab + ti * br, qe)) * distance;
                    param = ti;
                }
            }
        }

        let sd = if (0.0..=1.0).contains(&param) {
            SignedDistance::new(min_distance, 0.0)
        } else if param < 0.5 {
            SignedDistance::new(
                min_distance,
                dot(self.direction(0.0).normalize(false), qa.normalize(false)).abs(),
            )
        } else {
            SignedDistance::new(
                min_distance,
                dot(
                    self.direction(1.0).normalize(false),
                    (p[2] - origin).normalize(false),
                )
                .abs(),
            )
        };
        (sd, param)
    }

    fn cubic_distance(&self, p: [Point2; 4], origin: Point2) -> (SignedDistance, f64) {
        let qa = p[0] - origin;
        let ab = p[1] - p[0];
        let br = p[2] - p[1] - ab;
        let as_ = (p[3] - p[2]) - (p[2] - p[1]) - br;

        let mut ep_dir = self.direction(0.0);
        let mut min_distance = non_zero_sign(cross(ep_dir, qa)) * qa.length(); // distance from A
        let mut param = -dot(qa, ep_dir) / dot(ep_dir, ep_dir);
        {
            let distance = (p[3] - origin).length(); // distance from B
            if distance < min_distance.abs() {
                ep_dir = self.direction(1.0);
                min_distance = non_zero_sign(cross(ep_dir, p[3] - origin)) * distance;
                param = dot(ep_dir - (p[3] - origin), ep_dir) / dot(ep_dir, ep_dir);
            }
        }
        // Iterative minimum distance search
        for i in 0..=CUBIC_SEARCH_STARTS {
            let mut t = 1.0 / f64::from(CUBIC_SEARCH_STARTS) * f64::from(i);
            let mut qe = qa + 3.0 * t * ab + 3.0 * t * t * br + t * t * t * as_;
            let mut d1 = 3.0 * ab + 6.0 * t * br + 3.0 * t * t * as_;
            let mut d2 = 6.0 * br + 6.0 * t * as_;
            let mut improved_t = t - dot(qe, d1) / (dot(d1, d1) + dot(qe, d2));
            if improved_t > 0.0 && improved_t < 1.0 {
                let mut remaining_steps = CUBIC_SEARCH_STEPS;
                loop {
                    t = improved_t;
                    qe = qa + 3.0 * t * ab + 3.0 * t * t * br + t * t * t * as_;
                    d1 = 3.0 * ab + 6.0 * t * br + 3.0 * t * t * as_;
                    remaining_steps -= 1;
                    if remaining_steps == 0 {
                        break;
                    }
                    d2 = 6.0 * br + 6.0 * t * as_;
                    improved_t = t - dot(qe, d1) / (dot(d1, d1) + dot(qe, d2));
                    if !(improved_t > 0.0 && improved_t < 1.0) {
                        break;
                    }
                }
                let distance = qe.length();
                if distance < min_distance.abs() {
                    min_distance = non_zero_sign(cross(d1, qe)) * distance;
                    param = t;
                }
            }
        }

        let sd = if (0.0..=1.0).contains(&param) {
            SignedDistance::new(min_distance, 0.0)
        } else if param < 0.5 {
            SignedDistance::new(
                min_distance,
                dot(self.direction(0.0).normalize(false), qa.normalize(false)).abs(),
            )
        } else {
            SignedDistance::new(
                min_distance,
                dot(
                    self.direction(1.0).normalize(false),
                    (p[3] - origin).normalize(false),
                )
                .abs(),
            )
        };
        (sd, param)
    }

    /// Replaces the distance by the perpendicular distance to the extension
    /// of the edge when the closest point lies beyond an end.
    pub fn distance_to_perpendicular_distance(
        &self,
        distance: &mut SignedDistance,
        origin: Point2,
        param: f64,
    ) {
        if param < 0.0 {
            let dir = self.direction(0.0).normalize(false);
            let aq = origin - self.point(0.0);
            let ts = dot(aq, dir);
            if ts < 0.0 {
                let perpendicular_distance = cross(aq, dir);
                if perpendicular_distance.abs() <= distance.distance.abs() {
                    distance.distance = perpendicular_distance;
                    distance.dot = 0.0;
                }
            }
        } else if param > 1.0 {
            let dir = self.direction(1.0).normalize(false);
            let bq = origin - self.point(1.0);
            let ts = dot(bq, dir);
            if ts > 0.0 {
                let perpendicular_distance = cross(bq, dir);
                if perpendicular_distance.abs() <= distance.distance.abs() {
                    distance.distance = perpendicular_distance;
                    distance.dot = 0.0;
                }
            }
        }
    }

    /// Intersections with the horizontal line at `y`: X coordinates in `x`,
    /// crossing directions in `dy`. Returns how many there are (at most 3).
    pub fn scanline_intersections(&self, x: &mut [f64; 3], dy: &mut [i32; 3], y: f64) -> usize {
        match self.curve {
            Curve::Linear(p) => {
                if (y >= p[0].y && y < p[1].y) || (y >= p[1].y && y < p[0].y) {
                    let param = (y - p[0].y) / (p[1].y - p[0].y);
                    x[0] = super::vector::mix_f(p[0].x, p[1].x, param);
                    dy[0] = sign(p[1].y - p[0].y);
                    return 1;
                }
                0
            }
            Curve::Quadratic(p) => quadratic_scanline(p, x, dy, y),
            Curve::Cubic(p) => cubic_scanline(p, x, dy, y),
        }
    }

    /// Grows `[x_min, y_min, x_max, y_max]` to contain the edge.
    pub fn bound(&self, b: &mut [f64; 4]) {
        match self.curve {
            Curve::Linear(p) => {
                point_bounds(p[0], b);
                point_bounds(p[1], b);
            }
            Curve::Quadratic(p) => {
                point_bounds(p[0], b);
                point_bounds(p[2], b);
                let bot = (p[1] - p[0]) - (p[2] - p[1]);
                if bot.x != 0.0 {
                    let param = (p[1].x - p[0].x) / bot.x;
                    if param > 0.0 && param < 1.0 {
                        point_bounds(self.point(param), b);
                    }
                }
                if bot.y != 0.0 {
                    let param = (p[1].y - p[0].y) / bot.y;
                    if param > 0.0 && param < 1.0 {
                        point_bounds(self.point(param), b);
                    }
                }
            }
            Curve::Cubic(p) => {
                point_bounds(p[0], b);
                point_bounds(p[3], b);
                let a0 = p[1] - p[0];
                let a1 = 2.0 * (p[2] - p[1] - a0);
                let a2 = p[3] - 3.0 * p[2] + 3.0 * p[1] - p[0];
                let mut params = [0.0; 3];
                let solutions = solve_quadratic(&mut params, a2.x, a1.x, a0.x);
                for &t in params.iter().take(solutions.max(0) as usize) {
                    if t > 0.0 && t < 1.0 {
                        point_bounds(self.point(t), b);
                    }
                }
                let solutions = solve_quadratic(&mut params, a2.y, a1.y, a0.y);
                for &t in params.iter().take(solutions.max(0) as usize) {
                    if t > 0.0 && t < 1.0 {
                        point_bounds(self.point(t), b);
                    }
                }
            }
        }
    }

    /// Swaps the start and end of the edge.
    #[cfg(test)]
    pub fn reverse(&mut self) {
        match &mut self.curve {
            Curve::Linear(p) => p.swap(0, 1),
            Curve::Quadratic(p) => p.swap(0, 2),
            Curve::Cubic(p) => {
                p.swap(0, 3);
                p.swap(1, 2);
            }
        }
    }

    /// Three edges of the same colour that together form this one.
    pub fn split_in_thirds(&self) -> [Self; 3] {
        let c = self.color;
        match self.curve {
            Curve::Linear(p) => [
                Self::colored(Curve::Linear([p[0], self.point(1.0 / 3.0)]), c),
                Self::colored(
                    Curve::Linear([self.point(1.0 / 3.0), self.point(2.0 / 3.0)]),
                    c,
                ),
                Self::colored(Curve::Linear([self.point(2.0 / 3.0), p[1]]), c),
            ],
            Curve::Quadratic(p) => [
                Self::colored(
                    Curve::Quadratic([p[0], mix(p[0], p[1], 1.0 / 3.0), self.point(1.0 / 3.0)]),
                    c,
                ),
                Self::colored(
                    Curve::Quadratic([
                        self.point(1.0 / 3.0),
                        mix(mix(p[0], p[1], 5.0 / 9.0), mix(p[1], p[2], 4.0 / 9.0), 0.5),
                        self.point(2.0 / 3.0),
                    ]),
                    c,
                ),
                Self::colored(
                    Curve::Quadratic([self.point(2.0 / 3.0), mix(p[1], p[2], 2.0 / 3.0), p[2]]),
                    c,
                ),
            ],
            Curve::Cubic(p) => {
                let t1 = 1.0 / 3.0;
                let t2 = 2.0 / 3.0;
                [
                    Self::colored(
                        Curve::Cubic([
                            p[0],
                            if p[0] == p[1] {
                                p[0]
                            } else {
                                mix(p[0], p[1], t1)
                            },
                            mix(mix(p[0], p[1], t1), mix(p[1], p[2], t1), t1),
                            self.point(t1),
                        ]),
                        c,
                    ),
                    Self::colored(
                        Curve::Cubic([
                            self.point(t1),
                            mix(
                                mix(mix(p[0], p[1], t1), mix(p[1], p[2], t1), t1),
                                mix(mix(p[1], p[2], t1), mix(p[2], p[3], t1), t1),
                                t2,
                            ),
                            mix(
                                mix(mix(p[0], p[1], t2), mix(p[1], p[2], t2), t2),
                                mix(mix(p[1], p[2], t2), mix(p[2], p[3], t2), t2),
                                t1,
                            ),
                            self.point(t2),
                        ]),
                        c,
                    ),
                    Self::colored(
                        Curve::Cubic([
                            self.point(t2),
                            mix(mix(p[1], p[2], t2), mix(p[2], p[3], t2), t2),
                            if p[2] == p[3] {
                                p[3]
                            } else {
                                mix(p[2], p[3], t2)
                            },
                            p[3],
                        ]),
                        c,
                    ),
                ]
            }
        }
    }

    /// The same quadratic curve as a cubic one; other edges are unchanged.
    pub fn to_cubic(self) -> Self {
        match self.curve {
            Curve::Quadratic(p) => Self::colored(
                Curve::Cubic([
                    p[0],
                    mix(p[0], p[1], 2.0 / 3.0),
                    mix(p[1], p[2], 1.0 / 3.0),
                    p[2],
                ]),
                self.color,
            ),
            _ => self,
        }
    }
}

fn linear_distance(p: [Point2; 2], origin: Point2) -> (SignedDistance, f64) {
    let aq = origin - p[0];
    let ab = p[1] - p[0];
    let param = dot(aq, ab) / dot(ab, ab);
    let eq = p[usize::from(param > 0.5)] - origin;
    let endpoint_distance = eq.length();
    if param > 0.0 && param < 1.0 {
        let ortho_distance = dot(ab.orthonormal(false), aq);
        if ortho_distance.abs() < endpoint_distance {
            return (SignedDistance::new(ortho_distance, 0.0), param);
        }
    }
    (
        SignedDistance::new(
            non_zero_sign(cross(aq, ab)) * endpoint_distance,
            dot(ab.normalize(false), eq.normalize(false)).abs(),
        ),
        param,
    )
}

fn quadratic_scanline(p: [Point2; 3], x: &mut [f64; 3], dy: &mut [i32; 3], y: f64) -> usize {
    let mut total = 0;
    let mut next_dy = if y > p[0].y { 1 } else { -1 };
    x[total] = p[0].x;
    if p[0].y == y {
        if p[0].y < p[1].y || (p[0].y == p[1].y && p[0].y < p[2].y) {
            dy[total] = 1;
            total += 1;
        } else {
            next_dy = 1;
        }
    }
    {
        let ab = p[1] - p[0];
        let br = p[2] - p[1] - ab;
        let mut t = [0.0; 3];
        let solutions = solve_quadratic(&mut t, br.y, 2.0 * ab.y, p[0].y - y);
        // Sort solutions
        if solutions >= 2 && t[0] > t[1] {
            t.swap(0, 1);
        }
        let mut i = 0;
        while i < solutions && total < 2 {
            let ti = t[i as usize];
            if (0.0..=1.0).contains(&ti) {
                x[total] = p[0].x + 2.0 * ti * ab.x + ti * ti * br.x;
                if f64::from(next_dy) * (ab.y + ti * br.y) >= 0.0 {
                    dy[total] = next_dy;
                    total += 1;
                    next_dy = -next_dy;
                }
            }
            i += 1;
        }
    }
    if p[2].y == y {
        if next_dy > 0 && total > 0 {
            total -= 1;
            next_dy = -1;
        }
        if (p[2].y < p[1].y || (p[2].y == p[1].y && p[2].y < p[0].y)) && total < 2 {
            x[total] = p[2].x;
            if next_dy < 0 {
                dy[total] = -1;
                total += 1;
                next_dy = 1;
            }
        }
    }
    if next_dy != (if y >= p[2].y { 1 } else { -1 }) {
        if total > 0 {
            total -= 1;
        } else {
            if (p[2].y - y).abs() < (p[0].y - y).abs() {
                x[total] = p[2].x;
            }
            dy[total] = next_dy;
            total += 1;
        }
    }
    total
}

fn cubic_scanline(p: [Point2; 4], x: &mut [f64; 3], dy: &mut [i32; 3], y: f64) -> usize {
    let mut total = 0;
    let mut next_dy = if y > p[0].y { 1 } else { -1 };
    x[total] = p[0].x;
    if p[0].y == y {
        if p[0].y < p[1].y
            || (p[0].y == p[1].y && (p[0].y < p[2].y || (p[0].y == p[2].y && p[0].y < p[3].y)))
        {
            dy[total] = 1;
            total += 1;
        } else {
            next_dy = 1;
        }
    }
    {
        let ab = p[1] - p[0];
        let br = p[2] - p[1] - ab;
        let as_ = (p[3] - p[2]) - (p[2] - p[1]) - br;
        let mut t = [0.0; 3];
        let solutions = solve_cubic(&mut t, as_.y, 3.0 * br.y, 3.0 * ab.y, p[0].y - y);
        // Sort solutions
        if solutions >= 2 {
            if t[0] > t[1] {
                t.swap(0, 1);
            }
            if solutions >= 3 && t[1] > t[2] {
                t.swap(1, 2);
                if t[0] > t[1] {
                    t.swap(0, 1);
                }
            }
        }
        let mut i = 0;
        while i < solutions && total < 3 {
            let ti = t[i as usize];
            if (0.0..=1.0).contains(&ti) {
                x[total] = p[0].x + 3.0 * ti * ab.x + 3.0 * ti * ti * br.x + ti * ti * ti * as_.x;
                if f64::from(next_dy) * (ab.y + 2.0 * ti * br.y + ti * ti * as_.y) >= 0.0 {
                    dy[total] = next_dy;
                    total += 1;
                    next_dy = -next_dy;
                }
            }
            i += 1;
        }
    }
    if p[3].y == y {
        if next_dy > 0 && total > 0 {
            total -= 1;
            next_dy = -1;
        }
        if (p[3].y < p[2].y
            || (p[3].y == p[2].y && (p[3].y < p[1].y || (p[3].y == p[1].y && p[3].y < p[0].y))))
            && total < 3
        {
            x[total] = p[3].x;
            if next_dy < 0 {
                dy[total] = -1;
                total += 1;
                next_dy = 1;
            }
        }
    }
    if next_dy != (if y >= p[3].y { 1 } else { -1 }) {
        if total > 0 {
            total -= 1;
        } else {
            if (p[3].y - y).abs() < (p[0].y - y).abs() {
                x[total] = p[3].x;
            }
            dy[total] = next_dy;
            total += 1;
        }
    }
    total
}

fn point_bounds(p: Point2, b: &mut [f64; 4]) {
    if p.x < b[0] {
        b[0] = p.x;
    }
    if p.y < b[1] {
        b[1] = p.y;
    }
    if p.x > b[2] {
        b[2] = p.x;
    }
    if p.y > b[3] {
        b[3] = p.y;
    }
}
