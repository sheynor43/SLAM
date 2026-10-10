//! Circle through three points (osu-framework's `CircularArcProperties`).

use std::f64::consts::PI;

use slam_formats::osu::Vec2;

use super::approximator::CIRCULAR_ARC_TOLERANCE;
use crate::precision;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/CircularArcProperties.cs
/// The circular arc through three points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircularArcProperties {
    /// Angle of the first point around the centre.
    pub theta_start: f64,
    /// Angle swept from the first to the last point, always non-negative.
    pub theta_range: f64,
    /// `1` for counter-clockwise (increasing angle), `-1` otherwise.
    pub direction: f64,
    /// Circle radius.
    pub radius: f32,
    /// Circle centre.
    pub centre: Vec2,
}

impl CircularArcProperties {
    /// Computes the arc through the first three points, or `None` when the triangle they form
    /// is (almost) degenerate.
    ///
    /// # Panics
    /// When fewer than three points are given.
    pub fn new(control_points: &[Vec2]) -> Option<Self> {
        let a = control_points[0];
        let b = control_points[1];
        let c = control_points[2];

        // If we have a degenerate triangle where a side-length is almost zero, then give up and
        // fallback to a more numerically stable method.
        if precision::almost_equals_f32(0.0, (b.y - a.y) * (c.x - a.x) - (b.x - a.x) * (c.y - a.y))
        {
            return None;
        }

        // See: https://en.wikipedia.org/wiki/Circumscribed_circle#Cartesian_coordinates_2
        let d = 2.0 * (a.x * (b - c).y + b.x * (c - a).y + c.x * (a - b).y);
        let a_sq = a.length_squared();
        let b_sq = b.length_squared();
        let c_sq = c.length_squared();

        let centre = Vec2::new(
            a_sq * (b - c).y + b_sq * (c - a).y + c_sq * (a - b).y,
            a_sq * (c - b).x + b_sq * (a - c).x + c_sq * (b - a).x,
        ) / d;

        let d_a = a - centre;
        let d_c = c - centre;

        let radius = d_a.length();

        let theta_start = f64::from(d_a.y).atan2(f64::from(d_a.x));
        let mut theta_end = f64::from(d_c.y).atan2(f64::from(d_c.x));

        while theta_end < theta_start {
            theta_end += 2.0 * PI;
        }

        let mut direction = 1.0;
        let mut theta_range = theta_end - theta_start;

        // Decide in which direction to draw the circle, depending on which side of AC B lies.
        let ortho_a_to_c = c - a;
        let ortho_a_to_c = Vec2::new(ortho_a_to_c.y, -ortho_a_to_c.x);

        if ortho_a_to_c.dot(b - a) < 0.0 {
            direction = -direction;
            theta_range = 2.0 * PI - theta_range;
        }

        Some(Self {
            theta_start,
            theta_range,
            direction,
            radius,
            centre,
        })
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Framework/Utils/PathApproximator.cs (CircularArcToPiecewiseLinear)
    /// Number of points approximating the arc: enough for the discrete curvature to stay within
    /// [`CIRCULAR_ARC_TOLERANCE`], and 2 for arcs smaller than the tolerance.
    ///
    /// `SliderPath.calculateSubPath` repeats this formula with the same types.
    pub fn amount_points(&self) -> i32 {
        if 2.0 * self.radius <= CIRCULAR_ARC_TOLERANCE {
            return 2;
        }

        let step = 2.0 * f64::from(1.0 - CIRCULAR_ARC_TOLERANCE / self.radius).acos();
        // `as` saturates and maps NaN to 0, like the double-to-int conversion of .NET 9+.
        ((self.theta_range / step).ceil() as i32).max(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_circle() {
        let points = [
            Vec2::new(100.0, 0.0),
            Vec2::new(70.710_68, 70.710_68),
            Vec2::new(0.0, 100.0),
        ];
        let arc = CircularArcProperties::new(&points).unwrap();
        assert!(arc.centre.length() < 1e-3);
        assert!((arc.radius - 100.0).abs() < 1e-3);
        assert!((arc.theta_range - PI / 2.0).abs() < 1e-5);
        assert_eq!(arc.direction, 1.0);
    }

    #[test]
    fn clockwise_arc_has_negative_direction() {
        let points = [
            Vec2::new(0.0, 100.0),
            Vec2::new(70.710_68, 70.710_68),
            Vec2::new(100.0, 0.0),
        ];
        let arc = CircularArcProperties::new(&points).unwrap();
        assert_eq!(arc.direction, -1.0);
        assert!((arc.theta_range - PI / 2.0).abs() < 1e-5);
    }

    #[test]
    fn collinear_points_are_degenerate() {
        let points = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(2.0, 2.0),
        ];
        assert_eq!(CircularArcProperties::new(&points), None);
    }

    #[test]
    fn huge_radius_saturates_the_point_count() {
        let arc = CircularArcProperties {
            theta_start: 0.0,
            theta_range: 1.0,
            direction: 1.0,
            radius: 1e30,
            centre: Vec2::ZERO,
        };
        // acos(1) = 0, so the ratio is +inf and saturates; SliderPath then falls back to bezier.
        assert_eq!(arc.amount_points(), i32::MAX);
    }
}
