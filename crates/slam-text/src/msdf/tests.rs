//! Tests of the MSDF generator against values computed by msdfgen v1.13.

// Expected values are printed by msdfgen with all 17 digits.
#![allow(clippy::excessive_precision, clippy::approx_constant)]

use skrifa::instance::{Location, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider, Tag};

use super::coloring::edge_coloring_simple;
use super::generate::{MsdfField, Projection, Range};
use super::outline::{OutlineShapePen, shape_msdf};
use super::segment::EdgeSegment;
use super::shape::{Contour, Shape};
use super::solver::{solve_cubic, solve_quadratic};
use super::vector::{Point2, Vector2, median};
use crate::{FontRegistry, UI_FAMILY};

const FIELDS: &str = include_str!("../../tests/data/msdfgen-fields.txt");

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    let tolerance = 1e-12 * expected.abs().max(1.0);
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual} != {expected}"
    );
}

#[test]
fn solvers_match_msdfgen() {
    #[rustfmt::skip]
    let cubics: [([f64; 4], i32, &[f64]); 7] = [
        ([1.0, -6.0, 11.0, -6.0], 3, &[1.0, 3.0, 2.0]),
        ([2.0, -3.0, -3.0, 2.0], 3, &[-1.0, 1.9999999999999996, 0.49999999999999989]),
        ([1.0, 0.0, 0.0, -8.0], 1, &[2.0]),
        ([1.0, -3.0, 3.0, -1.0], 2, &[1.0, 1.0]),
        ([0.0, 1.0, -3.0, 2.0], 2, &[2.0, 1.0]),
        ([1e-9, 1.0, -3.0, 2.0], 2, &[2.0, 1.0]),
        ([1.0, 2.0, 3.0, 4.0], 1, &[-1.6506291914393882]),
    ];
    for ([a, b, c, d], n, roots) in cubics {
        let mut x = [0.0; 3];
        assert_eq!(solve_cubic(&mut x, a, b, c, d), n, "cubic {a} {b} {c} {d}");
        for (i, &r) in roots.iter().enumerate() {
            assert_close(x[i], r, "cubic root");
        }
    }
    #[rustfmt::skip]
    let quadratics: [([f64; 3], i32, &[f64]); 7] = [
        ([1.0, -3.0, 2.0], 2, &[2.0, 1.0]),
        ([1.0, 2.0, 1.0], 1, &[-1.0]),
        ([1.0, 0.0, 1.0], 0, &[]),
        ([0.0, 2.0, -4.0], 1, &[2.0]),
        ([0.0, 0.0, 0.0], -1, &[]),
        ([0.0, 0.0, 1.0], 0, &[]),
        ([1e-13, 1.0, -1.0], 1, &[1.0]),
    ];
    for ([a, b, c], n, roots) in quadratics {
        let mut x = [0.0; 3];
        assert_eq!(solve_quadratic(&mut x, a, b, c), n, "quadratic {a} {b} {c}");
        for (i, &r) in roots.iter().enumerate() {
            assert_close(x[i], r, "quadratic root");
        }
    }
}

fn test_segments() -> [EdgeSegment; 3] {
    [
        EdgeSegment::linear(p(0.0, 0.0), p(4.0, 1.0)),
        EdgeSegment::quadratic(p(0.0, 0.0), p(2.0, 4.0), p(4.0, 0.0)),
        EdgeSegment::cubic(p(0.0, 0.0), p(1.0, 3.0), p(3.0, -1.0), p(4.0, 2.0)),
    ]
}

#[test]
fn segment_distances_match_msdfgen() {
    let origins = [
        p(2.0, 2.0),
        p(-1.0, -1.0),
        p(5.0, 0.5),
        p(1.5, 0.2),
        p(3.7, 1.9),
        p(2.0, -3.0),
    ];
    // Per segment and origin: distance, dot, param, perpendicular distance, its dot.
    #[rustfmt::skip]
    let expected: [[[f64; 5]; 6]; 3] = [
        [
            [-1.4552137502179978, 0.0, 0.58823529411764708, -1.4552137502179978, 0.0],
            [1.4142135623730951, 0.8574929257125441, -0.29411764705882354, 0.72760687510899891, 0.0],
            [1.1180339887498949, 0.75925660236529657, 1.2058823529411764, 0.72760687510899891, 0.0],
            [0.16977493752543307, 0.0, 0.36470588235294121, 0.16977493752543307, 0.0],
            [-0.94588893764169846, 0.0, 0.98235294117647054, -0.94588893764169846, 0.0],
            [3.3954987505086613, 0.0, 0.29411764705882354, 3.3954987505086613, 0.0],
        ],
        [
            [2.2204460492503131e-16, 0.0, 0.49999999999999994, 2.2204460492503131e-16, 0.0],
            [-1.4142135623730951, 0.94868329805051366, -0.29999999999999999, -0.44721359549995793, 0.0],
            [-1.1180339887498949, 0.0, 1.0, -1.1180339887498949, 0.0],
            [1.2064677745515404, 0.0, 0.12379151766416724, 1.2064677745515404, 0.0],
            [-0.78377319173346138, 0.0, 0.77905738995113705, -0.78377319173346138, 0.0],
            [3.6055512754639891, 0.49613893835683387, -0.40000000000000002, 3.1304951684997055, 0.0],
        ],
        [
            [-0.95197550483888971, 0.0, 0.43907791421032022, -0.95197550483888971, 0.0],
            [-1.4142135623730951, 0.89442719099991574, -0.40000000000000002, -0.63245553203367577, 0.0],
            [1.5469928172408036, 0.0, 0.9072822590542069, 1.5469928172408036, 0.0],
            [0.91559476031058384, 0.0, 0.44911902984921837, 0.91559476031058384, 0.0],
            [-0.24864933670465192, 0.0, 0.97778151979713324, -0.24864933670465192, 0.0],
            [3.6055512754639891, 0.61394061351492046, -0.69999999999999996, 2.8460498941515411, 0.0],
        ],
    ];
    for (segment, expected) in test_segments().iter().zip(&expected) {
        for (&origin, e) in origins.iter().zip(expected) {
            let (distance, param) = segment.signed_distance(origin);
            assert_close(distance.distance, e[0], "distance");
            assert_close(distance.dot, e[1], "dot");
            assert_close(param, e[2], "param");
            let mut perpendicular = distance;
            segment.distance_to_perpendicular_distance(&mut perpendicular, origin, param);
            assert_close(perpendicular.distance, e[3], "perpendicular distance");
            assert_close(perpendicular.dot, e[4], "perpendicular dot");
        }
    }
}

#[test]
fn segment_scanlines_and_bounds_match_msdfgen() {
    let ys = [-0.5, 0.0, 0.5, 1.0, 1.5, 2.0, 3.0];
    #[rustfmt::skip]
    let expected: [[&[(f64, i32)]; 7]; 3] = [
        [&[], &[(0.0, 1)], &[(2.0, 1)], &[], &[], &[], &[]],
        [
            &[], &[(0.0, 1), (4.0, -1)],
            &[(0.26794919243112281, 1), (3.7320508075688772, -1)],
            &[(0.58578643762690485, 1), (3.4142135623730949, -1)],
            &[(1.0, 1), (3.0, -1)], &[], &[],
        ],
        [
            &[], &[(0.0, 1)], &[(0.20706326175593501, 1)],
            &[(0.59717070562576269, 1), (2.0000000000000018, -1), (3.4028292943742362, 1)],
            &[(3.7929367382440633, 1)], &[], &[],
        ],
    ];
    let bounds = [
        [0.0, 0.0, 4.0, 1.0],
        [0.0, 0.0, 4.0, 2.0],
        [0.0, 0.0, 4.0, 2.0],
    ];
    for ((segment, expected), bound) in test_segments().iter().zip(&expected).zip(bounds) {
        for (&y, &e) in ys.iter().zip(expected) {
            let mut x = [0.0; 3];
            let mut dy = [0; 3];
            let n = segment.scanline_intersections(&mut x, &mut dy, y);
            assert_eq!(n, e.len(), "intersection count at y = {y}");
            for (i, &(ex, edy)) in e.iter().enumerate() {
                assert_close(x[i], ex, "intersection x");
                assert_eq!(dy[i], edy);
            }
        }
        let mut b = [1e240, 1e240, -1e240, -1e240];
        segment.bound(&mut b);
        assert_eq!(b, bound);
    }
}

fn polygon(points: &[Point2]) -> Contour {
    let n = points.len();
    Contour {
        edges: (0..n)
            .map(|i| EdgeSegment::linear(points[i], points[(i + 1) % n]))
            .collect(),
    }
}

fn circle(c: Point2, r: f64, ccw: bool) -> Contour {
    let k = 0.5522847498307936 * r;
    let pts = [
        p(c.x + r, c.y),
        p(c.x, c.y + r),
        p(c.x - r, c.y),
        p(c.x, c.y - r),
    ];
    let tangents = [
        Vector2::new(0.0, k),
        Vector2::new(-k, 0.0),
        Vector2::new(0.0, -k),
        Vector2::new(k, 0.0),
    ];
    let mut contour = Contour {
        edges: (0..4)
            .map(|i| {
                let j = (i + 1) % 4;
                EdgeSegment::cubic(pts[i], pts[i] + tangents[i], pts[j] - tangents[j], pts[j])
            })
            .collect(),
    };
    if !ccw {
        contour.reverse();
    }
    contour
}

fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Contour {
    polygon(&[p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)])
}

fn reference_shape(name: &str) -> Shape {
    use EdgeSegment as E;
    let contours = match name {
        "square" => vec![square(2.0, 2.0, 8.0, 8.0)],
        "square_cw" => vec![polygon(&[
            p(2.0, 2.0),
            p(2.0, 8.0),
            p(8.0, 8.0),
            p(8.0, 2.0),
        ])],
        "overlap" => vec![square(2.0, 2.0, 8.0, 8.0), square(5.0, 4.0, 11.0, 10.0)],
        "ring" => vec![
            circle(p(7.0, 7.0), 5.0, true),
            circle(p(7.0, 7.0), 2.5, false),
        ],
        "line_quad" => vec![Contour {
            edges: vec![
                EdgeSegment::linear(p(2.0, 2.0), p(10.0, 2.0)),
                EdgeSegment::quadratic(p(10.0, 2.0), p(10.0, 9.0), p(2.0, 2.0)),
            ],
        }],
        // Sharp convergent corners: deconverge and every branch of the
        // convergent curve ordering.
        "cusp_quad" => vec![edges(vec![
            E::quadratic(p(2.0, 2.0), p(6.0, 5.0), p(10.0, 5.0)),
            E::quadratic(p(10.0, 5.0), p(6.0, 5.0), p(2.0, 8.0)),
            E::linear(p(2.0, 8.0), p(2.0, 2.0)),
        ])],
        "cusp_both_degenerate" => vec![edges(vec![
            E::cubic(p(2.0, 2.0), p(4.0, 6.0), p(10.0, 5.0), p(10.0, 5.0)),
            E::cubic(p(10.0, 5.0), p(10.0, 5.0), p(4.0, 6.0), p(3.0, 3.0)),
            E::linear(p(3.0, 3.0), p(2.0, 2.0)),
        ])],
        "cusp_before_degenerate" => vec![edges(vec![
            E::cubic(p(2.0, 2.0), p(4.0, 6.0), p(10.0, 5.0), p(10.0, 5.0)),
            E::quadratic(p(10.0, 5.0), p(4.0, 6.0), p(3.0, 9.0)),
            E::linear(p(3.0, 9.0), p(2.0, 2.0)),
        ])],
        "cusp_after_degenerate" => vec![edges(vec![
            E::quadratic(p(2.0, 8.0), p(4.0, 6.0), p(10.0, 5.0)),
            E::cubic(p(10.0, 5.0), p(10.0, 5.0), p(4.0, 6.0), p(3.0, 2.0)),
            E::linear(p(3.0, 2.0), p(2.0, 8.0)),
        ])],
        "cusp_line_cubic" => vec![edges(vec![
            E::linear(p(2.0, 5.0), p(11.0, 5.0)),
            E::cubic(p(11.0, 5.0), p(5.0, 5.0), p(6.0, 10.0), p(2.0, 9.0)),
            E::linear(p(2.0, 9.0), p(2.0, 5.0)),
        ])],
        // A single edge is split in thirds and coloured as a teardrop.
        "single_cubic" => vec![edges(vec![E::cubic(
            p(2.0, 2.0),
            p(14.0, 6.0),
            p(6.0, 14.0),
            p(2.0, 2.0),
        )])],
        // Two-edge teardrops: split into six parts, corner at index 0 or 1.
        "lens0" => vec![edges(vec![
            E::quadratic(p(2.0, 2.0), p(12.0, 2.0), p(12.0, 8.0)),
            E::quadratic(p(12.0, 8.0), p(12.0, 14.0), p(2.0, 2.0)),
        ])],
        "lens1" => vec![edges(vec![
            E::quadratic(p(12.0, 8.0), p(12.0, 14.0), p(2.0, 2.0)),
            E::quadratic(p(2.0, 2.0), p(12.0, 2.0), p(12.0, 8.0)),
        ])],
        // Edges on pixel centres: ambiguous signs.
        "half_square" => vec![square(2.5, 2.5, 7.5, 7.5)],
        "inverted_half" => vec![polygon(&[
            p(2.5, 2.5),
            p(2.5, 7.5),
            p(7.5, 7.5),
            p(7.5, 2.5),
        ])],
        // Cubic with three scanline roots, also through its end points.
        "s_cubic" => vec![edges(vec![
            E::cubic(p(1.0, 5.0), p(5.0, 12.0), p(9.0, -2.0), p(13.0, 5.0)),
            E::linear(p(13.0, 5.0), p(13.0, 1.0)),
            E::linear(p(13.0, 1.0), p(1.0, 1.0)),
            E::linear(p(1.0, 1.0), p(1.0, 5.0)),
        ])],
        "s_cubic_half" => vec![edges(vec![
            E::cubic(p(1.0, 5.5), p(5.0, 12.5), p(9.0, -1.5), p(13.0, 5.5)),
            E::linear(p(13.0, 5.5), p(13.0, 1.5)),
            E::linear(p(13.0, 1.5), p(1.0, 1.5)),
            E::linear(p(1.0, 1.5), p(1.0, 5.5)),
        ])],
        "self_overlap_star" => vec![polygon(&[
            p(7.0, 1.0),
            p(10.5, 12.0),
            p(1.5, 5.0),
            p(12.5, 5.0),
            p(3.5, 12.0),
        ])],
        // A quadratic edge lying on a scanline.
        "horizontal_quad_tangent" => {
            let mut bar = square(3.0, 7.5, 11.0, 9.5);
            bar.edges[0] = E::quadratic(p(3.0, 7.5), p(7.0, 7.5), p(11.0, 7.5));
            vec![
                edges(vec![
                    E::quadratic(p(2.0, 2.0), p(7.0, 12.0), p(12.0, 2.0)),
                    E::linear(p(12.0, 2.0), p(2.0, 2.0)),
                ]),
                bar,
            ]
        }
        _ => panic!("unknown reference shape {name}"),
    };
    Shape { contours }
}

fn edges(edges: Vec<EdgeSegment>) -> Contour {
    Contour { edges }
}

/// `<name> | a b c | d e` lines of the reference data.
fn reference_lines(kind: &str) -> impl Iterator<Item = (&'static str, Vec<Vec<f64>>)> + '_ {
    FIELDS
        .lines()
        .filter_map(move |l| l.strip_prefix(kind)?.strip_prefix(' '))
        .map(|l| {
            let mut parts = l.split(" | ");
            let name = parts.next().unwrap();
            let groups = parts
                .map(|c| c.split(' ').map(|v| v.parse().unwrap()).collect())
                .collect();
            (name, groups)
        })
}

const UNIT: Projection = Projection {
    scale: Vector2::new(1.0, 1.0),
    translate: Vector2::ZERO,
};

#[test]
fn edge_colors_match_msdfgen() {
    let mut checked = 0;
    for (name, expected) in reference_lines("colors") {
        let mut shape = reference_shape(name);
        shape.normalize();
        edge_coloring_simple(&mut shape, 3.0, 0);
        let actual: Vec<Vec<f64>> = shape
            .contours
            .iter()
            .map(|c| c.edges.iter().map(|e| f64::from(e.color)).collect())
            .collect();
        assert_eq!(actual, expected, "{name}");
        checked += 1;
    }
    assert_eq!(checked, 19);
}

/// Control points after `normalize` (deconverged corners, split edges).
#[test]
fn normalized_points_match_msdfgen() {
    let mut checked = 0;
    for (name, expected) in reference_lines("points") {
        let mut shape = reference_shape(name);
        shape.normalize();
        edge_coloring_simple(&mut shape, 3.0, 0);
        let actual: Vec<Vec<f64>> = shape
            .contours
            .iter()
            .map(|c| {
                c.edges
                    .iter()
                    .flat_map(|e| {
                        let points = e.control_points();
                        std::iter::once((points.len() - 1) as f64)
                            .chain(points.iter().flat_map(|q| [q.x, q.y]))
                    })
                    .collect()
            })
            .collect();
        assert_eq!(actual, expected, "{name}");
        checked += 1;
    }
    assert_eq!(checked, 14);
}

/// Every channel of every pixel, bit for bit: squares of both orientations,
/// overlapping squares, a ring with a reversed hole and the edge cases of
/// [`reference_shape`].
#[test]
fn reference_fields_match_msdfgen() {
    let mut checked = 0;
    for line in FIELDS.lines().filter(|l| l.starts_with("field ")) {
        let mut values = line.split(' ').skip(1);
        let name = values.next().unwrap();
        let width: usize = values.next().unwrap().parse().unwrap();
        let height: usize = values.next().unwrap().parse().unwrap();
        let expected: Vec<f32> = values.map(|v| v.parse().unwrap()).collect();
        assert_eq!(expected.len(), width * height * 3);

        let field = shape_msdf(
            reference_shape(name),
            width,
            height,
            &UNIT,
            Range::symmetric(4.0),
        );
        for (i, (px, e)) in field.pixels.iter().zip(expected.chunks(3)).enumerate() {
            for c in 0..3 {
                assert!(
                    px[c].to_bits() == e[c].to_bits(),
                    "{name}: pixel ({}, {}) channel {c}: {} != {}",
                    i % width,
                    i / width,
                    px[c],
                    e[c],
                );
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 19);
}

#[test]
fn reversed_contour_gives_the_same_inside() {
    let range = Range::symmetric(4.0);
    let ccw = shape_msdf(reference_shape("square"), 10, 10, &UNIT, range);
    let cw = shape_msdf(reference_shape("square_cw"), 10, 10, &UNIT, range);
    assert_eq!(inside_mask(&ccw), inside_mask(&cw));
    // Centre inside, corner outside.
    assert!(inside_mask(&ccw)[5 * 10 + 5]);
    assert!(!inside_mask(&ccw)[0]);
}

fn inside_mask(field: &MsdfField) -> Vec<bool> {
    field
        .pixels
        .iter()
        .map(|px| median(px[0], px[1], px[2]) > 0.5)
        .collect()
}

/// Flattens an outline into closed polylines, independently of the generator.
#[derive(Default)]
struct PolylinePen {
    polylines: Vec<Vec<Point2>>,
    current: Vec<Point2>,
}

const FLATTEN_STEPS: usize = 64;

impl PolylinePen {
    fn last(&self) -> Point2 {
        *self.current.last().expect("move_to comes first")
    }

    fn flush(&mut self) {
        let polyline = std::mem::take(&mut self.current);
        if polyline.len() > 1 {
            self.polylines.push(polyline);
        }
    }
}

impl OutlinePen for PolylinePen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.flush();
        self.current.push(p(f64::from(x), f64::from(y)));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.current.push(p(f64::from(x), f64::from(y)));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (a, b, c) = (
            self.last(),
            p(f64::from(cx0), f64::from(cy0)),
            p(f64::from(x), f64::from(y)),
        );
        for i in 1..=FLATTEN_STEPS {
            let t = i as f64 / FLATTEN_STEPS as f64;
            let u = 1.0 - t;
            self.current.push(u * u * a + 2.0 * u * t * b + t * t * c);
        }
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (a, b, c, d) = (
            self.last(),
            p(f64::from(cx0), f64::from(cy0)),
            p(f64::from(cx1), f64::from(cy1)),
            p(f64::from(x), f64::from(y)),
        );
        for i in 1..=FLATTEN_STEPS {
            let t = i as f64 / FLATTEN_STEPS as f64;
            let u = 1.0 - t;
            self.current
                .push(u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d);
        }
    }

    fn close(&mut self) {
        self.flush();
    }
}

/// Non-zero winding number of closed polylines around `q`.
fn winding_number(polylines: &[Vec<Point2>], q: Point2) -> i32 {
    let mut winding = 0;
    for polyline in polylines {
        for i in 0..polyline.len() {
            let (a, b) = (polyline[i], polyline[(i + 1) % polyline.len()]);
            if (a.y <= q.y) != (b.y <= q.y) {
                let x = a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x);
                if x > q.x {
                    winding += if b.y > a.y { 1 } else { -1 };
                }
            }
        }
    }
    winding
}

fn distance_to_polylines(polylines: &[Vec<Point2>], q: Point2) -> f64 {
    let mut best = f64::MAX;
    for polyline in polylines {
        for i in 0..polyline.len() {
            let (a, b) = (polyline[i], polyline[(i + 1) % polyline.len()]);
            let ab = b - a;
            let len2 = ab.x * ab.x + ab.y * ab.y;
            let t = if len2 > 0.0 {
                (((q - a).x * ab.x + (q - a).y * ab.y) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            best = best.min((q - (a + t * ab)).length());
        }
    }
    best
}

/// The inside of the field of glyphs with overlaps matches a non-zero
/// rasterization of the outline everywhere except a one-pixel band around the
/// outline. Nunito Sans "1" and "x" are single self-intersecting contours,
/// Exo 2 "x" and "k" are separate overlapping contours.
#[test]
fn overlapping_glyph_sign_matches_nonzero_fill() {
    let registry = FontRegistry::with_builtin();
    for (family, chars) in [(UI_FAMILY, ['1', 'x']), ("Exo 2", ['x', 'k'])] {
        check_glyph_sign(&registry, family, &chars);
    }
}

fn check_glyph_sign(registry: &FontRegistry, family: &str, chars: &[char]) {
    const EM_PX: f64 = 32.0;
    const PAD: f64 = 2.0;
    let face = registry
        .resolve(registry.family(family).unwrap(), 700)
        .unwrap();
    let (data, index) = registry.face_data(face.font).unwrap();
    let font = FontRef::from_index(data, index).unwrap();
    let location: Location = font.axes().location([(Tag::new(b"wght"), 700.0)]);
    let upem = f64::from(font.metrics(Size::unscaled(), &location).units_per_em);
    let s = EM_PX / upem;

    for &c in chars {
        let glyph = font
            .outline_glyphs()
            .get(font.charmap().map(c).unwrap())
            .unwrap();
        let settings = || DrawSettings::unhinted(Size::unscaled(), &location);
        let mut pen = OutlineShapePen::default();
        glyph.draw(settings(), &mut pen).unwrap();
        let shape = pen.finish();
        let mut reference = PolylinePen::default();
        glyph.draw(settings(), &mut reference).unwrap();
        reference.flush();

        let [l, b, r, t] = shape.bounds().unwrap();
        let x0 = (l * s).floor() - PAD;
        let y0 = (b * s).floor() - PAD;
        let width = ((r * s).ceil() - x0 + PAD) as usize;
        let height = ((t * s).ceil() - y0 + PAD) as usize;
        let projection = Projection {
            scale: Vector2::new(s, s),
            translate: Vector2::new(-x0 / s, -y0 / s),
        };
        let field = shape_msdf(shape, width, height, &projection, Range::symmetric(4.0 / s));

        let mut compared = 0;
        let mut filled = 0;
        for y in 0..height {
            for x in 0..width {
                let q = projection.unproject(p(x as f64 + 0.5, y as f64 + 0.5));
                if distance_to_polylines(&reference.polylines, q) * s < 1.0 {
                    continue;
                }
                let fill = winding_number(&reference.polylines, q) != 0;
                let px = field.pixels[y * width + x];
                let inside = median(px[0], px[1], px[2]) > 0.5;
                assert_eq!(inside, fill, "{family} '{c}': pixel ({x}, {y})");
                compared += 1;
                filled += usize::from(fill);
            }
        }
        assert!(
            compared > width * height / 2,
            "{family} '{c}': too few pixels compared"
        );
        assert!(filled > 20, "{family} '{c}': glyph should cover pixels");
    }
}
