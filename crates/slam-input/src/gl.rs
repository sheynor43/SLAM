//! The window's OpenGL context, handed to the draw thread (ADR-0018).

use std::ffi::{CStr, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, ThreadId};

use sdl3_sys::video::{
    SDL_GL_CONTEXT_DEBUG_FLAG, SDL_GL_CONTEXT_FLAGS, SDL_GL_CONTEXT_MAJOR_VERSION,
    SDL_GL_CONTEXT_MINOR_VERSION, SDL_GL_CONTEXT_PROFILE_CORE, SDL_GL_CONTEXT_PROFILE_MASK,
    SDL_GL_CreateContext, SDL_GL_DEPTH_SIZE, SDL_GL_DOUBLEBUFFER, SDL_GL_DestroyContext,
    SDL_GL_GetProcAddress, SDL_GL_MakeCurrent, SDL_GL_SetAttribute, SDL_GL_SetSwapInterval,
    SDL_GL_SwapWindow, SDL_GLContext, SDL_Window,
};

use crate::window::sdl_error;

/// Drawable size in pixels, written by the event loop and read by the draw thread.
/// Packed into one atomic so a reader never sees the width of one size and the height
/// of another.
#[derive(Debug, Default)]
pub(crate) struct PixelSize(AtomicU64);

impl PixelSize {
    pub(crate) fn store(&self, width: i32, height: i32) {
        let packed = (width.max(0) as u64) << 32 | height.max(0) as u64;
        self.0.store(packed, Ordering::Relaxed);
    }

    fn load(&self) -> (u32, u32) {
        let packed = self.0.load(Ordering::Relaxed);
        ((packed >> 32) as u32, packed as u32)
    }
}

/// Context attributes for the next window and context: OpenGL 3.3 core, double
/// buffered, no depth buffer, debug context in debug builds.
pub(crate) fn set_attributes() {
    let flags = if cfg!(debug_assertions) {
        SDL_GL_CONTEXT_DEBUG_FLAG.0
    } else {
        0
    };
    for (attr, value) in [
        (SDL_GL_CONTEXT_MAJOR_VERSION, 3),
        (SDL_GL_CONTEXT_MINOR_VERSION, 3),
        (SDL_GL_CONTEXT_PROFILE_MASK, SDL_GL_CONTEXT_PROFILE_CORE.0),
        (SDL_GL_CONTEXT_FLAGS, flags),
        (SDL_GL_DOUBLEBUFFER, 1),
        (SDL_GL_DEPTH_SIZE, 0),
    ] {
        // SAFETY: SDL video is initialised; plain attribute store. A rejected value
        // shows up as a failure in SDL_GL_CreateContext.
        unsafe { SDL_GL_SetAttribute(attr, value) };
    }
}

/// An OpenGL context of an [`crate::InputWindow`]. Created on the main thread, then
/// made current, used and released by the draw thread (ADR-0018). It is current on at
/// most one thread: [`GlContext::make_current`] fails while another thread holds it.
///
/// Dropping it destroys the context, unless it is still current on another thread:
/// then the context is leaked rather than destroyed under that thread. Likewise, if
/// the window is dropped while the context is alive, the window and SDL are leaked.
/// Stop the draw thread and drop the context before the window.
pub struct GlContext {
    window: *mut SDL_Window,
    context: SDL_GLContext,
    size: Arc<PixelSize>,
    /// The thread the context is current on, if any.
    current_on: Option<ThreadId>,
}

// SAFETY: the context is current on at most one thread (`current_on`, checked by
// `make_current` and `Drop`), and SDL's GL calls used here keep no thread-bound state
// besides the current context (ADR-0018). The window pointer stays valid while `size`
// is shared with the window, whose Drop leaks the window in that case.
unsafe impl Send for GlContext {}

impl GlContext {
    /// # Safety
    /// `window` is a valid window created with `SDL_WINDOW_OPENGL`, called on the
    /// thread that initialised SDL; `size` is the window's shared size.
    pub(crate) unsafe fn create(
        window: *mut SDL_Window,
        size: Arc<PixelSize>,
    ) -> Result<Self, String> {
        // SAFETY: guaranteed by the caller. CreateContext makes the context current on
        // this thread; the swap interval applies to it; then it is released.
        unsafe {
            let context = SDL_GL_CreateContext(window);
            if context.is_null() {
                return Err(sdl_error());
            }
            if !SDL_GL_SetSwapInterval(0) || !SDL_GL_MakeCurrent(window, std::ptr::null_mut()) {
                // Read before DestroyContext, which may overwrite it.
                let err = sdl_error();
                SDL_GL_DestroyContext(context);
                return Err(err);
            }
            Ok(Self {
                window,
                context,
                size,
                current_on: None,
            })
        }
    }

    /// Makes the context current on the calling thread. Fails if it is current on
    /// another thread.
    pub fn make_current(&mut self) -> Result<(), String> {
        let this = thread::current().id();
        match self.current_on {
            Some(id) if id == this => return Ok(()),
            Some(_) => return Err("GL context is current on another thread".into()),
            None => {}
        }
        // SAFETY: window and context are valid (see `Send`); the context is current on
        // no thread.
        if unsafe { SDL_GL_MakeCurrent(self.window, self.context) } {
            self.current_on = Some(this);
            Ok(())
        } else {
            Err(sdl_error())
        }
    }

    /// Detaches the context from the calling thread. Fails if it is current on
    /// another thread.
    pub fn release_current(&mut self) -> Result<(), String> {
        if self
            .current_on
            .is_some_and(|id| id != thread::current().id())
        {
            return Err("GL context is current on another thread".into());
        }
        // SAFETY: the window is valid; a null context releases the current one.
        if unsafe { SDL_GL_MakeCurrent(self.window, std::ptr::null_mut()) } {
            self.current_on = None;
            Ok(())
        } else {
            Err(sdl_error())
        }
    }

    /// Address of a GL function of the current context, or null.
    pub fn get_proc_address(&self, name: &CStr) -> *const c_void {
        // SAFETY: `name` is a valid C string.
        match unsafe { SDL_GL_GetProcAddress(name.as_ptr()) } {
            Some(f) => f as *const c_void,
            None => std::ptr::null(),
        }
    }

    /// Presents the back buffer. Swap interval is 0, so it does not wait for vsync.
    pub fn swap(&mut self) -> Result<(), String> {
        // SAFETY: the window is valid and the context is current on this thread.
        if unsafe { SDL_GL_SwapWindow(self.window) } {
            Ok(())
        } else {
            Err(sdl_error())
        }
    }

    /// Drawable size in pixels as last reported by the event loop. Lock-free.
    pub fn size_in_pixels(&self) -> (u32, u32) {
        self.size.load()
    }
}

impl Drop for GlContext {
    fn drop(&mut self) {
        if self
            .current_on
            .is_some_and(|id| id != thread::current().id())
        {
            // Destroying it would leave that thread with a dangling current context.
            // Keep the window's count raised too, so the window is leaked with it.
            std::mem::forget(Arc::clone(&self.size));
            return;
        }
        // SAFETY: the context was created by `create` and is destroyed once; it is
        // current on no other thread (SDL releases it first if current on this one);
        // the window outlives it (see the struct docs).
        unsafe { SDL_GL_DestroyContext(self.context) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_size_round_trips() {
        let size = PixelSize::default();
        assert_eq!(size.load(), (0, 0));
        size.store(3840, 2160);
        assert_eq!(size.load(), (3840, 2160));
        size.store(i32::MAX, 1);
        assert_eq!(size.load(), (i32::MAX as u32, 1));
        size.store(-5, 7);
        assert_eq!(size.load(), (0, 7));
    }

    #[test]
    fn context_current_elsewhere_is_leaked_with_its_window() {
        let other = std::thread::spawn(|| thread::current().id())
            .join()
            .unwrap();
        let size = Arc::new(PixelSize::default());
        // The leak path makes no SDL calls, so null handles are never touched.
        drop(GlContext {
            window: std::ptr::null_mut(),
            context: std::ptr::null_mut(),
            size: Arc::clone(&size),
            current_on: Some(other),
        });
        assert!(
            Arc::strong_count(&size) > 1,
            "the window must see a live context"
        );
    }
}
