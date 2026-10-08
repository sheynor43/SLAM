//! Glue between the SDL window's GL context (`slam-input`) and the OpenGL backend
//! (`slam-render`), which do not depend on each other (ADR-0018).

use std::ffi::{CStr, c_void};

use slam_input::GlContext;
use slam_render::Extent;
use slam_render::gl::GlSurface;

/// [`GlContext`] as a [`GlSurface`] for [`slam_render::gl::GlDevice`].
pub struct SdlGlSurface(pub GlContext);

impl GlSurface for SdlGlSurface {
    fn make_current(&mut self) -> Result<(), String> {
        self.0.make_current()
    }

    fn release_current(&mut self) -> Result<(), String> {
        self.0.release_current()
    }

    fn get_proc_address(&self, name: &CStr) -> *const c_void {
        self.0.get_proc_address(name)
    }

    fn swap_buffers(&mut self) -> Result<(), String> {
        self.0.swap()
    }

    fn size_in_pixels(&self) -> Extent {
        let (width, height) = self.0.size_in_pixels();
        Extent::new(width, height)
    }
}
