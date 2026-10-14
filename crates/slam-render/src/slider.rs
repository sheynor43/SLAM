//! Slider bodies (ADR-0041): a static mesh of the whole path, built once at load; a
//! distance prepass into an `R16Float` target with [`Blend::Max`](crate::Blend::Max),
//! so overlapping parts of the path do not blend twice; a composite pass that turns
//! the distance into the skin's gradient. Snaking clips the path in the vertex
//! shader from a uniform range, so the mesh never changes.
//!
//! Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Lines/Path.DrawNode.cs,
//! osu.Framework/Resources/Shaders/sh_PathPrepass.fs (the shape of the result; the
//! geometry differs, see [`SliderMesh`]).

use crate::hal::{VertexAttribute, VertexFormat, VertexLayout};

mod render;
pub use render::{SliderBuffer, SliderDraw, SliderError, SliderRenderer};

/// One corner of a segment's quad. All four corners of a segment carry the same
/// segment; the vertex shader clips it to the snaking range and places the corner.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SliderVertex {
    /// Segment start in path coordinates (osu! pixels).
    pub start: [f32; 2],
    /// Segment end.
    pub end: [f32; 2],
    /// Path length at the start and at the end of the segment.
    pub distance: [f32; 2],
    /// `x`: 0 at the start side, 1 at the end side; `y`: −1 or 1 across the path.
    pub corner: [f32; 2],
}

const _: () = assert!(size_of::<SliderVertex>() == 32);

/// Vertex layout of [`SliderVertex`] for `shaders/slider_prepass.wgsl`.
pub const SLIDER_LAYOUT: VertexLayout<'static> = VertexLayout {
    stride: size_of::<SliderVertex>() as u32,
    attributes: &[
        VertexAttribute {
            location: 0,
            format: VertexFormat::Float32x2,
            offset: 0,
        },
        VertexAttribute {
            location: 1,
            format: VertexFormat::Float32x2,
            offset: 8,
        },
        VertexAttribute {
            location: 2,
            format: VertexFormat::Float32x2,
            offset: 16,
        },
        VertexAttribute {
            location: 3,
            format: VertexFormat::Float32x2,
            offset: 24,
        },
    ],
};

/// Corners of a quad in the order of the shared quad indices (0-1-2, 2-3-0).
const CORNERS: [[f32; 2]; 4] = [[0.0, -1.0], [1.0, -1.0], [1.0, 1.0], [0.0, 1.0]];

/// Axis-aligned rectangle, `min` inclusive, `max` inclusive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Bounds {
    /// Grown by `by` on every side.
    pub fn inflate(self, by: f32) -> Self {
        Self {
            min: [self.min[0] - by, self.min[1] - by],
            max: [self.max[0] + by, self.max[1] + by],
        }
    }
}

/// The body of one slider: one quad per segment of the calculated path.
///
/// Unlike osu-framework, segments on one line are not merged (that moves the shape by
/// less than its 0.01 precision), and each quad covers the whole capsule around its
/// segment: the shape comes from the distance computed per fragment, so round caps
/// and joins are exact. osu-framework shortens quads at joins (`startOffset · sin θ`)
/// and can leave a few pixels of the capsule uncovered there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SliderMesh {
    vertices: Vec<SliderVertex>,
    bounds: Option<Bounds>,
    length: f32,
}

impl SliderMesh {
    /// Builds the mesh of a path: `points` relative to the slider (osu! pixels), and
    /// `cumulative` path length at each point (same length). A single point makes a
    /// zero-length segment, drawn as a circle; no points make an empty mesh.
    ///
    /// # Panics
    /// If the slices differ in length.
    pub fn new(points: &[[f32; 2]], cumulative: &[f64]) -> Self {
        assert_eq!(points.len(), cumulative.len(), "one length per point");
        let Some((&first, rest)) = points.split_first() else {
            return Self::default();
        };
        let segments = rest.len().max(1);
        let mut vertices = Vec::with_capacity(segments * 4);
        let mut push = |start: [f32; 2], end: [f32; 2], distance: [f32; 2]| {
            for corner in CORNERS {
                vertices.push(SliderVertex {
                    start,
                    end,
                    distance,
                    corner,
                });
            }
        };
        if rest.is_empty() {
            let d = cumulative[0] as f32;
            push(first, first, [d, d]);
        }
        for i in 1..points.len() {
            push(
                points[i - 1],
                points[i],
                [cumulative[i - 1] as f32, cumulative[i] as f32],
            );
        }
        let mut bounds = Bounds {
            min: first,
            max: first,
        };
        for p in rest {
            bounds.min = [bounds.min[0].min(p[0]), bounds.min[1].min(p[1])];
            bounds.max = [bounds.max[0].max(p[0]), bounds.max[1].max(p[1])];
        }
        Self {
            vertices,
            bounds: Some(bounds),
            length: cumulative[cumulative.len() - 1] as f32,
        }
    }

    /// Four vertices per segment.
    pub fn vertices(&self) -> &[SliderVertex] {
        &self.vertices
    }

    pub fn segments(&self) -> usize {
        self.vertices.len() / 4
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// Bounds of the path's points, without the radius; `None` when empty.
    pub fn bounds(&self) -> Option<Bounds> {
        self.bounds
    }

    /// Path length at the last point.
    pub fn length(&self) -> f32 {
        self.length
    }

    /// The segments the vertex shader draws for the snaking range `d0..=d1` of path
    /// length, in path order: each segment that touches the range, clipped to it. A
    /// CPU mirror of `shaders/slider_prepass.wgsl`, for tests and hit areas.
    pub fn clipped(&self, d0: f32, d1: f32) -> impl Iterator<Item = ([f32; 2], [f32; 2])> + '_ {
        self.vertices
            .as_chunks::<4>()
            .0
            .iter()
            .filter_map(move |quad| clip_segment(&quad[0], d0, d1))
    }
}

/// Clips a segment to the range `d0..=d1`, as `SliderPath.interpolateVertices`; `None`
/// if they do not touch. Must match `clip` in `shaders/slider_prepass.wgsl`.
pub fn clip_segment(v: &SliderVertex, d0: f32, d1: f32) -> Option<([f32; 2], [f32; 2])> {
    let [ds, de] = v.distance;
    if ds > d1 || de < d0 {
        return None;
    }
    let at = |d: f32| {
        let len = de - ds;
        if len <= SAME_DISTANCE {
            return v.start;
        }
        let w = (d.clamp(ds, de) - ds) / len;
        [
            v.start[0] + (v.end[0] - v.start[0]) * w,
            v.start[1] + (v.end[1] - v.start[1]) * w,
        ]
    };
    Some((at(d0), at(d1)))
}

/// Segments shorter than this interpolate to their start, like
/// `Precision.AlmostEquals` in `SliderPath.interpolateVertices` (which compares
/// `f64` lengths with 1e-7; `f32` lengths of long sliders cannot resolve that).
pub const SAME_DISTANCE: f32 = 1e-4;

/// Texels a [`SliderGradient`] holds: a path radius up to 64 osu! pixels, the largest
/// at circle size 0.
pub const MAX_GRADIENT_TEXELS: usize = 128;

/// The colour across a slider body, from the edge (texel 0) to the centre: the
/// texture osu-framework's `SmoothPath` builds, here sent as uniforms (ADR-0041).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliderGradient {
    width: u32,
    /// RGBA bytes, `r` in the low byte.
    texels: [u32; MAX_GRADIENT_TEXELS],
}

impl SliderGradient {
    /// The gradient of a path of `radius` (osu! pixels) whose colour at `position`
    /// (0 at the edge, 1 at the centre) is `colour_at`, as straight-alpha RGBA.
    /// Paths wider than [`MAX_GRADIENT_TEXELS`] are sampled at that many texels.
    // Ported from osu!lazer 2026.1005.0-lazer (osu-framework 2026.921.1):
    // osu.Framework/Graphics/Lines/SmoothPath.cs (validateTexture)
    pub fn new(radius: f32, colour_at: impl Fn(f32) -> [f32; 4]) -> Self {
        let wanted = (radius.max(1.0) as i32).saturating_mul(2);
        let width = (wanted.max(2) as usize).min(MAX_GRADIENT_TEXELS);
        if wanted as usize > MAX_GRADIENT_TEXELS {
            tracing::warn!("slider radius {radius} needs {wanted} gradient texels, using {width}");
        }
        const AA_PORTION: f32 = 0.02;
        let mut texels = [0; MAX_GRADIENT_TEXELS];
        for (i, texel) in texels.iter_mut().enumerate().take(width) {
            let progress = i as f32 / (width - 1) as f32;
            let [r, g, b, a] = colour_at(progress);
            let a = a * (progress / AA_PORTION).min(1.0);
            *texel = u32::from_le_bytes([byte(r), byte(g), byte(b), byte(a)]);
        }
        Self {
            width: width as u32,
            texels,
        }
    }

    /// Texels in use.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// RGBA bytes of texel `i` (transparent past the width).
    pub fn texel(&self, i: usize) -> [u8; 4] {
        self.texels.get(i).copied().unwrap_or(0).to_le_bytes()
    }

    /// The colour the composite shader samples at prepass value `value`: linear
    /// between the two nearest texel centres, clamped at the ends, with zero alpha
    /// where the value is 0 (`sh_Path.fs`). A CPU mirror of `shaders/slider_body.wgsl`.
    pub fn sample(&self, value: f32) -> [f32; 4] {
        let x = value * self.width as f32 - 0.5;
        let i0 = x.floor().clamp(0.0, (self.width - 1) as f32);
        let i1 = (i0 + 1.0).min((self.width - 1) as f32);
        let t = (x - i0).clamp(0.0, 1.0);
        let (a, b) = (self.texel(i0 as usize), self.texel(i1 as usize));
        let mut out = [0.0; 4];
        for c in 0..4 {
            let (a, b) = (f32::from(a[c]) / 255.0, f32::from(b[c]) / 255.0);
            out[c] = a + (b - a) * t;
        }
        if value <= 0.0 {
            out[3] = 0.0;
        }
        out
    }
}

/// A float colour channel to a byte, rounding to nearest, as ImageSharp's `Rgba32`
/// from `Vector4` (scale by 255, add 0.5, clamp, truncate). Unverified: ImageSharp's
/// source is not among the references.
fn byte(x: f32) -> u8 {
    (x * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// The `Body` uniform block shared by `shaders/slider_prepass.wgsl` and
/// `shaders/slider_body.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BodyUniforms {
    /// Path coordinates to clip space, column-major.
    pub projection: [f32; 16],
    /// Radius, snaking range start and end (path length), unused.
    pub path: [f32; 4],
    /// Composite rectangle in path coordinates: min x, min y, max x, max y.
    pub bounds: [f32; 4],
    /// Straight-alpha colour multiplying the body.
    pub tint: [f32; 4],
    /// Gradient width in texels, unused ×3.
    pub gradient: [f32; 4],
    pub texels: [u32; MAX_GRADIENT_TEXELS],
}

const _: () = assert!(size_of::<BodyUniforms>() == 640);

impl BodyUniforms {
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: `repr(C)` of `f32`/`u32` arrays only, 4-byte aligned without
        // padding (size asserted above), so every byte is initialised.
        unsafe { std::slice::from_raw_parts((self as *const Self).cast::<u8>(), size_of::<Self>()) }
    }
}

#[cfg(test)]
mod tests;
