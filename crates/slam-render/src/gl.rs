//! OpenGL 3.3 core backend (ADR-0003). The context comes from a [`GlSurface`]; it is
//! made current on the draw thread and released there before shutdown (ADR-0018).

use std::ffi::{CStr, c_void};

use glow::HasContext;

use crate::hal::{Color, DeviceError, Extent, Limits, TextureDesc, TextureFormat};

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

/// OpenGL 3.3 core device. Created, used and released on the draw thread.
pub struct GlDevice<S: GlSurface> {
    /// Declared before `surface`: on unwind it drops first, while the context is
    /// still current (see `into_surface`).
    gl: glow::Context,
    surface: S,
    limits: Limits,
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
        // SAFETY: the context is current; plain state query.
        let max_texture_size = unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) };
        Ok(Self {
            gl,
            surface,
            limits: Limits {
                max_texture_size: max_texture_size.max(0) as u32,
            },
        })
    }

    /// Detaches the context from this thread and returns the surface.
    pub fn into_surface(self) -> Result<S, DeviceError> {
        let Self {
            gl, mut surface, ..
        } = self;
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
}

impl<S: GlSurface> crate::hal::Device for GlDevice<S> {
    type Texture = GlTexture;

    fn limits(&self) -> Limits {
        self.limits
    }

    fn create_texture(
        &mut self,
        desc: &TextureDesc,
        data: Option<&[u8]>,
    ) -> Result<GlTexture, DeviceError> {
        desc.validate(&self.limits, data)?;
        let (internal, format) = match desc.format {
            TextureFormat::Rgba8 => (glow::RGBA8, glow::RGBA),
            TextureFormat::Rgba8Srgb => (glow::SRGB8_ALPHA8, glow::RGBA),
            TextureFormat::R8 => (glow::R8, glow::RED),
        };
        let gl = &self.gl;
        // SAFETY: the context is current on this thread; `validate` checked that the
        // size is within GL limits and that `data` holds exactly the texture's pixels,
        // tightly packed (unpack alignment 1).
        unsafe {
            // Drop errors left by earlier calls, so the check below sees only ours.
            while gl.get_error() != glow::NO_ERROR {}
            let raw = gl.create_texture().map_err(DeviceError::Backend)?;
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
            let error = gl.get_error();
            if error != glow::NO_ERROR {
                gl.delete_texture(raw);
                return Err(DeviceError::Backend(format!(
                    "texture upload failed: GL error 0x{error:x}"
                )));
            }
            Ok(GlTexture { raw, desc: *desc })
        }
    }

    fn destroy_texture(&mut self, texture: GlTexture) {
        // SAFETY: the context is current; the texture was created by this device and
        // is consumed here, so it is deleted once.
        unsafe { self.gl.delete_texture(texture.raw) };
    }

    fn begin_frame(&mut self, clear: Color) -> Extent {
        let size = self.surface.size_in_pixels();
        let gl = &self.gl;
        // SAFETY: the context is current; state calls with valid enums.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, size.width as i32, size.height as i32);
            gl.clear_color(clear.r, clear.g, clear.b, clear.a);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        size
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
