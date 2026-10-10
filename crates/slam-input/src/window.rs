use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use sdl3_sys::events::{
    SDL_EVENT_QUIT, SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED, SDL_Event, SDL_EventType, SDL_PushEvent,
    SDL_QuitEvent, SDL_WaitEvent,
};
use sdl3_sys::hints::{SDL_HINT_INVALID_PARAM_CHECKS, SDL_SetHint};
use sdl3_sys::init::{SDL_INIT_VIDEO, SDL_Init, SDL_Quit};
use sdl3_sys::video::{
    SDL_CreateWindow, SDL_DestroyWindow, SDL_GetCurrentVideoDriver, SDL_GetWindowSizeInPixels,
    SDL_SetWindowFullscreen, SDL_WINDOW_HIGH_PIXEL_DENSITY, SDL_WINDOW_OPENGL,
    SDL_WINDOW_RESIZABLE, SDL_Window, SDL_WindowFlags,
};

use crate::gl::{self, GlContext, PixelSize};
use crate::{EventSink, InputCounters, InputEvent, TickMapper, convert};

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
    #[error("the window was not created with OpenGL support")]
    NotOpenGl,
    #[error("a GlContext for this window already exists")]
    GlContextExists,
    #[error("OpenGL context creation failed: {0}")]
    GlContext(String),
    #[error("SDL_SetWindowFullscreen failed: {0}")]
    Fullscreen(String),
}

/// The SDL window and its event loop. Must be created and run on the OS main thread:
/// SDL treats the thread that calls `SDL_Init` as main, which Linux and Windows
/// tolerate from any thread but macOS will not. Not `Send`: SDL video calls are bound
/// to the thread that initialised SDL.
pub struct InputWindow {
    window: *mut SDL_Window,
    opengl: bool,
    /// Drawable size for the draw thread, updated by [`InputWindow::run`]. Shared with
    /// the [`GlContext`], if any: a strong count above one means the context is alive.
    size: Arc<PixelSize>,
    counters: Arc<InputCounters>,
    /// Whether [`QuitHandle`]s may push events; cleared when the window is dropped,
    /// before `SDL_Quit`.
    alive: Arc<Mutex<bool>>,
}

/// Asks [`InputWindow::run`] to return, from any thread. Outliving the window is
/// harmless: requests after the window is dropped do nothing.
#[derive(Clone)]
pub struct QuitHandle {
    alive: Arc<Mutex<bool>>,
}

impl QuitHandle {
    /// Queues a quit event, so `run` returns after the events queued before it.
    /// Returns whether the event was queued. Takes a lock: not for hot paths.
    pub fn request(&self) -> bool {
        let alive = self.alive.lock().unwrap_or_else(PoisonError::into_inner);
        if !*alive {
            return false;
        }
        let mut event = SDL_Event {
            quit: SDL_QuitEvent {
                r#type: SDL_EVENT_QUIT,
                ..Default::default()
            },
        };
        // SAFETY: SDL is initialised while `alive` is set, and the window's drop
        // waits for this lock before shutting SDL down. SDL_PushEvent is thread-safe.
        unsafe { SDL_PushEvent(&mut event) }
    }
}

impl InputWindow {
    /// A window without a GPU surface. A Wayland compositor does not show it (and so
    /// delivers it no input) until a frame is attached; use
    /// [`InputWindow::new_opengl`] for a window the renderer draws into.
    pub fn new(title: &str, width: i32, height: i32) -> Result<Self, WindowError> {
        Self::create(title, width, height, SDL_WindowFlags(0))
    }

    /// A window for the OpenGL 3.3 core renderer; create its context with
    /// [`InputWindow::create_gl_context`].
    pub fn new_opengl(title: &str, width: i32, height: i32) -> Result<Self, WindowError> {
        Self::create(title, width, height, SDL_WINDOW_OPENGL)
    }

    fn create(
        title: &str,
        width: i32,
        height: i32,
        extra: SDL_WindowFlags,
    ) -> Result<Self, WindowError> {
        let title = CString::new(title).map_err(|_| WindowError::InvalidTitle)?;
        if SDL_ACTIVE.swap(true, Ordering::AcqRel) {
            return Err(WindowError::AlreadyActive);
        }
        // Fast parameter checks only: full checks validate every window pointer in a
        // global registry under an rwlock, on each frame's swap (ADR-0018). Our
        // pointers are valid by construction. SDL_Quit resets hints, so set it on
        // every initialisation.
        // SAFETY: valid C strings; hints may be set before SDL_Init.
        unsafe { SDL_SetHint(SDL_HINT_INVALID_PARAM_CHECKS, c"1".as_ptr()) };
        // SAFETY: SDL is not initialised (guarded by `SDL_ACTIVE`).
        if !unsafe { SDL_Init(SDL_INIT_VIDEO) } {
            let err = WindowError::Init(sdl_error());
            // SAFETY: undoes the partial initialisation.
            unsafe { SDL_Quit() };
            SDL_ACTIVE.store(false, Ordering::Release);
            return Err(err);
        }
        let opengl = extra.0 & SDL_WINDOW_OPENGL.0 != 0;
        if opengl {
            // Attributes pick the EGL/GLX config at window creation, so set them first.
            gl::set_attributes();
        }
        let flags = SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY | extra;
        // SAFETY: SDL video is initialised; `title` is a valid C string.
        let window = unsafe { SDL_CreateWindow(title.as_ptr(), width, height, flags) };
        if window.is_null() {
            let err = WindowError::CreateWindow(sdl_error());
            // SAFETY: no window was created; shuts SDL down.
            unsafe { SDL_Quit() };
            SDL_ACTIVE.store(false, Ordering::Release);
            return Err(err);
        }
        let size = Arc::new(PixelSize::default());
        let (mut w, mut h) = (0, 0);
        // SAFETY: `window` is valid; `w` and `h` are valid out pointers.
        if unsafe { SDL_GetWindowSizeInPixels(window, &mut w, &mut h) } {
            size.store(w, h);
        }
        Ok(Self {
            window,
            opengl,
            size,
            counters: Arc::default(),
            alive: Arc::new(Mutex::new(true)),
        })
    }

    /// Creates the window's OpenGL 3.3 core context (with the debug flag in debug
    /// builds) and sets swap interval 0. The context is left current on no thread:
    /// the draw thread takes it over (ADR-0018). One context per window.
    pub fn create_gl_context(&mut self) -> Result<GlContext, WindowError> {
        if !self.opengl {
            return Err(WindowError::NotOpenGl);
        }
        if Arc::get_mut(&mut self.size).is_none() {
            return Err(WindowError::GlContextExists);
        }
        // SAFETY: `window` is valid, was created with SDL_WINDOW_OPENGL, and this is
        // the thread that initialised SDL.
        unsafe { GlContext::create(self.window, Arc::clone(&self.size)) }
            .map_err(WindowError::GlContext)
    }

    /// Switches between windowed and borderless desktop fullscreen. The change is
    /// asynchronous: the new drawable size arrives through [`InputWindow::run`].
    pub fn set_fullscreen(&mut self, fullscreen: bool) -> Result<(), WindowError> {
        // SAFETY: `window` is valid and this is the thread that initialised SDL. No
        // display mode is set, so fullscreen uses the desktop mode.
        if unsafe { SDL_SetWindowFullscreen(self.window, fullscreen) } {
            Ok(())
        } else {
            Err(WindowError::Fullscreen(sdl_error()))
        }
    }

    /// Name of the SDL video driver in use (`wayland`, `x11`, `windows`, ...).
    pub fn video_driver(&self) -> String {
        // SAFETY: SDL video is initialised while the window exists.
        let ptr = unsafe { SDL_GetCurrentVideoDriver() };
        if ptr.is_null() {
            return String::new();
        }
        // SAFETY: SDL returns a static NUL-terminated string.
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }

    /// Counters updated by [`InputWindow::run`]; share them with the thread that shows
    /// them.
    pub fn counters(&self) -> &Arc<InputCounters> {
        &self.counters
    }

    /// A handle that makes [`InputWindow::run`] return as if the window was closed.
    pub fn quit_handle(&self) -> QuitHandle {
        QuitHandle {
            alive: Arc::clone(&self.alive),
        }
    }

    /// Blocks on OS events until the window is closed or a [`QuitHandle`] asks. Keyboard and mouse events are
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
            self.counters.record_poll();
            // SAFETY: `type` is the common prefix of every SDL_Event variant.
            match SDL_EventType(unsafe { event.r#type }) {
                SDL_EVENT_QUIT => return Ok(()),
                SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED => {
                    // SAFETY: the event type says this is a window event.
                    let window = unsafe { event.window };
                    self.size.store(window.data1, window.data2);
                }
                _ => {
                    if dispatch(&event, sink, mapper) {
                        self.counters.record_queued();
                    }
                }
            }
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
        // `get_mut` synchronises with the drop of a GlContext on another thread, so
        // its SDL calls happen before the teardown below.
        *self.alive.lock().unwrap_or_else(PoisonError::into_inner) = false;
        if Arc::get_mut(&mut self.size).is_none() {
            // A GlContext still points at the window, possibly current on the draw
            // thread: leak the window and SDL rather than destroy them under it.
            return;
        }
        // SAFETY: `window` was created by `new` and is destroyed exactly once, on the
        // thread that owns SDL.
        unsafe {
            SDL_DestroyWindow(self.window);
            SDL_Quit();
        }
        SDL_ACTIVE.store(false, Ordering::Release);
    }
}

pub(crate) fn sdl_error() -> String {
    let ptr = sdl3_sys::error::SDL_GetError();
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: SDL returns a valid NUL-terminated string owned by SDL.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}
