//! Drawing [`SliderMesh`]es: GPU buffers per slider, the shared prepass target and
//! the two passes of ADR-0041.

use crate::hal::{
    Blend, BufferDesc, BufferKind, BufferUpdate, Color, DescError, Device, DeviceError, DrawError,
    Extent, IndexFormat, PipelineDesc, RenderTargetDesc, TextureFormat, VertexAttribute,
    VertexFormat, VertexLayout,
};
use crate::shaders;
use crate::sprite::{MAX_QUADS_PER_DRAW, Projection, quad_indices};

use super::{BodyUniforms, Bounds, SLIDER_LAYOUT, SliderGradient, SliderMesh, SliderVertex};

/// Unit quad corners of the composite pass, in the shared quad index order.
const UNIT_QUAD: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

const CORNER_LAYOUT: VertexLayout<'static> = VertexLayout {
    stride: 8,
    attributes: &[VertexAttribute {
        location: 0,
        format: VertexFormat::Float32x2,
        offset: 0,
    }],
};

/// A slider's mesh on the GPU, made by [`SliderRenderer::upload`]; free it with
/// [`SliderRenderer::release`].
pub struct SliderBuffer<D: Device> {
    vertices: D::Buffer,
    segments: u32,
    bounds: Bounds,
}

impl<D: Device> SliderBuffer<D> {
    pub fn segments(&self) -> u32 {
        self.segments
    }
}

/// How to draw one slider body this frame.
#[derive(Clone, Copy, Debug)]
pub struct SliderDraw<'a> {
    /// Path coordinates (osu! pixels, relative to the mesh's points) to clip space of
    /// the destination.
    pub projection: &'a Projection,
    /// Path radius in path coordinates.
    pub radius: f32,
    /// Snaking range: path length at the visible start and end.
    pub range: (f32, f32),
    /// Straight-alpha colour multiplying the body (fades).
    pub tint: [f32; 4],
    pub gradient: &'a SliderGradient,
}

#[derive(Debug, thiserror::Error)]
pub enum SliderError {
    #[error("more than {draws} slider draws this frame")]
    OutOfDraws { draws: usize },
    #[error("no prepass target: call begin_frame with a non-empty size first")]
    NoTarget,
    #[error("destination is {destination:?}, the prepass target {target:?}")]
    SizeMismatch { destination: Extent, target: Extent },
    #[error(transparent)]
    Draw(#[from] DrawError),
    #[error(transparent)]
    Desc(#[from] DescError),
    #[error(transparent)]
    Device(#[from] DeviceError),
}

/// Draws slider bodies (ADR-0041). Owns the pipelines, the prepass target (resized
/// in [`SliderRenderer::begin_frame`]) and a ring of uniform buffers, one per draw,
/// so no buffer is written twice in a frame. Drawing allocates nothing.
pub struct SliderRenderer<D: Device> {
    prepass: D::Pipeline,
    composite: D::Pipeline,
    indices: D::Buffer,
    quad: D::Buffer,
    uniforms: Vec<D::Buffer>,
    next: usize,
    /// Draws since `begin_frame`.
    draws: usize,
    /// Draws allowed per frame; half the ring, so a frame never reuses a buffer the
    /// previous frame wrote.
    per_frame: usize,
    target: Option<(D::RenderTarget, Extent)>,
    /// Uniforms of the current draw.
    scratch: BodyUniforms,
}

impl<D: Device> SliderRenderer<D> {
    /// Creates the pipelines and buffers for up to `per_frame` slider draws a frame;
    /// on failure, frees what was created. Each draw is one visible body; dense maps
    /// show dozens at once, so size it for the most a map can show (a uniform buffer
    /// of 640 bytes each, two per draw).
    pub fn new(device: &mut D, per_frame: usize) -> Result<Self, DeviceError> {
        let per_frame = per_frame.max(1);
        let mut pipelines = Vec::with_capacity(2);
        let mut buffers = Vec::with_capacity(2 + per_frame * 2);
        if let Err(e) = Self::create(device, per_frame * 2, &mut pipelines, &mut buffers) {
            pipelines
                .into_iter()
                .for_each(|p| device.destroy_pipeline(p));
            buffers.into_iter().for_each(|b| device.destroy_buffer(b));
            return Err(e);
        }
        let mut pipelines = pipelines.into_iter();
        let (Some(prepass), Some(composite)) = (pipelines.next(), pipelines.next()) else {
            unreachable!("both pipelines were created")
        };
        let mut buffers = buffers.into_iter();
        let (Some(indices), Some(quad)) = (buffers.next(), buffers.next()) else {
            unreachable!("index and quad buffers were created first")
        };
        Ok(Self {
            prepass,
            composite,
            indices,
            quad,
            uniforms: buffers.collect(),
            next: 0,
            draws: 0,
            per_frame,
            target: None,
            scratch: BodyUniforms {
                projection: [0.0; 16],
                path: [0.0; 4],
                bounds: [0.0; 4],
                tint: [0.0; 4],
                gradient: [0.0; 4],
                texels: [0; super::MAX_GRADIENT_TEXELS],
            },
        })
    }

    fn create(
        device: &mut D,
        ring: usize,
        pipelines: &mut Vec<D::Pipeline>,
        buffers: &mut Vec<D::Buffer>,
    ) -> Result<(), DeviceError> {
        pipelines.push(device.create_pipeline(&PipelineDesc {
            shader: &shaders::SLIDER_PREPASS,
            layout: SLIDER_LAYOUT,
            blend: Blend::Max,
        })?);
        pipelines.push(device.create_pipeline(&PipelineDesc {
            shader: &shaders::SLIDER_BODY,
            layout: CORNER_LAYOUT,
            blend: Blend::Alpha,
        })?);
        let indices = quad_indices(MAX_QUADS_PER_DRAW);
        buffers.push(device.create_buffer(
            &BufferDesc {
                kind: BufferKind::Index,
                size: indices.len() as u64,
                update: BufferUpdate::Static,
            },
            Some(&indices),
        )?);
        let quad: Vec<u8> = UNIT_QUAD
            .iter()
            .flatten()
            .flat_map(|f| f.to_ne_bytes())
            .collect();
        buffers.push(device.create_buffer(
            &BufferDesc {
                kind: BufferKind::Vertex,
                size: quad.len() as u64,
                update: BufferUpdate::Static,
            },
            Some(&quad),
        )?);
        for _ in 0..ring {
            buffers.push(device.create_buffer(
                &BufferDesc {
                    kind: BufferKind::Uniform,
                    size: size_of::<BodyUniforms>() as u64,
                    update: BufferUpdate::Dynamic,
                },
                None,
            )?);
        }
        Ok(())
    }

    /// Puts a mesh on the GPU (at load, not in the frame); `None` for an empty mesh.
    pub fn upload(
        &mut self,
        device: &mut D,
        mesh: &SliderMesh,
    ) -> Result<Option<SliderBuffer<D>>, DeviceError> {
        let Some(bounds) = mesh.bounds() else {
            return Ok(None);
        };
        let vertices = mesh.vertices();
        // SAFETY: `SliderVertex` is `repr(C)` of `f32` arrays only, without padding
        // (32 bytes, asserted in the parent module), so its memory is initialised
        // bytes; `u8` has no alignment requirement.
        let bytes = unsafe {
            std::slice::from_raw_parts(vertices.as_ptr().cast::<u8>(), size_of_val(vertices))
        };
        let buffer = device.create_buffer(
            &BufferDesc {
                kind: BufferKind::Vertex,
                size: bytes.len() as u64,
                update: BufferUpdate::Static,
            },
            Some(bytes),
        )?;
        Ok(Some(SliderBuffer {
            vertices: buffer,
            segments: mesh.segments() as u32,
            bounds,
        }))
    }

    pub fn release(&mut self, device: &mut D, buffer: SliderBuffer<D>) {
        device.destroy_buffer(buffer.vertices);
    }

    /// Starts a frame whose destination is `size` pixels: resets the draw count and
    /// makes the prepass target that size (recreated only when the size changes; an
    /// empty size drops it).
    pub fn begin_frame(&mut self, device: &mut D, size: Extent) -> Result<(), DeviceError> {
        self.draws = 0;
        if self.target.as_ref().is_some_and(|(_, s)| *s == size) {
            return Ok(());
        }
        if let Some((target, _)) = self.target.take() {
            device.destroy_render_target(target);
        }
        if !size.is_empty() {
            let target = device.create_render_target(&RenderTargetDesc {
                size,
                format: TextureFormat::R16Float,
            })?;
            self.target = Some((target, size));
        }
        Ok(())
    }

    /// Draws one body into `destination` (the surface if `None`; it must be the
    /// size given to `begin_frame`), which stays the current pass afterwards. Changes
    /// the bound pipeline, buffers and texture slot 0.
    pub fn draw(
        &mut self,
        device: &mut D,
        buffer: &SliderBuffer<D>,
        body: &SliderDraw<'_>,
        destination: Option<&D::RenderTarget>,
    ) -> Result<(), SliderError> {
        let Some((target, target_size)) = &self.target else {
            return Err(SliderError::NoTarget);
        };
        // A zero radius draws nothing (`PathDrawNode.Draw`).
        if body.radius <= 0.0 || body.radius.is_nan() {
            return Ok(());
        }
        if self.draws == self.per_frame {
            return Err(SliderError::OutOfDraws {
                draws: self.per_frame,
            });
        }
        let uniforms = &self.uniforms[self.next];
        self.next = (self.next + 1) % self.uniforms.len();
        self.draws += 1;

        let bounds = buffer.bounds.inflate(body.radius);
        let s = &mut self.scratch;
        s.projection = body.projection.0;
        s.path = [body.radius, body.range.0, body.range.1, 0.0];
        s.bounds = [bounds.min[0], bounds.min[1], bounds.max[0], bounds.max[1]];
        s.tint = body.tint;
        s.gradient = [body.gradient.width() as f32, 0.0, 0.0, 0.0];
        s.texels = body.gradient.texels;
        device.write_buffer(uniforms, 0, s.bytes())?;

        device.begin_pass(Some(target), Some(Color::TRANSPARENT));
        device.set_pipeline(&self.prepass);
        device.set_uniform_buffer(0, uniforms)?;
        device.set_index_buffer(&self.indices, IndexFormat::U16)?;
        let stride = size_of::<SliderVertex>() as u64;
        let mut first = 0;
        while first < buffer.segments {
            let quads = (buffer.segments - first).min(MAX_QUADS_PER_DRAW);
            device.set_vertex_buffer(&buffer.vertices, u64::from(first) * 4 * stride)?;
            device.draw_indexed(0..quads * 6)?;
            first += quads;
        }

        let destination_size = device.begin_pass(destination, None);
        if destination_size != *target_size {
            // The composite maps the destination's pixels onto the target's one to
            // one (the window was resized during the frame, or a wrong destination).
            return Err(SliderError::SizeMismatch {
                destination: destination_size,
                target: *target_size,
            });
        }
        device.set_pipeline(&self.composite);
        device.set_uniform_buffer(0, uniforms)?;
        let prepass = device.render_target_texture(target);
        device.set_texture(0, prepass)?;
        device.set_vertex_buffer(&self.quad, 0)?;
        device.draw_indexed(0..6)?;
        Ok(())
    }

    /// Frees every GPU resource.
    pub fn destroy(self, device: &mut D) {
        device.destroy_pipeline(self.prepass);
        device.destroy_pipeline(self.composite);
        device.destroy_buffer(self.indices);
        device.destroy_buffer(self.quad);
        self.uniforms
            .into_iter()
            .for_each(|b| device.destroy_buffer(b));
        if let Some((target, _)) = self.target {
            device.destroy_render_target(target);
        }
    }
}
