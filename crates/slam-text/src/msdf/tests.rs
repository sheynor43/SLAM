//! Tests of the MSDF generator against values computed by msdfgen v1.13.

// Expected values are printed by msdfgen with all 17 digits.
#![allow(clippy::excessive_precision, clippy::approx_constant)]

use skrifa::instance::{Location, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider, Tag};

use super::coloring::edge_coloring_simple;
use super::error_correction::{corrected_pixels, msdf_error_correction};
use super::generate::{MsdfField, Projection, Range, generate_msdf};
use super::outline::{
    OutlineShapePen, colored_shape, overlapping_msdf, shape_msdf, uncorrected_msdf,
};
use super::scanline::{FillRule, Scanline};
use super::segment::{Curve, EdgeSegment, WHITE};
use super::shape::{Contour, Shape};
use super::solver::{solve_cubic, solve_quadratic};
use super::union::resolve_union;
use super::vector::{Point2, Vector2, median};
use crate::{FontRegistry, UI_FAMILY};

const FIELDS: &str = include_str!("../../tests/data/msdfgen-fields.txt");
const CORRECTED: &str = include_str!("../../tests/data/msdfgen-corrected.txt");
const SIMPLE: &str = include_str!("../../tests/data/msdfgen-simple.txt");

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
        // Both contours the wrong way round, for `orient_contours`.
        "ring_inverted" => vec![
            circle(p(7.0, 7.0), 5.0, false),
            circle(p(7.0, 7.0), 2.5, true),
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

        let field = uncorrected_msdf(
            &colored_shape(reference_shape(name)),
            width,
            height,
            &UNIT,
            Range::symmetric(4.0),
        );
        assert_field_bits(name, &field, &expected);
        checked += 1;
    }
    assert_eq!(checked, 19);
}

fn assert_field_bits(name: &str, field: &MsdfField, expected: &[f32]) {
    assert_eq!(expected.len(), field.pixels.len() * 3);
    for (i, (px, e)) in field.pixels.iter().zip(expected.chunks(3)).enumerate() {
        for c in 0..3 {
            assert!(
                px[c].to_bits() == e[c].to_bits(),
                "{name}: pixel ({}, {}) channel {c}: {} != {}",
                i % field.width,
                i / field.width,
                px[c],
                e[c],
            );
        }
    }
}

/// The whole pipeline with error correction, bit for bit, at a unit and a
/// non-uniform projection.
#[test]
fn corrected_fields_match_msdfgen() {
    let mut checked = 0;
    let mut corrected = 0;
    for line in CORRECTED.lines().filter(|l| l.starts_with("corrected ")) {
        let mut values = line.split(' ').skip(1);
        let name = values.next().unwrap();
        let width: usize = values.next().unwrap().parse().unwrap();
        let height: usize = values.next().unwrap().parse().unwrap();
        let expected: Vec<f32> = values.map(|v| v.parse().unwrap()).collect();
        let (shape_name, projection, range) = match name.strip_suffix("@scaled") {
            Some(base) => (
                base,
                Projection {
                    scale: Vector2::new(1.5, 2.0),
                    translate: Vector2::new(0.5, -0.25),
                },
                Range::symmetric(3.0),
            ),
            None => (name, UNIT, Range::symmetric(4.0)),
        };

        let shape = colored_shape(reference_shape(shape_name));
        let before = uncorrected_msdf(&shape, width, height, &projection, range);
        let mut field = before.clone();
        msdf_error_correction(&mut field, &shape, &projection, range);
        assert_field_bits(name, &field, &expected);
        assert_eq!(
            field,
            overlapping_msdf(
                reference_shape(shape_name),
                width,
                height,
                &projection,
                range
            )
        );
        corrected += corrected_pixels(&before, &field);
        checked += 1;
    }
    assert_eq!(checked, 22);
    // msdfgen corrects 195 texels in these fields.
    assert_eq!(corrected, 195);
}

/// The pipeline after the union, bit for bit: `orient_contours`, normalize,
/// coloring, the field without overlap support and without the scanline
/// pass, error correction.
#[test]
fn simple_fields_match_msdfgen() {
    let mut checked = 0;
    for line in SIMPLE.lines().filter(|l| l.starts_with("simple ")) {
        let mut values = line.split(' ').skip(1);
        let name = values.next().unwrap();
        let width: usize = values.next().unwrap().parse().unwrap();
        let height: usize = values.next().unwrap().parse().unwrap();
        let expected: Vec<f32> = values.map(|v| v.parse().unwrap()).collect();
        let (shape_name, projection, range) = match name.strip_suffix("@scaled") {
            Some(base) => (
                base,
                Projection {
                    scale: Vector2::new(1.5, 2.0),
                    translate: Vector2::new(0.5, -0.25),
                },
                Range::symmetric(3.0),
            ),
            None => (name, UNIT, Range::symmetric(4.0)),
        };

        let mut shape = reference_shape(shape_name);
        shape.orient_contours();
        let shape = colored_shape(shape);
        let mut field = MsdfField::new(width, height);
        generate_msdf(&mut field, &shape, &projection, range, false);
        msdf_error_correction(&mut field, &shape, &projection, range);
        assert_field_bits(name, &field, &expected);
        checked += 1;
    }
    assert_eq!(checked, 17);
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

/// A glyph of a built-in family, laid out like `generate_glyph`.
struct GlyphCase {
    shape: Shape,
    /// The outline flattened independently of the generator.
    polylines: Vec<Vec<Point2>>,
    projection: Projection,
    range: Range,
    width: usize,
    height: usize,
    /// Atlas pixels per font unit.
    s: f64,
}

fn glyph_case(registry: &FontRegistry, family: &str, c: char) -> GlyphCase {
    glyph_case_at(registry, family, 700, c).unwrap()
}

/// `None` if the face has no glyph for `c` or its outline is empty.
fn glyph_case_at(registry: &FontRegistry, family: &str, weight: u16, c: char) -> Option<GlyphCase> {
    const EM_PX: f64 = 32.0;
    const PAD: f64 = 2.0;
    let face = registry
        .resolve(registry.family(family).unwrap(), weight)
        .unwrap();
    let (data, index) = registry.face_data(face.font).unwrap();
    let font = FontRef::from_index(data, index).unwrap();
    let location: Location = font
        .axes()
        .location([(Tag::new(b"wght"), f32::from(weight))]);
    let upem = f64::from(font.metrics(Size::unscaled(), &location).units_per_em);
    let s = EM_PX / upem;

    let glyph = font.outline_glyphs().get(font.charmap().map(c)?)?;
    let settings = || DrawSettings::unhinted(Size::unscaled(), &location);
    let mut pen = OutlineShapePen::default();
    glyph.draw(settings(), &mut pen).unwrap();
    let shape = pen.finish();
    let mut reference = PolylinePen::default();
    glyph.draw(settings(), &mut reference).unwrap();
    reference.flush();

    let [l, b, r, t] = shape.bounds()?;
    let x0 = (l * s).floor() - PAD;
    let y0 = (b * s).floor() - PAD;
    Some(GlyphCase {
        shape,
        polylines: reference.polylines,
        projection: Projection {
            scale: Vector2::new(s, s),
            translate: Vector2::new(-x0 / s, -y0 / s),
        },
        range: Range::symmetric(4.0 / s),
        width: ((r * s).ceil() - x0 + PAD) as usize,
        height: ((t * s).ceil() - y0 + PAD) as usize,
        s,
    })
}

fn check_glyph_sign(registry: &FontRegistry, family: &str, chars: &[char]) {
    for &c in chars {
        let g = glyph_case(registry, family, c);
        let (width, height) = (g.width, g.height);
        let field = shape_msdf(g.shape, width, height, &g.projection, g.range);

        let mut compared = 0;
        let mut filled = 0;
        for y in 0..height {
            for x in 0..width {
                let q = g.projection.unproject(p(x as f64 + 0.5, y as f64 + 0.5));
                if distance_to_polylines(&g.polylines, q) * g.s < 1.0 {
                    continue;
                }
                let fill = winding_number(&g.polylines, q) != 0;
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

/// Bilinear sample of the field median at pixel coordinates `(x, y)`, as the
/// shader samples the atlas.
fn sample_median(field: &MsdfField, x: f64, y: f64) -> f32 {
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
    let at = |dx: f64, dy: f64| {
        let cx = (x0 + dx).clamp(0.0, field.width as f64 - 1.0) as usize;
        let cy = (y0 + dy).clamp(0.0, field.height as f64 - 1.0) as usize;
        field.pixels[cy * field.width + cx]
    };
    let (a, b, c, d) = (at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0), at(1.0, 1.0));
    let ch = |i: usize| {
        let bottom = a[i] + (b[i] - a[i]) * tx;
        let top = c[i] + (d[i] - c[i]) * tx;
        bottom + (top - bottom) * ty
    };
    median(ch(0), ch(1), ch(2))
}

/// Subpixel samples at least a pixel away from the outline whose interpolated
/// sign disagrees with a non-zero fill of the outline.
fn interpolation_artifacts(g: &GlyphCase, field: &MsdfField) -> usize {
    const STEPS: usize = 4;
    let mut artifacts = 0;
    for sy in 0..(g.height - 1) * STEPS {
        for sx in 0..(g.width - 1) * STEPS {
            let x = 0.5 + (sx as f64 + 0.5) / STEPS as f64;
            let y = 0.5 + (sy as f64 + 0.5) / STEPS as f64;
            let q = g.projection.unproject(p(x, y));
            if distance_to_polylines(&g.polylines, q) * g.s < 1.0 {
                continue;
            }
            let fill = winding_number(&g.polylines, q) != 0;
            artifacts += usize::from((sample_median(field, x, y) > 0.5) != fill);
        }
    }
    artifacts
}

/// Error correction removes interpolation artifacts at sharp corners and
/// overlaps without introducing new ones.
#[test]
fn error_correction_reduces_glyph_artifacts() {
    let registry = FontRegistry::with_builtin();
    let (mut total_before, mut total_after, mut corrected) = (0, 0, 0);
    for (family, chars) in [
        (UI_FAMILY, ['A', '4', 'V', 'W', 'M', 'x', '1', 'Ж']),
        ("Exo 2", ['A', '4', 'V', 'W', 'M', 'x', 'k', 'Ж']),
    ] {
        for c in chars {
            let g = glyph_case(&registry, family, c);
            let shape = colored_shape(g.shape.clone());
            let before = uncorrected_msdf(&shape, g.width, g.height, &g.projection, g.range);
            let mut after = before.clone();
            msdf_error_correction(&mut after, &shape, &g.projection, g.range);

            let (b, a) = (
                interpolation_artifacts(&g, &before),
                interpolation_artifacts(&g, &after),
            );
            assert!(a <= b, "{family} '{c}': {b} artifacts before, {a} after");
            total_before += b;
            total_after += a;
            corrected += corrected_pixels(&before, &after);
        }
    }
    assert!(corrected > 0);
    assert!(
        total_after < total_before,
        "{total_before} artifacts before correction, {total_after} after"
    );
}

/// `generate_glyph` emits the field top row first: row `r` of the bitmap is
/// row `height - 1 - r` of the field (row 0 at the bottom). "T" is not
/// vertically symmetric.
#[test]
fn glyph_bitmap_rows_are_top_first() {
    let registry = FontRegistry::with_builtin();
    let face = registry
        .resolve(registry.family(UI_FAMILY).unwrap(), 700)
        .unwrap();
    let (data, index) = registry.face_data(face.font).unwrap();
    let font = FontRef::from_index(data, index).unwrap();
    let glyph_id = font.charmap().map('T').unwrap().to_u32() as u16;
    let bitmap = super::generate_glyph(
        &registry,
        super::GlyphKey {
            font: face.font,
            weight: 700,
            glyph_id,
        },
    )
    .unwrap()
    .unwrap();

    let g = glyph_case(&registry, UI_FAMILY, 'T');
    let (width, height) = (g.width, g.height);
    let field = shape_msdf(g.shape, width, height, &g.projection, g.range);
    assert_eq!(bitmap.image.size.width as usize, width);
    assert_eq!(bitmap.image.size.height as usize, height);
    let expected: Vec<u8> = field
        .pixels
        .chunks_exact(width)
        .rev()
        .flatten()
        .flat_map(|px| {
            let [r, g, b] = px.map(super::to_u8);
            [r, g, b, 255]
        })
        .collect();
    assert_eq!(bitmap.image.pixels, expected);
    assert_ne!(field.pixels[..width], field.pixels[(height - 1) * width..]);
}

// Contour union (ADR-0063)

fn shape_winding(shape: &Shape, line: &mut Scanline, q: Point2) -> i32 {
    shape.scanline(line, q.y);
    line.sum_intersections(q.x)
}

/// Signed area of a shape made of lines.
fn polygon_area(shape: &Shape) -> f64 {
    let mut area = 0.0;
    for edge in shape.contours.iter().flat_map(|c| &c.edges) {
        let (a, b) = (edge.point(0.0), edge.point(1.0));
        area += 0.5 * (a.x * b.y - b.x * a.y);
    }
    area
}

/// The union of `shape`, checked on a grid off the integer coordinates of
/// the test shapes: the same non-zero fill as `shape`, and winding 0 or one
/// common ±1 everywhere (no overlaps left, consistent orientation).
fn checked_union(shape: &Shape, contours: usize) -> Shape {
    const STEPS: usize = 97;
    let union = resolve_union(shape).unwrap();
    assert_eq!(union.contours.len(), contours, "{union:?}");
    let [x0, y0, x1, y1] = shape.bounds().unwrap();
    let mut line = Scanline::default();
    let mut filled_sign = 0;
    for j in 0..=STEPS {
        for i in 0..=STEPS {
            let q = p(
                x0 - 0.5 + (x1 - x0 + 1.0) * (i as f64 + 0.31) / STEPS as f64,
                y0 - 0.5 + (y1 - y0 + 1.0) * (j as f64 + 0.27) / STEPS as f64,
            );
            let (before, after) = (
                shape_winding(shape, &mut line, q),
                shape_winding(&union, &mut line, q),
            );
            assert_eq!(before != 0, after != 0, "fill at {q:?}");
            assert!(after.abs() <= 1, "winding {after} at {q:?}");
            if after != 0 {
                assert!(
                    filled_sign == 0 || filled_sign == after,
                    "orientation at {q:?}"
                );
                filled_sign = after;
            }
        }
    }
    union
}

fn shape_of(contours: Vec<Contour>) -> Shape {
    Shape { contours }
}

/// A circle of eight quadratic curves.
fn quadratic_circle(c: Point2, r: f64) -> Contour {
    let step = std::f64::consts::FRAC_PI_4;
    let control = r / (0.5 * step).cos();
    let at = |radius: f64, angle: f64| p(c.x + radius * angle.cos(), c.y + radius * angle.sin());
    // Ends computed once, so consecutive edges meet exactly.
    let ends: Vec<Point2> = (0..8).map(|i| at(r, i as f64 * step)).collect();
    Contour {
        edges: (0..8)
            .map(|i| {
                let control = at(control, (i as f64 + 0.5) * step);
                EdgeSegment::quadratic(ends[i], control, ends[(i + 1) % 8])
            })
            .collect(),
    }
}

/// Two bars of a plus drawn as one contour: the horizontal bar from (-1, -1)
/// around back to (-1, -1), then the vertical bar from there.
#[test]
fn union_of_a_cross_drawn_as_one_contour() {
    let cross = polygon(&[
        p(-1.0, -1.0),
        p(3.0, -1.0),
        p(3.0, 1.0),
        p(-3.0, 1.0),
        p(-3.0, -1.0),
        p(-1.0, -1.0),
        p(-1.0, -3.0),
        p(1.0, -3.0),
        p(1.0, 3.0),
        p(-1.0, 3.0),
    ]);
    let union = checked_union(&shape_of(vec![cross]), 1);
    assert_eq!(union.edge_count(), 12);
    assert_eq!(polygon_area(&union).abs(), 20.0);
}

/// A bow tie crossing itself at (1, 1): two triangles of opposite
/// orientation, both filled by the non-zero rule.
#[test]
fn union_splits_a_figure_eight() {
    let eight = polygon(&[p(0.0, 0.0), p(2.0, 2.0), p(2.0, 0.0), p(0.0, 2.0)]);
    let union = checked_union(&shape_of(vec![eight]), 2);
    assert!(union.contours.iter().all(|c| c.edges.len() == 3));
    assert_eq!(polygon_area(&union).abs(), 2.0);
}

/// Squares touching at a corner stay two contours.
#[test]
fn union_keeps_squares_touching_at_a_corner_apart() {
    let shape = shape_of(vec![square(0.0, 0.0, 1.0, 1.0), square(1.0, 1.0, 2.0, 2.0)]);
    let union = checked_union(&shape, 2);
    assert!(union.contours.iter().all(|c| c.edges.len() == 4));
}

/// Squares sharing an edge merge; the shared edge disappears.
#[test]
fn union_merges_squares_sharing_an_edge() {
    let shape = shape_of(vec![square(0.0, 0.0, 1.0, 1.0), square(1.0, 0.0, 2.0, 1.0)]);
    let union = checked_union(&shape, 1);
    assert_eq!(union.edge_count(), 6);
    assert_eq!(polygon_area(&union).abs(), 2.0);
}

/// A triangle whose vertex lies on the bottom edge of a square and which
/// crosses the right edge: the bottom edge is split at the vertex.
#[test]
fn union_splits_an_edge_at_a_vertex_on_it() {
    let triangle = polygon(&[p(2.0, 0.0), p(6.0, -2.0), p(6.0, 2.0)]);
    let shape = shape_of(vec![square(0.0, 0.0, 4.0, 4.0), triangle]);
    let union = checked_union(&shape, 1);
    // Square 16, triangle 8, overlap: the triangle (2,0) (4,1) (4,0) of area 1.
    assert!((polygon_area(&union).abs() - 23.0).abs() < 1e-9);
}

/// A hole of opposite orientation stays; one of the same orientation is
/// filled by the non-zero rule and disappears.
#[test]
fn union_keeps_holes_and_drops_same_orientation_insets() {
    let mut hole = square(1.0, 1.0, 3.0, 3.0);
    hole.reverse();
    let union = checked_union(&shape_of(vec![square(0.0, 0.0, 4.0, 4.0), hole]), 2);
    assert_eq!(polygon_area(&union).abs(), 12.0);

    let inset = square(1.0, 1.0, 3.0, 3.0);
    let union = checked_union(&shape_of(vec![square(0.0, 0.0, 4.0, 4.0), inset]), 1);
    assert_eq!(polygon_area(&union).abs(), 16.0);
}

/// A contour drawn twice is kept once (reversed: the union orients outer
/// contours clockwise).
#[test]
fn union_drops_duplicate_contours() {
    let shape = shape_of(vec![quadratic_circle(p(0.0, 0.0), 2.0); 2]);
    let union = checked_union(&shape, 1);
    let mut contour = shape.contours[0].clone();
    contour.reverse();
    assert_eq!(union.contours[0].edges.len(), contour.edges.len());
    assert!(
        contour
            .edges
            .iter()
            .all(|e| union.contours[0].edges.contains(e))
    );
}

/// Curve with curve (quadratic and cubic) and line with curve crossings.
#[test]
fn union_of_overlapping_curves() {
    checked_union(
        &shape_of(vec![
            quadratic_circle(p(0.0, 0.0), 2.0),
            quadratic_circle(p(1.5, 0.5), 2.0),
        ]),
        1,
    );
    checked_union(
        &shape_of(vec![
            circle(p(0.0, 0.0), 2.0, true),
            circle(p(1.5, 0.5), 2.0, true),
        ]),
        1,
    );
    // The reversed circle cancels the corner of the square and stays filled
    // outside it: two contours touching at (0, 2) and (1, 3).
    checked_union(
        &shape_of(vec![
            square(0.0, 0.0, 3.0, 3.0),
            quadratic_circle(p(3.0, 1.5), 1.2),
            circle(p(0.0, 3.0), 1.0, false),
        ]),
        2,
    );
}

/// Collinear edges overlapping in part, with ends inside each other.
#[test]
fn union_of_partly_overlapping_collinear_edges() {
    let shape = shape_of(vec![square(0.0, 0.0, 1.0, 1.0), square(1.0, 0.5, 2.0, 1.5)]);
    let union = checked_union(&shape, 1);
    assert_eq!(polygon_area(&union).abs(), 2.0);
}

/// Neighbouring edges that are collinear within the snap distance but not
/// parallel, as fractional stems of variable fonts.
#[test]
fn union_of_nearly_collinear_edges() {
    let shape = shape_of(vec![
        square(0.0, 0.0, 1.0, 1.0),
        polygon(&[p(1.0, 0.2), p(2.0, 0.2), p(2.0, 0.8), p(1.0 + 1e-9, 0.8)]),
    ]);
    checked_union(&shape, 1);
}

/// A short edge nearly collinear with a much longer one, skewed so that the
/// far end of the long edge leaves the snap distance of the short one.
#[test]
fn union_of_a_short_edge_skewed_along_a_long_one() {
    // Square side 1000; the second contour's left edge runs from (1000, 400)
    // to (1000 + 1e-6, 410): within snap (1e-4) of the square's edge, which
    // is not within snap of the short edge's line far away.
    let shape = shape_of(vec![
        square(0.0, 0.0, 1000.0, 1000.0),
        polygon(&[
            p(1000.0, 400.0),
            p(1010.0, 400.0),
            p(1010.0, 410.0),
            p(1000.0 + 1e-6, 410.0),
        ]),
    ]);
    checked_union(&shape, 1);
}

/// A nearly flat quadratic curve along a line, as a fractional stem of a
/// variable font, overlapped by a square.
#[test]
fn union_of_a_nearly_flat_curve_on_a_line() {
    let flat = EdgeSegment::quadratic(p(1.0, 0.0), p(1.5, 1e-9), p(2.0, 0.0));
    let mut contour = polygon(&[
        p(0.0, 0.0),
        p(1.0, 0.0),
        p(2.0, 0.0),
        p(2.0, 1.0),
        p(0.0, 1.0),
    ]);
    contour.edges[1] = flat;
    let shape = shape_of(vec![contour, square(1.5, -1.0, 3.0, 0.0)]);
    checked_union(&shape, 1);
}

/// A circle touching a square at its vertex on the square's edge.
#[test]
fn union_of_a_curve_touching_a_line() {
    let shape = shape_of(vec![
        quadratic_circle(p(0.0, 0.0), 1.0),
        square(1.0, -1.0, 3.0, 1.0),
    ]);
    checked_union(&shape, 2);
}

/// A quadratic curve with collinear control points continuing a line,
/// overlapped by a square.
#[test]
fn union_of_a_degenerate_curve_on_a_line() {
    let flat = EdgeSegment {
        curve: Curve::Quadratic([p(1.0, 0.0), p(1.5, 0.0), p(2.0, 0.0)]),
        color: WHITE,
    };
    let mut contour = polygon(&[
        p(0.0, 0.0),
        p(1.0, 0.0),
        p(2.0, 0.0),
        p(2.0, 1.0),
        p(0.0, 1.0),
    ]);
    contour.edges[1] = flat;
    let shape = shape_of(vec![contour, square(1.5, -1.0, 3.0, 0.5)]);
    checked_union(&shape, 1);
}

/// Curves running along each other make the union fail; the glyph is then
/// generated by the overlapping-contour pipeline.
#[test]
fn failed_union_falls_back_to_the_overlapping_pipeline() {
    let circle = quadratic_circle(p(7.0, 7.0), 5.0);
    let mut halved = circle.clone();
    let Curve::Quadratic([a, b, c]) = halved.edges[0].curve else {
        unreachable!("quadratic_circle is made of quadratic curves")
    };
    let (ab, bc) = (0.5 * (a + b), 0.5 * (b + c));
    let m = 0.5 * (ab + bc);
    halved.edges.splice(
        0..1,
        [
            EdgeSegment::quadratic(a, ab, m),
            EdgeSegment::quadratic(m, bc, c),
        ],
    );
    let shape = shape_of(vec![circle, halved]);
    assert!(resolve_union(&shape).is_err());
    let range = Range::symmetric(4.0);
    assert_eq!(
        shape_msdf(shape.clone(), 14, 14, &UNIT, range),
        overlapping_msdf(shape, 14, 14, &UNIT, range)
    );
}

/// ASCII and Cyrillic of the built-in families at weights 400 and 700.
fn glyph_set(registry: &FontRegistry) -> Vec<(&'static str, u16, char, GlyphCase)> {
    let chars: Vec<char> = ('\u{21}'..='\u{7e}')
        .chain('А'..='я')
        .chain(['Ё', 'ё'])
        .collect();
    let mut set = Vec::new();
    for family in [UI_FAMILY, "Exo 2"] {
        for weight in [400, 700] {
            for &c in &chars {
                if let Some(g) = glyph_case_at(registry, family, weight, c) {
                    set.push((family, weight, c, g));
                }
            }
        }
    }
    set
}

/// Every glyph of the set resolves without the fallback, and the inside of
/// its field matches a non-zero fill of the outline except within a pixel of
/// the outline.
#[test]
fn glyph_set_unions_and_keeps_the_nonzero_fill() {
    let registry = FontRegistry::with_builtin();
    let set = glyph_set(&registry);
    assert!(set.len() > 600, "{} glyphs", set.len());
    let mut line = Scanline::default();
    let failed: Vec<String> = set
        .iter()
        .filter_map(|(family, weight, c, g)| {
            let union = resolve_union(&g.shape);
            union
                .is_err()
                .then(|| format!("{family} {weight} '{c}': {union:?}"))
        })
        .collect();
    assert!(failed.is_empty(), "{failed:#?}");
    for (family, weight, c, g) in set {
        let (width, height) = (g.width, g.height);
        let field = shape_msdf(g.shape.clone(), width, height, &g.projection, g.range);
        for y in 0..height {
            g.shape
                .scanline(&mut line, g.projection.unproject_y(y as f64 + 0.5));
            for x in 0..width {
                let fill = line.filled(g.projection.unproject_x(x as f64 + 0.5), FillRule::NonZero);
                let px = field.pixels[y * width + x];
                if (median(px[0], px[1], px[2]) > 0.5) == fill {
                    continue;
                }
                let q = g.projection.unproject(p(x as f64 + 0.5, y as f64 + 0.5));
                assert!(
                    distance_to_polylines(&g.polylines, q) * g.s < 1.0,
                    "{family} {weight} '{c}': pixel ({x}, {y})"
                );
            }
        }
    }
}

/// Rendering error of `field` at `scale` screen pixels per atlas pixel: the
/// coverage of a bilinear median sample with the screen-space range against
/// a 4×4 non-zero fill of the outline. Returns the number of screen pixels
/// off by more than 0.5 and the summed absolute difference.
fn render_error(g: &GlyphCase, field: &MsdfField, scale: usize) -> (usize, f64) {
    const SS: usize = 4;
    let (w, h, s) = (g.width * scale, g.height * scale, scale as f64);
    let mut exact = vec![0u8; w * h];
    let mut line = Scanline::default();
    for sy in 0..h {
        for j in 0..SS {
            let y = (sy as f64 + (j as f64 + 0.5) / SS as f64) / s;
            g.shape.scanline(&mut line, g.projection.unproject_y(y));
            for sx in 0..w {
                for i in 0..SS {
                    let x = (sx as f64 + (i as f64 + 0.5) / SS as f64) / s;
                    exact[sy * w + sx] +=
                        u8::from(line.filled(g.projection.unproject_x(x), FillRule::NonZero));
                }
            }
        }
    }
    let (mut wrong, mut sum) = (0, 0.0);
    for sy in 0..h {
        for sx in 0..w {
            let m = sample_median(field, (sx as f64 + 0.5) / s, (sy as f64 + 0.5) / s);
            let distance = (f64::from(m) - 0.5) * 4.0 * s;
            let coverage = (distance + 0.5).clamp(0.0, 1.0);
            let diff = (coverage - f64::from(exact[sy * w + sx]) / (SS * SS) as f64).abs();
            wrong += usize::from(diff > 0.5);
            sum += diff;
        }
    }
    (wrong, sum)
}

/// `(union, overlapping)` rendering errors of a glyph at scales 1 and 4,
/// with the fields quantized to 8 bits like the atlas (a channel without
/// edges of its colour holds `-inf`).
fn union_and_overlapping_errors(g: &GlyphCase) -> [[(usize, f64); 2]; 2] {
    let union = shape_msdf(g.shape.clone(), g.width, g.height, &g.projection, g.range);
    let overlapping = overlapping_msdf(g.shape.clone(), g.width, g.height, &g.projection, g.range);
    [union, overlapping].map(|mut field| {
        for v in field.pixels.iter_mut().flatten() {
            *v = f32::from(super::to_u8(*v)) / 255.0;
        }
        [1, 4].map(|scale| render_error(g, &field, scale))
    })
}

/// With the union the glyph set renders no worse than with the
/// overlapping-contour pipeline (ADR-0062): in screen pixels off by more than
/// 0.5 and in summed error at scales 1 and 4, over the whole set and for each
/// glyph measured in ADR-0063 (weight 700).
#[test]
fn union_renders_no_worse_than_overlapping() {
    const ADR_GLYPHS: [(&str, char); 8] = [
        (UI_FAMILY, '1'),
        (UI_FAMILY, 'x'),
        (UI_FAMILY, 'M'),
        (UI_FAMILY, 'W'),
        (UI_FAMILY, '4'),
        (UI_FAMILY, 'A'),
        ("Exo 2", 'x'),
        ("Exo 2", 'A'),
    ];
    let registry = FontRegistry::with_builtin();
    let mut totals = [[(0, 0.0); 2]; 2];
    let mut checked = 0;
    for (family, weight, c, g) in glyph_set(&registry) {
        let errors = union_and_overlapping_errors(&g);
        for (total, e) in totals.iter_mut().flatten().zip(errors.iter().flatten()) {
            total.0 += e.0;
            total.1 += e.1;
        }
        if weight == 700 && ADR_GLYPHS.contains(&(family, c)) {
            for (u, o) in errors[0].iter().zip(&errors[1]) {
                assert!(
                    u.0 <= o.0,
                    "{family} '{c}': union {:?}, overlapping {:?}",
                    errors[0],
                    errors[1]
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, ADR_GLYPHS.len());
    for (u, o) in totals[0].iter().zip(&totals[1]) {
        assert!(
            u.0 <= o.0 && u.1 <= o.1,
            "union {:?}, overlapping {:?}",
            totals[0],
            totals[1]
        );
    }
}
