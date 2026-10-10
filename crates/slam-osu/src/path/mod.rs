//! Slider path geometry: curve approximation and the path trimmed or extended to its expected
//! length.

mod approximator;
mod circular_arc;

use std::cmp::Ordering;

use slam_formats::osu::{PathControlPoint, PathType, Vec2};

pub use circular_arc::CircularArcProperties;

use crate::precision;

/// The evaluated path of a slider (lazer's `SliderPath`).
///
/// Lazer evaluates the path lazily and re-evaluates it on edits; here the path is immutable and
/// evaluated once on construction.
#[derive(Debug, Clone, PartialEq)]
pub struct SliderPath {
    control_points: Vec<PathControlPoint>,
    expected_distance: Option<f64>,
    optimise_catmull: bool,
    calculated_path: Vec<Vec2>,
    cumulative_length: Vec<f64>,
    calculated_length: f64,
    segment_end_distances: Vec<f64>,
}

impl SliderPath {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/SliderPath.cs (calculatePath, calculateLength)
    /// Evaluates a path.
    ///
    /// `expected_distance` is the length the path is trimmed or extended to; `None` keeps the
    /// approximated length. `optimise_catmull` drops Catmull vertices closer than 6 px like
    /// osu!stable's draw-time optimisation; lazer's osu!standard `Slider` enables it, while the
    /// path of the decoder's `ConvertSlider` does not.
    pub fn new(
        control_points: Vec<PathControlPoint>,
        expected_distance: Option<f64>,
        optimise_catmull: bool,
    ) -> SliderPath {
        let mut path = SliderPath {
            control_points,
            expected_distance,
            optimise_catmull,
            calculated_path: Vec::new(),
            cumulative_length: Vec::new(),
            calculated_length: 0.0,
            segment_end_distances: Vec::new(),
        };

        let (segment_ends, optimised_length) = path.calculate_path();
        path.calculate_length(&segment_ends, optimised_length);
        path
    }

    /// The control points, relative to the slider position.
    pub fn control_points(&self) -> &[PathControlPoint] {
        &self.control_points
    }

    /// The length the path was trimmed or extended to, if any.
    pub fn expected_distance(&self) -> Option<f64> {
        self.expected_distance
    }

    /// The length after trimming or extension (`SliderPath.Distance`).
    pub fn distance(&self) -> f64 {
        self.cumulative_length.last().copied().unwrap_or(0.0)
    }

    /// The approximated length before trimming or extension (`SliderPath.CalculatedDistance`).
    pub fn calculated_distance(&self) -> f64 {
        self.calculated_length
    }

    /// Path vertices after trimming or extension (`SliderPath.CalculatedPath`).
    pub fn calculated_path(&self) -> &[Vec2] {
        &self.calculated_path
    }

    /// Distance along the path of each vertex.
    ///
    /// Usually one entry per vertex. When extension is skipped because the last two vertices
    /// coincide, lazer leaves one extra entry equal to the last; it is kept for parity.
    pub fn cumulative_length(&self) -> &[f64] {
        &self.cumulative_length
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/SliderPath.cs (PositionAt)
    /// The position at `progress`, clamped to `[0, 1]` (0 is the start of the path, 1 the end).
    pub fn position_at(&self, progress: f64) -> Vec2 {
        let d = self.progress_to_distance(progress);
        self.interpolate_vertices(self.index_of_distance(d), d)
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/SliderPath.cs (GetPathToProgress)
    /// Replaces the contents of `path` with the vertices between progress `p0` and `p1`, with
    /// interpolated end points.
    ///
    /// Does not allocate when `path` has a capacity of at least
    /// `calculated_path().len() + 2`.
    pub fn path_to_progress(&self, path: &mut Vec<Vec2>, p0: f64, p1: f64) {
        let d0 = self.progress_to_distance(p0);
        let d1 = self.progress_to_distance(p1);

        path.clear();

        let mut i = 0;
        while i < self.calculated_path.len() && self.cumulative_length[i] < d0 {
            i += 1;
        }

        path.push(self.interpolate_vertices(i, d0));

        while i < self.calculated_path.len() && self.cumulative_length[i] <= d1 {
            path.push(self.calculated_path[i]);
            i += 1;
        }

        path.push(self.interpolate_vertices(i, d1));
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/SliderPath.cs (GetSegmentEnds)
    /// Progress at which each control point segment ends. Values above 1 lie beyond a trimmed
    /// path; values below 1 for the last segment mean the path was extended.
    pub fn segment_ends(&self) -> impl Iterator<Item = f64> + '_ {
        let distance = self.distance();
        self.segment_end_distances.iter().map(move |d| d / distance)
    }

    /// Approximates every segment; returns the indices of the segment end vertices and the
    /// length removed by the Catmull optimisation.
    fn calculate_path(&mut self) -> (Vec<usize>, f64) {
        let mut segment_ends = Vec::new();
        let mut optimised_length = 0.0;

        let count = self.control_points.len();
        if count == 0 {
            return (segment_ends, optimised_length);
        }

        let vertices: Vec<Vec2> = self.control_points.iter().map(|p| p.position).collect();
        let mut start = 0;

        for i in 0..count {
            if self.control_points[i].path_type.is_none() && i < count - 1 {
                continue;
            }

            // The current vertex ends the segment.
            let segment_vertices = &vertices[start..=i];
            let segment_type = self.control_points[start]
                .path_type
                .unwrap_or(PathType::Linear);

            // No need to calculate path when there is only 1 vertex.
            if segment_vertices.len() == 1 {
                self.calculated_path.push(segment_vertices[0]);
            } else {
                let sub_path = calculate_sub_path(
                    segment_vertices,
                    segment_type,
                    self.optimise_catmull,
                    &mut optimised_length,
                );

                // Skip the first vertex if it is the same as the last vertex from the previous
                // segment.
                let skip_first = matches!(
                    (self.calculated_path.last(), sub_path.first()),
                    (Some(last), Some(first)) if last == first
                );

                let skip = usize::from(skip_first);
                self.calculated_path.extend_from_slice(&sub_path[skip..]);
            }

            if i > 0 {
                // Remember the index of the segment end. The first vertex is always present.
                segment_ends.push(self.calculated_path.len() - 1);
            }

            // Start the new segment at the current vertex.
            start = i;
        }

        (segment_ends, optimised_length)
    }

    fn calculate_length(&mut self, segment_ends: &[usize], optimised_length: f64) {
        self.calculated_length = optimised_length;
        self.cumulative_length.clear();
        self.cumulative_length.push(0.0);

        for pair in self.calculated_path.windows(2) {
            let diff = pair[1] - pair[0];
            self.calculated_length += f64::from(diff.length());
            self.cumulative_length.push(self.calculated_length);
        }

        // Store the distances of the segment ends now, because after shortening the indices may
        // be out of range.
        self.segment_end_distances = segment_ends
            .iter()
            .map(|&i| self.cumulative_length[i])
            .collect();

        let Some(expected_distance) = self.expected_distance else {
            return;
        };
        if self.calculated_length == expected_distance {
            return;
        }

        let path = &mut self.calculated_path;
        let cumulative = &mut self.cumulative_length;

        // In osu-stable, if the last two path points of a slider are equal, extension is not
        // performed.
        if path.len() >= 2
            && path[path.len() - 1] == path[path.len() - 2]
            && expected_distance > self.calculated_length
        {
            cumulative.push(self.calculated_length);
            return;
        }

        // The last length is always incorrect.
        cumulative.pop();

        // -1 when the path is empty.
        let mut path_end_index = path.len() as isize - 1;

        if self.calculated_length > expected_distance {
            // The path will be shortened further, in which case we should trim any more
            // unnecessary lengths and their associated path segments.
            while cumulative.last().is_some_and(|&l| l >= expected_distance) {
                cumulative.pop();
                path.remove(path_end_index as usize);
                path_end_index -= 1;
            }
        }

        if path_end_index <= 0 {
            // The expected distance is negative or zero.
            cumulative.push(0.0);
            return;
        }

        let end = path_end_index as usize;
        // The direction of the segment to shorten or lengthen.
        let dir = (path[end] - path[end - 1]).normalized();

        // `path_end_index > 0` leaves at least one length.
        let last_length = cumulative[cumulative.len() - 1];
        path[end] = path[end - 1] + dir * (expected_distance - last_length) as f32;
        cumulative.push(expected_distance);
    }

    fn index_of_distance(&self, d: f64) -> usize {
        match dotnet_binary_search(&self.cumulative_length, d) {
            Ok(i) | Err(i) => i,
        }
    }

    fn progress_to_distance(&self, progress: f64) -> f64 {
        // Math.Clamp keeps NaN, like f64::clamp.
        progress.clamp(0.0, 1.0) * self.distance()
    }

    fn interpolate_vertices(&self, i: usize, d: f64) -> Vec2 {
        let path = &self.calculated_path;

        let (Some(&first), Some(&last)) = (path.first(), path.last()) else {
            return Vec2::ZERO;
        };

        if i == 0 {
            return first;
        }
        if i >= path.len() {
            return last;
        }

        let p0 = path[i - 1];
        let p1 = path[i];

        let d0 = self.cumulative_length[i - 1];
        let d1 = self.cumulative_length[i];

        // Avoid division by and almost-zero number in case two points are extremely close to
        // each other.
        if precision::almost_equals_f64(d0, d1) {
            return p0;
        }

        let w = (d - d0) / (d1 - d0);
        p0 + (p1 - p0) * w as f32
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/SliderPath.cs (calculateSubPath)
fn calculate_sub_path(
    sub_control_points: &[Vec2],
    path_type: PathType,
    optimise_catmull: bool,
    optimised_length: &mut f64,
) -> Vec<Vec2> {
    match path_type {
        PathType::Linear => return approximator::linear_to_piecewise_linear(sub_control_points),
        PathType::PerfectCurve => {
            if let Some(path) = perfect_curve_sub_path(sub_control_points) {
                return path;
            }
        }
        PathType::Catmull => {
            let sub_path = approximator::catmull_to_piecewise_linear(sub_control_points);
            if !optimise_catmull {
                return sub_path;
            }
            return optimise_catmull_path(&sub_path, optimised_length);
        }
        PathType::Bezier | PathType::BSpline { .. } => {}
    }

    let degree = match path_type {
        PathType::BSpline { degree } => degree,
        // Point counts of a parsed path fit in i32.
        _ => sub_control_points.len() as i32,
    };
    approximator::bspline_to_piecewise_linear(sub_control_points, degree)
}

/// The circular arc approximation, or `None` when the segment falls back to a bezier curve:
/// not exactly three points, a degenerate triangle, or an arc needing 1000 points or more.
fn perfect_curve_sub_path(sub_control_points: &[Vec2]) -> Option<Vec<Vec2>> {
    if sub_control_points.len() != 3 {
        return None;
    }

    let arc = CircularArcProperties::new(sub_control_points)?;

    // 1000 subpoints requires an arc length of at least ~120 thousand to occur.
    if arc.amount_points() >= 1000 {
        return None;
    }

    let sub_path = approximator::circular_arc_to_piecewise_linear(sub_control_points);

    // If for some reason a circular arc could not be fit to the 3 given points, fall back to
    // a numerically stable bezier approximation.
    (!sub_path.is_empty()).then_some(sub_path)
}

/// Keeps only Catmull vertices at least 6 px apart, plus the last vertex of every knot and of
/// the path, and adds the length removed by this to `optimised_length`.
fn optimise_catmull_path(sub_path: &[Vec2], optimised_length: &mut f64) -> Vec<Vec2> {
    /// `PathApproximator.catmull_detail * 2`: vertices per knot.
    const CATMULL_SEGMENT_LENGTH: usize = approximator::CATMULL_DETAIL as usize * 2;

    let mut optimised_path = Vec::with_capacity(sub_path.len());

    let mut last_start: Option<Vec2> = None;
    let mut length_removed_since_start = 0.0;

    for (i, &point) in sub_path.iter().enumerate() {
        let Some(start) = last_start else {
            optimised_path.push(point);
            last_start = Some(point);
            continue;
        };

        // `last_start` is set by an earlier iteration, so `i > 0`.
        let dist_from_start = f64::from(start.distance(point));
        length_removed_since_start += f64::from(sub_path[i - 1].distance(point));

        // Either 6px from the start, the last vertex at every knot, or the end of the path.
        if dist_from_start > 6.0 || (i + 1) % CATMULL_SEGMENT_LENGTH == 0 || i == sub_path.len() - 1
        {
            optimised_path.push(point);
            *optimised_length += length_removed_since_start - dist_from_start;

            last_start = None;
            length_removed_since_start = 0.0;
        }
    }

    optimised_path
}

/// .NET's `List<double>.BinarySearch`: `Ok` with the index of some equal element, or `Err`
/// with the insertion index. With duplicates, the element found is the one .NET finds.
fn dotnet_binary_search(list: &[f64], value: f64) -> Result<usize, usize> {
    let mut lo: isize = 0;
    // Slice lengths never exceed isize::MAX.
    let mut hi: isize = list.len() as isize - 1;

    while lo <= hi {
        let i = lo + ((hi - lo) >> 1);
        match dotnet_compare(list[i as usize], value) {
            Ordering::Equal => return Ok(i as usize),
            Ordering::Less => lo = i + 1,
            Ordering::Greater => hi = i - 1,
        }
    }

    Err(lo as usize)
}

/// `double.CompareTo`: NaN is smaller than every number and equal to itself; `-0.0 == 0.0`.
fn dotnet_compare(a: f64, b: f64) -> Ordering {
    match a.partial_cmp(&b) {
        Some(order) => order,
        None if a.is_nan() && b.is_nan() => Ordering::Equal,
        None if a.is_nan() => Ordering::Less,
        None => Ordering::Greater,
    }
}

#[cfg(test)]
mod tests;
