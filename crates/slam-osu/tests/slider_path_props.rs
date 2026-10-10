//! Property tests of slider path geometry.

use proptest::prelude::*;
use slam_formats::osu::{PathControlPoint, PathType, Vec2};
use slam_osu::SliderPath;

fn path_type() -> impl Strategy<Value = PathType> {
    prop_oneof![
        Just(PathType::Linear),
        Just(PathType::Bezier),
        Just(PathType::Catmull),
        Just(PathType::PerfectCurve),
        (1..6i32).prop_map(|degree| PathType::BSpline { degree }),
    ]
}

/// Coordinates in a generous playfield range; a third of them integral like most files, the
/// rest with fractions like files written by lazer.
fn coordinate() -> impl Strategy<Value = f32> {
    prop_oneof![(-600i16..1200).prop_map(f32::from), -600.0f32..1200.0,]
}

fn control_point(first: bool) -> impl Strategy<Value = PathControlPoint> {
    let new_segment = if first {
        Just(true).boxed()
    } else {
        prop::bool::weighted(0.2).boxed()
    };
    (coordinate(), coordinate(), new_segment, path_type()).prop_map(|(x, y, new_segment, t)| {
        PathControlPoint {
            position: Vec2::new(x, y),
            path_type: new_segment.then_some(t),
        }
    })
}

fn control_points() -> impl Strategy<Value = Vec<PathControlPoint>> {
    let rest = prop::collection::vec((control_point(false), prop::bool::weighted(0.25)), 1..8);
    (control_point(true), rest).prop_map(|(first, rest)| {
        let mut points = vec![first];
        for (mut point, repeat_previous) in rest {
            // Stacked knots (repeated positions) are common in real maps and hit the
            // zero-length segment and skipped-extension rules.
            if repeat_previous {
                point.position = points[points.len() - 1].position;
            }
            points.push(point);
        }
        points
    })
}

fn is_finite(v: Vec2) -> bool {
    v.x.is_finite() && v.y.is_finite()
}

fn close(a: Vec2, b: Vec2) -> bool {
    let tolerance = 1e-3 * (1.0 + a.length().max(b.length()));
    a.distance(b) <= tolerance
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn path_is_finite_and_monotonic(
        points in control_points(),
        expected in prop::option::of(1.0f64..5000.0),
        optimise_catmull in any::<bool>(),
    ) {
        let path = SliderPath::new(points, expected, optimise_catmull);

        prop_assert!(path.calculated_path().iter().all(|&p| is_finite(p)));
        prop_assert!(path.cumulative_length().iter().all(|l| l.is_finite()));
        // Lengths after the first start from the length removed by the Catmull optimisation,
        // which rounding can make slightly negative (as in lazer), so the first pair may
        // decrease when the optimisation is on.
        let lengths = path.cumulative_length();
        let monotonic_from = usize::from(optimise_catmull).min(lengths.len());
        prop_assert!(lengths[monotonic_from..].windows(2).all(|w| w[0] <= w[1]));

        for i in 0..=20 {
            prop_assert!(is_finite(path.position_at(f64::from(i) / 20.0)));
        }
    }

    #[test]
    fn distance_matches_expected_distance(
        points in control_points(),
        expected in 1.0f64..5000.0,
        optimise_catmull in any::<bool>(),
    ) {
        let path = SliderPath::new(points, Some(expected), optimise_catmull);
        let calculated = path.calculated_distance();
        let vertices = path.calculated_path();

        // Extension is skipped when the last two vertices coincide (as in osu!stable).
        let extension_skipped = vertices.len() >= 2
            && vertices[vertices.len() - 1] == vertices[vertices.len() - 2]
            && expected > calculated;

        if extension_skipped {
            prop_assert_eq!(path.distance(), calculated);
        } else {
            prop_assert_eq!(path.distance(), expected);
        }
    }

    #[test]
    fn distance_without_expected_distance_is_the_approximated_length(
        points in control_points(),
    ) {
        let path = SliderPath::new(points, None, false);
        prop_assert_eq!(path.distance(), path.calculated_distance());
    }

    #[test]
    fn ends_of_the_path_are_its_first_and_last_vertices(
        points in control_points(),
        expected in prop::option::of(1.0f64..5000.0),
    ) {
        let path = SliderPath::new(points, expected, false);
        let vertices = path.calculated_path();

        prop_assert_eq!(path.position_at(0.0), vertices[0]);
        prop_assert!(close(path.position_at(1.0), vertices[vertices.len() - 1]));
    }
}
