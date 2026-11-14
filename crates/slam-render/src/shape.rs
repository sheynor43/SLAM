//! Interface shapes (ADR-0116): rounded, optionally skewed rectangles, filled or
//! outlined, tinting a texture region, with a sweeping sheen or a pulsing glow (ADR-0123), drawn
//! by `shaders/ui_shape.wgsl` through a [`SpriteRenderer`](crate::sprite::SpriteRenderer)
//! made with [`SHAPE_LAYOUT`]. A [`ShapeBatch`] also takes plain [`Sprite`]s (square
//! corners, no effects), so a screen draws everything but its text into one batch, in
//! order.

use std::mem::size_of;

use crate::atlas::UvRect;
use crate::hal::{Blend, Color, VertexAttribute, VertexFormat, VertexLayout};
use crate::sprite::{Batch, PushSprite, QuadSource, Sprite, extend_batches};
use crate::textures::TextureId;

/// One corner of a shape, as `shaders/ui_shape.wgsl` reads it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeVertex {
    /// Pixels of the target, origin top left, y down.
    pub position: [f32; 2],
    /// sRGB-encoded, straight alpha (ADR-0020).
    pub colour: [f32; 4],
    pub uv: [f32; 2],
    /// Pixels from the shape's centre before the skew.
    pub local: [f32; 2],
    /// Half the shape's width and height in pixels.
    pub half_size: [f32; 2],
    /// Corner radius; outline width (0: filled; negative: glow width); sheen strength or
    /// glow pulse depth; phase in cycles.
    pub params: [f32; 4],
}

const _: () = assert!(size_of::<ShapeVertex>() == 64);

pub const SHAPE_ATTRIBUTES: [VertexAttribute; 6] = [
    VertexAttribute {
        location: 0,
        format: VertexFormat::Float32x2,
        offset: 0,
    },
    VertexAttribute {
        location: 1,
        format: VertexFormat::Float32x4,
        offset: 8,
    },
    VertexAttribute {
        location: 2,
        format: VertexFormat::Float32x2,
        offset: 24,
    },
    VertexAttribute {
        location: 3,
        format: VertexFormat::Float32x2,
        offset: 32,
    },
    VertexAttribute {
        location: 4,
        format: VertexFormat::Float32x2,
        offset: 40,
    },
    VertexAttribute {
        location: 5,
        format: VertexFormat::Float32x4,
        offset: 48,
    },
];

pub const SHAPE_LAYOUT: VertexLayout<'static> = VertexLayout {
    stride: size_of::<ShapeVertex>() as u32,
    attributes: &SHAPE_ATTRIBUTES,
};

/// Pixels the quad reaches past the shape on every side, so the shader can smooth the
/// edge outside it.
pub const EDGE_MARGIN: f32 = 1.0;

/// A rounded rectangle, sheared along x by `skew`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// Top-left corner of the unskewed rectangle, in target pixels.
    pub position: [f32; 2],
    pub size: [f32; 2],
    /// Corner radius in pixels, up to half the smaller side.
    pub radius: f32,
    /// Shear in radians: positive leans the top to the right (`/`), about the centre
    /// line, so the shape's centre stays put.
    pub skew: f32,
    /// Width of the outline in pixels, inside the edge; 0 fills.
    pub outline: f32,
    /// Multiplies the texture; sRGB-encoded, straight alpha.
    pub colour: Color,
    pub texture: TextureId,
    pub uv: UvRect,
    pub blend: Blend,
    /// A fill's sheen strength, or a glow's pulse depth, 0 to 1 (0: none).
    pub sheen: f32,
    /// Where the sheen's sweep or the glow's pulse is, in cycles; the batch's drift is
    /// added (ADR-0123).
    pub phase: f32,
    /// Width in pixels of a soft glow outside the edge: then the shape is only that glow,
    /// pulsing by `sheen` (0: none).
    pub glow: f32,
}

impl Shape {
    /// A filled, unskewed, square-cornered shape tinting `uv` of `texture` (the white
    /// texel for a plain fill) with `colour`, without effects.
    pub const fn new(
        texture: TextureId,
        uv: UvRect,
        position: [f32; 2],
        size: [f32; 2],
        colour: Color,
    ) -> Self {
        Self {
            position,
            size,
            radius: 0.0,
            skew: 0.0,
            outline: 0.0,
            colour,
            texture,
            uv,
            blend: Blend::Alpha,
            sheen: 0.0,
            phase: 0.0,
            glow: 0.0,
        }
    }

    /// Horizontal shift of the top edge from the bottom edge.
    pub fn skew_offset(&self) -> f32 {
        self.size[1] * self.skew.tan()
    }

    /// Whether the target point `p` is inside the shape (square corners, sheared): what
    /// a mouse hit tests.
    pub fn contains(&self, p: [f32; 2]) -> bool {
        let [x, y] = self.position;
        let [w, h] = self.size;
        if p[1] < y || p[1] > y + h {
            return false;
        }
        let shift = (y + h * 0.5 - p[1]) * self.skew.tan();
        let px = p[0] - shift;
        px >= x && px <= x + w
    }
}

/// A frame's interface quads in draw order: shapes and plain sprites. Reuse it across
/// frames with [`ShapeBatch::clear`].
#[derive(Debug, Default)]
pub struct ShapeBatch {
    vertices: Vec<ShapeVertex>,
    batches: Vec<Batch>,
    /// Added to every shape's phase: the cycles the frame is at.
    drift: f32,
}

impl ShapeBatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Room for `quads` quads before the first frame grows the buffers.
    pub fn with_capacity(quads: usize) -> Self {
        Self {
            vertices: Vec::with_capacity(quads * 4),
            batches: Vec::with_capacity(quads),
            drift: 0.0,
        }
    }

    /// Sets the cycles added to the phase of the shapes recorded from now on: the frame
    /// sets it from the interface time, so screens need no clock. Kept by
    /// [`ShapeBatch::clear`].
    pub fn set_drift(&mut self, cycles: f32) {
        // Wrapped, so that a long session keeps the shader's precision.
        self.drift = cycles.rem_euclid(1024.0);
    }

    /// Forgets the quads, keeping the memory.
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.batches.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// Number of quads recorded.
    pub fn len(&self) -> usize {
        self.vertices.len() / 4
    }

    /// Corners, four per quad: top left, top right, bottom right, bottom left.
    pub fn vertices(&self) -> &[ShapeVertex] {
        &self.vertices
    }

    pub fn batches(&self) -> &[Batch] {
        &self.batches
    }

    /// Scales the quads from the `first`-th on by `scale` about `pivot`: a layer shrunk or
    /// grown after it was recorded. The shape's size, radius, outline and glow follow, so
    /// it stays the same shape; the skew angle and the colours are kept.
    pub fn scale_from(&mut self, first: usize, pivot: [f32; 2], scale: f32) {
        if scale == 1.0 {
            return;
        }
        for v in self.vertices.iter_mut().skip(first * 4) {
            v.position = [
                pivot[0] + (v.position[0] - pivot[0]) * scale,
                pivot[1] + (v.position[1] - pivot[1]) * scale,
            ];
            v.local = [v.local[0] * scale, v.local[1] * scale];
            v.half_size = [v.half_size[0] * scale, v.half_size[1] * scale];
            // Radius and edge width (outline or glow) are lengths.
            v.params[0] *= scale;
            v.params[1] *= scale;
        }
    }

    /// Records `shape` on top of what was recorded before. Empty shapes are skipped.
    pub fn push_shape(&mut self, shape: &Shape) {
        let [w, h] = shape.size;
        if !(w > 0.0 && h > 0.0) {
            return;
        }
        self.extend(shape.texture, shape.blend, 1);
        let half = [w * 0.5, h * 0.5];
        let centre = [shape.position[0] + half[0], shape.position[1] + half[1]];
        let radius = shape.radius.clamp(0.0, half[0].min(half[1]));
        let glow = shape.glow.max(0.0);
        let edge = if glow > 0.0 {
            -glow
        } else {
            shape.outline.max(0.0)
        };
        let Color { r, g, b, a } = shape.colour;
        let tan = shape.skew.tan();
        let UvRect { u0, v0, u1, v1 } = shape.uv;
        // The quad reaches EDGE_MARGIN (and the glow) past the shape; the texture
        // coordinates go on at the same rate, so the shape's own edges still take the
        // region's edges.
        let margin = EDGE_MARGIN + glow;
        let (mx, my) = (margin / w, margin / h);
        let params = [
            radius,
            edge,
            shape.sheen.clamp(0.0, 1.0),
            shape.phase + self.drift,
        ];
        for (fx, fy) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let tx = if fx == 0.0 { -mx } else { 1.0 + mx };
            let ty = if fy == 0.0 { -my } else { 1.0 + my };
            let local = [(tx - 0.5) * w, (ty - 0.5) * h];
            self.vertices.push(ShapeVertex {
                position: [centre[0] + local[0] - local[1] * tan, centre[1] + local[1]],
                colour: [r, g, b, a],
                uv: [u0 + (u1 - u0) * tx, v0 + (v1 - v0) * ty],
                local,
                half_size: half,
                params,
            });
        }
    }

    /// Records `sprite` as a plain quad: square corners, no effects, its own rotation and
    /// texture coordinates, like [`SpriteBatch::push`](crate::sprite::SpriteBatch::push).
    pub fn push(&mut self, sprite: &Sprite) {
        self.extend(sprite.texture, sprite.blend, 1);
        let [w, h] = sprite.size;
        let half = [w.abs() * 0.5, h.abs() * 0.5];
        let (sin, cos) = sprite.rotation.sin_cos();
        let Color { r, g, b, a } = sprite.colour;
        let UvRect { u0, v0, u1, v1 } = sprite.uv;
        let [ox, oy] = sprite.origin;
        let [px, py] = sprite.position;
        for (fx, fy) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let (x, y) = (w * fx - ox, h * fy - oy);
            // Inside the shape everywhere: the rounded-box distance stays negative, so
            // coverage is 1 and the quad draws like a sprite.
            self.vertices.push(ShapeVertex {
                position: [px + x * cos - y * sin, py + x * sin + y * cos],
                colour: [r, g, b, a],
                uv: [u0 + (u1 - u0) * fx, v0 + (v1 - v0) * fy],
                local: [0.0, 0.0],
                half_size: [half[0] + 1.0, half[1] + 1.0],
                params: [0.0; 4],
            });
        }
    }

    fn extend(&mut self, texture: TextureId, blend: Blend, quads: u32) {
        let quad = self.len() as u32;
        extend_batches(&mut self.batches, quad, texture, blend, quads);
    }
}

impl PushSprite for ShapeBatch {
    fn push_sprite(&mut self, sprite: &Sprite) {
        self.push(sprite);
    }
}

impl QuadSource for ShapeBatch {
    fn vertex_bytes(&self) -> &[u8] {
        // SAFETY: `ShapeVertex` is `repr(C)` of `f32`s only, without padding (64 bytes,
        // asserted above), so its memory is fully initialised bytes; `u8` has no
        // alignment requirement.
        unsafe {
            std::slice::from_raw_parts(
                self.vertices.as_ptr().cast::<u8>(),
                size_of_val(self.vertices.as_slice()),
            )
        }
    }

    fn quad_batches(&self) -> &[Batch] {
        &self.batches
    }

    fn quad_count(&self) -> u32 {
        self.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: TextureId = TextureId(1);

    fn shape(position: [f32; 2], size: [f32; 2]) -> Shape {
        Shape::new(WHITE, UvRect::FULL, position, size, Color::WHITE)
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4
    }

    #[test]
    fn scale_from_scales_only_the_later_shapes_about_the_pivot() {
        let mut batch = ShapeBatch::new();
        let kept = Shape {
            radius: 4.0,
            ..shape([0.0, 0.0], [10.0, 10.0])
        };
        batch.push_shape(&kept);
        batch.push_shape(&Shape {
            radius: 4.0,
            outline: 2.0,
            skew: 0.2,
            ..shape([10.0, 20.0], [40.0, 10.0])
        });
        let before = batch.vertices().to_vec();
        batch.scale_from(1, [10.0, 20.0], 2.0);
        let v = batch.vertices();
        assert_eq!(v[..4], before[..4]);
        // The pivot stays; other points move away from it.
        let (a, b) = (before[4], v[4]);
        assert!(close(
            b.position,
            [
                10.0 + (a.position[0] - 10.0) * 2.0,
                20.0 + (a.position[1] - 20.0) * 2.0
            ]
        ));
        assert!(close(b.local, [a.local[0] * 2.0, a.local[1] * 2.0]));
        assert!(close(
            b.half_size,
            [a.half_size[0] * 2.0, a.half_size[1] * 2.0]
        ));
        assert_eq!(b.params[0], a.params[0] * 2.0);
        assert_eq!(b.params[1], a.params[1] * 2.0);
        assert_eq!(b.params[2], a.params[2]);
        assert_eq!(b.colour, a.colour);
        batch.scale_from(0, [0.0, 0.0], 1.0);
        assert_eq!(batch.vertices()[4], b);
    }

    #[test]
    fn a_shape_reaches_the_margin_past_its_edges() {
        let mut batch = ShapeBatch::new();
        batch.push_shape(&Shape {
            radius: 50.0,
            ..shape([10.0, 20.0], [40.0, 10.0])
        });
        let v = batch.vertices();
        assert_eq!(v.len(), 4);
        assert!(close(v[0].position, [9.0, 19.0]));
        assert!(close(v[2].position, [51.0, 31.0]));
        assert!(close(v[0].local, [-21.0, -6.0]));
        assert!(close(v[0].half_size, [20.0, 5.0]));
        // The radius is held to half the smaller side.
        assert_eq!(v[0].params[0], 5.0);
        // The texture coordinates go on past the edges at the same rate.
        assert!(close(v[0].uv, [-1.0 / 40.0, -0.1]));
        assert!(close(v[2].uv, [1.0 + 1.0 / 40.0, 1.1]));
    }

    #[test]
    fn the_skew_shears_about_the_centre_line() {
        let mut batch = ShapeBatch::new();
        let skewed = Shape {
            skew: std::f32::consts::FRAC_PI_4,
            ..shape([0.0, 0.0], [20.0, 10.0])
        };
        batch.push_shape(&skewed);
        let v = batch.vertices();
        // Top corners 6 px right (5 px above the centre plus the margin), bottom 6 px
        // left; the local coordinates stay unsheared.
        assert!(close(v[0].position, [-1.0 + 6.0, -1.0]));
        assert!(close(v[3].position, [-1.0 - 6.0, 11.0]));
        assert!(close(v[0].local, [-11.0, -6.0]));
        assert!((skewed.skew_offset() - 10.0).abs() < 1e-4);
        assert!(skewed.contains([24.0, 0.5]));
        assert!(!skewed.contains([2.0, 0.5]));
        assert!(skewed.contains([-4.0, 9.5]));
        assert!(!skewed.contains([18.0, 9.5]));
        assert!(!skewed.contains([10.0, 10.5]));
    }

    #[test]
    fn shapes_and_sprites_share_batches_by_texture_and_blend() {
        let mut batch = ShapeBatch::new();
        batch.push_shape(&shape([0.0, 0.0], [4.0, 4.0]));
        batch.push(&Sprite::new(WHITE, UvRect::FULL, [0.0, 0.0], [4.0, 4.0]));
        batch.push(&Sprite::new(
            TextureId(2),
            UvRect::FULL,
            [0.0, 0.0],
            [4.0, 4.0],
        ));
        batch.push_shape(&Shape {
            blend: Blend::Additive,
            ..shape([0.0, 0.0], [4.0, 4.0])
        });
        // Empty shapes are skipped.
        batch.push_shape(&shape([0.0, 0.0], [0.0, 4.0]));
        assert_eq!(batch.len(), 4);
        let runs: Vec<_> = batch
            .batches()
            .iter()
            .map(|b| (b.first_quad, b.quads))
            .collect();
        assert_eq!(runs, [(0, 2), (2, 1), (3, 1)]);
        assert_eq!(batch.quad_count(), 4);
        assert_eq!(batch.vertex_bytes().len(), 4 * 4 * 64);
    }

    #[test]
    fn the_drift_adds_to_the_phase() {
        let mut batch = ShapeBatch::new();
        batch.set_drift(1026.5);
        batch.clear();
        batch.push_shape(&Shape {
            phase: 0.25,
            ..shape([0.0, 0.0], [4.0, 4.0])
        });
        assert_eq!(batch.vertices()[0].params[3], 2.75);
    }

    #[test]
    fn a_glow_reaches_past_the_shape_and_marks_itself_negative() {
        let mut batch = ShapeBatch::new();
        batch.push_shape(&Shape {
            glow: 6.0,
            outline: 2.0,
            sheen: 0.4,
            ..shape([10.0, 10.0], [20.0, 10.0])
        });
        let v = batch.vertices();
        assert!(close(v[0].position, [3.0, 3.0]));
        assert_eq!(v[0].params[1], -6.0);
        assert_eq!(v[0].params[2], 0.4);
    }

    #[test]
    fn a_sprite_is_a_plain_quad_inside_its_shape() {
        let mut batch = ShapeBatch::new();
        batch.push(&Sprite {
            origin: [2.0, 1.0],
            ..Sprite::new(WHITE, UvRect::FULL, [10.0, 10.0], [4.0, 2.0])
        });
        let v = batch.vertices();
        assert!(close(v[0].position, [8.0, 9.0]));
        assert!(close(v[2].position, [12.0, 11.0]));
        assert!(
            v.iter()
                .all(|v| v.params == [0.0; 4] && v.local == [0.0, 0.0])
        );
        assert!(
            v.iter()
                .all(|v| v.half_size[0] > 0.0 && v.half_size[1] > 0.0)
        );
    }

    #[test]
    fn recording_again_does_not_allocate() {
        let mut batch = ShapeBatch::with_capacity(4);
        let record = |batch: &mut ShapeBatch| {
            batch.clear();
            batch.push_shape(&Shape {
                radius: 3.0,
                skew: 0.2,
                sheen: 0.5,
                glow: 4.0,
                ..shape([0.0, 0.0], [10.0, 10.0])
            });
            batch.push(&Sprite::new(
                TextureId(2),
                UvRect::FULL,
                [0.0, 0.0],
                [1.0, 1.0],
            ));
        };
        record(&mut batch);
        let ((), allocs) = slam_testkit::count_allocs(|| record(&mut batch));
        assert_eq!(allocs.total(), 0, "recording again allocated: {allocs}");
    }
}
