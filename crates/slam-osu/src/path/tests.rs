use super::*;

fn v(x: f32, y: f32) -> Vec2 {
    Vec2::new(x, y)
}

/// Control points where only the first one has a type, like lazer's
/// `SliderPath(PathType, Vector2[], double?)` constructor.
fn points(path_type: PathType, positions: &[Vec2]) -> Vec<PathControlPoint> {
    positions
        .iter()
        .enumerate()
        .map(|(i, &position)| PathControlPoint {
            position,
            path_type: (i == 0).then_some(path_type),
        })
        .collect()
}

fn linear(positions: &[Vec2], expected: Option<f64>) -> SliderPath {
    SliderPath::new(points(PathType::Linear, positions), expected, false)
}

#[test]
fn empty_path() {
    let path = SliderPath::new(Vec::new(), Some(100.0), false);
    assert!(path.calculated_path().is_empty());
    assert_eq!(path.distance(), 0.0);
    assert_eq!(path.position_at(0.5), Vec2::ZERO);
}

#[test]
fn linear_path_without_expected_distance() {
    let path = linear(&[v(0.0, 0.0), v(30.0, 40.0), v(30.0, 140.0)], None);
    assert_eq!(path.distance(), 150.0);
    assert_eq!(path.calculated_distance(), 150.0);
    assert_eq!(path.cumulative_length(), &[0.0, 50.0, 150.0]);
    assert_eq!(path.position_at(0.0), v(0.0, 0.0));
    assert_eq!(path.position_at(1.0 / 3.0), v(30.0, 40.0));
    assert_eq!(path.position_at(1.0), v(30.0, 140.0));
    assert_eq!(path.position_at(2.0), v(30.0, 140.0));
    assert_eq!(path.position_at(-1.0), v(0.0, 0.0));
}

#[test]
fn shortened_path_drops_vertices_past_the_end() {
    let path = linear(&[v(0.0, 0.0), v(100.0, 0.0), v(100.0, 100.0)], Some(50.0));
    assert_eq!(path.distance(), 50.0);
    assert_eq!(path.calculated_distance(), 200.0);
    assert_eq!(path.calculated_path(), &[v(0.0, 0.0), v(50.0, 0.0)]);
    assert_eq!(path.cumulative_length(), &[0.0, 50.0]);
}

#[test]
fn lengthened_path_extends_the_last_segment() {
    let path = linear(&[v(0.0, 0.0), v(0.0, 100.0)], Some(150.0));
    assert_eq!(path.distance(), 150.0);
    assert_eq!(path.calculated_path(), &[v(0.0, 0.0), v(0.0, 150.0)]);
}

#[test]
fn equal_last_points_prevent_extension() {
    let path = linear(&[v(0.0, 0.0), v(100.0, 0.0), v(100.0, 0.0)], Some(150.0));
    assert_eq!(path.distance(), 100.0);
    // Lazer keeps an extra cumulative length in this case.
    assert_eq!(path.cumulative_length(), &[0.0, 100.0, 100.0, 100.0]);
    assert_eq!(path.position_at(1.0), v(100.0, 0.0));
}

#[test]
fn equal_last_points_still_allow_shortening() {
    let path = linear(&[v(0.0, 0.0), v(100.0, 0.0), v(100.0, 0.0)], Some(40.0));
    assert_eq!(path.distance(), 40.0);
    assert_eq!(path.calculated_path(), &[v(0.0, 0.0), v(40.0, 0.0)]);
}

#[test]
fn zero_or_negative_expected_distance_collapses_the_path() {
    for expected in [0.0, -20.0] {
        let path = linear(&[v(0.0, 0.0), v(100.0, 0.0)], Some(expected));
        assert_eq!(path.distance(), 0.0, "expected {expected}");
        assert_eq!(path.position_at(1.0), v(0.0, 0.0));
    }
}

#[test]
fn single_point_path() {
    let path = linear(&[v(5.0, 5.0)], Some(100.0));
    assert_eq!(path.calculated_path(), &[v(5.0, 5.0)]);
    assert_eq!(path.distance(), 0.0);
}

#[test]
fn segments_share_their_joint_vertex() {
    let control_points = vec![
        PathControlPoint {
            position: v(0.0, 0.0),
            path_type: Some(PathType::Linear),
        },
        PathControlPoint {
            position: v(100.0, 0.0),
            path_type: Some(PathType::Linear),
        },
        PathControlPoint {
            position: v(100.0, 100.0),
            path_type: None,
        },
    ];
    let path = SliderPath::new(control_points, Some(150.0), false);
    assert_eq!(path.calculated_path().len(), 3);
    let ends: Vec<f64> = path.segment_ends().collect();
    assert_eq!(ends, vec![100.0 / 150.0, 200.0 / 150.0]);
}

#[test]
fn path_to_progress_interpolates_end_points() {
    let path = linear(&[v(0.0, 0.0), v(100.0, 0.0), v(100.0, 100.0)], None);
    let mut out = vec![v(9.0, 9.0)];
    path.path_to_progress(&mut out, 0.25, 0.75);
    assert_eq!(out, vec![v(50.0, 0.0), v(100.0, 0.0), v(100.0, 50.0)]);
}

#[test]
fn perfect_curve_with_wrong_point_count_is_bezier() {
    let positions = [v(0.0, 0.0), v(50.0, 50.0), v(100.0, 0.0), v(150.0, 50.0)];
    let perfect = SliderPath::new(points(PathType::PerfectCurve, &positions), None, false);
    let bezier = SliderPath::new(points(PathType::Bezier, &positions), None, false);
    assert_eq!(perfect.calculated_path(), bezier.calculated_path());
}

#[test]
fn perfect_curve_follows_the_circle() {
    let positions = [v(0.0, 0.0), v(100.0, 100.0), v(200.0, 0.0)];
    let path = SliderPath::new(points(PathType::PerfectCurve, &positions), None, false);
    let centre = v(100.0, 0.0);
    for p in path.calculated_path() {
        assert!((p.distance(centre) - 100.0).abs() < 1e-3);
    }
    // Half of a circle of radius 100, approximated by chords.
    assert!((path.distance() - std::f64::consts::PI * 100.0).abs() < 0.5);
}

#[test]
fn huge_perfect_curve_falls_back_to_bezier() {
    // Nearly collinear, but not degenerate: the radius is huge, so the arc needs too many
    // points.
    let positions = [v(0.0, 0.0), v(5000.0, 0.5), v(10000.0, 0.0)];
    let perfect = SliderPath::new(points(PathType::PerfectCurve, &positions), None, false);
    let bezier = SliderPath::new(points(PathType::Bezier, &positions), None, false);
    assert_eq!(perfect.calculated_path(), bezier.calculated_path());
}

#[test]
fn catmull_optimisation_keeps_the_length() {
    let positions = [
        v(0.0, 0.0),
        v(0.0, 0.0),
        v(100.0, 50.0),
        v(100.0, 50.0),
        v(200.0, 0.0),
    ];
    let plain = SliderPath::new(points(PathType::Catmull, &positions), None, false);
    let optimised = SliderPath::new(points(PathType::Catmull, &positions), None, true);

    assert!(optimised.calculated_path().len() < plain.calculated_path().len());
    // The removed length is added back, so the path is not stretched to make up for it.
    assert!((optimised.calculated_distance() - plain.calculated_distance()).abs() < 1e-3);
}

#[test]
fn dotnet_binary_search_finds_the_middle_duplicate() {
    let list = [0.0, 1.0, 1.0, 1.0, 1.0, 2.0];
    // lo = 0, hi = 5 -> i = 2.
    assert_eq!(dotnet_binary_search(&list, 1.0), Ok(2));
    assert_eq!(dotnet_binary_search(&list, 1.5), Err(5));
    assert_eq!(dotnet_binary_search(&list, -1.0), Err(0));
    assert_eq!(dotnet_binary_search(&list, 3.0), Err(6));
    assert_eq!(dotnet_binary_search(&[], 3.0), Err(0));
    assert_eq!(dotnet_binary_search(&list, -0.0), Ok(0));
}

#[test]
fn nan_progress_is_the_start() {
    // Math.Clamp keeps NaN, and the search for NaN lands before every length.
    let path = linear(&[v(10.0, 0.0), v(100.0, 0.0)], None);
    assert_eq!(path.position_at(f64::NAN), v(10.0, 0.0));
}

#[test]
fn segment_ends_beyond_a_trimmed_path() {
    let control_points = vec![
        PathControlPoint {
            position: v(0.0, 0.0),
            path_type: Some(PathType::Linear),
        },
        PathControlPoint {
            position: v(100.0, 0.0),
            path_type: Some(PathType::Linear),
        },
        PathControlPoint {
            position: v(100.0, 100.0),
            path_type: None,
        },
    ];
    let path = SliderPath::new(control_points, Some(50.0), false);
    let ends: Vec<f64> = path.segment_ends().collect();
    assert_eq!(ends, vec![2.0, 4.0]);
}

#[test]
fn path_to_progress_with_reversed_range() {
    let path = linear(&[v(0.0, 0.0), v(100.0, 0.0)], None);
    let mut out = Vec::new();
    path.path_to_progress(&mut out, 0.75, 0.25);
    // Lazer's loops yield the start and end points only.
    assert_eq!(out, vec![v(75.0, 0.0), v(25.0, 0.0)]);
}
