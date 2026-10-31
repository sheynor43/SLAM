//! Sprite batcher (ADR-0020). A [`SpriteBatch`] records a frame's sprites on the CPU,
//! in draw order, merging neighbours that share a texture and blending into one batch;
//! a [`SpriteRenderer`] uploads the recording once and draws each batch. Neither
//! allocates once its buffers have grown to the frame's size.

use std::mem::size_of;
use std::ops::Range;

use crate::atlas::UvRect;
use crate::hal::{
    Blend, BufferDesc, BufferKind, BufferUpdate, Color, DescError, Device, DeviceError, DrawError,
    Extent, IndexFormat, PipelineDesc, Shader, VertexAttribute, VertexFormat, VertexLayout,
};
use crate::shaders;
use crate::textures::TextureId;

/// One corner of a sprite, as `shaders/sprite.wgsl` reads it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteVertex {
    /// Pixels of the target, origin top left, y down.
    pub position: [f32; 2],
    /// sRGB-encoded, straight alpha (ADR-0020).
    pub colour: [f32; 4],
    pub uv: [f32; 2],
}

const _: () = assert!(size_of::<SpriteVertex>() == 32);

pub const SPRITE_ATTRIBUTES: [VertexAttribute; 3] = [
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
];

pub const SPRITE_LAYOUT: VertexLayout<'static> = VertexLayout {
    stride: size_of::<SpriteVertex>() as u32,
    attributes: &SPRITE_ATTRIBUTES,
};

/// A textured, tinted rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    /// Where `origin` lands, in target pixels.
    pub position: [f32; 2],
    pub size: [f32; 2],
    /// Pivot of placement and rotation, in pixels from the sprite's top-left corner.
    pub origin: [f32; 2],
    /// Radians, clockwise on screen.
    pub rotation: f32,
    /// Multiplies the texture; sRGB-encoded, straight alpha.
    pub colour: Color,
    pub texture: TextureId,
    pub uv: UvRect,
    pub blend: Blend,
}

impl Sprite {
    /// An unrotated, untinted, alpha-blended sprite with its top-left corner at
    /// `position`.
    pub const fn new(texture: TextureId, uv: UvRect, position: [f32; 2], size: [f32; 2]) -> Self {
        Self {
            position,
            size,
            origin: [0.0, 0.0],
            rotation: 0.0,
            colour: Color::WHITE,
            texture,
            uv,
            blend: Blend::Alpha,
        }
    }
}

/// Consecutive quads drawn with one texture and blending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Batch {
    pub texture: TextureId,
    pub blend: Blend,
    pub first_quad: u32,
    pub quads: u32,
}

/// A frame's sprites in draw order. Reuse it across frames with [`SpriteBatch::clear`].
#[derive(Debug, Default)]
pub struct SpriteBatch {
    vertices: Vec<SpriteVertex>,
    batches: Vec<Batch>,
}

impl SpriteBatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Room for `quads` sprites before the first frame grows the buffers, however often
    /// their textures and blending change.
    pub fn with_capacity(quads: usize) -> Self {
        Self {
            vertices: Vec::with_capacity(quads * 4),
            batches: Vec::with_capacity(quads),
        }
    }

    /// Forgets the sprites, keeping the memory.
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.batches.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// Number of sprites recorded.
    pub fn len(&self) -> usize {
        self.vertices.len() / 4
    }

    /// Corners, four per sprite: top left, top right, bottom right, bottom left.
    pub fn vertices(&self) -> &[SpriteVertex] {
        &self.vertices
    }

    pub fn batches(&self) -> &[Batch] {
        &self.batches
    }

    /// Moves the sprites from the `first`-th on by `map` and multiplies their colours by
    /// `tint` ([`tint_srgb`]): a layer placed, tinted and faded after it was recorded.
    pub fn transform_from(&mut self, first: usize, map: &Affine, tint: [f32; 4]) {
        let identity = map.is_identity();
        let tint = Tint::new(tint);
        for vertex in self.vertices.iter_mut().skip(first * 4) {
            if !identity {
                vertex.position = map.apply(vertex.position);
            }
            vertex.colour = tint.apply(vertex.colour);
        }
    }

    /// Records `sprite` on top of those recorded before.
    pub fn push(&mut self, sprite: &Sprite) {
        self.extend_batch(sprite, 1);
        let at = Placement::new(sprite);
        self.vertices.extend_from_slice(&[
            at.vertex(0.0, 0.0),
            at.vertex(1.0, 0.0),
            at.vertex(1.0, 1.0),
            at.vertex(0.0, 1.0),
        ]);
    }

    /// Records `sprite` like [`push`](Self::push) but leaves out an octagon inscribed in
    /// the ellipse around its centre with semi-axes `hole` (fractions of the size, up to
    /// 0.5): [`RING_QUADS`] quads instead of one. Where the texture is fully transparent
    /// inside that ellipse (an approach circle), it draws what the single quad would with
    /// less fill: the texture coordinates are an affine function of the position, so
    /// every pixel left in samples the same point, up to the rounding of interpolating
    /// over other triangles (a last-bit difference at most). A hole under
    /// [`MIN_RING_HOLE`] on either axis is not worth the vertices: one quad.
    pub fn push_ring(&mut self, sprite: &Sprite, hole: [f32; 2]) {
        if !(hole[0] >= MIN_RING_HOLE && hole[1] >= MIN_RING_HOLE) {
            self.push(sprite);
            return;
        }
        let [hx, hy] = hole.map(|h| h.min(0.5));
        self.extend_batch(sprite, RING_QUADS as u32);
        let at = Placement::new(sprite);
        // Rays every 45°: the inner ends on the ellipse, the outer ones alternately at
        // the middles of the sides and at the corners, so each outer segment runs along
        // one side and the quads tile the sprite outside the octagon exactly. Shared
        // corners are computed once, so neighbouring quads meet without gaps.
        let d = std::f32::consts::FRAC_1_SQRT_2;
        let dirs = [
            (1.0, 0.0),
            (d, d),
            (0.0, 1.0),
            (-d, d),
            (-1.0, 0.0),
            (-d, -d),
            (0.0, -1.0),
            (d, -d),
        ];
        let inner = dirs.map(|(x, y)| at.vertex(0.5 + hx * x, 0.5 + hy * y));
        let outer = [
            (1.0, 0.5),
            (1.0, 1.0),
            (0.5, 1.0),
            (0.0, 1.0),
            (0.0, 0.5),
            (0.0, 0.0),
            (0.5, 0.0),
            (1.0, 0.0),
        ]
        .map(|(x, y)| at.vertex(x, y));
        for k in 0..RING_QUADS {
            let next = (k + 1) % RING_QUADS;
            // The same winding as `push`'s corners.
            self.vertices
                .extend_from_slice(&[inner[k], outer[k], outer[next], inner[next]]);
        }
    }

    /// Accounts for `quads` quads of `sprite`'s texture and blend.
    fn extend_batch(&mut self, sprite: &Sprite, quads: u32) {
        // A u32 quad index overflows only past 2^32 sprites (128 GiB of vertices).
        let quad = self.len() as u32;
        match self.batches.last_mut() {
            Some(last) if last.texture == sprite.texture && last.blend == sprite.blend => {
                last.quads += quads;
            }
            _ => self.batches.push(Batch {
                texture: sprite.texture,
                blend: sprite.blend,
                first_quad: quad,
                quads,
            }),
        }
    }
}

/// Quads of [`SpriteBatch::push_ring`].
pub const RING_QUADS: usize = 8;
/// The smallest hole, as a fraction of the size on each axis, that
/// [`SpriteBatch::push_ring`] leaves out: below it the octagon saves less fill than the
/// extra vertices cost.
pub const MIN_RING_HOLE: f32 = 0.2;

/// Where a sprite's unit square goes: position, rotation, colour and texture rectangle.
struct Placement {
    size: [f32; 2],
    origin: [f32; 2],
    position: [f32; 2],
    sin: f32,
    cos: f32,
    colour: [f32; 4],
    uv: UvRect,
}

impl Placement {
    fn new(sprite: &Sprite) -> Self {
        let (sin, cos) = sprite.rotation.sin_cos();
        let Color { r, g, b, a } = sprite.colour;
        Self {
            size: sprite.size,
            origin: sprite.origin,
            position: sprite.position,
            sin,
            cos,
            colour: [r, g, b, a],
            uv: sprite.uv,
        }
    }

    /// The vertex at `(fx, fy)` of the unit square (0, 0 the top left, 1, 1 the bottom
    /// right). The corners take the texture rectangle's edges exactly.
    fn vertex(&self, fx: f32, fy: f32) -> SpriteVertex {
        let lerp = |a: f32, b: f32, t: f32| match t {
            0.0 => a,
            1.0 => b,
            _ => a + (b - a) * t,
        };
        let [w, h] = self.size;
        let [ox, oy] = self.origin;
        let [px, py] = self.position;
        let UvRect { u0, v0, u1, v1 } = self.uv;
        let (x, y) = (lerp(0.0, w, fx) - ox, lerp(0.0, h, fy) - oy);
        SpriteVertex {
            position: [
                px + x * self.cos - y * self.sin,
                py + x * self.sin + y * self.cos,
            ],
            colour: self.colour,
            uv: [lerp(u0, u1, fx), lerp(v0, v1, fy)],
        }
    }
}

/// A 2D affine map of target pixels: `p ↦ (a·x + c·y + tx, b·x + d·y + ty)`. Places a
/// layer drawn in place somewhere else: scaled and rotated about a point, moved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// Scales by `scale` and rotates by `rotation` (radians, clockwise on screen) about
    /// `pivot`, then moves by `offset`.
    pub fn about(pivot: [f32; 2], scale: f32, rotation: f32, offset: [f32; 2]) -> Self {
        let (sin, cos) = rotation.sin_cos();
        let (a, b, c, d) = (cos * scale, sin * scale, -sin * scale, cos * scale);
        let [px, py] = pivot;
        Self {
            a,
            b,
            c,
            d,
            tx: px - (a * px + c * py) + offset[0],
            ty: py - (b * px + d * py) + offset[1],
        }
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    pub fn apply(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [
            self.a * x + self.c * y + self.tx,
            self.b * x + self.d * y + self.ty,
        ]
    }

    /// `self` after `inner`: `p ↦ self(inner(p))`.
    pub fn then_after(&self, inner: &Affine) -> Self {
        Self {
            a: self.a * inner.a + self.c * inner.b,
            b: self.b * inner.a + self.d * inner.b,
            c: self.a * inner.c + self.c * inner.d,
            d: self.b * inner.c + self.d * inner.d,
            tx: self.a * inner.tx + self.c * inner.ty + self.tx,
            ty: self.b * inner.tx + self.d * inner.ty + self.ty,
        }
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Colour/SRGBColour.cs (operator *)
/// `colour` tinted by `tint`, both sRGB-encoded with straight alpha: the colours multiply
/// in linear light, as a parent's colour multiplies a child's; the alphas multiply as
/// they are.
pub fn tint_srgb(colour: [f32; 4], tint: [f32; 4]) -> [f32; 4] {
    Tint::new(tint).apply(colour)
}

/// A tint with its colour in linear light, for many colours.
struct Tint {
    srgb: [f32; 4],
    linear: [f32; 3],
    white: bool,
}

impl Tint {
    fn new(srgb: [f32; 4]) -> Self {
        Self {
            srgb,
            linear: [0, 1, 2].map(|i| srgb_to_linear(srgb[i])),
            white: srgb[..3] == [1.0; 3],
        }
    }

    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Colour/SRGBColour.cs (operator *)
    /// A white side gives the other colour as it is.
    fn apply(&self, colour: [f32; 4]) -> [f32; 4] {
        let alpha = colour[3] * self.srgb[3];
        if self.white {
            [colour[0], colour[1], colour[2], alpha]
        } else if colour[..3] == [1.0; 3] {
            [self.srgb[0], self.srgb[1], self.srgb[2], alpha]
        } else {
            let [r, g, b] =
                [0, 1, 2].map(|i| linear_to_srgb(srgb_to_linear(colour[i]) * self.linear[i]));
            [r, g, b, alpha]
        }
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Extensions/Color4Extensions/Color4Extensions.cs (ToLinear)
fn srgb_to_linear(c: f32) -> f32 {
    if c == 1.0 {
        1.0
    } else if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Extensions/Color4Extensions/Color4Extensions.cs (ToSRGB)
fn linear_to_srgb(c: f32) -> f32 {
    if c == 1.0 {
        1.0
    } else if c < 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Maps target pixels (origin top left, y down) to clip space: a column-major 4x4
/// matrix, the `Globals` uniform block of `shaders/sprite.wgsl`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection(pub [f32; 16]);

impl Projection {
    /// Pixels of a target of `size`.
    pub fn pixels(size: Extent) -> Self {
        let (w, h) = (size.width.max(1) as f32, size.height.max(1) as f32);
        #[rustfmt::skip]
        let m = [
            2.0 / w, 0.0, 0.0, 0.0,
            0.0, -2.0 / h, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            -1.0, 1.0, 0.0, 1.0,
        ];
        Self(m)
    }

    /// The projection of points `p` drawn at `offset + p * scale` by this one: for
    /// geometry in its own units (a slider path in osu! pixels) placed on the target.
    pub fn scaled(&self, scale: f32, [ox, oy]: [f32; 2]) -> Self {
        let mut m = self.0;
        for row in 0..4 {
            let (x, y) = (self.0[row], self.0[4 + row]);
            m[row] = x * scale;
            m[4 + row] = y * scale;
            m[12 + row] = x * ox + y * oy + self.0[12 + row];
        }
        Self(m)
    }

    /// The projection of points `p` drawn at `map(p)` by this one.
    pub fn transformed(&self, map: &Affine) -> Self {
        let mut m = self.0;
        for row in 0..4 {
            let (x, y) = (self.0[row], self.0[4 + row]);
            m[row] = x * map.a + y * map.b;
            m[4 + row] = x * map.c + y * map.d;
            m[12 + row] = x * map.tx + y * map.ty + self.0[12 + row];
        }
        Self(m)
    }

    /// Clip-space x and y of a point.
    pub fn apply(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        let m = &self.0;
        [m[0] * x + m[4] * y + m[12], m[1] * x + m[5] * y + m[13]]
    }
}

/// What one [`SpriteRenderer::draw`] did, for the frame-time overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpriteStats {
    pub sprites: u32,
    pub batches: u32,
    pub draw_calls: u32,
}

impl std::ops::AddAssign for SpriteStats {
    fn add_assign(&mut self, rhs: Self) {
        self.sprites += rhs.sprites;
        self.batches += rhs.batches;
        self.draw_calls += rhs.draw_calls;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpriteError {
    #[error("texture {0:?} is not available")]
    MissingTexture(TextureId),
    #[error("more than {slots} sprite draws this frame")]
    OutOfSlots { slots: usize },
    #[error(transparent)]
    Draw(#[from] DrawError),
    #[error(transparent)]
    Desc(#[from] DescError),
    /// Growing a vertex buffer failed.
    #[error(transparent)]
    Device(#[from] DeviceError),
}

/// Quads one indexed draw can address with `u16` indices.
pub const MAX_QUADS_PER_DRAW: u32 = 1 << 14;

/// Smallest vertex buffer a slot creates, in bytes.
const MIN_VERTEX_BUFFER: u64 = 64 * 1024;

/// Slots for up to three draws per frame.
pub const DEFAULT_SLOTS: usize = 3;

/// Vertex and projection buffers for one [`SpriteRenderer::draw`].
struct Slot<B> {
    vertices: Option<(B, u64)>,
    globals: B,
}

/// Draws [`SpriteBatch`]es with `shaders/sprite.wgsl`, or another shader with the same
/// vertices and bindings (`shaders/msdf.wgsl` for text). Each draw uses the next of a
/// ring of `slots` buffer sets, continuing across frames (ADR-0020). At most `slots`
/// draws are allowed between [`SpriteRenderer::begin_frame`] calls, so no buffer is
/// written twice in a frame (the `write_buffer` contract); spare slots space out
/// rewrites of a buffer the GPU may still be reading from an earlier frame.
pub struct SpriteRenderer<D: Device> {
    /// Indexed by [`blend_index`].
    pipelines: [D::Pipeline; BLEND_MODES],
    indices: D::Buffer,
    slots: Vec<Slot<D::Buffer>>,
    next: usize,
    /// Draws since `begin_frame`.
    draws: usize,
}

const BLEND_MODES: usize = 5;

const fn blend_index(blend: Blend) -> usize {
    match blend {
        Blend::Replace => 0,
        Blend::Alpha => 1,
        Blend::Premultiplied => 2,
        Blend::Additive => 3,
        Blend::Max => 4,
    }
}

/// `u16` indices of two triangles per quad (corners 0-1-2 and 2-3-0).
pub(crate) fn quad_indices(quads: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(quads as usize * 12);
    for q in 0..quads {
        let base = (q * 4) as u16;
        for i in [0, 1, 2, 2, 3, 0] {
            out.extend_from_slice(&(base + i).to_ne_bytes());
        }
    }
    out
}

impl<D: Device> SpriteRenderer<D> {
    /// Creates the pipelines and buffers; on failure, frees what was created.
    pub fn new(device: &mut D, slots: usize) -> Result<Self, DeviceError> {
        Self::with_shader(device, slots, &shaders::SPRITE)
    }

    /// Like [`SpriteRenderer::new`], drawing with `shader`, which takes
    /// [`SPRITE_LAYOUT`] vertices and the bindings of `shaders/sprite.wgsl`.
    pub fn with_shader(device: &mut D, slots: usize, shader: &Shader) -> Result<Self, DeviceError> {
        let mut pipelines = Vec::with_capacity(BLEND_MODES);
        let mut buffers = Vec::with_capacity(slots.max(1) + 1);
        match Self::create(device, shader, slots.max(1), &mut pipelines, &mut buffers) {
            Ok(()) => {}
            Err(e) => {
                pipelines
                    .into_iter()
                    .for_each(|p| device.destroy_pipeline(p));
                buffers.into_iter().for_each(|b| device.destroy_buffer(b));
                return Err(e);
            }
        }
        let mut buffers = buffers.into_iter();
        let indices = buffers.next().expect("created first");
        let slots = buffers
            .map(|globals| Slot {
                vertices: None,
                globals,
            })
            .collect();
        let Ok(pipelines) = <[D::Pipeline; BLEND_MODES]>::try_from(pipelines) else {
            unreachable!("one pipeline per blend mode")
        };
        Ok(Self {
            pipelines,
            indices,
            slots,
            next: 0,
            draws: 0,
        })
    }

    /// Pushes the pipelines (in [`blend_index`] order), then the index buffer and
    /// one uniform buffer per slot.
    fn create(
        device: &mut D,
        shader: &Shader,
        slots: usize,
        pipelines: &mut Vec<D::Pipeline>,
        buffers: &mut Vec<D::Buffer>,
    ) -> Result<(), DeviceError> {
        for blend in [
            Blend::Replace,
            Blend::Alpha,
            Blend::Premultiplied,
            Blend::Additive,
            Blend::Max,
        ] {
            debug_assert_eq!(blend_index(blend), pipelines.len());
            pipelines.push(device.create_pipeline(&PipelineDesc {
                shader,
                layout: SPRITE_LAYOUT,
                blend,
            })?);
        }
        let indices = quad_indices(MAX_QUADS_PER_DRAW);
        buffers.push(device.create_buffer(
            &BufferDesc {
                kind: BufferKind::Index,
                size: indices.len() as u64,
                update: BufferUpdate::Static,
            },
            Some(&indices),
        )?);
        let globals = BufferDesc {
            kind: BufferKind::Uniform,
            size: size_of::<Projection>() as u64,
            update: BufferUpdate::Dynamic,
        };
        for _ in 0..slots {
            buffers.push(device.create_buffer(&globals, None)?);
        }
        Ok(())
    }

    /// Starts counting the frame's draws; call once per frame before drawing.
    pub fn begin_frame(&mut self) {
        self.draws = 0;
    }

    /// Frees the device resources.
    pub fn destroy(self, device: &mut D) {
        for pipeline in self.pipelines {
            device.destroy_pipeline(pipeline);
        }
        device.destroy_buffer(self.indices);
        for slot in self.slots {
            device.destroy_buffer(slot.globals);
            if let Some((buffer, _)) = slot.vertices {
                device.destroy_buffer(buffer);
            }
        }
    }

    /// Draws `batch` into the current pass. `texture` resolves the batch's texture
    /// ids; every one must resolve, or nothing is drawn. Leaves the pipeline, buffers
    /// and texture slot 0 bound. Call [`SpriteRenderer::begin_frame`] every frame, or
    /// draws past the first `slots` fail with [`SpriteError::OutOfSlots`].
    pub fn draw<'t>(
        &mut self,
        device: &mut D,
        batch: &SpriteBatch,
        projection: &Projection,
        texture: impl Fn(TextureId) -> Option<&'t D::Texture>,
    ) -> Result<SpriteStats, SpriteError>
    where
        D::Texture: 't,
    {
        let uploaded = self.upload(device, batch, projection, &texture)?;
        self.draw_range(device, &uploaded, batch, 0..batch.len() as u32, texture)
    }

    /// Puts `batch` on the GPU for [`SpriteRenderer::draw_range`], taking one of the
    /// frame's slots (none for an empty batch). Every texture of the batch must resolve,
    /// or nothing is uploaded. Lets other drawing (slider bodies) go between parts of
    /// one batch without a slot per part.
    pub fn upload<'t>(
        &mut self,
        device: &mut D,
        batch: &SpriteBatch,
        projection: &Projection,
        texture: impl Fn(TextureId) -> Option<&'t D::Texture>,
    ) -> Result<UploadedBatch, SpriteError>
    where
        D::Texture: 't,
    {
        if batch.is_empty() {
            return Ok(UploadedBatch {
                slot: None,
                quads: 0,
            });
        }
        if let Some(b) = batch.batches.iter().find(|b| texture(b.texture).is_none()) {
            return Err(SpriteError::MissingTexture(b.texture));
        }
        if self.draws == self.slots.len() {
            return Err(SpriteError::OutOfSlots {
                slots: self.slots.len(),
            });
        }
        let current = self.next;
        self.next = (current + 1) % self.slots.len();
        let slot = &mut self.slots[current];

        // SAFETY: `SpriteVertex` is `repr(C)` of `f32`s only, without padding (32
        // bytes, asserted above), so its memory is fully initialised bytes; `u8` has
        // no alignment requirement.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                batch.vertices.as_ptr().cast::<u8>(),
                size_of_val(batch.vertices.as_slice()),
            )
        };
        let needed = bytes.len() as u64;
        if slot
            .vertices
            .as_ref()
            .is_none_or(|(_, size)| *size < needed)
        {
            if let Some((old, _)) = slot.vertices.take() {
                device.destroy_buffer(old);
            }
            let size = needed.max(MIN_VERTEX_BUFFER).next_power_of_two();
            let desc = BufferDesc {
                kind: BufferKind::Vertex,
                size,
                update: BufferUpdate::Dynamic,
            };
            slot.vertices = Some((device.create_buffer(&desc, None)?, size));
        }
        self.draws += 1;
        let Some((vertices, _)) = &slot.vertices else {
            unreachable!("created above")
        };
        device.write_buffer(vertices, 0, bytes)?;
        // SAFETY: `Projection` is `repr(transparent)` over 16 `f32`s; see above.
        let globals = unsafe {
            std::slice::from_raw_parts(projection.0.as_ptr().cast::<u8>(), size_of::<Projection>())
        };
        device.write_buffer(&slot.globals, 0, globals)?;
        Ok(UploadedBatch {
            slot: Some(current),
            quads: batch.len() as u32,
        })
    }

    /// Draws the sprites `quads` (indices in recording order) of `batch`, uploaded as
    /// `uploaded` this frame, into the current pass. Binds everything it needs, so
    /// other drawing may happen between ranges; leaves the pipeline, buffers and
    /// texture slot 0 bound. The batch must not change between upload and draws.
    pub fn draw_range<'t>(
        &mut self,
        device: &mut D,
        uploaded: &UploadedBatch,
        batch: &SpriteBatch,
        quads: Range<u32>,
        texture: impl Fn(TextureId) -> Option<&'t D::Texture>,
    ) -> Result<SpriteStats, SpriteError>
    where
        D::Texture: 't,
    {
        debug_assert_eq!(
            uploaded.quads,
            batch.len() as u32,
            "batch changed after upload"
        );
        let quads = quads.start..quads.end.min(uploaded.quads);
        let Some(slot) = uploaded.slot.filter(|_| !quads.is_empty()) else {
            return Ok(SpriteStats::default());
        };
        let slot = &self.slots[slot];
        let Some((vertices, _)) = &slot.vertices else {
            unreachable!("an uploaded slot has vertices")
        };
        device.set_uniform_buffer(0, &slot.globals)?;
        device.set_index_buffer(&self.indices, IndexFormat::U16)?;

        let mut stats = SpriteStats {
            sprites: quads.end - quads.start,
            batches: 0,
            draw_calls: 0,
        };
        let stride = u64::from(SPRITE_LAYOUT.stride);
        // Batches are in quad order; skip those ending before the range.
        let from = batch
            .batches
            .partition_point(|b| b.first_quad + b.quads <= quads.start);
        for b in &batch.batches[from..] {
            if b.first_quad >= quads.end {
                break;
            }
            stats.batches += 1;
            device.set_pipeline(&self.pipelines[blend_index(b.blend)]);
            let tex = texture(b.texture).ok_or(SpriteError::MissingTexture(b.texture))?;
            device.set_texture(0, tex)?;
            let mut first = b.first_quad.max(quads.start);
            let end = (b.first_quad + b.quads).min(quads.end);
            while first < end {
                let quads = (end - first).min(MAX_QUADS_PER_DRAW);
                device.set_vertex_buffer(vertices, u64::from(first) * 4 * stride)?;
                device.draw_indexed(0..quads * 6)?;
                stats.draw_calls += 1;
                first += quads;
            }
        }
        Ok(stats)
    }
}

/// A [`SpriteBatch`] put on the GPU by [`SpriteRenderer::upload`] for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadedBatch {
    /// The slot holding the vertices; none for an empty batch.
    slot: Option<usize>,
    quads: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_device::{Call, Fake};

    slam_testkit::install_counting_allocator!();

    const RED: Color = Color::rgb(1.0, 0.0, 0.0);

    fn sprite(texture: u32, blend: Blend) -> Sprite {
        Sprite {
            blend,
            ..Sprite::new(TextureId(texture), UvRect::FULL, [0.0, 0.0], [1.0, 1.0])
        }
    }

    fn positions(batch: &SpriteBatch) -> Vec<[f32; 2]> {
        batch.vertices().iter().map(|v| v.position).collect()
    }

    fn assert_close(actual: &[[f32; 2]], expected: &[[f32; 2]]) {
        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected) {
            assert!(
                (a[0] - e[0]).abs() < 1e-4 && (a[1] - e[1]).abs() < 1e-4,
                "{actual:?} != {expected:?}"
            );
        }
    }

    /// A rotated, tinted sprite on part of an atlas page.
    fn ring_sprite() -> Sprite {
        Sprite {
            origin: [60.0, 50.0],
            rotation: 0.4,
            colour: RED,
            ..Sprite::new(
                TextureId(2),
                UvRect {
                    u0: 0.25,
                    v0: 0.5,
                    u1: 0.75,
                    v1: 0.625,
                },
                [300.0, 200.0],
                [120.0, 100.0],
            )
        }
    }

    /// Signed area of the quad, from its corners in order.
    fn quad_area(q: &[SpriteVertex]) -> f32 {
        (0..4)
            .map(|i| {
                let (a, b) = (q[i].position, q[(i + 1) % 4].position);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f32>()
            / 2.0
    }

    #[test]
    fn a_ring_samples_the_texture_like_the_whole_quad() {
        let s = ring_sprite();
        let mut whole = SpriteBatch::with_capacity(1);
        whole.push(&s);
        let mut ring = SpriteBatch::with_capacity(RING_QUADS);
        ring.push_ring(&s, [0.4, 0.45]);
        assert_eq!(ring.len(), RING_QUADS);
        assert_eq!(
            ring.batches(),
            [Batch {
                texture: TextureId(2),
                blend: Blend::Alpha,
                first_quad: 0,
                quads: RING_QUADS as u32,
            }]
        );
        // The whole quad's texture coordinates as an affine function of the position.
        let c = whole.vertices();
        let (o, ex, ey) = (c[0], c[1], c[3]);
        let sub = |a: [f32; 2], b: [f32; 2]| [a[0] - b[0], a[1] - b[1]];
        let (dx, dy) = (sub(ex.position, o.position), sub(ey.position, o.position));
        let det = dx[0] * dy[1] - dx[1] * dy[0];
        let uv_at = |p: [f32; 2]| {
            let r = sub(p, o.position);
            let a = (r[0] * dy[1] - r[1] * dy[0]) / det;
            let b = (dx[0] * r[1] - dx[1] * r[0]) / det;
            [
                o.uv[0] + (ex.uv[0] - o.uv[0]) * a,
                o.uv[1] + (ey.uv[1] - o.uv[1]) * b,
            ]
        };
        for v in ring.vertices() {
            let uv = uv_at(v.position);
            assert!((uv[0] - v.uv[0]).abs() < 1e-5 && (uv[1] - v.uv[1]).abs() < 1e-5);
            assert_eq!(v.colour, c[0].colour);
        }
        // The quads keep `push`'s winding and cover the sprite less the octagon
        // inscribed in the ellipse of semi-axes 48 × 45 pixels.
        let whole_area = quad_area(c);
        let quads: Vec<f32> = ring.vertices().chunks(4).map(quad_area).collect();
        assert!(quads.iter().all(|a| a.signum() == whole_area.signum()));
        let octagon = 2.0 * std::f32::consts::SQRT_2 * 48.0 * 45.0;
        let covered: f32 = quads.iter().sum();
        assert!((covered - (whole_area.abs() - octagon) * whole_area.signum()).abs() < 0.5);
        // The outer corners are the whole quad's, bit for bit.
        for corner in c {
            assert!(ring.vertices().iter().any(|v| v == corner));
        }
    }

    #[test]
    fn rings_of_uneven_holes_are_made_of_convex_quads() {
        let s = ring_sprite();
        let mut whole = SpriteBatch::with_capacity(1);
        whole.push(&s);
        let whole_area = quad_area(whole.vertices());
        let [w, h] = s.size;
        for [hx, hy] in [[0.5, 0.2], [0.2, 0.5], [0.45, 0.25], [0.7, 0.3]] {
            let mut ring = SpriteBatch::with_capacity(RING_QUADS);
            ring.push_ring(&s, [hx, hy]);
            for q in ring.vertices().chunks(4) {
                // No turn goes against the whole quad's: convex, and split into two
                // triangles along either diagonal without folding.
                for i in 0..4 {
                    let (a, b, c) = (
                        q[i].position,
                        q[(i + 1) % 4].position,
                        q[(i + 2) % 4].position,
                    );
                    let turn = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
                    // A hole of half the size meets the side: that triangle is empty.
                    assert!(turn * whole_area >= 0.0, "{hx}, {hy}: {q:?}");
                }
            }
            let octagon = 2.0 * std::f32::consts::SQRT_2 * hx.min(0.5) * w * hy.min(0.5) * h;
            let covered: f32 = ring.vertices().chunks(4).map(quad_area).sum();
            let expected = (whole_area.abs() - octagon) * whole_area.signum();
            assert!((covered - expected).abs() < 0.5, "{hx}, {hy}");
        }
    }

    #[test]
    fn a_small_hole_draws_one_quad() {
        let s = ring_sprite();
        let mut whole = SpriteBatch::with_capacity(1);
        whole.push(&s);
        for hole in [[0.0, 0.0], [0.1, 0.4], [0.4, 0.1], [f32::NAN, 0.4]] {
            let mut ring = SpriteBatch::with_capacity(1);
            ring.push_ring(&s, hole);
            assert_eq!(ring.vertices(), whole.vertices(), "{hole:?}");
        }
    }

    #[test]
    fn rings_extend_the_batch_of_the_same_texture() {
        let mut batch = SpriteBatch::with_capacity(RING_QUADS + 2);
        let s = ring_sprite();
        batch.push(&s);
        batch.push_ring(&s, [0.45, 0.45]);
        batch.push(&sprite(7, Blend::Alpha));
        let quads: Vec<_> = batch
            .batches()
            .iter()
            .map(|b| (b.first_quad, b.quads))
            .collect();
        assert_eq!(
            quads,
            [(0, RING_QUADS as u32 + 1), (RING_QUADS as u32 + 1, 1)]
        );
    }

    #[test]
    fn affine_about_keeps_its_pivot_and_turns_clockwise() {
        let map = Affine::about([10.0, 20.0], 2.0, std::f32::consts::FRAC_PI_2, [0.0, 0.0]);
        assert_close(&[map.apply([10.0, 20.0])], &[[10.0, 20.0]]);
        // Right of the pivot goes below it (y down), twice as far.
        assert_close(&[map.apply([11.0, 20.0])], &[[10.0, 22.0]]);
        let moved = Affine::about([10.0, 20.0], 1.0, 0.0, [3.0, -4.0]);
        assert_close(&[moved.apply([0.0, 0.0])], &[[3.0, -4.0]]);
        assert!(Affine::default().is_identity());
    }

    #[test]
    fn then_after_applies_the_inner_map_first() {
        let inner = Affine::about([0.0, 0.0], 1.0, 0.0, [5.0, 0.0]);
        let outer = Affine::about([0.0, 0.0], 2.0, 0.0, [0.0, 0.0]);
        let both = outer.then_after(&inner);
        // (1, 1) → (6, 1) → (12, 2).
        assert_close(&[both.apply([1.0, 1.0])], &[[12.0, 2.0]]);
        let p = [3.0, -7.0];
        let a = Affine::about([1.0, 2.0], 0.7, 0.3, [4.0, 5.0]);
        let b = Affine::about([-3.0, 8.0], 1.3, -1.1, [-2.0, 1.0]);
        assert_close(&[a.then_after(&b).apply(p)], &[a.apply(b.apply(p))]);
    }

    #[test]
    fn transformed_projection_projects_the_mapped_point() {
        let projection = Projection::pixels(Extent::new(800, 600));
        let map = Affine::about([400.0, 300.0], 0.85, 0.017, [0.0, 10.0]);
        let scaled = projection.transformed(&map).scaled(1.5, [100.0, 50.0]);
        for p in [[0.0, 0.0], [12.0, -30.0], [200.0, 150.0]] {
            let placed = [100.0 + p[0] * 1.5, 50.0 + p[1] * 1.5];
            assert_close(&[scaled.apply(p)], &[projection.apply(map.apply(placed))]);
        }
        assert_eq!(projection.transformed(&Affine::IDENTITY), projection);
    }

    #[test]
    fn transform_from_moves_and_tints_only_the_later_sprites() {
        let mut batch = SpriteBatch::new();
        batch.push(&sprite(1, Blend::Alpha));
        batch.push(&Sprite {
            colour: Color {
                r: 0.2,
                g: 1.0,
                b: 0.0,
                a: 0.5,
            },
            ..sprite(1, Blend::Alpha)
        });
        let map = Affine::about([0.0, 0.0], 1.0, 0.0, [10.0, 0.0]);
        let grey = 128.0 / 255.0;
        batch.transform_from(1, &map, [grey, grey, grey, 0.5]);
        assert_eq!(batch.vertices()[0].position, [0.0, 0.0]);
        assert_eq!(batch.vertices()[0].colour, [1.0; 4]);
        assert_eq!(batch.vertices()[4].position, [10.0, 0.0]);
        let [r, g, b, a] = batch.vertices()[4].colour;
        // In linear light: white times the grey is the grey, 0.2 goes darker than 0.2·grey.
        assert!((g - grey).abs() < 1e-5);
        assert!((r - 0.080).abs() < 0.002, "{r}");
        assert_eq!((b, a), (0.0, 0.25));
    }

    #[test]
    fn tint_srgb_with_white_changes_nothing() {
        let c = [0.3, 0.6, 0.9, 0.7];
        assert_eq!(tint_srgb(c, [1.0; 4]), c);
    }

    #[test]
    fn quad_corners_follow_position_size_and_uv() {
        let mut batch = SpriteBatch::new();
        let uv = UvRect {
            u0: 0.25,
            v0: 0.5,
            u1: 0.75,
            v1: 1.0,
        };
        batch.push(&Sprite {
            colour: RED,
            ..Sprite::new(TextureId(3), uv, [10.0, 20.0], [4.0, 2.0])
        });
        assert_eq!(
            positions(&batch),
            [[10.0, 20.0], [14.0, 20.0], [14.0, 22.0], [10.0, 22.0]]
        );
        let uvs: Vec<_> = batch.vertices().iter().map(|v| v.uv).collect();
        assert_eq!(uvs, [[0.25, 0.5], [0.75, 0.5], [0.75, 1.0], [0.25, 1.0]]);
        assert!(
            batch
                .vertices()
                .iter()
                .all(|v| v.colour == [1.0, 0.0, 0.0, 1.0])
        );
    }

    #[test]
    fn origin_is_placed_at_position() {
        let mut batch = SpriteBatch::new();
        batch.push(&Sprite {
            origin: [2.0, 1.0],
            ..Sprite::new(TextureId(0), UvRect::FULL, [10.0, 20.0], [4.0, 2.0])
        });
        assert_eq!(
            positions(&batch),
            [[8.0, 19.0], [12.0, 19.0], [12.0, 21.0], [8.0, 21.0]]
        );
    }

    #[test]
    fn rotates_clockwise_around_origin() {
        let mut batch = SpriteBatch::new();
        batch.push(&Sprite {
            origin: [2.0, 1.0],
            rotation: std::f32::consts::FRAC_PI_2,
            ..Sprite::new(TextureId(0), UvRect::FULL, [10.0, 20.0], [4.0, 2.0])
        });
        // A quarter turn clockwise (y down): the top edge becomes the right edge.
        assert_close(
            &positions(&batch),
            &[[11.0, 18.0], [11.0, 22.0], [9.0, 22.0], [9.0, 18.0]],
        );
    }

    #[test]
    fn merges_neighbours_and_keeps_order() {
        let mut batch = SpriteBatch::new();
        for s in [
            sprite(1, Blend::Alpha),
            sprite(1, Blend::Alpha),
            sprite(2, Blend::Alpha),
            sprite(2, Blend::Additive),
            sprite(2, Blend::Additive),
            sprite(1, Blend::Alpha),
        ] {
            batch.push(&s);
        }
        let b = |texture, blend, first_quad, quads| Batch {
            texture: TextureId(texture),
            blend,
            first_quad,
            quads,
        };
        assert_eq!(
            batch.batches(),
            [
                b(1, Blend::Alpha, 0, 2),
                b(2, Blend::Alpha, 2, 1),
                b(2, Blend::Additive, 3, 2),
                b(1, Blend::Alpha, 5, 1),
            ]
        );
        assert_eq!(batch.len(), 6);
        batch.clear();
        assert!(batch.is_empty());
        assert!(batch.batches().is_empty());
    }

    #[test]
    fn projection_maps_pixels_to_clip_space() {
        let p = Projection::pixels(Extent::new(200, 100));
        assert_eq!(p.apply([0.0, 0.0]), [-1.0, 1.0]);
        assert_eq!(p.apply([200.0, 100.0]), [1.0, -1.0]);
        assert_eq!(p.apply([100.0, 25.0]), [0.0, 0.5]);
    }

    #[test]
    fn scaled_projection_places_local_points() {
        let p = Projection::pixels(Extent::new(200, 100));
        let local = p.scaled(2.0, [50.0, 10.0]);
        assert_close(&[local.apply([0.0, 0.0])], &[p.apply([50.0, 10.0])]);
        assert_close(&[local.apply([25.0, 20.0])], &[p.apply([100.0, 50.0])]);
    }

    #[test]
    fn quad_indices_cover_u16_range() {
        let bytes = quad_indices(MAX_QUADS_PER_DRAW);
        let indices: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_ne_bytes(*c))
            .collect();
        assert_eq!(&indices[..12], &[0, 1, 2, 2, 3, 0, 4, 5, 6, 6, 7, 4]);
        assert_eq!(indices.iter().max(), Some(&u16::MAX));
        assert_eq!(indices.len() as u32, MAX_QUADS_PER_DRAW * 6);
    }

    /// Texture ids resolve to themselves times 10; id 0 is missing.
    fn resolve(id: TextureId) -> Option<&'static u32> {
        const TEXTURES: [u32; 4] = [0, 10, 20, 30];
        TEXTURES.get(id.0 as usize).filter(|t| **t != 0)
    }

    fn setup(slots: usize) -> (Fake, SpriteRenderer<Fake>) {
        let mut device = Fake::default();
        let renderer = SpriteRenderer::new(&mut device, slots).expect("renderer");
        (device, renderer)
    }

    const P: Projection = Projection([0.0; 16]);

    #[test]
    fn draws_each_batch_with_its_pipeline_and_texture() {
        let (mut d, mut r) = setup(3);
        let mut batch = SpriteBatch::new();
        for s in [
            sprite(1, Blend::Alpha),
            sprite(1, Blend::Alpha),
            sprite(2, Blend::Additive),
        ] {
            batch.push(&s);
        }
        let stats = r.draw(&mut d, &batch, &P, resolve).expect("draw");
        assert_eq!(
            stats,
            SpriteStats {
                sprites: 3,
                batches: 2,
                draw_calls: 2
            }
        );
        let Some(Call::Vertices { buffer: vb, .. }) = d.draws().get(2).copied() else {
            panic!("{:?}", d.log)
        };
        assert_eq!(
            d.draws(),
            [
                Call::Pipeline(Blend::Alpha),
                Call::Texture(10),
                Call::Vertices {
                    buffer: vb,
                    offset: 0
                },
                Call::DrawIndexed(12),
                Call::Pipeline(Blend::Additive),
                Call::Texture(20),
                Call::Vertices {
                    buffer: vb,
                    offset: 2 * 4 * 32
                },
                Call::DrawIndexed(6),
            ]
        );
        // One upload of all vertices, one of the projection.
        let writes: Vec<_> = d
            .log
            .iter()
            .filter_map(|c| match c {
                Call::Write { len, .. } => Some(*len),
                _ => None,
            })
            .collect();
        assert_eq!(writes, [3 * 4 * 32, 64]);
    }

    #[test]
    fn draws_ranges_of_one_upload() {
        let (mut d, mut r) = setup(1);
        let mut batch = SpriteBatch::new();
        for s in [
            sprite(1, Blend::Alpha),
            sprite(1, Blend::Alpha),
            sprite(1, Blend::Alpha),
            sprite(2, Blend::Additive),
        ] {
            batch.push(&s);
        }
        let uploaded = r.upload(&mut d, &batch, &P, resolve).expect("upload");
        let first = r
            .draw_range(&mut d, &uploaded, &batch, 0..2, resolve)
            .expect("first part");
        let rest = r
            .draw_range(&mut d, &uploaded, &batch, 2..4, resolve)
            .expect("second part");
        let empty = r
            .draw_range(&mut d, &uploaded, &batch, 4..9, resolve)
            .expect("past the end");
        assert_eq!(
            [first, rest, empty],
            [
                SpriteStats {
                    sprites: 2,
                    batches: 1,
                    draw_calls: 1
                },
                SpriteStats {
                    sprites: 2,
                    batches: 2,
                    draw_calls: 2
                },
                SpriteStats::default(),
            ]
        );
        let quad = 4 * 32;
        let draws: Vec<_> = d
            .draws()
            .into_iter()
            .filter_map(|c| match c {
                Call::Vertices { offset, .. } => Some(Call::Vertices { buffer: 0, offset }),
                Call::DrawIndexed(_) | Call::Texture(_) => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(
            draws,
            [
                Call::Texture(10),
                Call::Vertices {
                    buffer: 0,
                    offset: 0
                },
                Call::DrawIndexed(12),
                Call::Texture(10),
                Call::Vertices {
                    buffer: 0,
                    offset: 2 * quad
                },
                Call::DrawIndexed(6),
                Call::Texture(20),
                Call::Vertices {
                    buffer: 0,
                    offset: 3 * quad
                },
                Call::DrawIndexed(6),
            ]
        );
        // One slot for the whole batch: a second upload this frame has none left.
        assert!(matches!(
            r.upload(&mut d, &batch, &P, resolve),
            Err(SpriteError::OutOfSlots { slots: 1 })
        ));
    }

    #[test]
    fn splits_batches_past_the_u16_range() {
        let (mut d, mut r) = setup(1);
        let mut batch = SpriteBatch::new();
        let n = MAX_QUADS_PER_DRAW * 2 + 5;
        for _ in 0..n {
            batch.push(&sprite(1, Blend::Alpha));
        }
        let stats = r.draw(&mut d, &batch, &P, resolve).expect("draw");
        assert_eq!(stats.batches, 1);
        assert_eq!(stats.draw_calls, 3);
        let draws: Vec<_> = d
            .draws()
            .into_iter()
            .filter_map(|c| match c {
                Call::Vertices { offset, .. } => Some(Call::Vertices { buffer: 0, offset }),
                Call::DrawIndexed(_) => Some(c),
                _ => None,
            })
            .collect();
        let quad = 4 * 32;
        let max = u64::from(MAX_QUADS_PER_DRAW);
        assert_eq!(
            draws,
            [
                Call::Vertices {
                    buffer: 0,
                    offset: 0
                },
                Call::DrawIndexed(MAX_QUADS_PER_DRAW * 6),
                Call::Vertices {
                    buffer: 0,
                    offset: max * quad
                },
                Call::DrawIndexed(MAX_QUADS_PER_DRAW * 6),
                Call::Vertices {
                    buffer: 0,
                    offset: 2 * max * quad
                },
                Call::DrawIndexed(5 * 6),
            ]
        );
    }

    #[test]
    fn rotates_through_slots_and_grows_buffers() {
        let (mut d, mut r) = setup(2);
        let mut batch = SpriteBatch::new();
        batch.push(&sprite(1, Blend::Alpha));
        let vertex_buffer = |d: &Fake| match d.draws()[2] {
            Call::Vertices { buffer, .. } => buffer,
            c => panic!("{c:?}"),
        };
        let mut used = Vec::new();
        for _ in 0..4 {
            d.log.clear();
            r.begin_frame();
            r.draw(&mut d, &batch, &P, resolve).expect("draw");
            used.push(vertex_buffer(&d));
        }
        assert_ne!(used[0], used[1]);
        assert_eq!(used[0], used[2]);
        assert_eq!(used[1], used[3]);

        // More than the minimum buffer: the slot's buffer is replaced by a larger one.
        let quads = MIN_VERTEX_BUFFER / (4 * 32) + 1;
        for _ in 0..quads {
            batch.push(&sprite(1, Blend::Alpha));
        }
        d.log.clear();
        r.begin_frame();
        r.draw(&mut d, &batch, &P, resolve).expect("draw");
        let grown = vertex_buffer(&d);
        assert_ne!(grown, used[0]);
        assert!(!d.live_buffers.iter().any(|(id, _)| *id == used[0]));
        let size = d
            .live_buffers
            .iter()
            .find(|(id, _)| *id == grown)
            .unwrap()
            .1;
        assert_eq!(size, (2 * MIN_VERTEX_BUFFER).next_power_of_two());

        r.destroy(&mut d);
        assert!(d.is_empty(), "{:?}", d.live_buffers);
    }

    #[test]
    fn missing_texture_draws_nothing() {
        let (mut d, mut r) = setup(1);
        let mut batch = SpriteBatch::new();
        batch.push(&sprite(1, Blend::Alpha));
        batch.push(&sprite(0, Blend::Alpha));
        let err = r.draw(&mut d, &batch, &P, resolve).unwrap_err();
        assert!(matches!(err, SpriteError::MissingTexture(TextureId(0))));
        assert!(d.log.is_empty(), "{:?}", d.log);
        // An empty batch draws nothing and needs no textures.
        batch.clear();
        assert_eq!(
            r.draw(&mut d, &batch, &P, resolve).unwrap(),
            SpriteStats::default()
        );
        assert!(d.log.is_empty());
    }

    #[test]
    fn steady_frames_do_not_allocate() {
        let (mut d, mut r) = setup(3);
        d.log.reserve(10_000);
        let mut batch = SpriteBatch::new();
        let mut frame = |batch: &mut SpriteBatch, d: &mut Fake| {
            d.log.clear();
            r.begin_frame();
            batch.clear();
            for i in 0..500u32 {
                batch.push(&Sprite {
                    rotation: i as f32 * 0.01,
                    ..sprite(1 + i / 100 % 3, Blend::Alpha)
                });
            }
            r.draw(d, batch, &P, resolve).expect("draw")
        };
        // Warm up: grow the batch and every slot's buffer.
        for _ in 0..3 {
            frame(&mut batch, &mut d);
        }
        let (stats, allocs) = slam_testkit::count_allocs(|| {
            let mut total = SpriteStats::default();
            for _ in 0..100 {
                total += frame(&mut batch, &mut d);
            }
            total
        });
        assert_eq!(allocs.total(), 0, "steady frames allocated: {allocs}");
        assert_eq!(stats.sprites, 500 * 100);
        assert_eq!(stats.draw_calls, 5 * 100);
    }

    #[test]
    fn limits_draws_per_frame_to_the_slots() {
        let (mut d, mut r) = setup(2);
        let mut batch = SpriteBatch::new();
        batch.push(&sprite(1, Blend::Alpha));
        r.draw(&mut d, &batch, &P, resolve).expect("first");
        r.draw(&mut d, &batch, &P, resolve).expect("second");
        d.log.clear();
        let err = r.draw(&mut d, &batch, &P, resolve).unwrap_err();
        assert!(matches!(err, SpriteError::OutOfSlots { slots: 2 }));
        assert!(d.log.is_empty());
        r.begin_frame();
        r.draw(&mut d, &batch, &P, resolve).expect("next frame");
    }

    #[test]
    fn splits_exactly_at_the_limit_and_mid_recording() {
        let (mut d, mut r) = setup(1);
        let mut batch = SpriteBatch::new();
        batch.push(&sprite(2, Blend::Alpha));
        for _ in 0..MAX_QUADS_PER_DRAW {
            batch.push(&sprite(1, Blend::Alpha));
        }
        let stats = r.draw(&mut d, &batch, &P, resolve).expect("draw");
        assert_eq!((stats.batches, stats.draw_calls), (2, 2));
        batch.push(&sprite(1, Blend::Alpha));
        d.log.clear();
        r.begin_frame();
        let stats = r.draw(&mut d, &batch, &P, resolve).expect("draw");
        assert_eq!((stats.batches, stats.draw_calls), (2, 3));
        let offsets: Vec<_> = d
            .draws()
            .into_iter()
            .filter_map(|c| match c {
                Call::Vertices { offset, .. } => Some(offset / (4 * 32)),
                _ => None,
            })
            .collect();
        assert_eq!(offsets, [0, 1, 1 + u64::from(MAX_QUADS_PER_DRAW)]);
    }

    #[test]
    fn sprite_shaders_fit_the_sprite_pipelines() {
        for shader in [&shaders::SPRITE, &shaders::MSDF] {
            let mut d = Fake::default();
            let r = SpriteRenderer::with_shader(&mut d, 1, shader).expect("renderer");
            assert!(r.pipelines.iter().all(|p| p.shader == shader.name));
            // The batcher binds what `sprite.wgsl` declares, at the same slots.
            assert_eq!(shader.glsl.bindings, shaders::SPRITE.glsl.bindings);
            r.destroy(&mut d);
        }
    }

    #[test]
    fn failed_creation_frees_what_was_made() {
        // 5 pipelines, the index buffer and 3 uniform buffers.
        for fail_after in 0..9 {
            let mut d = Fake::failing_after(fail_after);
            assert!(SpriteRenderer::new(&mut d, 3).is_err());
            assert!(d.is_empty(), "fail after {fail_after}");
        }
        let mut d = Fake::failing_after(9);
        let mut r = SpriteRenderer::new(&mut d, 3).expect("renderer");
        // A failed buffer growth leaves the slot empty, to be made on the next draw.
        let mut batch = SpriteBatch::new();
        batch.push(&sprite(1, Blend::Alpha));
        assert!(matches!(
            r.draw(&mut d, &batch, &P, resolve),
            Err(SpriteError::Device(_))
        ));
        d.fail_after = None;
        r.begin_frame();
        r.draw(&mut d, &batch, &P, resolve).expect("draw");
        r.destroy(&mut d);
        assert!(d.is_empty());
    }
}
