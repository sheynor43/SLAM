//! SLAM's own icon set (ADR-0182): original drawings as outline paths on a 24x24 grid
//! (y down), turned into MSDF glyphs like text glyphs so they stay crisp at any size.
//!
//! Every icon is built from a few primitives (circles, rounded rectangles, rounded
//! polygons, strokes and arc bands). Fills are wound one way and holes the other, so the
//! union under the non-zero rule gives the shape: fills may overlap, a hole must lie
//! inside a single fill.

use skrifa::outline::OutlinePen;

/// Side of the drawing grid; one em of an icon glyph.
pub const GRID: f32 = 24.0;

type Pt = [f32; 2];

#[derive(Clone, Copy, Debug)]
enum Seg {
    Line(Pt),
    Quad(Pt, Pt),
    Cubic(Pt, Pt, Pt),
}

impl Seg {
    fn end(self) -> Pt {
        match self {
            Seg::Line(p) | Seg::Quad(_, p) | Seg::Cubic(_, _, p) => p,
        }
    }

    fn points(self) -> [Option<Pt>; 3] {
        match self {
            Seg::Line(p) => [Some(p), None, None],
            Seg::Quad(c, p) => [Some(c), Some(p), None],
            Seg::Cubic(a, b, p) => [Some(a), Some(b), Some(p)],
        }
    }

    fn map(self, f: &impl Fn(Pt) -> Pt) -> Seg {
        match self {
            Seg::Line(p) => Seg::Line(f(p)),
            Seg::Quad(c, p) => Seg::Quad(f(c), f(p)),
            Seg::Cubic(a, b, p) => Seg::Cubic(f(a), f(b), f(p)),
        }
    }
}

/// One closed contour.
#[derive(Clone, Debug)]
struct Contour {
    start: Pt,
    segs: Vec<Seg>,
}

fn on_circle(c: Pt, r: f32, deg: f32) -> Pt {
    let a = deg.to_radians();
    [c[0] + r * a.cos(), c[1] + r * a.sin()]
}

impl Contour {
    fn new(start: Pt) -> Self {
        Self {
            start,
            segs: Vec::new(),
        }
    }

    fn end(&self) -> Pt {
        self.segs.last().map_or(self.start, |s| s.end())
    }

    fn line(&mut self, p: Pt) {
        self.segs.push(Seg::Line(p));
    }

    fn quad(&mut self, c: Pt, p: Pt) {
        self.segs.push(Seg::Quad(c, p));
    }

    fn cubic(&mut self, a: Pt, b: Pt, p: Pt) {
        self.segs.push(Seg::Cubic(a, b, p));
    }

    /// Appends a circular arc of centre `c` and radius `r` from angle `a0` to `a1` (degrees,
    /// 0 to the right, growing clockwise on screen); the pen must be at the start already.
    fn arc(&mut self, c: Pt, r: f32, a0: f32, a1: f32) {
        let pieces = ((a1 - a0).abs() / 90.0).ceil().max(1.0) as u32;
        let step = (a1 - a0) / pieces as f32;
        for i in 0..pieces {
            let t0 = (a0 + step * i as f32).to_radians();
            let t1 = (a0 + step * (i + 1) as f32).to_radians();
            let k = 4.0 / 3.0 * ((t1 - t0) / 4.0).tan() * r;
            let p0 = [c[0] + r * t0.cos(), c[1] + r * t0.sin()];
            let p3 = [c[0] + r * t1.cos(), c[1] + r * t1.sin()];
            self.cubic(
                [p0[0] - k * t0.sin(), p0[1] + k * t0.cos()],
                [p3[0] + k * t1.sin(), p3[1] - k * t1.cos()],
                p3,
            );
        }
    }

    /// Signed area of the flattened contour (shoelace, y down: positive is clockwise on
    /// screen).
    fn area(&self) -> f32 {
        let mut sum = 0.0;
        let mut prev = self.start;
        let mut add = |p: Pt, prev: &mut Pt| {
            sum += prev[0] * p[1] - p[0] * prev[1];
            *prev = p;
        };
        for seg in &self.segs {
            let from = prev;
            match *seg {
                Seg::Line(p) => add(p, &mut prev),
                Seg::Quad(c, p) => {
                    for i in 1..=8 {
                        let t = i as f32 / 8.0;
                        let u = 1.0 - t;
                        let q = [
                            u * u * from[0] + 2.0 * u * t * c[0] + t * t * p[0],
                            u * u * from[1] + 2.0 * u * t * c[1] + t * t * p[1],
                        ];
                        add(q, &mut prev);
                    }
                }
                Seg::Cubic(a, b, p) => {
                    for i in 1..=8 {
                        let t = i as f32 / 8.0;
                        let u = 1.0 - t;
                        let q = [
                            u * u * u * from[0]
                                + 3.0 * u * u * t * a[0]
                                + 3.0 * u * t * t * b[0]
                                + t * t * t * p[0],
                            u * u * u * from[1]
                                + 3.0 * u * u * t * a[1]
                                + 3.0 * u * t * t * b[1]
                                + t * t * t * p[1],
                        ];
                        add(q, &mut prev);
                    }
                }
            }
        }
        let s = self.start;
        sum += prev[0] * s[1] - s[0] * prev[1];
        sum / 2.0
    }

    fn reversed(&self) -> Contour {
        let mut points = vec![self.start];
        points.extend(self.segs.iter().map(|s| s.end()));
        let mut out = Contour::new(self.end());
        for (i, seg) in self.segs.iter().enumerate().rev() {
            let to = points[i];
            out.segs.push(match *seg {
                Seg::Line(_) => Seg::Line(to),
                Seg::Quad(c, _) => Seg::Quad(c, to),
                Seg::Cubic(a, b, _) => Seg::Cubic(b, a, to),
            });
        }
        out
    }

    fn map(&mut self, f: impl Fn(Pt) -> Pt) {
        self.start = f(self.start);
        for seg in &mut self.segs {
            *seg = seg.map(&f);
        }
    }
}

fn rotate(p: Pt, deg: f32, pivot: Pt) -> Pt {
    let (s, c) = deg.to_radians().sin_cos();
    let (dx, dy) = (p[0] - pivot[0], p[1] - pivot[1]);
    [pivot[0] + dx * c - dy * s, pivot[1] + dx * s + dy * c]
}

fn circle(cx: f32, cy: f32, r: f32) -> Contour {
    let mut c = Contour::new([cx + r, cy]);
    c.arc([cx, cy], r, 0.0, 360.0);
    c
}

fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Contour {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut c = Contour::new([x + r, y]);
    c.line([x + w - r, y]);
    c.arc([x + w - r, y + r], r, -90.0, 0.0);
    c.line([x + w, y + h - r]);
    c.arc([x + w - r, y + h - r], r, 0.0, 90.0);
    c.line([x + r, y + h]);
    c.arc([x + r, y + h - r], r, 90.0, 180.0);
    c.line([x, y + r]);
    c.arc([x + r, y + r], r, 180.0, 270.0);
    c
}

/// A polygon with every corner rounded by a quadratic curve that starts `r` (at most
/// half an edge) away from the corner.
fn rpoly(points: &[Pt], r: f32) -> Contour {
    let n = points.len();
    let toward = |from: Pt, to: Pt| {
        let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
        let len = dx.hypot(dy).max(1e-6);
        let d = r.min(len / 2.0);
        [from[0] + dx / len * d, from[1] + dy / len * d]
    };
    let corner = |i: usize| {
        let (prev, v, next) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
        (toward(v, prev), v, toward(v, next))
    };
    let (_, _, b0) = corner(0);
    let mut c = Contour::new(b0);
    for i in 1..=n {
        let (a, v, b) = corner(i % n);
        c.line(a);
        c.quad(v, b);
    }
    c
}

fn capsule(a: Pt, b: Pt, w: f32) -> Contour {
    bar(a, b, w, w / 2.0)
}

/// A rounded bar of width `w` and corner radius `r` along `a` to `b`, reaching `w / 2`
/// past each end.
fn bar(a: Pt, b: Pt, w: f32, r: f32) -> Contour {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    let deg = dy.atan2(dx).to_degrees();
    let mut c = rrect(0.0, -w / 2.0, len + w, w, r);
    c.map(|p| {
        let p = rotate([p[0] - w / 2.0, p[1]], deg, [0.0, 0.0]);
        [p[0] + a[0], p[1] + a[1]]
    });
    c
}

/// A pointed pen: a rounded body from `top` to a tip, `w` wide.
fn pen(top: Pt, tip: Pt, w: f32) -> Contour {
    let (dx, dy) = (tip[0] - top[0], tip[1] - top[1]);
    let len = dx.hypot(dy);
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy * w / 2.0, ux * w / 2.0);
    let neck = [tip[0] - ux * w * 1.1, tip[1] - uy * w * 1.1];
    rpoly(
        &[
            [top[0] + nx, top[1] + ny],
            [neck[0] + nx, neck[1] + ny],
            tip,
            [neck[0] - nx, neck[1] - ny],
            [top[0] - nx, top[1] - ny],
        ],
        0.9,
    )
}

/// The icon being drawn.
#[derive(Default)]
struct Canvas {
    contours: Vec<Contour>,
}

impl Canvas {
    /// Adds a filled contour, wound the way solid shapes are.
    fn fill(&mut self, c: Contour) {
        self.contours
            .push(if c.area() > 0.0 { c.reversed() } else { c });
    }

    /// Adds a hole, which must lie inside one fill.
    fn hole(&mut self, c: Contour) {
        self.contours
            .push(if c.area() > 0.0 { c } else { c.reversed() });
    }

    /// A stroked arc of centre `c`, mid radius `r`, width `w`, round caps.
    fn arc_band(&mut self, c: Pt, r: f32, w: f32, a0: f32, a1: f32) {
        let (ro, ri) = (r + w / 2.0, r - w / 2.0);
        let mut band = Contour::new(on_circle(c, ro, a0));
        band.arc(c, ro, a0, a1);
        band.line(on_circle(c, ri, a1));
        band.arc(c, ri, a1, a0);
        self.fill(band);
        let cap = on_circle(c, r, a0);
        self.fill(circle(cap[0], cap[1], w / 2.0));
        let cap = on_circle(c, r, a1);
        self.fill(circle(cap[0], cap[1], w / 2.0));
    }

    /// Rotates the contours added since the first `from` by `deg` degrees about `pivot`.
    fn rotate_from(&mut self, from: usize, deg: f32, pivot: Pt) {
        for c in &mut self.contours[from..] {
            c.map(|p| rotate(p, deg, pivot));
        }
    }
}

/// Icons of the set. The order is the glyph id of [`crate::GlyphKey::icon`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Icon {
    /// Speaker with sound waves.
    Audio,
    /// Clock.
    Offset,
    /// Keyboard.
    Keys,
    /// Monitor.
    Layout,
    /// Palette.
    Skin,
    /// Gauge.
    Frame,
    /// Target.
    Gameplay,
    /// Folder.
    Library,
    /// Tablet with a pen.
    Tablet,
    /// Linked nodes.
    Feedback,
    /// Cog.
    Gear,
    /// House.
    Home,
    /// Music note.
    Note,
    /// Play triangle.
    Play,
    /// Two bars.
    Pause,
    /// Bar and a triangle pointing left.
    Prev,
    /// Triangle pointing right and a bar.
    Next,
    /// Push pin.
    Pin,
    /// Plus.
    Plus,
    /// Counter-clockwise arrow.
    Reset,
    /// Sparkles.
    Mods,
    /// Die.
    Random,
}

impl Icon {
    /// Every icon, by glyph id.
    pub const ALL: [Icon; 22] = [
        Icon::Audio,
        Icon::Offset,
        Icon::Keys,
        Icon::Layout,
        Icon::Skin,
        Icon::Frame,
        Icon::Gameplay,
        Icon::Library,
        Icon::Tablet,
        Icon::Feedback,
        Icon::Gear,
        Icon::Home,
        Icon::Note,
        Icon::Play,
        Icon::Pause,
        Icon::Prev,
        Icon::Next,
        Icon::Pin,
        Icon::Plus,
        Icon::Reset,
        Icon::Mods,
        Icon::Random,
    ];

    /// The glyph id of this icon.
    pub fn id(self) -> u16 {
        self as u16
    }

    /// The icon with glyph id `id`.
    pub fn from_id(id: u16) -> Option<Icon> {
        Self::ALL.get(usize::from(id)).copied()
    }

    fn canvas(self) -> Canvas {
        let mut c = Canvas::default();
        match self {
            Icon::Audio => audio(&mut c),
            Icon::Offset => clock(&mut c),
            Icon::Keys => keyboard(&mut c),
            Icon::Layout => monitor(&mut c),
            Icon::Skin => palette(&mut c),
            Icon::Frame => gauge(&mut c),
            Icon::Gameplay => target(&mut c),
            Icon::Library => folder(&mut c),
            Icon::Tablet => tablet(&mut c),
            Icon::Feedback => network(&mut c),
            Icon::Gear => gear(&mut c),
            Icon::Home => home(&mut c),
            Icon::Note => note(&mut c),
            Icon::Play => play(&mut c),
            Icon::Pause => pause(&mut c),
            Icon::Prev => skip(&mut c, false),
            Icon::Next => skip(&mut c, true),
            Icon::Pin => pin(&mut c),
            Icon::Plus => plus(&mut c),
            Icon::Reset => reset(&mut c),
            Icon::Mods => sparkles(&mut c),
            Icon::Random => die(&mut c),
        }
        c
    }

    /// Feeds the outline to `pen` in font convention: y up, the baseline at the bottom of
    /// the grid, points on a 1/1024 grid so shared corners coincide exactly.
    pub fn draw(self, pen: &mut impl OutlinePen) {
        let q = |p: Pt| {
            [
                (p[0] * 1024.0).round() / 1024.0,
                ((GRID - p[1]) * 1024.0).round() / 1024.0,
            ]
        };
        for c in self.canvas().contours {
            let s = q(c.start);
            pen.move_to(s[0], s[1]);
            for seg in &c.segs {
                match *seg {
                    Seg::Line(p) => {
                        let p = q(p);
                        pen.line_to(p[0], p[1]);
                    }
                    Seg::Quad(a, p) => {
                        let (a, p) = (q(a), q(p));
                        pen.quad_to(a[0], a[1], p[0], p[1]);
                    }
                    Seg::Cubic(a, b, p) => {
                        let (a, b, p) = (q(a), q(b), q(p));
                        pen.curve_to(a[0], a[1], b[0], b[1], p[0], p[1]);
                    }
                }
            }
            pen.close();
        }
    }

    /// Bounds of every control point of the outline on the grid (y down):
    /// `[min_x, min_y, max_x, max_y]`.
    pub fn bounds(self) -> [f32; 4] {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        let mut grow = |p: Pt| {
            b = [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ];
        };
        for c in self.canvas().contours {
            grow(c.start);
            for seg in &c.segs {
                for p in seg.points().into_iter().flatten() {
                    grow(p);
                }
            }
        }
        b
    }
}

fn audio(c: &mut Canvas) {
    c.fill(rpoly(
        &[
            [2.5, 9.5],
            [6.5, 9.5],
            [11.5, 5.0],
            [11.5, 19.0],
            [6.5, 14.5],
            [2.5, 14.5],
        ],
        1.3,
    ));
    c.arc_band([11.0, 12.0], 5.4, 2.0, -50.0, 50.0);
    c.arc_band([11.0, 12.0], 9.4, 2.0, -50.0, 50.0);
}

fn clock(c: &mut Canvas) {
    c.fill(circle(12.0, 12.0, 10.0));
    c.hole(rpoly(
        &[
            [10.9, 5.8],
            [13.1, 5.8],
            [13.1, 10.9],
            [17.2, 10.9],
            [17.2, 13.1],
            [10.9, 13.1],
        ],
        1.0,
    ));
}

fn keyboard(c: &mut Canvas) {
    c.fill(rrect(1.5, 5.0, 21.0, 14.0, 3.2));
    for i in 0..5 {
        c.hole(rrect(4.4 + 3.65 * i as f32, 7.9, 2.2, 2.2, 0.8));
    }
    for i in 0..4 {
        c.hole(rrect(6.2 + 3.65 * i as f32, 11.3, 2.2, 2.2, 0.8));
    }
    c.hole(rrect(7.0, 14.7, 10.0, 2.0, 0.9));
}

fn monitor(c: &mut Canvas) {
    c.fill(rrect(2.0, 3.5, 20.0, 13.5, 2.6));
    c.hole(rrect(4.5, 6.0, 15.0, 8.5, 1.0));
    c.fill(rrect(10.2, 16.0, 3.6, 3.6, 0.5));
    c.fill(rrect(6.5, 19.0, 11.0, 2.3, 1.15));
}

fn palette(c: &mut Canvas) {
    let centre = [12.0, 12.0];
    let (r, mid, notch): (f32, f32, f32) = (10.0, 48.0, 3.4);
    // Half the angle the notch circle cuts out of the big circle.
    let half = 2.0 * (notch / (2.0 * r)).asin().to_degrees();
    let n_centre = on_circle(centre, r, mid);
    let from = on_circle(centre, r, mid + half);
    let to = on_circle(centre, r, mid - half + 360.0);
    let mut body = Contour::new(from);
    body.arc(centre, r, mid + half, mid - half + 360.0);
    // The notch circle runs from `to` back to `from`, on the side facing the centre.
    let angle = |p: Pt| (p[1] - n_centre[1]).atan2(p[0] - n_centre[0]).to_degrees();
    let (a_to, a_from) = (angle(to), angle(from));
    let toward = mid + 180.0;
    let norm = |a: f32| a.rem_euclid(360.0);
    let sweep_up = norm(a_from - a_to);
    let into = norm(toward - a_to);
    let end = if into < sweep_up {
        a_to + sweep_up
    } else {
        a_to - (360.0 - sweep_up)
    };
    body.arc(n_centre, notch, a_to, end);
    c.fill(body);
    for (x, y) in [(7.2, 10.4), (10.0, 6.6), (14.6, 6.6), (18.0, 10.2)] {
        c.hole(circle(x, y, 1.65));
    }
}

fn gauge(c: &mut Canvas) {
    let centre = [12.0, 12.8];
    c.arc_band(centre, 8.9, 2.4, 135.0, 405.0);
    c.fill(capsule(centre, [16.2, 8.6], 2.2));
    c.fill(circle(centre[0], centre[1], 2.7));
}

fn target(c: &mut Canvas) {
    c.fill(circle(12.0, 12.0, 10.0));
    c.hole(circle(12.0, 12.0, 7.8));
    c.fill(circle(12.0, 12.0, 5.7));
    c.hole(circle(12.0, 12.0, 3.5));
    c.fill(circle(12.0, 12.0, 2.0));
}

fn folder(c: &mut Canvas) {
    c.fill(rpoly(
        &[
            [2.0, 5.5],
            [9.6, 5.5],
            [12.0, 8.2],
            [22.0, 8.2],
            [22.0, 19.5],
            [2.0, 19.5],
        ],
        1.8,
    ));
}

fn network(c: &mut Canvas) {
    let (top, left, right) = ([12.0, 5.6], [5.4, 18.0], [18.6, 18.0]);
    c.fill(capsule(top, left, 1.9));
    c.fill(capsule(top, right, 1.9));
    c.fill(capsule(left, right, 1.9));
    for p in [top, left, right] {
        c.fill(circle(p[0], p[1], 3.3));
    }
}

fn tablet(c: &mut Canvas) {
    c.fill(rrect(1.5, 4.5, 13.5, 16.0, 2.6));
    c.hole(rrect(4.0, 7.0, 8.5, 11.0, 1.2));
    c.fill(pen([20.8, 3.0], [18.0, 15.4], 3.4));
}

fn gear(c: &mut Canvas) {
    c.fill(circle(12.0, 12.0, 8.0));
    for i in 0..8 {
        let a = 45.0 * i as f32;
        let (from, to) = (
            on_circle([12.0, 12.0], 7.0, a),
            on_circle([12.0, 12.0], 8.4, a),
        );
        c.fill(bar(from, to, 3.8, 0.9));
    }
    c.hole(circle(12.0, 12.0, 3.4));
}

fn home(c: &mut Canvas) {
    c.fill(rpoly(&[[12.0, 2.6], [22.6, 12.0], [1.4, 12.0]], 1.4));
    c.fill(rpoly(
        &[[4.4, 10.0], [19.6, 10.0], [19.6, 20.6], [4.4, 20.6]],
        1.6,
    ));
    c.hole(rrect(9.8, 13.4, 4.4, 5.4, 2.2));
}

fn note(c: &mut Canvas) {
    c.fill(circle(7.8, 17.6, 3.9));
    c.fill(rrect(10.6, 3.4, 2.3, 14.4, 1.1));
    let mut flag = Contour::new([12.2, 3.4]);
    flag.cubic([14.2, 6.4], [19.6, 6.6], [19.2, 12.6]);
    flag.cubic([18.2, 10.7], [16.0, 10.0], [12.2, 10.6]);
    c.fill(flag);
}

fn play(c: &mut Canvas) {
    c.fill(rpoly(&[[6.5, 4.0], [20.0, 12.0], [6.5, 20.0]], 2.4));
}

fn pause(c: &mut Canvas) {
    c.fill(rrect(5.5, 4.5, 4.6, 15.0, 1.5));
    c.fill(rrect(13.9, 4.5, 4.6, 15.0, 1.5));
}

fn skip(c: &mut Canvas, next: bool) {
    let x = |v: f32| if next { 24.0 - v } else { v };
    c.fill(rrect(if next { x(7.4) } else { 4.6 }, 5.0, 2.8, 14.0, 1.1));
    c.fill(rpoly(
        &[[x(19.5), 4.8], [x(8.0), 12.0], [x(19.5), 19.2]],
        2.2,
    ));
}

fn pin(c: &mut Canvas) {
    let from = c.contours.len();
    c.fill(rrect(7.0, 2.6, 10.0, 3.2, 1.5));
    c.fill(rpoly(
        &[[8.4, 5.4], [15.6, 5.4], [17.6, 13.4], [6.4, 13.4]],
        1.2,
    ));
    c.fill(capsule([12.0, 13.0], [12.0, 21.0], 1.9));
    c.rotate_from(from, 35.0, [12.0, 12.0]);
}

fn plus(c: &mut Canvas) {
    c.fill(rrect(10.7, 4.5, 2.6, 15.0, 1.3));
    c.fill(rrect(4.5, 10.7, 15.0, 2.6, 1.3));
}

fn reset(c: &mut Canvas) {
    let centre = [12.0, 12.8];
    c.arc_band(centre, 7.6, 2.4, 275.0, 530.0);
    let at = on_circle(centre, 7.6, 275.0);
    // Tangent at the arrow end, against the growing angle: pointing left.
    c.fill(rpoly(
        &[
            [at[0] + 1.0, at[1] - 4.2],
            [at[0] - 4.6, at[1]],
            [at[0] + 1.0, at[1] + 4.2],
        ],
        0.9,
    ));
}

fn star(c: &mut Canvas, centre: Pt, r: f32, k: f32) {
    let tips: Vec<Pt> = (0..4)
        .map(|i| on_circle(centre, r, 90.0 * i as f32 - 90.0))
        .collect();
    let mut s = Contour::new(tips[0]);
    for i in 1..=4 {
        let (a, b) = (tips[i - 1], tips[i % 4]);
        let ctrl = [
            centre[0] + k * ((a[0] + b[0]) / 2.0 - centre[0]),
            centre[1] + k * ((a[1] + b[1]) / 2.0 - centre[1]),
        ];
        s.quad(ctrl, b);
    }
    c.fill(s);
}

fn sparkles(c: &mut Canvas) {
    star(c, [10.2, 13.4], 9.2, 0.3);
    star(c, [19.0, 5.2], 3.8, 0.3);
}

fn die(c: &mut Canvas) {
    c.fill(rrect(3.0, 3.0, 18.0, 18.0, 4.4));
    for (x, y) in [
        (8.2, 8.2),
        (15.8, 8.2),
        (12.0, 12.0),
        (8.2, 15.8),
        (15.8, 15.8),
    ] {
        c.hole(circle(x, y, 1.5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msdf::generate_icon;
    use crate::{GlyphKey, PADDING};

    #[test]
    fn every_icon_has_contours_inside_the_grid() {
        for icon in Icon::ALL {
            assert_eq!(Icon::from_id(icon.id()), Some(icon));
            let canvas = icon.canvas();
            assert!(!canvas.contours.is_empty(), "{icon:?}");
            assert!(canvas.contours.iter().all(|c| !c.segs.is_empty()));
            let [x0, y0, x1, y1] = icon.bounds();
            assert!(
                x0 >= 0.0 && y0 >= 0.0 && x1 <= GRID && y1 <= GRID,
                "{icon:?} {:?}",
                icon.bounds()
            );
            // Big enough to read, and centred within a grid unit and a half.
            assert!(x1 - x0 > 12.0 && y1 - y0 > 12.0, "{icon:?} too small");
            let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            assert!(
                (cx - 12.0).abs() < 1.6 && (cy - 12.0).abs() < 1.6,
                "{icon:?} off centre {cx} {cy}"
            );
        }
    }

    #[test]
    fn every_icon_makes_a_bitmap() {
        for hires in [false, true] {
            for icon in Icon::ALL {
                let key = GlyphKey::icon(icon, hires);
                assert_eq!(key.as_icon(), Some(icon));
                let bitmap = generate_icon(key).unwrap().unwrap();
                let side = key.em_px() as u32 + 2 * PADDING;
                assert!(bitmap.image.size.width <= side && bitmap.image.size.height <= side);
                assert!(bitmap.image.size.width > 8 && bitmap.image.size.height > 8);
            }
        }
    }

    /// Whether the field at grid point `(x, y)` (y down) is inside the shape.
    fn inside(icon: Icon, x: f32, y: f32) -> bool {
        let key = GlyphKey::icon(icon, false);
        let bitmap = generate_icon(key).unwrap().unwrap();
        let s = key.em_px() / GRID;
        let px = (x * s - bitmap.offset[0]).floor() as usize;
        let py = ((y - GRID) * s - bitmap.offset[1]).floor() as usize;
        let w = bitmap.image.size.width as usize;
        let i = (py * w + px) * 4;
        let p = &bitmap.image.pixels;
        let (r, g, b) = (p[i], p[i + 1], p[i + 2]);
        r.max(g).min(r.min(g).max(b)) > 127
    }

    #[test]
    fn solids_are_filled_and_holes_are_empty() {
        // Die: body filled, centre pip a hole, outside empty.
        assert!(inside(Icon::Random, 5.0, 12.0));
        assert!(!inside(Icon::Random, 12.0, 12.0));
        // Target: ring, gap, ring, gap, dot.
        assert!(inside(Icon::Gameplay, 12.0, 2.8));
        assert!(!inside(Icon::Gameplay, 12.0, 5.0));
        assert!(inside(Icon::Gameplay, 12.0, 7.5));
        assert!(!inside(Icon::Gameplay, 12.0, 9.2));
        assert!(inside(Icon::Gameplay, 12.0, 12.0));
        // Clock: face filled, hands cut out.
        assert!(inside(Icon::Offset, 5.0, 12.0));
        assert!(!inside(Icon::Offset, 12.0, 8.0));
    }

    #[test]
    fn icon_keys_do_not_collide_with_text_keys() {
        use crate::test_support::{Rig, key_of};
        let rig = Rig::new(16, 8);
        let text = key_of(&rig.registry, 400, 'A');
        for icon in Icon::ALL {
            for hires in [false, true] {
                let key = GlyphKey::icon(icon, hires);
                assert_ne!(key, text);
                assert!(rig.registry.face_data(key.font).is_none());
                assert_ne!(key, GlyphKey::icon(icon, !hires));
            }
        }
        assert_eq!(text.as_icon(), None);
    }

    /// Writes every icon's field as PGM (median channel) plus its placement under the
    /// directory in `SLAM_ICON_PREVIEW`; a developer's aid for drawing icons.
    #[test]
    fn dump_preview_when_asked() {
        let Ok(dir) = std::env::var("SLAM_ICON_PREVIEW") else {
            return;
        };
        for icon in Icon::ALL {
            let bitmap = generate_icon(GlyphKey::icon(icon, true)).unwrap().unwrap();
            let (w, h) = (bitmap.image.size.width, bitmap.image.size.height);
            let mut out = format!("P5\n{w} {h}\n255\n").into_bytes();
            for px in bitmap
                .image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| &p[..])
            {
                let (r, g, b) = (px[0], px[1], px[2]);
                out.push(r.max(g).min(r.min(g).max(b)));
            }
            std::fs::write(format!("{dir}/{icon:?}.pgm"), out).unwrap();
            std::fs::write(
                format!("{dir}/{icon:?}.txt"),
                format!("{} {}", bitmap.offset[0], bitmap.offset[1]),
            )
            .unwrap();
        }
    }
}
