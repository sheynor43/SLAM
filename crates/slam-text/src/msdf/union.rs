//! Non-zero union of a shape's contours before field generation (ADR-0063):
//! what msdfgen's `resolveShapeGeometry` does with Skia, without Skia.
//!
//! Edges are split where they cross, pieces with exactly one filled side
//! (non-zero rule of the original shape, by a scanline across the piece) are
//! kept, oriented with the filled side on the right (outer contours clockwise,
//! as TrueType and `orientContours`, so an outline without crossings comes out
//! unchanged) and stitched into closed contours through shared vertices.

use super::segment::{Curve, EdgeSegment, WHITE};
use super::shape::{Contour, Shape};
use super::solver::{solve_cubic, solve_quadratic};
use super::vector::{Point2, Vector2, cross, dot, mix};

/// Crossings accepted per shape before giving up.
const MAX_CROSSINGS: usize = 4096;
/// Subdivision leaves accepted per pair of curves; more means the curves
/// run along each other.
const MAX_PAIR_LEAVES: usize = 64;
/// Subdivision depth limit for a pair of curves.
const MAX_DEPTH: u32 = 48;
/// Distance at which points are the same vertex, relative to the shape size.
const SNAP: f64 = 1e-7;
/// Curve subdivision stops at boxes of this size, relative to the shape size.
const LEAF: f64 = 1e-4;
/// Lines whose direction cross product is below this fraction of the
/// product of their lengths are parallel.
const PARALLEL: f64 = 1e-12;
const NEWTON_STEPS: usize = 16;

/// Why the union failed; the caller falls back to the original outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UnionError {
    #[error("more than {MAX_CROSSINGS} edge crossings")]
    TooManyCrossings,
    #[error("edges run along each other")]
    Coincident,
    #[error("the boundary does not close")]
    Open,
    #[error("an edge piece misses its scanline")]
    Unclassified,
}

/// The non-zero union of `shape`: closed contours without crossings,
/// oriented by [`Shape::orient_contours`].
pub fn resolve_union(shape: &Shape) -> Result<Shape, UnionError> {
    let Some([x0, y0, x1, y1]) = shape.bounds() else {
        return Ok(Shape::default());
    };
    let extent = (x1 - x0).max(y1 - y0).max(1.0);
    let tol = Tolerances {
        snap: SNAP * extent,
        leaf: LEAF * extent,
    };

    let mut edges = Vec::with_capacity(shape.edge_count());
    let mut vertices = Vertices::default();
    // (first vertex, last vertex) of every edge
    let mut ends = Vec::with_capacity(edges.capacity());
    for contour in &shape.contours {
        for edge in &contour.edges {
            let points = edge.control_points();
            let a = vertices.exact(points[0]);
            let b = vertices.exact(points[points.len() - 1]);
            edges.push(*edge);
            ends.push((a, b));
        }
    }

    // Split points of every edge: (parameter, vertex)
    let mut splits: Vec<Vec<(f64, usize)>> = vec![Vec::new(); edges.len()];
    let mut hits = Vec::new();
    let mut crossings = 0;
    for i in 0..edges.len() {
        for j in i + 1..edges.len() {
            hits.clear();
            edge_crossings(&edges[i], &edges[j], &tol, &mut hits)?;
            for &(ti, tj) in &hits {
                let v = vertices.near(edges[i].point(ti), tol.snap);
                for (k, t) in [(i, ti), (j, tj)] {
                    if v != ends[k].0 && v != ends[k].1 {
                        splits[k].push((t, v));
                        crossings += 1;
                    }
                }
                if crossings > MAX_CROSSINGS {
                    return Err(UnionError::TooManyCrossings);
                }
            }
        }
    }

    let mut kept: Vec<Piece> = Vec::new();
    for (source, (&(start, end), points)) in ends.iter().zip(&mut splits).enumerate() {
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Each vertex once, at its first parameter along the edge.
        let mut seen = vec![start, end];
        points.retain(|&(_, v)| {
            let new = !seen.contains(&v);
            seen.push(v);
            new
        });
        let mut rest = edges[source];
        let (mut from, mut t0) = (start, 0.0);
        for &(t, v) in points.iter() {
            if t <= t0 || t >= 1.0 {
                continue;
            }
            let (left, right) = split(&rest, (t - t0) / (1.0 - t0));
            let piece = (left, from, v, t0, t);
            keep_piece(&edges, source, tol.snap, &vertices, piece, &mut kept)?;
            rest = right;
            (from, t0) = (v, t);
        }
        let piece = (rest, from, end, t0, 1.0);
        keep_piece(&edges, source, tol.snap, &vertices, piece, &mut kept)?;
    }

    let mut union = stitch(&kept, vertices.points.len())?;
    union.orient_contours();
    Ok(union)
}

struct Tolerances {
    snap: f64,
    leaf: f64,
}

/// Distinct points of the shape: edge ends and crossings.
#[derive(Default)]
struct Vertices {
    points: Vec<Point2>,
}

impl Vertices {
    /// The vertex at exactly `p`; contour edges meet at exactly equal points.
    fn exact(&mut self, p: Point2) -> usize {
        match self.points.iter().position(|&q| q == p) {
            Some(i) => i,
            None => {
                self.points.push(p);
                self.points.len() - 1
            }
        }
    }

    /// The nearest vertex within `snap` of `p` (the first of equally near
    /// ones), or a new one.
    fn near(&mut self, p: Point2, snap: f64) -> usize {
        let mut nearest: Option<(usize, f64)> = None;
        for (i, &q) in self.points.iter().enumerate() {
            let distance = (q - p).length();
            if distance <= snap && nearest.is_none_or(|(_, d)| distance < d) {
                nearest = Some((i, distance));
            }
        }
        match nearest {
            Some((i, _)) => i,
            None => {
                self.points.push(p);
                self.points.len() - 1
            }
        }
    }
}

/// A piece of an edge between two vertices, filled side on the right.
#[derive(Clone, Copy)]
struct Piece {
    edge: EdgeSegment,
    from: usize,
    to: usize,
}

/// `piece` is `(edge, from, to, t0, t1)`: the part of `edges[source]`
/// between parameters `t0` and `t1`, running from vertex `from` to `to`.
/// Snaps its ends to the vertices and keeps it if exactly one side is filled,
/// reversed if that side is the left one. Duplicates of a kept piece
/// (coincident edges of overlapping contours) are dropped.
fn keep_piece(
    edges: &[EdgeSegment],
    source: usize,
    snap: f64,
    vertices: &Vertices,
    (mut edge, from, to, t0, t1): (EdgeSegment, usize, usize, f64, f64),
    kept: &mut Vec<Piece>,
) -> Result<(), UnionError> {
    set_ends(&mut edge, vertices.points[from], vertices.points[to]);
    if hull_length(&edge) == 0.0 || (from == to && matches!(edge.curve, Curve::Linear(_))) {
        return Ok(());
    }
    let (left, right) = piece_sides(edges, source, 0.5 * (t0 + t1), snap)?;
    if left == right {
        return Ok(());
    }
    let piece = if right {
        Piece { edge, from, to }
    } else {
        edge.reverse();
        Piece {
            edge,
            from: to,
            to: from,
        }
    };
    let duplicate = kept.iter().any(|k| {
        k.from == piece.from && k.to == piece.to && same_points(&k.edge, &piece.edge, snap)
    });
    if !duplicate {
        kept.push(piece);
    }
    Ok(())
}

/// Whether the non-zero fill of `edges` is on the left and on the right of
/// `edges[source]` at parameter `t` (the middle of a piece, taken on the
/// source edge rather than the snapped piece).
///
/// A scanline across the piece (horizontal unless the piece runs more
/// horizontally than vertically) gives the winding on both sides exactly,
/// however thin the regions next to the piece are. Edges crossing the
/// scanline within `snap` of the piece run along it: they count towards the
/// high side only, so a coincident edge of the same direction makes both
/// copies boundary pieces and one of the opposite direction makes neither.
fn piece_sides(
    edges: &[EdgeSegment],
    source: usize,
    t: f64,
    snap: f64,
) -> Result<(bool, bool), UnionError> {
    let middle = edges[source].point(t);
    let direction = edges[source].direction(t);
    if !direction.is_nonzero() {
        return Err(UnionError::Unclassified);
    }
    // Scan along x at y = middle.y, or along y at x = middle.x on the
    // transposed shape (transposing negates windings, filled stays filled).
    let transpose = direction.x.abs() > direction.y.abs();
    let (along, at) = if transpose {
        (middle.y, middle.x)
    } else {
        (middle.x, middle.y)
    };
    let (mut xs, mut dys) = ([0.0; 3], [0; 3]);
    // The crossing of the source edge nearest the middle: (index, distance)
    let mut own: Option<(usize, f64)> = None;
    let n = scan(&edges[source], transpose, at, &mut xs, &mut dys);
    for (k, x) in xs[..n].iter().enumerate() {
        let distance = (x - along).abs();
        if own.is_none_or(|(_, d)| distance < d) {
            own = Some((k, distance));
        }
    }
    let Some((own, distance)) = own else {
        return Err(UnionError::Unclassified);
    };
    let own_dy = dys[own];
    if own_dy == 0 || distance > snap {
        return Err(UnionError::Unclassified);
    }
    let (mut low, mut along_piece) = (0, 0);
    for (i, edge) in edges.iter().enumerate() {
        for k in 0..scan(edge, transpose, at, &mut xs, &mut dys) {
            if i == source && k == own {
                continue;
            }
            if xs[k] < along - snap {
                low += dys[k];
            } else if xs[k] <= along + snap {
                along_piece += dys[k];
            }
        }
    }
    let low_filled = low != 0;
    let high_filled = low + own_dy + along_piece != 0;
    // The left normal (-d.y, d.x) points to the low side when d.y > 0
    // (scan along x) or d.x < 0 (scan along y).
    let left_is_low = if transpose {
        direction.x < 0.0
    } else {
        direction.y > 0.0
    };
    Ok(if left_is_low {
        (low_filled, high_filled)
    } else {
        (high_filled, low_filled)
    })
}

/// Scanline intersections of `edge` with the line y = `at`, or with x = `at`
/// when `transpose` (positions along y, directions along x).
fn scan(
    edge: &EdgeSegment,
    transpose: bool,
    at: f64,
    xs: &mut [f64; 3],
    dys: &mut [i32; 3],
) -> usize {
    if !transpose {
        return edge.scanline_intersections(xs, dys, at);
    }
    let mut transposed = *edge;
    let points: &mut [Point2] = match &mut transposed.curve {
        Curve::Linear(p) => p,
        Curve::Quadratic(p) => p,
        Curve::Cubic(p) => p,
    };
    for p in points {
        *p = Point2::new(p.y, p.x);
    }
    transposed.scanline_intersections(xs, dys, at)
}

fn same_points(a: &EdgeSegment, b: &EdgeSegment, snap: f64) -> bool {
    let (a, b) = (a.control_points(), b.control_points());
    a.len() == b.len() && a.iter().zip(b).all(|(&p, &q)| (p - q).length() <= snap)
}

fn hull_length(edge: &EdgeSegment) -> f64 {
    edge.control_points()
        .windows(2)
        .map(|w| (w[1] - w[0]).length())
        .sum()
}

/// Joins pieces into closed contours. At a vertex with several unused
/// outgoing pieces the one turning furthest right is taken, so shapes that
/// touch at a vertex stay separate contours.
fn stitch(pieces: &[Piece], vertex_count: usize) -> Result<Shape, UnionError> {
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); vertex_count];
    for (i, piece) in pieces.iter().enumerate() {
        outgoing[piece.from].push(i);
    }
    let mut used = vec![false; pieces.len()];
    let mut shape = Shape::default();
    for first in 0..pieces.len() {
        if used[first] {
            continue;
        }
        let mut contour = Contour::default();
        let mut current = first;
        loop {
            used[current] = true;
            contour.edges.push(pieces[current].edge);
            let incoming = pieces[current].edge.direction(1.0);
            let candidates = outgoing[pieces[current].to]
                .iter()
                .copied()
                .filter(|&k| !used[k] || k == first);
            let mut best: Option<(usize, Vector2)> = None;
            for k in candidates {
                let out = pieces[k].edge.direction(0.0);
                if best.is_none_or(|(_, b)| turns_more_right(incoming, out, b)) {
                    best = Some((k, out));
                }
            }
            match best {
                Some((k, _)) if k == first => break,
                Some((k, _)) => current = k,
                None => return Err(UnionError::Open),
            }
        }
        shape.contours.push(contour);
    }
    Ok(shape)
}

/// Whether turning from direction `incoming` into `a` is a sharper right
/// turn than into `b`: the signed turn angle in (-π, π] of `a` is smaller.
/// Exact sign tests instead of `atan2`, so the choice is the same everywhere.
fn turns_more_right(incoming: Vector2, a: Vector2, b: Vector2) -> bool {
    // Order of the angle ranges: right turn, straight on, left turn, back.
    let range = |v: Vector2| {
        let (c, d) = (cross(incoming, v), dot(incoming, v));
        if c < 0.0 {
            0
        } else if c == 0.0 && d > 0.0 {
            1
        } else if c > 0.0 {
            2
        } else {
            3
        }
    };
    let (ra, rb) = (range(a), range(b));
    if ra != rb {
        return ra < rb;
    }
    // Same range: within a right or left turn, `a` comes first if `b` lies
    // counter-clockwise of it.
    (ra == 0 || ra == 2) && cross(a, b) > 0.0
}

/// Splits `edge` at `t` by de Casteljau's algorithm.
fn split(edge: &EdgeSegment, t: f64) -> (EdgeSegment, EdgeSegment) {
    let (a, b) = match edge.curve {
        Curve::Linear([p0, p1]) => {
            let m = mix(p0, p1, t);
            (Curve::Linear([p0, m]), Curve::Linear([m, p1]))
        }
        Curve::Quadratic([p0, p1, p2]) => {
            let (a, b) = (mix(p0, p1, t), mix(p1, p2, t));
            let m = mix(a, b, t);
            (Curve::Quadratic([p0, a, m]), Curve::Quadratic([m, b, p2]))
        }
        Curve::Cubic([p0, p1, p2, p3]) => {
            let (a, b, c) = (mix(p0, p1, t), mix(p1, p2, t), mix(p2, p3, t));
            let (ab, bc) = (mix(a, b, t), mix(b, c, t));
            let m = mix(ab, bc, t);
            (Curve::Cubic([p0, a, ab, m]), Curve::Cubic([m, bc, c, p3]))
        }
    };
    (
        EdgeSegment {
            curve: a,
            color: WHITE,
        },
        EdgeSegment {
            curve: b,
            color: WHITE,
        },
    )
}

fn set_ends(edge: &mut EdgeSegment, a: Point2, b: Point2) {
    match &mut edge.curve {
        Curve::Linear(p) => *p = [a, b],
        Curve::Quadratic(p) => (p[0], p[2]) = (a, b),
        Curve::Cubic(p) => (p[0], p[3]) = (a, b),
    }
}

/// Parameter pairs `(ta, tb)` where edges `a` and `b` meet, including touches
/// and the ends of collinear overlaps. Shared ends are reported too; the caller
/// drops them through vertex identity.
fn edge_crossings(
    a: &EdgeSegment,
    b: &EdgeSegment,
    tol: &Tolerances,
    out: &mut Vec<(f64, f64)>,
) -> Result<(), UnionError> {
    let (ba, bb) = (hull_box(a), hull_box(b));
    if ba[2] < bb[0] - tol.snap
        || bb[2] < ba[0] - tol.snap
        || ba[3] < bb[1] - tol.snap
        || bb[3] < ba[1] - tol.snap
    {
        return Ok(());
    }
    let (pa, pb) = (a.control_points(), b.control_points());
    // Identical edges of overlapping contours: the fill test resolves them.
    if pa.len() == pb.len() && (pa.iter().eq(pb) || pa.iter().eq(pb.iter().rev())) {
        return Ok(());
    }
    match (a.curve, b.curve) {
        (Curve::Linear(la), Curve::Linear(lb)) => line_line(la, lb, tol.snap, out),
        (Curve::Linear(l), _) => line_curve(l, b, tol.snap, |s, t| out.push((s, t)))?,
        (_, Curve::Linear(l)) => line_curve(l, a, tol.snap, |s, t| out.push((t, s)))?,
        _ => curve_curve(a, b, tol, out)?,
    }
    Ok(())
}

/// Box of the control points `[x_min, y_min, x_max, y_max]`.
fn hull_box(edge: &EdgeSegment) -> [f64; 4] {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in edge.control_points() {
        b[0] = b[0].min(p.x);
        b[1] = b[1].min(p.y);
        b[2] = b[2].max(p.x);
        b[3] = b[3].max(p.y);
    }
    b
}

/// Whether `t` lies on a segment of length `length`, with `snap` slack.
fn on_segment(t: f64, length: f64, snap: f64) -> bool {
    let slack = snap / length;
    (-slack..=1.0 + slack).contains(&t)
}

fn line_line(a: [Point2; 2], b: [Point2; 2], snap: f64, out: &mut Vec<(f64, f64)>) {
    let (da, db) = (a[1] - a[0], b[1] - b[0]);
    let (la, lb) = (da.length(), db.length());
    if la == 0.0 || lb == 0.0 {
        return;
    }
    // Ends of either segment within `snap` of the inside of the other: the
    // ends of (nearly) collinear overlaps, whatever the angle and the order
    // of the segments, and vertices touching an edge.
    for (tb, p) in [(0.0, b[0]), (1.0, b[1])] {
        if let Some(s) = inside_segment(a, p, snap) {
            out.push((s, tb));
        }
    }
    for (ta, p) in [(0.0, a[0]), (1.0, a[1])] {
        if let Some(t) = inside_segment(b, p, snap) {
            out.push((ta, t));
        }
    }
    let denom = cross(da, db);
    if denom.abs() <= PARALLEL * la * lb {
        return;
    }
    let ab = b[0] - a[0];
    let s = cross(ab, db) / denom;
    let t = cross(ab, da) / denom;
    if on_segment(s, la, snap) && on_segment(t, lb, snap) {
        out.push((s.clamp(0.0, 1.0), t.clamp(0.0, 1.0)));
    }
}

/// Parameter in (0, 1) of the point of segment `l` nearest `p`, if `p` lies
/// within `snap` of it.
fn inside_segment(l: [Point2; 2], p: Point2, snap: f64) -> Option<f64> {
    let d = l[1] - l[0];
    let length = d.length();
    let s = dot(p - l[0], d) / (length * length);
    (s > 0.0 && s < 1.0 && cross(d, p - l[0]).abs() <= snap * length).then_some(s)
}

/// Calls `hit(s, t)` for the parameters `s` on line `l` and `t` on `curve`
/// where they meet.
fn line_curve(
    l: [Point2; 2],
    curve: &EdgeSegment,
    snap: f64,
    mut hit: impl FnMut(f64, f64),
) -> Result<(), UnionError> {
    let d = l[1] - l[0];
    let length = d.length();
    if length == 0.0 {
        return Ok(());
    }
    // A curve whose control points all lie within `snap` of the line runs
    // along it: like collinear lines, the ends of each inside the other.
    let flat = curve
        .control_points()
        .iter()
        .all(|&p| cross(d, p - l[0]).abs() <= snap * length);
    if flat {
        for t in [0.0, 1.0] {
            let s = dot(curve.point(t) - l[0], d) / (length * length);
            if s > 0.0 && s < 1.0 {
                hit(s, t);
            }
        }
        for (s, end) in [(0.0, l[0]), (1.0, l[1])] {
            let (roots, n) = curve_params_on_line(curve, end, d);
            for &t in &roots[..n] {
                hit(s, t);
            }
        }
        return Ok(());
    }
    // The curve in power form, substituted into the implicit line equation.
    let mut roots = [0.0; 3];
    let n = match curve.curve {
        Curve::Quadratic([p0, p1, p2]) => {
            let a = p0 - 2.0 * p1 + p2;
            let b = 2.0 * (p1 - p0);
            solve_quadratic(&mut roots, cross(d, a), cross(d, b), cross(d, p0 - l[0]))
        }
        Curve::Cubic([p0, p1, p2, p3]) => {
            let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
            let b = 3.0 * p0 - 6.0 * p1 + 3.0 * p2;
            let c = 3.0 * (p1 - p0);
            solve_cubic(
                &mut roots,
                cross(d, a),
                cross(d, b),
                cross(d, c),
                cross(d, p0 - l[0]),
            )
        }
        Curve::Linear(_) => unreachable!("line pairs are handled by line_line"),
    };
    if n < 0 {
        // Only for a curve exactly on the line, which `flat` catches.
        return Err(UnionError::Coincident);
    }
    let curve_length = hull_length(curve);
    for &t in &roots[..n as usize] {
        if !on_segment(t, curve_length, snap) {
            continue;
        }
        let t = t.clamp(0.0, 1.0);
        let s = dot(curve.point(t) - l[0], d) / (length * length);
        if on_segment(s, length, snap) {
            hit(s.clamp(0.0, 1.0), t);
        }
    }
    Ok(())
}

/// Parameters in (0, 1) where `curve`, lying on a line of direction `d`,
/// passes through `p`: roots of `dot(curve(t) - p, d) = 0`, and their count.
fn curve_params_on_line(curve: &EdgeSegment, p: Point2, d: Vector2) -> ([f64; 3], usize) {
    let mut roots = [0.0; 3];
    let n = match curve.curve {
        Curve::Quadratic([p0, p1, p2]) => {
            let a = p0 - 2.0 * p1 + p2;
            let b = 2.0 * (p1 - p0);
            solve_quadratic(&mut roots, dot(d, a), dot(d, b), dot(d, p0 - p))
        }
        Curve::Cubic([p0, p1, p2, p3]) => {
            let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
            let b = 3.0 * p0 - 6.0 * p1 + 3.0 * p2;
            let c = 3.0 * (p1 - p0);
            solve_cubic(&mut roots, dot(d, a), dot(d, b), dot(d, c), dot(d, p0 - p))
        }
        Curve::Linear(_) => 0,
    };
    let mut inside = [0.0; 3];
    let mut count = 0;
    for &t in &roots[..n.max(0) as usize] {
        if t > 0.0 && t < 1.0 {
            inside[count] = t;
            count += 1;
        }
    }
    (inside, count)
}

/// Crossings of two curves: subdivision down to small boxes, then Newton's
/// method on `a(ta) = b(tb)` from every leaf.
fn curve_curve(
    a: &EdgeSegment,
    b: &EdgeSegment,
    tol: &Tolerances,
    out: &mut Vec<(f64, f64)>,
) -> Result<(), UnionError> {
    let mut leaves = Vec::new();
    subdivide(a, [0.0, 1.0], b, [0.0, 1.0], tol, 0, &mut leaves)?;
    for (ta, tb) in leaves {
        let Some((ta, tb)) = refine(a, b, ta, tb, tol.snap) else {
            continue;
        };
        let p = a.point(ta);
        if out
            .iter()
            .all(|&(u, _)| (a.point(u) - p).length() > tol.snap)
        {
            out.push((ta, tb));
        }
    }
    Ok(())
}

fn subdivide(
    a: &EdgeSegment,
    ra: [f64; 2],
    b: &EdgeSegment,
    rb: [f64; 2],
    tol: &Tolerances,
    depth: u32,
    leaves: &mut Vec<(f64, f64)>,
) -> Result<(), UnionError> {
    let (ba, bb) = (hull_box(a), hull_box(b));
    if ba[2] < bb[0] - tol.snap
        || bb[2] < ba[0] - tol.snap
        || ba[3] < bb[1] - tol.snap
        || bb[3] < ba[1] - tol.snap
    {
        return Ok(());
    }
    let size = |b: [f64; 4]| (b[2] - b[0]).max(b[3] - b[1]);
    if (size(ba) <= tol.leaf && size(bb) <= tol.leaf) || depth >= MAX_DEPTH {
        if leaves.len() >= MAX_PAIR_LEAVES {
            return Err(UnionError::Coincident);
        }
        leaves.push((0.5 * (ra[0] + ra[1]), 0.5 * (rb[0] + rb[1])));
        return Ok(());
    }
    let (ma, mb) = (0.5 * (ra[0] + ra[1]), 0.5 * (rb[0] + rb[1]));
    let (a0, a1) = split(a, 0.5);
    let (b0, b1) = split(b, 0.5);
    subdivide(&a0, [ra[0], ma], &b0, [rb[0], mb], tol, depth + 1, leaves)?;
    subdivide(&a0, [ra[0], ma], &b1, [mb, rb[1]], tol, depth + 1, leaves)?;
    subdivide(&a1, [ma, ra[1]], &b0, [rb[0], mb], tol, depth + 1, leaves)?;
    subdivide(&a1, [ma, ra[1]], &b1, [mb, rb[1]], tol, depth + 1, leaves)
}

/// Newton's method on `a(ta) - b(tb) = 0`; `None` if the curves stay
/// further apart than `snap` (the leaf boxes only nearly touched).
fn refine(
    a: &EdgeSegment,
    b: &EdgeSegment,
    mut ta: f64,
    mut tb: f64,
    snap: f64,
) -> Option<(f64, f64)> {
    for _ in 0..NEWTON_STEPS {
        let f: Vector2 = a.point(ta) - b.point(tb);
        if f.length() <= 1e-6 * snap {
            break;
        }
        let (da, db) = (derivative(a, ta), derivative(b, tb));
        let det = cross(db, da);
        if det == 0.0 {
            break;
        }
        // [da, -db] (dta, dtb) = -f by Cramer's rule
        ta = (ta + cross(f, db) / det).clamp(0.0, 1.0);
        tb = (tb - cross(da, f) / det).clamp(0.0, 1.0);
    }
    ((a.point(ta) - b.point(tb)).length() <= snap).then_some((ta, tb))
}

/// First derivative at `t` (`EdgeSegment::direction` is scaled by the degree).
fn derivative(edge: &EdgeSegment, t: f64) -> Vector2 {
    match edge.curve {
        Curve::Linear([p0, p1]) => p1 - p0,
        Curve::Quadratic([p0, p1, p2]) => 2.0 * mix(p1 - p0, p2 - p1, t),
        Curve::Cubic([p0, p1, p2, p3]) => {
            3.0 * mix(mix(p1 - p0, p2 - p1, t), mix(p2 - p1, p3 - p2, t), t)
        }
    }
}
