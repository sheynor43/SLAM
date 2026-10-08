//! Sprite batcher (ADR-0020). A [`SpriteBatch`] records a frame's sprites on the CPU,
//! in draw order, merging neighbours that share a texture and blending into one batch;
//! a [`SpriteRenderer`] uploads the recording once and draws each batch. Neither
//! allocates once its buffers have grown to the frame's size.

use std::mem::size_of;

use crate::atlas::UvRect;
use crate::hal::{
    Blend, BufferDesc, BufferKind, BufferUpdate, Color, DescError, Device, DeviceError, DrawError,
    Extent, IndexFormat, PipelineDesc, VertexAttribute, VertexFormat, VertexLayout,
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

    /// Room for `quads` sprites before the first frame grows the buffers.
    pub fn with_capacity(quads: usize) -> Self {
        Self {
            vertices: Vec::with_capacity(quads * 4),
            batches: Vec::with_capacity(quads.min(64)),
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

    /// Records `sprite` on top of those recorded before.
    pub fn push(&mut self, sprite: &Sprite) {
        // A u32 quad index overflows only past 2^32 sprites (128 GiB of vertices).
        let quad = self.len() as u32;
        match self.batches.last_mut() {
            Some(last) if last.texture == sprite.texture && last.blend == sprite.blend => {
                last.quads += 1;
            }
            _ => self.batches.push(Batch {
                texture: sprite.texture,
                blend: sprite.blend,
                first_quad: quad,
                quads: 1,
            }),
        }
        let [w, h] = sprite.size;
        let [ox, oy] = sprite.origin;
        let [px, py] = sprite.position;
        let (sin, cos) = sprite.rotation.sin_cos();
        let Color { r, g, b, a } = sprite.colour;
        let UvRect { u0, v0, u1, v1 } = sprite.uv;
        let corner = |x: f32, y: f32, u: f32, v: f32| {
            let (x, y) = (x - ox, y - oy);
            SpriteVertex {
                position: [px + x * cos - y * sin, py + x * sin + y * cos],
                colour: [r, g, b, a],
                uv: [u, v],
            }
        };
        self.vertices.extend_from_slice(&[
            corner(0.0, 0.0, u0, v0),
            corner(w, 0.0, u1, v0),
            corner(w, h, u1, v1),
            corner(0.0, h, u0, v1),
        ]);
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

/// Draws [`SpriteBatch`]es with `shaders/sprite.wgsl`. Each draw uses the next of a
/// ring of `slots` buffer sets, continuing across frames (ADR-0020). At most `slots`
/// draws are allowed between [`SpriteRenderer::begin_frame`] calls, so no buffer is
/// written twice in a frame (the `write_buffer` contract); spare slots space out
/// rewrites of a buffer the GPU may still be reading from an earlier frame.
pub struct SpriteRenderer<D: Device> {
    /// Indexed by [`blend_index`].
    pipelines: [D::Pipeline; 4],
    indices: D::Buffer,
    slots: Vec<Slot<D::Buffer>>,
    next: usize,
    /// Draws since `begin_frame`.
    draws: usize,
}

const fn blend_index(blend: Blend) -> usize {
    match blend {
        Blend::Replace => 0,
        Blend::Alpha => 1,
        Blend::Premultiplied => 2,
        Blend::Additive => 3,
    }
}

/// `u16` indices of two triangles per quad (corners 0-1-2 and 2-3-0).
fn quad_indices(quads: u32) -> Vec<u8> {
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
        let mut pipelines = Vec::with_capacity(4);
        let mut buffers = Vec::with_capacity(slots.max(1) + 1);
        match Self::create(device, slots.max(1), &mut pipelines, &mut buffers) {
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
        let Ok(pipelines) = <[D::Pipeline; 4]>::try_from(pipelines) else {
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
        slots: usize,
        pipelines: &mut Vec<D::Pipeline>,
        buffers: &mut Vec<D::Buffer>,
    ) -> Result<(), DeviceError> {
        for blend in [
            Blend::Replace,
            Blend::Alpha,
            Blend::Premultiplied,
            Blend::Additive,
        ] {
            debug_assert_eq!(blend_index(blend), pipelines.len());
            pipelines.push(device.create_pipeline(&PipelineDesc {
                shader: &shaders::SPRITE,
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
        if batch.is_empty() {
            return Ok(SpriteStats::default());
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
        device.set_uniform_buffer(0, &slot.globals)?;
        device.set_index_buffer(&self.indices, IndexFormat::U16)?;

        let mut stats = SpriteStats {
            sprites: batch.len() as u32,
            batches: batch.batches.len() as u32,
            draw_calls: 0,
        };
        let stride = u64::from(SPRITE_LAYOUT.stride);
        for b in &batch.batches {
            device.set_pipeline(&self.pipelines[blend_index(b.blend)]);
            let tex = texture(b.texture).ok_or(SpriteError::MissingTexture(b.texture))?;
            device.set_texture(0, tex)?;
            let mut first = b.first_quad;
            let end = b.first_quad + b.quads;
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
    fn failed_creation_frees_what_was_made() {
        // 4 pipelines, the index buffer and 3 uniform buffers.
        for fail_after in 0..8 {
            let mut d = Fake::failing_after(fail_after);
            assert!(SpriteRenderer::new(&mut d, 3).is_err());
            assert!(d.is_empty(), "fail after {fail_after}");
        }
        let mut d = Fake::failing_after(8);
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
