//! Piecewise-linear approximation of slider curves (osu-framework's `PathApproximator`).
//!
//! Only the curve types a slider path uses are ported; the editor's fitting helpers are not.

use slam_formats::osu::Vec2;

use super::circular_arc::CircularArcProperties;

/// `PathApproximator.BEZIER_TOLERANCE`.
const BEZIER_TOLERANCE: f32 = 0.25;

/// `PathApproximator.catmull_detail`: pieces per control point quadruplet.
pub(crate) const CATMULL_DETAIL: i32 = 50;

/// `PathApproximator.circular_arc_tolerance`.
pub(crate) const CIRCULAR_ARC_TOLERANCE: f32 = 0.1;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/PathApproximator.cs (BezierToPiecewiseLinear)
/// Approximates a bezier curve: a B-spline whose degree equals the point count minus one.
pub fn bezier_to_piecewise_linear(control_points: &[Vec2]) -> Vec<Vec2> {
    let degree = (control_points.len() as i32 - 1).max(1);
    bspline_to_piecewise_linear(control_points, degree)
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/PathApproximator.cs (BSplineToPiecewiseLinear)
/// Approximates a clamped uniform B-spline of the given degree by splitting it into bezier
/// segments at its knots and adaptively subdividing them until they are flat enough.
///
/// A degree below 1 is clamped to 1 (lazer throws); a degree above the point count minus one
/// is lowered to it.
pub fn bspline_to_piecewise_linear(control_points: &[Vec2], degree: i32) -> Vec<Vec2> {
    if control_points.len() < 2 {
        return control_points.to_vec();
    }

    let point_count = control_points.len() - 1;
    // Clamped to [1, point_count], so the cast cannot truncate.
    let degree = (degree.max(1) as usize).min(point_count);

    let mut output = Vec::new();
    let mut to_flatten = bspline_to_bezier_internal(control_points, degree);
    let mut free_buffers: Vec<Vec<Vec2>> = Vec::new();

    let count = degree + 1;
    let mut midpoints = vec![Vec2::ZERO; count];
    let mut left_child = vec![Vec2::ZERO; count];
    let mut right_half = vec![Vec2::ZERO; count];
    let mut approx_buffer = vec![Vec2::ZERO; degree * 2 + 1];

    // Depth-first refinement with an explicit stack, popping the leftmost curve first.
    while let Some(mut parent) = to_flatten.pop() {
        if bezier_is_flat_enough(&parent) {
            bezier_approximate(
                &parent,
                &mut output,
                &mut midpoints,
                &mut right_half,
                &mut approx_buffer,
                count,
            );
            free_buffers.push(parent);
            continue;
        }

        let mut right_child = free_buffers
            .pop()
            .unwrap_or_else(|| vec![Vec2::ZERO; count]);
        bezier_subdivide(
            &parent,
            &mut left_child,
            &mut right_child,
            &mut midpoints,
            count,
        );
        parent.copy_from_slice(&left_child);

        to_flatten.push(right_child);
        to_flatten.push(parent);
    }

    output.push(control_points[point_count]);
    output
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/PathApproximator.cs (CatmullToPiecewiseLinear, catmullFindPoint)
/// Approximates a Catmull-Rom spline with [`CATMULL_DETAIL`] segment pairs per control point.
pub fn catmull_to_piecewise_linear(control_points: &[Vec2]) -> Vec<Vec2> {
    let segments = control_points.len().saturating_sub(1);
    let mut result = Vec::with_capacity(segments * CATMULL_DETAIL as usize * 2);

    for i in 0..segments {
        let v1 = if i > 0 {
            control_points[i - 1]
        } else {
            control_points[i]
        };
        let v2 = control_points[i];
        // `i < Length - 1` always holds inside the loop.
        let v3 = control_points[i + 1];
        let v4 = if i + 2 < control_points.len() {
            control_points[i + 2]
        } else {
            v3 + v3 - v2
        };

        for c in 0..CATMULL_DETAIL {
            result.push(catmull_find_point(
                v1,
                v2,
                v3,
                v4,
                c as f32 / CATMULL_DETAIL as f32,
            ));
            result.push(catmull_find_point(
                v1,
                v2,
                v3,
                v4,
                (c + 1) as f32 / CATMULL_DETAIL as f32,
            ));
        }
    }

    result
}

fn catmull_find_point(v1: Vec2, v2: Vec2, v3: Vec2, v4: Vec2, t: f32) -> Vec2 {
    let t2 = t * t;
    let t3 = t * t2;

    let x = 0.5
        * (2.0 * v2.x
            + (-v1.x + v3.x) * t
            + (2.0 * v1.x - 5.0 * v2.x + 4.0 * v3.x - v4.x) * t2
            + (-v1.x + 3.0 * v2.x - 3.0 * v3.x + v4.x) * t3);
    let y = 0.5
        * (2.0 * v2.y
            + (-v1.y + v3.y) * t
            + (2.0 * v1.y - 5.0 * v2.y + 4.0 * v3.y - v4.y) * t2
            + (-v1.y + 3.0 * v2.y - 3.0 * v3.y + v4.y) * t3);

    Vec2::new(x, y)
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/PathApproximator.cs (CircularArcToPiecewiseLinear)
/// Approximates a circular arc through three points; falls back to a bezier curve when the
/// points are (almost) collinear.
pub fn circular_arc_to_piecewise_linear(control_points: &[Vec2]) -> Vec<Vec2> {
    let Some(pr) = CircularArcProperties::new(control_points) else {
        return bezier_to_piecewise_linear(control_points);
    };

    let amount_points = pr.amount_points();
    let mut output = Vec::with_capacity(amount_points as usize);

    for i in 0..amount_points {
        let fract = f64::from(i) / f64::from(amount_points - 1);
        let theta = pr.theta_start + pr.direction * fract * pr.theta_range;
        let o = Vec2::new(theta.cos() as f32, theta.sin() as f32) * pr.radius;
        output.push(pr.centre + o);
    }

    output
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/PathApproximator.cs (LinearToPiecewiseLinear)
/// A linear path is its own approximation.
pub fn linear_to_piecewise_linear(control_points: &[Vec2]) -> Vec<Vec2> {
    control_points.to_vec()
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/PathApproximator.cs (bSplineToBezierInternal)
/// Splits a B-spline into bezier segments at its knots (Boehm's algorithm). Returns a stack
/// whose top is the first segment.
fn bspline_to_bezier_internal(control_points: &[Vec2], degree: usize) -> Vec<Vec<Vec2>> {
    let point_count = control_points.len() - 1;
    let mut points = control_points.to_vec();

    if degree == point_count {
        // B-spline subdivision unnecessary, degenerate to single bezier.
        return vec![points];
    }

    let mut result = Vec::with_capacity(point_count - degree + 1);

    for i in 0..point_count - degree {
        let mut sub_bezier = vec![Vec2::ZERO; degree + 1];
        sub_bezier[0] = points[i];

        // Destructively insert the knot degree-1 times via Boehm's algorithm.
        for j in 0..degree - 1 {
            sub_bezier[j + 1] = points[i + 1];

            for k in 1..degree - j {
                let l = k.min(point_count - degree - i);
                // Counts are bounded by the point count of a parsed path.
                let lf = l as f32;
                let divisor = (l + 1) as f32;
                points[i + k] = (lf * points[i + k] + points[i + k + 1]) / divisor;
            }
        }

        sub_bezier[degree] = points[i + 1];
        result.push(sub_bezier);
    }

    result.push(points[point_count - degree..].to_vec());
    // Reversed so that popping yields the segments in order.
    result.reverse();
    result
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/PathApproximator.cs (bezierIsFlatEnough)
/// Whether the second-order finite differences of the control points are within tolerance.
fn bezier_is_flat_enough(control_points: &[Vec2]) -> bool {
    for i in 1..control_points.len().saturating_sub(1) {
        let d = control_points[i - 1] - 2.0 * control_points[i] + control_points[i + 1];
        if d.length_squared() > BEZIER_TOLERANCE * BEZIER_TOLERANCE * 4.0 {
            return false;
        }
    }

    true
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/PathApproximator.cs (bezierSubdivide)
/// Splits a bezier curve of `count` control points into its left and right halves.
fn bezier_subdivide(
    control_points: &[Vec2],
    l: &mut [Vec2],
    r: &mut [Vec2],
    midpoints: &mut [Vec2],
    count: usize,
) {
    midpoints[..count].copy_from_slice(&control_points[..count]);

    for i in 0..count {
        l[i] = midpoints[0];
        r[count - i - 1] = midpoints[count - i - 1];

        for j in 0..count - i - 1 {
            midpoints[j] = (midpoints[j] + midpoints[j + 1]) / 2.0;
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/PathApproximator.cs (bezierApproximate)
/// Appends a piecewise-linear approximation of a flat bezier curve with as many points as it
/// has control points, except the last one.
fn bezier_approximate(
    control_points: &[Vec2],
    output: &mut Vec<Vec2>,
    midpoints: &mut [Vec2],
    r: &mut [Vec2],
    l: &mut [Vec2],
    count: usize,
) {
    // Lazer uses the midpoint buffer as the right half too; the values are the same.
    bezier_subdivide(control_points, l, r, midpoints, count);

    l[count..2 * count - 1].copy_from_slice(&r[1..count]);

    output.push(control_points[0]);

    for i in 1..count - 1 {
        let index = 2 * i;
        let p = 0.25 * (l[index - 1] + 2.0 * l[index] + l[index + 1]);
        output.push(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    fn almost(a: Vec2, b: Vec2, eps: f32) -> bool {
        (a.x - b.x).abs() <= eps && (a.y - b.y).abs() <= eps
    }

    // Values from osu-framework's TestPathApproximator.TestBSpline.
    #[test]
    fn bspline_matches_framework_test() {
        let points = [
            v(0.0, 0.0),
            v(1.0, 0.0),
            v(1.0, -1.0),
            v(-1.0, -1.0),
            v(-1.0, 1.0),
            v(3.0, 2.0),
            v(3.0, 0.0),
        ];

        let approximated = bspline_to_piecewise_linear(&points, 4);
        assert_eq!(approximated.len(), 29);
        assert!(almost(approximated[0], points[0], 1e-4));
        assert!(almost(approximated[28], points[6], 1e-4));
        assert!(almost(approximated[10], v(-0.11415, -0.69065), 1e-4));
    }

    #[test]
    fn short_inputs_are_returned_as_is() {
        assert!(bspline_to_piecewise_linear(&[], 3).is_empty());
        assert_eq!(
            bspline_to_piecewise_linear(&[v(1.0, 2.0)], 3),
            vec![v(1.0, 2.0)]
        );
    }

    #[test]
    fn straight_bezier_is_already_flat() {
        let points = [v(0.0, 0.0), v(50.0, 0.0), v(100.0, 0.0)];
        assert_eq!(
            bezier_to_piecewise_linear(&points),
            vec![v(0.0, 0.0), v(50.0, 0.0), v(100.0, 0.0)]
        );
    }

    #[test]
    fn catmull_emits_two_points_per_piece() {
        let points = [v(0.0, 0.0), v(10.0, 10.0), v(20.0, 0.0)];
        let path = catmull_to_piecewise_linear(&points);
        assert_eq!(path.len(), 2 * CATMULL_DETAIL as usize * 2);
        assert_eq!(path[0], points[0]);
        assert_eq!(*path.last().unwrap(), points[2]);
    }

    #[test]
    fn collinear_arc_falls_back_to_bezier() {
        let points = [v(0.0, 0.0), v(50.0, 0.0), v(100.0, 0.0)];
        assert_eq!(
            circular_arc_to_piecewise_linear(&points),
            bezier_to_piecewise_linear(&points)
        );
    }
}
