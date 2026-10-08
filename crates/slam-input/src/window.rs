use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};

use sdl3_sys::events::{SDL_EVENT_QUIT, SDL_Event, SDL_EventType, SDL_WaitEvent};
use sdl3_sys::init::{SDL_INIT_VIDEO, SDL_Init, SDL_Quit};
use sdl3_sys::surface::{SDL_FillSurfaceRect, SDL_MapSurfaceRGB};
use sdl3_sys::video::{
    SDL_CreateWindow, SDL_DestroyWindow, SDL_DestroyWindowSurface, SDL_GetWindowSurface,
    SDL_UpdateWindowSurface, SDL_WINDOW_HIGH_PIXEL_DENSITY, SDL_WINDOW_RESIZABLE, SDL_Window,
};

use crate::{EventSink, InputEvent, TickMapper, convert};

/// SDL supports a single initialisation per process at a time.
static SDL_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, thiserror::Error)]
pub enum WindowError {
    #[error("an InputWindow already exists")]
    AlreadyActive,
    #[error("window title contains a NUL byte")]
    InvalidTitle,
    #[error("SDL_Init failed: {0}")]
    Init(String),
    #[error("SDL_CreateWindow failed: {0}")]
    CreateWindow(String),
    #[error("SDL_WaitEvent failed: {0}")]
    WaitEvent(String),
    #[error("window surface fill failed: {0}")]
    Fill(String),
}

/// The SDL window and its event loop. Must be created and run on the OS main thread:
/// SDL treats the thread that calls `SDL_Init` as main, which Linux and Windows
/// tolerate from any thread but macOS will not. Not `Send`: SDL video calls are bound
/// to the thread that initialised SDL.
pub struct InputWindow {
    window: *mut SDL_Window,
    /// A software surface from [`InputWindow::fill_placeholder`] is attached.
    has_surface: bool,
}

impl InputWindow {
    pub fn new(title: &str, width: i32, height: i32) -> Result<Self, WindowError> {
        let title = CString::new(title).map_err(|_| WindowError::InvalidTitle)?;
        if SDL_ACTIVE.swap(true, Ordering::AcqRel) {
            return Err(WindowError::AlreadyActive);
        }
        // SAFETY: SDL is not initialised (guarded by `SDL_ACTIVE`).
        if !unsafe { SDL_Init(SDL_INIT_VIDEO) } {
            let err = WindowError::Init(sdl_error());
            // SAFETY: undoes the partial initialisation.
            unsafe { SDL_Quit() };
            SDL_ACTIVE.store(false, Ordering::Release);
            return Err(err);
        }
        let flags = SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY;
        // SAFETY: SDL video is initialised; `title` is a valid C string.
        let window = unsafe { SDL_CreateWindow(title.as_ptr(), width, height, flags) };
        if window.is_null() {
            let err = WindowError::CreateWindow(sdl_error());
            // SAFETY: no window was created; shuts SDL down.
            unsafe { SDL_Quit() };
            SDL_ACTIVE.store(false, Ordering::Release);
            return Err(err);
        }
        Ok(Self {
            window,
            has_surface: false,
        })
    }

    /// Fills the window with a solid colour through SDL's software surface.
    ///
    /// Temporary, until the renderer owns the window: a Wayland compositor does not
    /// map (show) a window, and so delivers it no input, before a frame is attached.
    /// SDL forbids mixing this surface with 3D APIs on one window: the renderer must
    /// call [`InputWindow::release_placeholder`] before creating its GPU surface.
    pub fn fill_placeholder(&mut self, r: u8, g: u8, b: u8) -> Result<(), WindowError> {
        // SAFETY: `window` is valid; the surface belongs to it and is used immediately.
        unsafe {
            let surface = SDL_GetWindowSurface(self.window);
            self.has_surface |= !surface.is_null();
            if surface.is_null()
                || !SDL_FillSurfaceRect(
                    surface,
                    std::ptr::null(),
                    SDL_MapSurfaceRGB(surface, r, g, b),
                )
                || !SDL_UpdateWindowSurface(self.window)
            {
                return Err(WindowError::Fill(sdl_error()));
            }
        }
        Ok(())
    }

    /// Detaches the software surface created by [`InputWindow::fill_placeholder`], if
    /// any, so a GPU surface can be created on the window.
    pub fn release_placeholder(&mut self) {
        if self.has_surface {
            // SAFETY: `window` is valid and has a window surface attached.
            unsafe { SDL_DestroyWindowSurface(self.window) };
            self.has_surface = false;
        }
    }

    /// Blocks on OS events until the window is closed. Keyboard and mouse events are
    /// stamped with `mapper` and pushed into `sink`; when the ring is full they are
    /// dropped and counted, never waited on. No allocations per event. `mapper` is
    /// recalibrated first, since SDL re-bases its clock on every initialisation.
    pub fn run<E, S>(
        &mut self,
        sink: &mut EventSink,
        mapper: &mut TickMapper<E, S>,
    ) -> Result<(), WindowError>
    where
        E: FnMut() -> u64,
        S: FnMut() -> u64,
    {
        mapper.calibrate();
        let mut event = SDL_Event::default();
        loop {
            // SAFETY: SDL is initialised on this thread; `event` is a valid out pointer.
            if !unsafe { SDL_WaitEvent(&mut event) } {
                return Err(WindowError::WaitEvent(sdl_error()));
            }
            // SAFETY: `type` is the common prefix of every SDL_Event variant.
            if SDL_EventType(unsafe { event.r#type }) == SDL_EVENT_QUIT {
                return Ok(());
            }
            dispatch(&event, sink, mapper);
        }
    }
}

/// Per-event work of [`InputWindow::run`]: convert, stamp in engine time, push.
/// Returns whether an event was queued. Public for allocation tests.
#[doc(hidden)]
pub fn dispatch<E, S>(
    event: &SDL_Event,
    sink: &mut EventSink,
    mapper: &mut TickMapper<E, S>,
) -> bool
where
    E: FnMut() -> u64,
    S: FnMut() -> u64,
{
    match convert(event) {
        Some((timestamp, kind)) => sink.push(InputEvent {
            time_ns: mapper.map(timestamp),
            kind,
        }),
        None => false,
    }
}

impl Drop for InputWindow {
    fn drop(&mut self) {
        // SAFETY: `window` was created by `new` and is destroyed exactly once, on the
        // thread that owns SDL.
        unsafe {
            SDL_DestroyWindow(self.window);
            SDL_Quit();
        }
        SDL_ACTIVE.store(false, Ordering::Release);
    }
}

fn sdl_error() -> String {
    let ptr = sdl3_sys::error::SDL_GetError();
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: SDL returns a valid NUL-terminated string owned by SDL.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}
