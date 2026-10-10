//! OpenGL 3.3 core backend (ADR-0003). The context comes from a [`GlSurface`]; it is
//! made current on the draw thread and released there before shutdown (ADR-0018).

use std::ffi::{CStr, c_void};
use std::ops::Range;

use glow::HasContext;

use crate::hal::{
    BindingKind, Blend, BufferDesc, BufferKind, BufferUpdate, Color, DescError, DeviceError,
    DrawError, Extent, IndexFormat, Limits, MAX_VERTEX_ATTRIBUTES, Origin, PipelineDesc,
    RenderTargetDesc, TextureDesc, TextureFormat, TextureUsage, VERTEX_ALIGNMENT, VertexAttribute,
    VertexFormat,
};

/// A window's OpenGL context as seen by the renderer. Implemented outside this crate
/// (for SDL3, by `slam-input` with glue in `slam-engine`), so `slam-render` does not
/// depend on the windowing library.
pub trait GlSurface: Send {
    /// Makes the context current on the calling thread.
    fn make_current(&mut self) -> Result<(), String>;

    /// Detaches the context from the calling thread.
    fn release_current(&mut self) -> Result<(), String>;

    /// Address of a GL function, or null. Requires the context to be current.
    fn get_proc_address(&self, name: &CStr) -> *const c_void;

    /// Presents the back buffer. Must not wait for vertical sync.
    fn swap_buffers(&mut self) -> Result<(), String>;

    /// Current drawable size in pixels. Must be cheap and must not allocate.
    fn size_in_pixels(&self) -> Extent;
}

/// Whether [`GlDevice::new`] enables GL debug output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlDebug {
    Off,
    /// Log GL errors and warnings through `tracing`, synchronously, so the message
    /// arrives on the call that caused it. Slows rendering; meant for debug builds.
    On,
}

impl GlDebug {
    /// `On` in debug builds, `Off` in release.
    pub const fn for_build() -> Self {
        if cfg!(debug_assertions) {
            Self::On
        } else {
            Self::Off
        }
    }
}

pub struct GlTexture {
    raw: glow::Texture,
    desc: TextureDesc,
}

impl GlTexture {
    pub fn desc(&self) -> &TextureDesc {
        &self.desc
    }
}

pub struct GlBuffer {
    raw: glow::Buffer,
    desc: BufferDesc,
}

impl GlBuffer {
    pub fn desc(&self) -> &BufferDesc {
        &self.desc
    }
}

/// A pipeline's vertex layout, stored inline so that binding it does not allocate.
#[derive(Clone, Copy)]
struct Layout {
    stride: u32,
    attributes: [VertexAttribute; MAX_VERTEX_ATTRIBUTES],
    count: usize,
}

impl Layout {
    fn attributes(&self) -> &[VertexAttribute] {
        &self.attributes[..self.count]
    }
}

pub struct GlPipeline {
    /// Linked programs indexed by [`Variant`].
    programs: [glow::Program; 2],
    layout: Layout,
    blend: Blend,
}

pub struct GlRenderTarget {
    framebuffer: glow::Framebuffer,
    texture: GlTexture,
}

impl GlRenderTarget {
    pub fn size(&self) -> Extent {
        self.texture.desc.size
    }
}

/// Which vertex stage of a [`crate::Shader`] a program uses: GL draws to the surface
/// as is, and to render targets with clip-space y flipped (ADR-0019).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    Surface = 0,
    Target = 1,
}

impl Variant {
    fn of(target: Option<glow::Framebuffer>) -> Self {
        if target.is_some() {
            Self::Target
        } else {
            Self::Surface
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BoundBuffer {
    raw: glow::Buffer,
    size: u64,
    offset: u64,
}

/// Draw state as last sent to GL, so redundant calls are skipped and the vertex
/// layout is applied only when the pipeline or vertex buffer changes.
struct State {
    target: Option<glow::Framebuffer>,
    /// Programs of the set pipeline; the one for the bound target is in use.
    programs: Option<[glow::Program; 2]>,
    layout: Option<Layout>,
    blend: Blend,
    vertex: Option<BoundBuffer>,
    index: Option<(BoundBuffer, IndexFormat)>,
    layout_dirty: bool,
    /// Bit `n` set: attribute array `n` is enabled.
    enabled_attributes: u32,
}

/// OpenGL 3.3 core device. Created, used and released on the draw thread.
///
/// One vertex array object stays bound for the device's life; vertex attribute
/// pointers are re-specified at draw time when the pipeline or vertex buffer changed.
/// Texture unit [`Limits::max_texture_slots`] (one past the last slot) is reserved for
/// uploads so they do not disturb bound textures.
pub struct GlDevice<S: GlSurface> {
    /// Declared before `surface`: on unwind it drops first, while the context is
    /// still current (see `into_surface`).
    gl: glow::Context,
    surface: S,
    limits: Limits,
    vertex_array: glow::VertexArray,
    state: State,
}

impl<S: GlSurface> GlDevice<S> {
    /// Makes the surface's context current on this thread and loads GL. Call on the
    /// thread that will draw; give the surface back with [`GlDevice::into_surface`] on
    /// the same thread.
    pub fn new(mut surface: S, debug: GlDebug) -> Result<Self, DeviceError> {
        surface.make_current().map_err(DeviceError::Surface)?;
        // SAFETY: the context is current on this thread, and `get_proc_address`
        // returns pointers to functions of that context (or null).
        let mut gl = unsafe {
            glow::Context::from_loader_function_cstr(|name| surface.get_proc_address(name))
        };
        let version = gl.version();
        if version.is_embedded || (version.major, version.minor) < (3, 3) {
            let found = format!(
                "{}.{}{}",
                version.major,
                version.minor,
                if version.is_embedded { " ES" } else { "" }
            );
            let _ = surface.release_current();
            return Err(DeviceError::Unsupported(format!(
                "OpenGL 3.3 core required, found {found}"
            )));
        }
        if debug == GlDebug::On {
            enable_debug_output(&mut gl);
        }
        // SAFETY: the context is current; plain state queries.
        let query = |name| unsafe { gl.get_parameter_i32(name) }.max(0) as u32;
        let limits = Limits {
            max_texture_size: query(glow::MAX_TEXTURE_SIZE),
            max_uniform_block_size: query(glow::MAX_UNIFORM_BLOCK_SIZE),
            // GL counts bindings for all stages; 12 per stage is what GL 3.3, D3D11 and
            // Vulkan all guarantee.
            max_uniform_buffer_slots: query(glow::MAX_UNIFORM_BUFFER_BINDINGS).min(12),
            // The last unit is reserved for uploads.
            max_texture_slots: query(glow::MAX_TEXTURE_IMAGE_UNITS).saturating_sub(1),
        };
        // SAFETY: the context is current.
        let vertex_array = match unsafe { gl.create_vertex_array() } {
            Ok(vertex_array) => vertex_array,
            Err(e) => {
                drop(gl);
                let _ = surface.release_current();
                return Err(DeviceError::Backend(e));
            }
        };
        // SAFETY: the context is current; `vertex_array` was just created. Blending
        // starts disabled, matching `State::blend`.
        unsafe {
            gl.bind_vertex_array(Some(vertex_array));
            gl.disable(glow::BLEND);
        }
        Ok(Self {
            gl,
            surface,
            limits,
            vertex_array,
            state: State {
                target: None,
                programs: None,
                layout: None,
                blend: Blend::Replace,
                vertex: None,
                index: None,
                layout_dirty: false,
                enabled_attributes: 0,
            },
        })
    }

    /// Driver description for reports: `GL_RENDERER` and `GL_VERSION`. Allocates.
    pub fn driver_info(&self) -> String {
        // SAFETY: the context is current on this thread; plain string queries.
        let (renderer, version) = unsafe {
            (
                self.gl.get_parameter_string(glow::RENDERER),
                self.gl.get_parameter_string(glow::VERSION),
            )
        };
        format!("{renderer}, OpenGL {version}")
    }

    /// Blocks until the GPU has finished all submitted commands (`glFinish`).
    /// Diagnostics only: separates waiting for the GPU from the cost of `present`.
    pub fn finish(&mut self) {
        // SAFETY: the context is current on this thread.
        unsafe { self.gl.finish() };
    }

    /// Detaches the context from this thread and returns the surface. Resources not
    /// destroyed by then are freed with the context.
    pub fn into_surface(self) -> Result<S, DeviceError> {
        let Self {
            gl,
            mut surface,
            vertex_array,
            ..
        } = self;
        // SAFETY: the context is current; the vertex array is owned by the device.
        unsafe { gl.delete_vertex_array(vertex_array) };
        // While the context is still current: glow's Drop unsets the debug callback
        // before freeing it, and that call needs the context.
        drop(gl);
        surface.release_current().map_err(DeviceError::Surface)?;
        Ok(surface)
    }

    /// Raw GL access for tests and backend-internal code.
    #[doc(hidden)]
    pub fn gl(&self) -> &glow::Context {
        &self.gl
    }

    /// Binds `target` (the surface if `None`) unless already bound, switching the
    /// set pipeline to the program for that kind of target.
    ///
    /// The render target variant reverses the winding: if face culling is ever
    /// enabled, the front face must switch here together with the program.
    fn bind_target(&mut self, target: Option<glow::Framebuffer>) {
        if self.state.target != target {
            let old = Variant::of(self.state.target);
            let new = Variant::of(target);
            // SAFETY: the context is current; `target` is a live framebuffer of this
            // device or the default one; set programs are live and linked.
            unsafe {
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, target);
                if let Some(programs) = self.state.programs
                    && old != new
                {
                    self.gl.use_program(Some(programs[new as usize]));
                }
            }
            self.state.target = target;
        }
    }

    /// The program in use, if a pipeline is set.
    fn current_program(&self) -> Option<glow::Program> {
        self.state
            .programs
            .map(|p| p[Variant::of(self.state.target) as usize])
    }

    fn apply_blend(&mut self, blend: Blend) {
        if self.state.blend == blend {
            return;
        }
        let gl = &self.gl;
        // SAFETY: the context is current; valid blend enums.
        unsafe {
            match blend {
                Blend::Replace => gl.disable(glow::BLEND),
                Blend::Alpha => {
                    gl.enable(glow::BLEND);
                    gl.blend_func_separate(
                        glow::SRC_ALPHA,
                        glow::ONE_MINUS_SRC_ALPHA,
                        glow::ONE,
                        glow::ONE,
                    );
                }
                Blend::Premultiplied => {
                    gl.enable(glow::BLEND);
                    gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
                }
                Blend::Additive => {
                    gl.enable(glow::BLEND);
                    gl.blend_func_separate(glow::SRC_ALPHA, glow::ONE, glow::ONE, glow::ONE);
                }
            }
        }
        self.state.blend = blend;
    }

    /// Checks that a pipeline and a vertex buffer are set and that `vertices` fit in
    /// the buffer, then applies the vertex layout if it changed.
    fn prepare_vertices(&mut self, vertices: Range<u32>) -> Result<(), DrawError> {
        let out_of_range = DrawError::OutOfRange {
            start: vertices.start,
            end: vertices.end,
        };
        let layout = self.state.layout.ok_or(DrawError::NoPipeline)?;
        let vertex = self.state.vertex.ok_or(DrawError::NoVertexBuffer)?;
        let end = u64::from(vertices.end)
            .checked_mul(u64::from(layout.stride))
            .and_then(|bytes| bytes.checked_add(vertex.offset));
        if vertices.start > vertices.end || end.is_none_or(|end| end > vertex.size) {
            return Err(out_of_range);
        }
        if !self.state.layout_dirty {
            return Ok(());
        }
        let gl = &self.gl;
        let mut enabled = 0u32;
        // SAFETY: the context is current with the device's vertex array bound; the
        // buffer is live (destroying it clears `state.vertex`); offsets are computed in
        // `u64` and clamped to `i32` (an attribute past the end of the buffer can only
        // be read through out-of-range indices, which the caller rules out); locations
        // are below 16 and the stride at most 2048 (`VertexLayout::validate`).
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex.raw));
            for a in layout.attributes() {
                let (components, kind, normalized) = match a.format {
                    VertexFormat::Float32x2 => (2, glow::FLOAT, false),
                    VertexFormat::Float32x3 => (3, glow::FLOAT, false),
                    VertexFormat::Float32x4 => (4, glow::FLOAT, false),
                    VertexFormat::Unorm8x4 => (4, glow::UNSIGNED_BYTE, true),
                };
                gl.vertex_attrib_pointer_f32(
                    a.location,
                    components,
                    kind,
                    normalized,
                    layout.stride as i32,
                    (vertex.offset + u64::from(a.offset)).min(i32::MAX as u64) as i32,
                );
                enabled |= 1 << a.location;
            }
            let previous = self.state.enabled_attributes;
            for location in 0..MAX_VERTEX_ATTRIBUTES as u32 {
                let bit = 1 << location;
                if enabled & bit != 0 && previous & bit == 0 {
                    gl.enable_vertex_attrib_array(location);
                } else if enabled & bit == 0 && previous & bit != 0 {
                    gl.disable_vertex_attrib_array(location);
                }
            }
        }
        self.state.enabled_attributes = enabled;
        self.state.layout_dirty = false;
        Ok(())
    }
}

/// Internal and pixel-transfer formats of a texture format.
fn gl_format(format: TextureFormat) -> (u32, u32) {
    match format {
        TextureFormat::Rgba8 => (glow::RGBA8, glow::RGBA),
        TextureFormat::Rgba8Srgb => (glow::SRGB8_ALPHA8, glow::RGBA),
        TextureFormat::R8 => (glow::R8, glow::RED),
    }
}

fn check_kind(buffer: &GlBuffer, expected: BufferKind) -> Result<(), DrawError> {
    if buffer.desc.kind == expected {
        Ok(())
    } else {
        Err(DrawError::BufferKind {
            expected,
            actual: buffer.desc.kind,
        })
    }
}

/// Drops errors left by earlier calls, so a later `gl_error` sees only new ones.
///
/// # Safety
/// The context must be current.
unsafe fn clear_errors(gl: &glow::Context) {
    // Bounded: after a context loss some drivers report errors forever.
    for _ in 0..32 {
        // SAFETY: guaranteed by the caller.
        if unsafe { gl.get_error() } == glow::NO_ERROR {
            break;
        }
    }
}

/// # Safety
/// The context must be current.
unsafe fn gl_error(gl: &glow::Context, what: &str) -> Result<(), DeviceError> {
    // SAFETY: guaranteed by the caller.
    match unsafe { gl.get_error() } {
        glow::NO_ERROR => Ok(()),
        error => Err(DeviceError::Backend(format!(
            "{what} failed: GL error 0x{error:x}"
        ))),
    }
}

/// # Safety
/// The context must be current.
unsafe fn compile_shader(
    gl: &glow::Context,
    shader: &str,
    stage: &str,
    kind: u32,
    source: &str,
) -> Result<glow::Shader, DeviceError> {
    // SAFETY: guaranteed by the caller; the shader is deleted on failure.
    unsafe {
        let raw = gl.create_shader(kind).map_err(DeviceError::Backend)?;
        gl.shader_source(raw, source);
        gl.compile_shader(raw);
        if !gl.get_shader_compile_status(raw) {
            let log = gl.get_shader_info_log(raw);
            gl.delete_shader(raw);
            return Err(DeviceError::Shader(format!(
                "{shader}: {stage}: {}",
                log.trim()
            )));
        }
        Ok(raw)
    }
}

/// Links `vertex` and `fragment` and binds `bindings` to their slots. The shaders stay
/// alive; the program in use is left unchanged.
///
/// # Safety
/// The context must be current; `vertex` and `fragment` must be compiled shaders.
unsafe fn link_program(
    gl: &glow::Context,
    shader: &str,
    vertex: glow::Shader,
    fragment: glow::Shader,
    bindings: &[crate::hal::Binding<'_>],
    in_use: Option<glow::Program>,
) -> Result<glow::Program, DeviceError> {
    // SAFETY: guaranteed by the caller; the program is deleted on failure and the
    // program in use is restored after setting sampler units.
    unsafe {
        let program = gl.create_program().map_err(DeviceError::Backend)?;
        gl.attach_shader(program, vertex);
        gl.attach_shader(program, fragment);
        gl.link_program(program);
        gl.detach_shader(program, vertex);
        gl.detach_shader(program, fragment);
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(DeviceError::Shader(format!(
                "{shader}: link: {}",
                log.trim()
            )));
        }
        gl.use_program(Some(program));
        for b in bindings {
            // Names come from naga's reflection of one stage, so each is found in the
            // program unless the driver dropped the resource as unused.
            match b.kind {
                BindingKind::UniformBuffer => match gl.get_uniform_block_index(program, b.name) {
                    Some(index) => gl.uniform_block_binding(program, index, b.slot),
                    None => tracing::debug!(shader, name = b.name, "uniform block not active"),
                },
                BindingKind::Texture => match gl.get_uniform_location(program, b.name) {
                    Some(location) => gl.uniform_1_i32(Some(&location), b.slot as i32),
                    None => tracing::debug!(shader, name = b.name, "sampler not active"),
                },
            }
        }
        gl.use_program(in_use);
        Ok(program)
    }
}

impl<S: GlSurface> crate::hal::Device for GlDevice<S> {
    type Texture = GlTexture;
    type Buffer = GlBuffer;
    type Pipeline = GlPipeline;
    type RenderTarget = GlRenderTarget;

    fn limits(&self) -> Limits {
        self.limits
    }

    fn create_texture(
        &mut self,
        desc: &TextureDesc,
        data: Option<&[u8]>,
    ) -> Result<GlTexture, DeviceError> {
        desc.validate(&self.limits, data)?;
        let (internal, format) = gl_format(desc.format);
        let gl = &self.gl;
        // SAFETY: the context is current on this thread; `validate` checked that the
        // size is within GL limits and that `data` holds exactly the texture's pixels,
        // tightly packed (unpack alignment 1). The upload unit is reserved, so no
        // texture bound by `set_texture` is replaced.
        unsafe {
            clear_errors(gl);
            let raw = gl.create_texture().map_err(DeviceError::Backend)?;
            gl.active_texture(glow::TEXTURE0 + self.limits.max_texture_slots);
            gl.bind_texture(glow::TEXTURE_2D, Some(raw));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                internal as i32,
                desc.size.width as i32,
                desc.size.height as i32,
                0,
                format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(data),
            );
            for (param, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            if let Err(e) = gl_error(gl, "texture upload") {
                gl.delete_texture(raw);
                return Err(e);
            }
            Ok(GlTexture { raw, desc: *desc })
        }
    }

    fn destroy_texture(&mut self, texture: GlTexture) {
        // SAFETY: the context is current; the texture was created by this device and
        // is consumed here, so it is deleted once. GL unbinds it from all units.
        unsafe { self.gl.delete_texture(texture.raw) };
    }

    fn write_texture(
        &mut self,
        texture: &GlTexture,
        origin: Origin,
        size: Extent,
        data: &[u8],
    ) -> Result<(), DescError> {
        texture.desc.check_write(origin, size, data.len())?;
        let format = gl_format(texture.desc.format).1;
        let gl = &self.gl;
        // SAFETY: the context is current; `check_write` keeps the region inside the
        // texture (whose size is within GL limits) and `data` exactly the region's
        // pixels, tightly packed (unpack alignment 1). The upload unit is reserved.
        unsafe {
            gl.active_texture(glow::TEXTURE0 + self.limits.max_texture_slots);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture.raw));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                origin.x as i32,
                origin.y as i32,
                size.width as i32,
                size.height as i32,
                format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(data)),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        Ok(())
    }

    fn create_buffer(
        &mut self,
        desc: &BufferDesc,
        data: Option<&[u8]>,
    ) -> Result<GlBuffer, DeviceError> {
        desc.validate(&self.limits, data)?;
        let Ok(size) = i32::try_from(desc.size) else {
            return Err(DeviceError::Unsupported(format!(
                "buffer of {} bytes is larger than i32::MAX",
                desc.size
            )));
        };
        let usage = match desc.update {
            BufferUpdate::Static => glow::STATIC_DRAW,
            BufferUpdate::Dynamic => glow::DYNAMIC_DRAW,
        };
        let gl = &self.gl;
        // SAFETY: the context is current; `size` is positive and `data`, if given,
        // holds exactly `size` bytes (`validate`). The copy-write target is used so
        // that the vertex array's index binding is left alone.
        unsafe {
            clear_errors(gl);
            let raw = gl.create_buffer().map_err(DeviceError::Backend)?;
            gl.bind_buffer(glow::COPY_WRITE_BUFFER, Some(raw));
            match data {
                Some(data) => gl.buffer_data_u8_slice(glow::COPY_WRITE_BUFFER, data, usage),
                None => gl.buffer_data_size(glow::COPY_WRITE_BUFFER, size, usage),
            }
            gl.bind_buffer(glow::COPY_WRITE_BUFFER, None);
            if let Err(e) = gl_error(gl, "buffer creation") {
                gl.delete_buffer(raw);
                return Err(e);
            }
            Ok(GlBuffer { raw, desc: *desc })
        }
    }

    fn destroy_buffer(&mut self, buffer: GlBuffer) {
        let state = &mut self.state;
        if state.vertex.is_some_and(|v| v.raw == buffer.raw) {
            state.vertex = None;
            state.layout_dirty = true;
        }
        if state.index.is_some_and(|(i, _)| i.raw == buffer.raw) {
            state.index = None;
        }
        // SAFETY: the context is current; the buffer was created by this device and
        // is consumed here. GL unbinds it from the bound vertex array and all targets.
        unsafe { self.gl.delete_buffer(buffer.raw) };
    }

    fn write_buffer(
        &mut self,
        buffer: &GlBuffer,
        offset: u64,
        data: &[u8],
    ) -> Result<(), DescError> {
        buffer.desc.check_write(offset, data.len())?;
        if data.is_empty() {
            return Ok(());
        }
        let gl = &self.gl;
        // SAFETY: the context is current; the range is inside the buffer (checked
        // above), whose size fits in `i32` (`create_buffer`).
        unsafe {
            gl.bind_buffer(glow::COPY_WRITE_BUFFER, Some(buffer.raw));
            gl.buffer_sub_data_u8_slice(glow::COPY_WRITE_BUFFER, offset as i32, data);
            gl.bind_buffer(glow::COPY_WRITE_BUFFER, None);
        }
        Ok(())
    }

    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<GlPipeline, DeviceError> {
        desc.validate(&self.limits)?;
        let gl = &self.gl;
        let shader = desc.shader;
        let glsl = &shader.glsl;
        let in_use = self.current_program();
        // SAFETY: the context is current; every shader is deleted before returning and
        // the surface program is deleted if the target one fails.
        let programs = unsafe {
            let compile =
                |stage, kind, source| compile_shader(gl, shader.name, stage, kind, source);
            let fragment = compile("fragment", glow::FRAGMENT_SHADER, glsl.fragment)?;
            let link = |name, source| {
                let vertex = compile(name, glow::VERTEX_SHADER, source)?;
                let program =
                    link_program(gl, shader.name, vertex, fragment, glsl.bindings, in_use);
                gl.delete_shader(vertex);
                program
            };
            let programs = (|| {
                let surface = link("surface vertex", glsl.vertex_surface)?;
                match link("render target vertex", glsl.vertex_target) {
                    Ok(target) => Ok([surface, target]),
                    Err(e) => {
                        gl.delete_program(surface);
                        Err(e)
                    }
                }
            })();
            gl.delete_shader(fragment);
            programs?
        };
        let mut attributes = [desc.layout.attributes[0]; MAX_VERTEX_ATTRIBUTES];
        attributes[..desc.layout.attributes.len()].copy_from_slice(desc.layout.attributes);
        Ok(GlPipeline {
            programs,
            layout: Layout {
                stride: desc.layout.stride,
                attributes,
                count: desc.layout.attributes.len(),
            },
            blend: desc.blend,
        })
    }

    fn destroy_pipeline(&mut self, pipeline: GlPipeline) {
        let gl = &self.gl;
        // SAFETY: the context is current; the program was created by this device and
        // is consumed here. It is unbound first so it is freed right away.
        unsafe {
            if self.state.programs == Some(pipeline.programs) {
                gl.use_program(None);
                self.state.programs = None;
                self.state.layout = None;
            }
            for program in pipeline.programs {
                gl.delete_program(program);
            }
        }
    }

    fn create_render_target(
        &mut self,
        desc: &RenderTargetDesc,
    ) -> Result<GlRenderTarget, DeviceError> {
        desc.validate(&self.limits)?;
        let texture = self.create_texture(&desc.texture_desc(), None)?;
        let gl = &self.gl;
        // SAFETY: the context is current; the texture is live and colour-renderable
        // (all `TextureFormat`s are in GL 3.3). The framebuffer binding is restored to
        // `state.target` before returning.
        unsafe {
            let framebuffer = match gl.create_framebuffer() {
                Ok(framebuffer) => framebuffer,
                Err(e) => {
                    gl.delete_texture(texture.raw);
                    return Err(DeviceError::Backend(e));
                }
            };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture.raw),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, self.state.target);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(framebuffer);
                gl.delete_texture(texture.raw);
                return Err(DeviceError::Backend(format!(
                    "render target incomplete: status 0x{status:x}"
                )));
            }
            Ok(GlRenderTarget {
                framebuffer,
                texture,
            })
        }
    }

    fn destroy_render_target(&mut self, target: GlRenderTarget) {
        if self.state.target == Some(target.framebuffer) {
            self.bind_target(None);
        }
        // SAFETY: the context is current; the framebuffer was created by this device,
        // is no longer bound and is consumed here.
        unsafe { self.gl.delete_framebuffer(target.framebuffer) };
        self.destroy_texture(target.texture);
    }

    fn render_target_texture<'a>(&self, target: &'a GlRenderTarget) -> &'a GlTexture {
        &target.texture
    }

    fn begin_frame(&mut self, clear: Color) -> Extent {
        self.begin_pass(None, Some(clear))
    }

    fn begin_pass(&mut self, target: Option<&GlRenderTarget>, clear: Option<Color>) -> Extent {
        let size = match target {
            Some(t) => t.size(),
            None => self.surface.size_in_pixels(),
        };
        self.bind_target(target.map(|t| t.framebuffer));
        let gl = &self.gl;
        // SAFETY: the context is current; state calls with valid enums.
        unsafe {
            gl.viewport(0, 0, size.width as i32, size.height as i32);
            if let Some(c) = clear {
                gl.clear_color(c.r, c.g, c.b, c.a);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
        }
        size
    }

    fn set_pipeline(&mut self, pipeline: &GlPipeline) {
        if self.state.programs != Some(pipeline.programs) {
            let program = pipeline.programs[Variant::of(self.state.target) as usize];
            // SAFETY: the context is current; the program is live and linked.
            unsafe { self.gl.use_program(Some(program)) };
            self.state.programs = Some(pipeline.programs);
            self.state.layout = Some(pipeline.layout);
            self.state.layout_dirty = true;
        }
        self.apply_blend(pipeline.blend);
    }

    fn set_vertex_buffer(&mut self, buffer: &GlBuffer, offset: u64) -> Result<(), DrawError> {
        check_kind(buffer, BufferKind::Vertex)?;
        if !offset.is_multiple_of(u64::from(VERTEX_ALIGNMENT)) {
            return Err(DrawError::Unaligned { offset });
        }
        if offset > buffer.desc.size {
            return Err(DrawError::OffsetPastEnd {
                offset,
                size: buffer.desc.size,
            });
        }
        let bound = Some(BoundBuffer {
            raw: buffer.raw,
            size: buffer.desc.size,
            offset,
        });
        if self.state.vertex != bound {
            self.state.vertex = bound;
            self.state.layout_dirty = true;
        }
        Ok(())
    }

    fn set_index_buffer(
        &mut self,
        buffer: &GlBuffer,
        format: IndexFormat,
    ) -> Result<(), DrawError> {
        check_kind(buffer, BufferKind::Index)?;
        if self.state.index.is_none_or(|(i, _)| i.raw != buffer.raw) {
            // SAFETY: the context is current with the device's vertex array bound, which
            // records the binding; the buffer is live.
            unsafe {
                self.gl
                    .bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(buffer.raw))
            };
        }
        self.state.index = Some((
            BoundBuffer {
                raw: buffer.raw,
                size: buffer.desc.size,
                offset: 0,
            },
            format,
        ));
        Ok(())
    }

    fn set_uniform_buffer(&mut self, slot: u32, buffer: &GlBuffer) -> Result<(), DrawError> {
        check_kind(buffer, BufferKind::Uniform)?;
        let max = self.limits.max_uniform_buffer_slots;
        if slot >= max {
            return Err(DrawError::Slot { slot, max });
        }
        // SAFETY: the context is current; the slot is below the binding limit and the
        // buffer is live.
        unsafe {
            self.gl
                .bind_buffer_base(glow::UNIFORM_BUFFER, slot, Some(buffer.raw))
        };
        Ok(())
    }

    fn set_texture(&mut self, slot: u32, texture: &GlTexture) -> Result<(), DrawError> {
        if !texture.desc.usage.contains(TextureUsage::SAMPLED) {
            return Err(DrawError::NotSampled);
        }
        let max = self.limits.max_texture_slots;
        if slot >= max {
            return Err(DrawError::Slot { slot, max });
        }
        // SAFETY: the context is current; the unit is below the limit and the texture
        // is live.
        unsafe {
            self.gl.active_texture(glow::TEXTURE0 + slot);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture.raw));
        }
        Ok(())
    }

    fn draw(&mut self, vertices: Range<u32>) -> Result<(), DrawError> {
        self.prepare_vertices(vertices.clone())?;
        if vertices.is_empty() {
            return Ok(());
        }
        // SAFETY: the context is current; a pipeline and vertex buffer are bound and
        // the range is inside the buffer, so it fits in `i32`.
        unsafe {
            self.gl.draw_arrays(
                glow::TRIANGLES,
                vertices.start as i32,
                vertices.len() as i32,
            );
        }
        Ok(())
    }

    fn draw_indexed(&mut self, indices: Range<u32>) -> Result<(), DrawError> {
        let out_of_range = DrawError::OutOfRange {
            start: indices.start,
            end: indices.end,
        };
        let (index, format) = self.state.index.ok_or(DrawError::NoIndexBuffer)?;
        let size = u64::from(format.size());
        if indices.start > indices.end || u64::from(indices.end) * size > index.size {
            return Err(out_of_range);
        }
        self.prepare_vertices(0..0)?;
        if indices.is_empty() {
            return Ok(());
        }
        let kind = match format {
            IndexFormat::U16 => glow::UNSIGNED_SHORT,
            IndexFormat::U32 => glow::UNSIGNED_INT,
        };
        // SAFETY: the context is current; pipeline, vertex and index buffers are bound
        // and the index range is inside the index buffer, whose size fits in `i32`.
        // Index values themselves are not checked (documented on the trait).
        unsafe {
            self.gl.draw_elements(
                glow::TRIANGLES,
                indices.len() as i32,
                kind,
                (u64::from(indices.start) * size) as i32,
            );
        }
        Ok(())
    }

    fn present(&mut self) -> Result<(), DeviceError> {
        self.surface.swap_buffers().map_err(DeviceError::Surface)
    }
}

/// Routes GL debug messages to `tracing`; notifications are filtered out. Without
/// `KHR_debug` (or GL 4.3) logs a warning and does nothing.
fn enable_debug_output(gl: &mut glow::Context) {
    if !gl.supports_debug() {
        tracing::warn!("GL debug output unavailable: no KHR_debug");
        return;
    }
    // SAFETY: the context is current and supports debug output (checked above).
    unsafe {
        gl.enable(glow::DEBUG_OUTPUT);
        gl.enable(glow::DEBUG_OUTPUT_SYNCHRONOUS);
        gl.debug_message_control(
            glow::DONT_CARE,
            glow::DONT_CARE,
            glow::DEBUG_SEVERITY_NOTIFICATION,
            &[],
            false,
        );
        gl.debug_message_callback(|source, kind, id, severity, message| {
            let source = debug_source(source);
            let kind = debug_type(kind);
            match severity {
                glow::DEBUG_SEVERITY_HIGH => {
                    tracing::error!(target: "slam_render::gl", source, kind, id, "{message}")
                }
                glow::DEBUG_SEVERITY_MEDIUM => {
                    tracing::warn!(target: "slam_render::gl", source, kind, id, "{message}")
                }
                _ => tracing::debug!(target: "slam_render::gl", source, kind, id, "{message}"),
            }
        });
    }
}

fn debug_source(source: u32) -> &'static str {
    match source {
        glow::DEBUG_SOURCE_API => "api",
        glow::DEBUG_SOURCE_WINDOW_SYSTEM => "window-system",
        glow::DEBUG_SOURCE_SHADER_COMPILER => "shader-compiler",
        glow::DEBUG_SOURCE_THIRD_PARTY => "third-party",
        glow::DEBUG_SOURCE_APPLICATION => "application",
        _ => "other",
    }
}

fn debug_type(kind: u32) -> &'static str {
    match kind {
        glow::DEBUG_TYPE_ERROR => "error",
        glow::DEBUG_TYPE_DEPRECATED_BEHAVIOR => "deprecated",
        glow::DEBUG_TYPE_UNDEFINED_BEHAVIOR => "undefined",
        glow::DEBUG_TYPE_PORTABILITY => "portability",
        glow::DEBUG_TYPE_PERFORMANCE => "performance",
        _ => "other",
    }
}
