//! The native window for a renderer with its own swapchain (D3D11, ADR-0082).

use std::ffi::c_void;
use std::sync::Arc;

use sdl3_sys::properties::SDL_GetPointerProperty;
use sdl3_sys::video::{SDL_GetWindowProperties, SDL_PROP_WINDOW_WIN32_HWND_POINTER, SDL_Window};

use crate::gl::PixelSize;
use crate::window::sdl_error;

/// The HWND of an [`crate::InputWindow`] and its drawable size, handed to the draw
/// thread. The draw thread makes and owns the swapchain; no SDL function is called
/// through this handle.
///
/// While it is alive, dropping the window leaks the window and SDL instead of
/// destroying them under the swapchain. Stop the draw thread (which releases the
/// swapchain before this handle) and drop the handle before the window.
pub struct NativeHandle {
    hwnd: *mut c_void,
    size: Arc<PixelSize>,
}

// SAFETY: the HWND is only passed to DXGI, which may use it from any thread; the size
// is atomic. The window outlives the handle (see the struct docs).
unsafe impl Send for NativeHandle {}

impl NativeHandle {
    /// # Safety
    /// `window` is a valid window, called on the thread that initialised SDL; `size` is
    /// the window's shared size.
    pub(crate) unsafe fn create(
        window: *mut SDL_Window,
        size: Arc<PixelSize>,
    ) -> Result<Self, String> {
        // SAFETY: guaranteed by the caller; the property name is a valid C string.
        let hwnd = unsafe {
            let props = SDL_GetWindowProperties(window);
            if props == 0 {
                return Err(sdl_error());
            }
            SDL_GetPointerProperty(
                props,
                SDL_PROP_WINDOW_WIN32_HWND_POINTER,
                std::ptr::null_mut(),
            )
        };
        if hwnd.is_null() {
            return Err("the window has no HWND (not the windows video driver)".into());
        }
        Ok(Self { hwnd, size })
    }

    /// The window's HWND.
    pub fn hwnd(&self) -> *mut c_void {
        self.hwnd
    }

    /// Drawable size in pixels as last reported by the event loop. Lock-free.
    pub fn size_in_pixels(&self) -> (u32, u32) {
        self.size.load()
    }
}
