//! Backend-independent render API: resource descriptions, their validation and the
//! device trait every backend implements. Only what M0 needs; grows with use.

use std::ops::{BitOr, Range};

/// RGBA colour, components in `0.0..=1.0`, written to the target as is: on the
/// default framebuffer (no sRGB conversion) the values are shown as sRGB-encoded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }
}

/// Size in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

impl Extent {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFormat {
    /// 8-bit RGBA, linear.
    Rgba8,
    /// 8-bit RGBA, sRGB-encoded colour, linear alpha.
    Rgba8Srgb,
    /// 8-bit single channel (masks, glyph coverage).
    R8,
}

impl TextureFormat {
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba8 | Self::Rgba8Srgb => 4,
            Self::R8 => 1,
        }
    }
}

/// How a texture is used. Combine with `|`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextureUsage(u8);

impl TextureUsage {
    /// Read by shaders.
    pub const SAMPLED: Self = Self(1);
    /// Drawn into as a render target.
    pub const RENDER_TARGET: Self = Self(1 << 1);
    /// Updated after creation.
    pub const COPY_DST: Self = Self(1 << 2);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl BitOr for TextureUsage {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureDesc {
    pub size: Extent,
    pub format: TextureFormat,
    pub usage: TextureUsage,
}

impl TextureDesc {
    /// Bytes of tightly packed pixel data for the whole texture, saturating at
    /// `u64::MAX`.
    pub const fn byte_len(&self) -> u64 {
        (self.size.width as u64 * self.size.height as u64)
            .saturating_mul(self.format.bytes_per_pixel() as u64)
    }

    /// Checks the description against device limits and, if given, the length of the
    /// initial pixel data (tightly packed rows, top row first).
    pub fn validate(&self, limits: &Limits, data: Option<&[u8]>) -> Result<(), DescError> {
        let Extent { width, height } = self.size;
        if self.size.is_empty() {
            return Err(DescError::EmptySize);
        }
        if width > limits.max_texture_size || height > limits.max_texture_size {
            return Err(DescError::TooLarge {
                width,
                height,
                max: limits.max_texture_size,
            });
        }
        if !self
            .usage
            .intersects(TextureUsage::SAMPLED | TextureUsage::RENDER_TARGET)
        {
            return Err(DescError::Unusable);
        }
        if let Some(data) = data {
            let expected = self.byte_len();
            if data.len() as u64 != expected {
                return Err(DescError::DataSize {
                    expected,
                    actual: data.len() as u64,
                });
            }
        }
        Ok(())
    }
}

/// Colour target to render into; its texture can then be sampled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderTargetDesc {
    pub size: Extent,
    pub format: TextureFormat,
}

impl RenderTargetDesc {
    /// The colour texture behind the target.
    pub const fn texture_desc(&self) -> TextureDesc {
        TextureDesc {
            size: self.size,
            format: self.format,
            usage: TextureUsage(TextureUsage::SAMPLED.0 | TextureUsage::RENDER_TARGET.0),
        }
    }

    /// sRGB targets are rejected: OpenGL would store unencoded values that read back
    /// decoded, unlike other APIs.
    pub fn validate(&self, limits: &Limits) -> Result<(), DescError> {
        if self.format == TextureFormat::Rgba8Srgb {
            return Err(DescError::TargetFormat {
                format: self.format,
            });
        }
        self.texture_desc().validate(limits, None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferKind {
    Vertex,
    /// Indices, `u16` or `u32` (chosen when bound, see [`IndexFormat`]).
    Index,
    /// A std140 uniform block.
    Uniform,
}

/// How often the contents change; a hint for the driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferUpdate {
    /// Written once, drawn many times.
    Static,
    /// Rewritten often, up to every frame.
    Dynamic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferDesc {
    pub kind: BufferKind,
    /// Size in bytes.
    pub size: u64,
    pub update: BufferUpdate,
}

impl BufferDesc {
    /// Checks the description against device limits and, if given, the length of the
    /// initial data (must fill the whole buffer).
    pub fn validate(&self, limits: &Limits, data: Option<&[u8]>) -> Result<(), DescError> {
        if self.size == 0 {
            return Err(DescError::EmptyBuffer);
        }
        if self.kind == BufferKind::Uniform {
            if self.size > u64::from(limits.max_uniform_block_size) {
                return Err(DescError::UniformTooLarge {
                    size: self.size,
                    max: limits.max_uniform_block_size,
                });
            }
            if !self.size.is_multiple_of(UNIFORM_ALIGNMENT) {
                return Err(DescError::UniformSize { size: self.size });
            }
        }
        if let Some(data) = data
            && data.len() as u64 != self.size
        {
            return Err(DescError::DataSize {
                expected: self.size,
                actual: data.len() as u64,
            });
        }
        Ok(())
    }

    /// Checks that `len` bytes written at `offset` stay inside the buffer.
    pub fn check_write(&self, offset: u64, len: usize) -> Result<(), DescError> {
        match offset.checked_add(len as u64) {
            Some(end) if end <= self.size => Ok(()),
            _ => Err(DescError::WriteOutOfBounds {
                offset,
                len: len as u64,
                size: self.size,
            }),
        }
    }
}

/// Uniform buffer sizes are a multiple of this (std140 rounds blocks up to a vec4).
pub const UNIFORM_ALIGNMENT: u64 = 16;

/// Vertex attributes per pipeline; the OpenGL 3.3 minimum.
pub const MAX_VERTEX_ATTRIBUTES: usize = 16;

/// Largest vertex stride in bytes (the D3D11 limit and the Vulkan minimum).
pub const MAX_VERTEX_STRIDE: u32 = 2048;

/// Vertex attribute offsets and strides are a multiple of this.
pub const VERTEX_ALIGNMENT: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VertexFormat {
    Float32x2,
    Float32x3,
    Float32x4,
    /// Four bytes read as floats in `0.0..=1.0` (packed colours).
    Unorm8x4,
}

impl VertexFormat {
    pub const fn size(self) -> u32 {
        match self {
            Self::Float32x2 => 8,
            Self::Float32x3 => 12,
            Self::Float32x4 => 16,
            Self::Unorm8x4 => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexAttribute {
    /// Shader input location (`layout(location = N)`).
    pub location: u32,
    pub format: VertexFormat,
    /// Byte offset inside a vertex.
    pub offset: u32,
}

/// Interleaved vertices from one buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexLayout<'a> {
    /// Bytes from one vertex to the next.
    pub stride: u32,
    pub attributes: &'a [VertexAttribute],
}

impl VertexLayout<'_> {
    /// Attributes must be 4-byte aligned, fit in the stride, not overlap and use
    /// distinct locations below [`MAX_VERTEX_ATTRIBUTES`].
    pub fn validate(&self) -> Result<(), DescError> {
        if self.stride == 0
            || self.stride > MAX_VERTEX_STRIDE
            || !self.stride.is_multiple_of(VERTEX_ALIGNMENT)
        {
            return Err(DescError::VertexStride {
                stride: self.stride,
            });
        }
        if self.attributes.is_empty() || self.attributes.len() > MAX_VERTEX_ATTRIBUTES {
            return Err(DescError::AttributeCount {
                count: self.attributes.len(),
            });
        }
        for (i, a) in self.attributes.iter().enumerate() {
            if a.location as usize >= MAX_VERTEX_ATTRIBUTES {
                return Err(DescError::AttributeLocation {
                    location: a.location,
                });
            }
            let end = u64::from(a.offset) + u64::from(a.format.size());
            if !a.offset.is_multiple_of(VERTEX_ALIGNMENT) || end > u64::from(self.stride) {
                return Err(DescError::AttributeOffset {
                    location: a.location,
                    offset: a.offset,
                });
            }
            for b in &self.attributes[..i] {
                if b.location == a.location {
                    return Err(DescError::AttributeLocation {
                        location: a.location,
                    });
                }
                let b_end = u64::from(b.offset) + u64::from(b.format.size());
                if u64::from(a.offset) < b_end && u64::from(b.offset) < end {
                    return Err(DescError::AttributeOverlap {
                        first: b.location,
                        second: a.location,
                    });
                }
            }
        }
        Ok(())
    }
}

/// How fragment output is combined with the target. `src` is the shader output,
/// `dst` the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    /// `src` replaces `dst`.
    Replace,
    /// Straight alpha: `src.rgb * src.a + dst.rgb * (1 - src.a)`; alpha
    /// `src.a + dst.a`, as osu-framework's `BlendingParameters.Mixture`.
    Alpha,
    /// Premultiplied alpha: `src + dst * (1 - src.a)`.
    Premultiplied,
    /// `src.rgb * src.a + dst.rgb`; alpha `src.a + dst.a`.
    Additive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingKind {
    UniformBuffer,
    Texture,
}

/// Connects a shader resource, by name, to a slot that buffers and textures are bound
/// to with [`Device::set_uniform_buffer`] and [`Device::set_texture`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding<'a> {
    /// Uniform block or sampler name in the shader.
    pub name: &'a str,
    pub kind: BindingKind,
    pub slot: u32,
}

/// Shaders and fixed-function state for drawing triangle lists. Shaders are GLSL
/// 3.30 core for now; translation through `naga` comes later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipelineDesc<'a> {
    pub vertex_shader: &'a str,
    pub fragment_shader: &'a str,
    pub layout: VertexLayout<'a>,
    pub bindings: &'a [Binding<'a>],
    pub blend: Blend,
}

impl PipelineDesc<'_> {
    pub fn validate(&self, limits: &Limits) -> Result<(), DescError> {
        if self.vertex_shader.trim().is_empty() || self.fragment_shader.trim().is_empty() {
            return Err(DescError::EmptyShader);
        }
        self.layout.validate()?;
        for (i, b) in self.bindings.iter().enumerate() {
            let max = match b.kind {
                BindingKind::UniformBuffer => limits.max_uniform_buffer_slots,
                BindingKind::Texture => limits.max_texture_slots,
            };
            if b.slot >= max {
                return Err(DescError::BindingSlot {
                    kind: b.kind,
                    slot: b.slot,
                    max,
                });
            }
            let earlier = &self.bindings[..i];
            if b.name.is_empty() || earlier.iter().any(|e| e.name == b.name) {
                return Err(DescError::BindingName);
            }
            if earlier.iter().any(|e| e.kind == b.kind && e.slot == b.slot) {
                return Err(DescError::BindingSlotTaken {
                    kind: b.kind,
                    slot: b.slot,
                });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexFormat {
    U16,
    U32,
}

impl IndexFormat {
    pub const fn size(self) -> u32 {
        match self {
            Self::U16 => 2,
            Self::U32 => 4,
        }
    }
}

/// Device limits that descriptions are validated against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_texture_size: u32,
    /// Bytes.
    pub max_uniform_block_size: u32,
    pub max_uniform_buffer_slots: u32,
    pub max_texture_slots: u32,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DescError {
    #[error("texture has zero size")]
    EmptySize,
    #[error("texture size {width}x{height} exceeds the device limit {max}")]
    TooLarge { width: u32, height: u32, max: u32 },
    #[error("texture usage includes neither SAMPLED nor RENDER_TARGET")]
    Unusable,
    #[error("initial data is {actual} bytes, expected {expected}")]
    DataSize { expected: u64, actual: u64 },
    #[error("{format:?} cannot be a render target")]
    TargetFormat { format: TextureFormat },
    #[error("buffer has zero size")]
    EmptyBuffer,
    #[error("uniform buffer of {size} bytes exceeds the device limit {max}")]
    UniformTooLarge { size: u64, max: u32 },
    #[error("uniform buffer size {size} is not a multiple of 16")]
    UniformSize { size: u64 },
    #[error("write of {len} bytes at {offset} is outside the {size}-byte buffer")]
    WriteOutOfBounds { offset: u64, len: u64, size: u64 },
    #[error("vertex stride {stride} is zero, over 2048 or not a multiple of 4")]
    VertexStride { stride: u32 },
    #[error("{count} vertex attributes, expected 1..=16")]
    AttributeCount { count: usize },
    #[error("vertex attribute location {location} is out of range or repeated")]
    AttributeLocation { location: u32 },
    #[error("vertex attribute {location} at offset {offset} is unaligned or past the stride")]
    AttributeOffset { location: u32, offset: u32 },
    #[error("vertex attributes {first} and {second} overlap")]
    AttributeOverlap { first: u32, second: u32 },
    #[error("shader source is empty")]
    EmptyShader,
    #[error("binding name is empty or repeated")]
    BindingName,
    #[error("{kind:?} slot {slot} exceeds the device limit {max}")]
    BindingSlot {
        kind: BindingKind,
        slot: u32,
        max: u32,
    },
    #[error("{kind:?} slot {slot} is bound twice")]
    BindingSlotTaken { kind: BindingKind, slot: u32 },
}

/// A draw-time mistake. Reported without allocating; the call has no effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DrawError {
    #[error("no pipeline set")]
    NoPipeline,
    #[error("no vertex buffer set")]
    NoVertexBuffer,
    #[error("no index buffer set")]
    NoIndexBuffer,
    #[error("buffer bound as {expected:?} was created as {actual:?}")]
    BufferKind {
        expected: BufferKind,
        actual: BufferKind,
    },
    #[error("texture is not SAMPLED")]
    NotSampled,
    #[error("slot {slot} exceeds the device limit {max}")]
    Slot { slot: u32, max: u32 },
    #[error("vertex buffer offset {offset} is not a multiple of 4")]
    Unaligned { offset: u64 },
    #[error("vertex buffer offset {offset} is past the end of the {size}-byte buffer")]
    OffsetPastEnd { offset: u64, size: u64 },
    #[error("range {start}..{end} is reversed or reads past the bound buffer")]
    OutOfRange { start: u32, end: u32 },
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error(transparent)]
    Desc(#[from] DescError),
    #[error("surface: {0}")]
    Surface(String),
    #[error("unsupported device: {0}")]
    Unsupported(String),
    #[error("shader: {0}")]
    Shader(String),
    #[error("backend: {0}")]
    Backend(String),
}

/// A render device bound to one surface. Used by the draw thread only.
///
/// Frame commands (`begin_frame`, `begin_pass`, `write_buffer`, the `set_*` calls,
/// `draw*` and `present`) are the hot path: they do not allocate, and report mistakes
/// with allocation-free errors (except `present`, whose surface errors carry text).
/// Draw state (pipeline, buffers, textures) persists across passes and frames until
/// replaced; a destroyed resource is unbound.
///
/// Coordinates follow OpenGL for now: clip-space y points up; the first uploaded
/// texture row is at v = 0; a render target's texture has v = 0 at the bottom of what
/// was drawn. Other backends flip to match (to be settled with shader translation).
pub trait Device {
    type Texture;
    type Buffer;
    type Pipeline;
    type RenderTarget;

    fn limits(&self) -> Limits;

    /// Creates a texture, optionally filled with `data` (see [`TextureDesc::validate`]).
    fn create_texture(
        &mut self,
        desc: &TextureDesc,
        data: Option<&[u8]>,
    ) -> Result<Self::Texture, DeviceError>;

    fn destroy_texture(&mut self, texture: Self::Texture);

    /// Creates a buffer filled from `data`, or with undefined contents (see
    /// [`BufferDesc::validate`]).
    fn create_buffer(
        &mut self,
        desc: &BufferDesc,
        data: Option<&[u8]>,
    ) -> Result<Self::Buffer, DeviceError>;

    fn destroy_buffer(&mut self, buffer: Self::Buffer);

    /// Overwrites `data.len()` bytes at `offset`.
    ///
    /// Portable use: write a region at most once per frame, before the first draw
    /// that reads it, and write uniform buffers whole. OpenGL also keeps the order of
    /// later writes; other backends are not required to.
    fn write_buffer(
        &mut self,
        buffer: &Self::Buffer,
        offset: u64,
        data: &[u8],
    ) -> Result<(), DescError>;

    /// Compiles and links the shaders; compiler output is in the error.
    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<Self::Pipeline, DeviceError>;

    fn destroy_pipeline(&mut self, pipeline: Self::Pipeline);

    fn create_render_target(
        &mut self,
        desc: &RenderTargetDesc,
    ) -> Result<Self::RenderTarget, DeviceError>;

    /// Destroys the target and its texture.
    fn destroy_render_target(&mut self, target: Self::RenderTarget);

    /// The target's colour texture, for sampling in a later pass. Sampling it while
    /// drawing into it is undefined.
    fn render_target_texture<'a>(&self, target: &'a Self::RenderTarget) -> &'a Self::Texture;

    /// Starts a frame: targets the surface, sets the viewport to its current size and
    /// clears it. Returns that size; it is empty while the window is minimised.
    fn begin_frame(&mut self, clear: Color) -> Extent;

    /// Switches drawing to `target` (the surface if `None`) with the viewport covering
    /// it, clearing it if `clear` is set. Returns the target's size.
    fn begin_pass(&mut self, target: Option<&Self::RenderTarget>, clear: Option<Color>) -> Extent;

    fn set_pipeline(&mut self, pipeline: &Self::Pipeline);

    /// Vertices are read from `offset` bytes into `buffer` (a multiple of 4) with the
    /// current pipeline's layout.
    fn set_vertex_buffer(&mut self, buffer: &Self::Buffer, offset: u64) -> Result<(), DrawError>;

    fn set_index_buffer(
        &mut self,
        buffer: &Self::Buffer,
        format: IndexFormat,
    ) -> Result<(), DrawError>;

    fn set_uniform_buffer(&mut self, slot: u32, buffer: &Self::Buffer) -> Result<(), DrawError>;

    fn set_texture(&mut self, slot: u32, texture: &Self::Texture) -> Result<(), DrawError>;

    /// Draws a triangle list from `vertices` of the vertex buffer. Every vertex in the
    /// range must fit whole, `stride` bytes, inside the buffer.
    fn draw(&mut self, vertices: Range<u32>) -> Result<(), DrawError>;

    /// Draws a triangle list from `indices` of the index buffer. Index values are not
    /// checked: the caller guarantees they address vertices inside the vertex buffer
    /// (out-of-range values are undefined behaviour on the GPU).
    fn draw_indexed(&mut self, indices: Range<u32>) -> Result<(), DrawError>;

    /// Shows the frame without waiting for vertical sync.
    fn present(&mut self) -> Result<(), DeviceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        max_texture_size: 4096,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };

    fn desc(width: u32, height: u32, format: TextureFormat) -> TextureDesc {
        TextureDesc {
            size: Extent::new(width, height),
            format,
            usage: TextureUsage::SAMPLED,
        }
    }

    #[test]
    fn accepts_valid_texture() {
        let d = desc(256, 128, TextureFormat::Rgba8);
        assert_eq!(d.validate(&LIMITS, None), Ok(()));
        assert_eq!(d.validate(&LIMITS, Some(&vec![0; 256 * 128 * 4])), Ok(()));
        let max = desc(4096, 4096, TextureFormat::R8);
        assert_eq!(max.validate(&LIMITS, None), Ok(()));
    }

    #[test]
    fn rejects_empty_size() {
        for (w, h) in [(0, 1), (1, 0), (0, 0)] {
            let d = desc(w, h, TextureFormat::Rgba8);
            assert_eq!(d.validate(&LIMITS, None), Err(DescError::EmptySize));
        }
    }

    #[test]
    fn rejects_size_over_limit() {
        for (w, h) in [(4097, 1), (1, 4097)] {
            let d = desc(w, h, TextureFormat::Rgba8);
            assert_eq!(
                d.validate(&LIMITS, None),
                Err(DescError::TooLarge {
                    width: w,
                    height: h,
                    max: 4096
                })
            );
        }
    }

    #[test]
    fn requires_sampled_or_render_target() {
        let mut d = desc(4, 4, TextureFormat::Rgba8);
        d.usage = TextureUsage::COPY_DST;
        assert_eq!(d.validate(&LIMITS, None), Err(DescError::Unusable));
        d.usage = TextureUsage::default();
        assert_eq!(d.validate(&LIMITS, None), Err(DescError::Unusable));
        d.usage = TextureUsage::RENDER_TARGET | TextureUsage::COPY_DST;
        assert_eq!(d.validate(&LIMITS, None), Ok(()));
    }

    #[test]
    fn checks_initial_data_length() {
        let d = desc(3, 2, TextureFormat::R8);
        assert_eq!(d.byte_len(), 6);
        assert_eq!(
            d.validate(&LIMITS, Some(&[0; 5])),
            Err(DescError::DataSize {
                expected: 6,
                actual: 5
            })
        );
        let d = desc(3, 2, TextureFormat::Rgba8Srgb);
        assert_eq!(
            d.validate(&LIMITS, Some(&[0; 6])),
            Err(DescError::DataSize {
                expected: 24,
                actual: 6
            })
        );
    }

    #[test]
    fn byte_len_saturates() {
        let d = desc(u32::MAX, u32::MAX, TextureFormat::Rgba8);
        assert_eq!(d.byte_len(), u64::MAX);
    }

    #[test]
    fn usage_flags_combine() {
        let u = TextureUsage::SAMPLED | TextureUsage::COPY_DST;
        assert!(u.contains(TextureUsage::SAMPLED));
        assert!(u.contains(TextureUsage::COPY_DST));
        assert!(!u.contains(TextureUsage::RENDER_TARGET));
        assert!(!u.contains(TextureUsage::SAMPLED | TextureUsage::RENDER_TARGET));
    }

    fn buffer(kind: BufferKind, size: u64) -> BufferDesc {
        BufferDesc {
            kind,
            size,
            update: BufferUpdate::Static,
        }
    }

    #[test]
    fn validates_buffers() {
        assert_eq!(
            buffer(BufferKind::Vertex, 12).validate(&LIMITS, None),
            Ok(())
        );
        assert_eq!(
            buffer(BufferKind::Index, 6).validate(&LIMITS, Some(&[0; 6])),
            Ok(())
        );
        assert_eq!(
            buffer(BufferKind::Vertex, 0).validate(&LIMITS, None),
            Err(DescError::EmptyBuffer)
        );
        assert_eq!(
            buffer(BufferKind::Vertex, 8).validate(&LIMITS, Some(&[0; 7])),
            Err(DescError::DataSize {
                expected: 8,
                actual: 7
            })
        );
    }

    #[test]
    fn validates_uniform_buffers() {
        assert_eq!(
            buffer(BufferKind::Uniform, 16384).validate(&LIMITS, None),
            Ok(())
        );
        assert_eq!(
            buffer(BufferKind::Uniform, 16400).validate(&LIMITS, None),
            Err(DescError::UniformTooLarge {
                size: 16400,
                max: 16384
            })
        );
        assert_eq!(
            buffer(BufferKind::Uniform, 24).validate(&LIMITS, None),
            Err(DescError::UniformSize { size: 24 })
        );
        // Only uniform buffers are rounded to vec4.
        assert_eq!(
            buffer(BufferKind::Vertex, 24).validate(&LIMITS, None),
            Ok(())
        );
    }

    #[test]
    fn checks_write_range() {
        let b = buffer(BufferKind::Vertex, 16);
        assert_eq!(b.check_write(0, 16), Ok(()));
        assert_eq!(b.check_write(12, 4), Ok(()));
        assert_eq!(b.check_write(16, 0), Ok(()));
        assert_eq!(
            b.check_write(12, 8),
            Err(DescError::WriteOutOfBounds {
                offset: 12,
                len: 8,
                size: 16
            })
        );
        assert!(b.check_write(u64::MAX, 1).is_err());
    }

    #[test]
    fn render_target_needs_valid_texture() {
        let t = RenderTargetDesc {
            size: Extent::new(64, 32),
            format: TextureFormat::Rgba8,
        };
        assert_eq!(t.validate(&LIMITS), Ok(()));
        let usage = t.texture_desc().usage;
        assert!(usage.contains(TextureUsage::SAMPLED | TextureUsage::RENDER_TARGET));
        let empty = RenderTargetDesc {
            size: Extent::new(0, 32),
            ..t
        };
        assert_eq!(empty.validate(&LIMITS), Err(DescError::EmptySize));
        let srgb = RenderTargetDesc {
            format: TextureFormat::Rgba8Srgb,
            ..t
        };
        assert_eq!(
            srgb.validate(&LIMITS),
            Err(DescError::TargetFormat {
                format: TextureFormat::Rgba8Srgb
            })
        );
        let huge = RenderTargetDesc {
            size: Extent::new(8192, 32),
            ..t
        };
        assert!(matches!(
            huge.validate(&LIMITS),
            Err(DescError::TooLarge { .. })
        ));
    }

    const fn attr(location: u32, format: VertexFormat, offset: u32) -> VertexAttribute {
        VertexAttribute {
            location,
            format,
            offset,
        }
    }

    const SPRITE: [VertexAttribute; 3] = [
        attr(0, VertexFormat::Float32x2, 0),
        attr(1, VertexFormat::Float32x2, 8),
        attr(2, VertexFormat::Unorm8x4, 16),
    ];

    fn layout(stride: u32, attributes: &[VertexAttribute]) -> Result<(), DescError> {
        VertexLayout { stride, attributes }.validate()
    }

    #[test]
    fn accepts_valid_vertex_layout() {
        assert_eq!(layout(20, &SPRITE), Ok(()));
        assert_eq!(layout(MAX_VERTEX_STRIDE, &SPRITE), Ok(()));
        // Padding after the last attribute is fine.
        assert_eq!(layout(32, &SPRITE), Ok(()));
        assert_eq!(VertexFormat::Float32x3.size(), 12);
        assert_eq!(VertexFormat::Float32x4.size(), 16);
    }

    #[test]
    fn rejects_bad_stride() {
        for stride in [0, 22, 2052, u32::MAX - 3] {
            assert_eq!(
                layout(stride, &SPRITE),
                Err(DescError::VertexStride { stride })
            );
        }
    }

    #[test]
    fn rejects_bad_attribute_count() {
        assert_eq!(layout(4, &[]), Err(DescError::AttributeCount { count: 0 }));
        let many: Vec<_> = (0..17)
            .map(|i| attr(i % 16, VertexFormat::Unorm8x4, i * 4))
            .collect();
        assert_eq!(
            layout(68, &many),
            Err(DescError::AttributeCount { count: 17 })
        );
    }

    #[test]
    fn rejects_attribute_past_stride_or_unaligned() {
        assert_eq!(
            layout(16, &SPRITE),
            Err(DescError::AttributeOffset {
                location: 2,
                offset: 16
            })
        );
        assert_eq!(
            layout(8, &[attr(0, VertexFormat::Unorm8x4, 2)]),
            Err(DescError::AttributeOffset {
                location: 0,
                offset: 2
            })
        );
    }

    #[test]
    fn rejects_bad_or_repeated_location() {
        assert_eq!(
            layout(8, &[attr(16, VertexFormat::Float32x2, 0)]),
            Err(DescError::AttributeLocation { location: 16 })
        );
        assert_eq!(
            layout(
                16,
                &[
                    attr(1, VertexFormat::Float32x2, 0),
                    attr(1, VertexFormat::Float32x2, 8)
                ]
            ),
            Err(DescError::AttributeLocation { location: 1 })
        );
    }

    #[test]
    fn rejects_overlapping_attributes() {
        assert_eq!(
            layout(
                16,
                &[
                    attr(0, VertexFormat::Float32x3, 0),
                    attr(1, VertexFormat::Float32x2, 8)
                ]
            ),
            Err(DescError::AttributeOverlap {
                first: 0,
                second: 1
            })
        );
    }

    const VS: &str = "#version 330 core\nvoid main() {}";
    const FS: &str = "#version 330 core\nvoid main() {}";

    fn pipeline<'a>(bindings: &'a [Binding<'a>]) -> PipelineDesc<'a> {
        PipelineDesc {
            vertex_shader: VS,
            fragment_shader: FS,
            layout: VertexLayout {
                stride: 20,
                attributes: &SPRITE,
            },
            bindings,
            blend: Blend::Alpha,
        }
    }

    const fn binding(name: &str, kind: BindingKind, slot: u32) -> Binding<'_> {
        Binding { name, kind, slot }
    }

    #[test]
    fn accepts_valid_pipeline() {
        let bindings = [
            binding("Globals", BindingKind::UniformBuffer, 0),
            binding("atlas", BindingKind::Texture, 0),
            binding("mask", BindingKind::Texture, 15),
        ];
        assert_eq!(pipeline(&bindings).validate(&LIMITS), Ok(()));
    }

    #[test]
    fn rejects_empty_shader() {
        let mut d = pipeline(&[]);
        d.fragment_shader = "  \n";
        assert_eq!(d.validate(&LIMITS), Err(DescError::EmptyShader));
        let mut d = pipeline(&[]);
        d.vertex_shader = "";
        assert_eq!(d.validate(&LIMITS), Err(DescError::EmptyShader));
    }

    #[test]
    fn pipeline_checks_layout() {
        let mut d = pipeline(&[]);
        d.layout.stride = 0;
        assert_eq!(
            d.validate(&LIMITS),
            Err(DescError::VertexStride { stride: 0 })
        );
    }

    #[test]
    fn rejects_binding_slot_over_limit() {
        let b = [binding("atlas", BindingKind::Texture, 16)];
        assert_eq!(
            pipeline(&b).validate(&LIMITS),
            Err(DescError::BindingSlot {
                kind: BindingKind::Texture,
                slot: 16,
                max: 16
            })
        );
        let b = [binding("Globals", BindingKind::UniformBuffer, 12)];
        assert_eq!(
            pipeline(&b).validate(&LIMITS),
            Err(DescError::BindingSlot {
                kind: BindingKind::UniformBuffer,
                slot: 12,
                max: 12
            })
        );
    }

    #[test]
    fn rejects_bad_binding_names() {
        let b = [binding("", BindingKind::Texture, 0)];
        assert_eq!(pipeline(&b).validate(&LIMITS), Err(DescError::BindingName));
        let b = [
            binding("a", BindingKind::Texture, 0),
            binding("a", BindingKind::UniformBuffer, 0),
        ];
        assert_eq!(pipeline(&b).validate(&LIMITS), Err(DescError::BindingName));
    }

    #[test]
    fn rejects_shared_slot_within_kind() {
        let b = [
            binding("a", BindingKind::Texture, 3),
            binding("b", BindingKind::Texture, 3),
        ];
        assert_eq!(
            pipeline(&b).validate(&LIMITS),
            Err(DescError::BindingSlotTaken {
                kind: BindingKind::Texture,
                slot: 3
            })
        );
        // Uniform buffers and textures have separate slots.
        let b = [
            binding("a", BindingKind::Texture, 3),
            binding("B", BindingKind::UniformBuffer, 3),
        ];
        assert_eq!(pipeline(&b).validate(&LIMITS), Ok(()));
    }

    #[test]
    fn index_format_sizes() {
        assert_eq!(IndexFormat::U16.size(), 2);
        assert_eq!(IndexFormat::U32.size(), 4);
    }
}
