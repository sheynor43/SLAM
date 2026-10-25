use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use sdl3_sys::events::{
    SDL_EVENT_KEY_DOWN, SDL_EVENT_MOUSE_MOTION, SDL_EVENT_QUIT, SDL_EVENT_WINDOW_ENTER_FULLSCREEN,
    SDL_EVENT_WINDOW_FOCUS_GAINED, SDL_EVENT_WINDOW_FOCUS_LOST, SDL_EVENT_WINDOW_LEAVE_FULLSCREEN,
    SDL_EVENT_WINDOW_MOUSE_ENTER, SDL_EVENT_WINDOW_MOUSE_LEAVE,
    SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED, SDL_EVENT_WINDOW_RESIZED, SDL_Event, SDL_EventType,
    SDL_PushEvent, SDL_QuitEvent, SDL_RegisterEvents, SDL_WaitEvent,
};
use sdl3_sys::hints::{SDL_HINT_INVALID_PARAM_CHECKS, SDL_HINT_MOUSE_AUTO_CAPTURE, SDL_SetHint};
use sdl3_sys::init::{SDL_INIT_VIDEO, SDL_Init, SDL_Quit};
use sdl3_sys::keyboard::{SDL_GetKeyboardState, SDL_StartTextInput, SDL_StopTextInput};
use sdl3_sys::mouse::{
    SDL_GetMouseState, SDL_HideCursor, SDL_SetWindowRelativeMouseMode, SDL_ShowCursor,
    SDL_WarpMouseInWindow,
};
use sdl3_sys::video::{
    SDL_CreateWindow, SDL_DestroyWindow, SDL_GetCurrentVideoDriver, SDL_GetWindowFlags,
    SDL_GetWindowSize, SDL_GetWindowSizeInPixels, SDL_SetWindowFullscreen, SDL_ShowWindow,
    SDL_WINDOW_FULLSCREEN, SDL_WINDOW_HIDDEN, SDL_WINDOW_HIGH_PIXEL_DENSITY,
    SDL_WINDOW_INPUT_FOCUS, SDL_WINDOW_MOUSE_FOCUS, SDL_WINDOW_OPENGL, SDL_WINDOW_RESIZABLE,
    SDL_Window, SDL_WindowFlags,
};

use crate::dialog::{self, FolderDialog};
use crate::gl::{self, GlContext, PixelSize};
use crate::mode::{WindowMode, WindowScale, WindowSettings, WindowState, cycles_mode, only_held};
use crate::relative::{Change, MouseSettings, RelativeMouse};
use crate::{
    EventSink, InputCounters, InputEvent, InputKind, TickMapper, convert, convert_text, windowing,
};

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
    /// Whether the OS cursor should be hidden over the window, set by any thread.
    cursor_hidden: Arc<AtomicBool>,
    /// Whether it is hidden now.
    cursor_applied: bool,
    /// Whether typed text should be delivered, set by any thread.
    text_wanted: Arc<AtomicBool>,
    /// Whether SDL delivers it now.
    text_applied: bool,
    /// Relative mode and sensitivity, set by any thread (ADR-0069).
    mouse_settings: MouseSettings,
    /// The relative mode setting applied last.
    relative_applied: bool,
    relative: RelativeMouse,
    /// The SDL event type of [`FolderDialog`] requests; 0 when none could be registered.
    dialog_event: u32,
    /// The SDL event type that wakes the window for a new [`WindowState`]; 0 when none
    /// could be registered.
    wake_event: u32,
    /// Mode, display and sizes, set by any thread (ADR-0070).
    window_settings: WindowSettings,
    /// The [`WindowState`] word applied last.
    state_applied: u64,
    /// Pixels per point, for other threads.
    scale: WindowScale,
    /// Whether the video driver is Wayland (fullscreen sizes are in pixels there).
    wayland: bool,
}

/// Shows or hides the OS cursor over the window, from any thread, without locks. The
/// window applies the wish before waiting for its next event, so a change takes effect
/// after the next event at the latest; until then a still pointer keeps its look. Set the
/// state wanted at the start before [`InputWindow::run`]. The default handle belongs to no
/// window: wishes on it change nothing (for tests).
#[derive(Clone, Default)]
pub struct CursorVisibility {
    hidden: Arc<AtomicBool>,
}

impl CursorVisibility {
    /// Asks for the OS cursor to be hidden over the window (the game draws its own) or
    /// shown.
    pub fn set_hidden(&self, hidden: bool) {
        self.hidden.store(hidden, Ordering::Relaxed);
    }

    /// The last wish.
    pub fn is_hidden(&self) -> bool {
        self.hidden.load(Ordering::Relaxed)
    }
}

/// Turns typed text ([`crate::InputKind::Text`]) on or off, from any thread, without
/// locks (ADR-0065). Like [`CursorVisibility`], the window applies the wish before
/// waiting for its next event, so a change takes effect after the next event at the
/// latest. Off at start: SDL then sends no text and shows no on-screen keyboard or IME.
/// The default handle belongs to no window (for tests).
#[derive(Clone, Default)]
pub struct TextInput {
    wanted: Arc<AtomicBool>,
}

impl TextInput {
    /// Asks for typed text to be delivered or not.
    pub fn set_enabled(&self, enabled: bool) {
        self.wanted.store(enabled, Ordering::Relaxed);
    }

    /// The last wish.
    pub fn is_enabled(&self) -> bool {
        self.wanted.load(Ordering::Relaxed)
    }
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

    /// Like [`InputWindow::new_opengl`], but in `state`: the window is created hidden and
    /// shown once in its mode, so a fullscreen start does not flash a window first.
    pub fn new_opengl_in(title: &str, state: WindowState) -> Result<Self, WindowError> {
        let [w, h] = state.windowed_size.map(i32::from);
        let mut window = Self::create(title, w, h, SDL_WINDOW_OPENGL | SDL_WINDOW_HIDDEN)?;
        window.window_settings.set(state);
        // Applied even when it matches the window as created, so a windowed start is
        // centred on its display too.
        window.state_applied = !window.window_settings.word();
        window.apply_window_state();
        // SAFETY: `window` is valid and this is the thread that initialised SDL.
        if !unsafe { SDL_ShowWindow(window.window) } {
            return Err(WindowError::CreateWindow(sdl_error()));
        }
        Ok(window)
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
        // SAFETY: SDL is initialised. Returns 0 when no event types are left.
        let first_event = unsafe { SDL_RegisterEvents(2) };
        let (dialog_event, wake_event) = match first_event {
            0 => (0, 0),
            first => (first, first + 1),
        };
        let scale = WindowScale::default();
        let (mut pw, mut ww) = (0, 0);
        // SAFETY: `window` is valid; the out pointers are valid. On failure the scale
        // stays 1 until the first resize.
        unsafe {
            SDL_GetWindowSizeInPixels(window, &mut pw, ptr::null_mut());
            SDL_GetWindowSize(window, &mut ww, ptr::null_mut());
        }
        scale.store(pw, ww);
        let wayland = video_driver() == "wayland";
        let alive = Arc::new(Mutex::new(true));
        let clamp = |v: i32| v.clamp(0, i32::from(u16::MAX)) as u16;
        // What SDL_CreateWindow did; `new_opengl_in` replaces it before showing.
        let created = WindowState {
            mode: WindowMode::Windowed,
            windowed_size: [clamp(width), clamp(height)],
            ..WindowState::default()
        };
        let window_settings = WindowSettings::new(created, Some(Arc::clone(&alive)), wake_event);
        // SAFETY: `window` is valid.
        let window_flags = unsafe { SDL_GetWindowFlags(window) };
        let (mut w, mut h) = (width, height);
        // SAFETY: `window` is valid; `w` and `h` are valid out pointers. On failure the
        // requested size stands.
        unsafe { SDL_GetWindowSize(window, &mut w, &mut h) };
        let mut relative = RelativeMouse::new(
            window_flags.0 & SDL_WINDOW_INPUT_FOCUS.0 != 0,
            window_flags.0 & SDL_WINDOW_MOUSE_FOCUS.0 != 0,
            [w as f32, h as f32],
        );
        // Off at this point, so nothing to apply.
        relative.set_confined(window_flags.0 & SDL_WINDOW_FULLSCREEN.0 != 0);
        Ok(Self {
            window,
            opengl,
            size,
            counters: Arc::default(),
            alive,
            cursor_hidden: Arc::default(),
            cursor_applied: false,
            text_wanted: Arc::default(),
            text_applied: false,
            mouse_settings: MouseSettings::default(),
            relative_applied: false,
            relative,
            dialog_event,
            wake_event,
            state_applied: window_settings.word(),
            window_settings,
            scale,
            wayland,
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
        video_driver()
    }

    /// A handle that sets the window mode, display and sizes (ADR-0070).
    pub fn window_settings(&self) -> WindowSettings {
        self.window_settings.clone()
    }

    /// The window's size in points, as SDL knows it now; `(0, 0)` on failure.
    pub fn size_in_points(&self) -> (i32, i32) {
        let (mut w, mut h) = (0, 0);
        // SAFETY: `window` is valid; `w` and `h` are valid out pointers.
        unsafe { SDL_GetWindowSize(self.window, &mut w, &mut h) };
        (w, h)
    }

    /// A handle that reads the window's pixels per point.
    pub fn window_scale(&self) -> WindowScale {
        self.scale.clone()
    }

    /// Applies the wished [`WindowState`] if it changed since the last time.
    fn apply_window_state(&mut self) {
        let word = self.window_settings.word();
        if word == self.state_applied {
            return;
        }
        self.state_applied = word;
        // SAFETY: `window` is valid and this is the thread that initialised SDL.
        unsafe { windowing::apply(self.window, WindowState::unpack(word), self.wayland) };
        // The mode change may end in a size SDL reports through events, or not at all
        // when it is unchanged: read the scale now too.
        self.store_scale();
    }

    /// Publishes pixels per point from the window's current sizes.
    fn store_scale(&self) {
        let (mut pixels, mut points) = (0, 0);
        // SAFETY: `window` is valid; the out pointers are valid or null. On failure the
        // zero sizes are ignored.
        unsafe {
            SDL_GetWindowSizeInPixels(self.window, &mut pixels, ptr::null_mut());
            SDL_GetWindowSize(self.window, &mut points, ptr::null_mut());
        }
        self.scale.store(pixels, points);
    }

    /// Counters updated by [`InputWindow::run`]; share them with the thread that shows
    /// them.
    pub fn counters(&self) -> &Arc<InputCounters> {
        &self.counters
    }

    /// A handle that shows or hides the OS cursor over the window.
    pub fn cursor_visibility(&self) -> CursorVisibility {
        CursorVisibility {
            hidden: Arc::clone(&self.cursor_hidden),
        }
    }

    /// Hides or shows the OS cursor as [`CursorVisibility`] last asked.
    fn apply_cursor(&mut self) {
        let hidden = self.cursor_hidden.load(Ordering::Relaxed);
        if hidden == self.cursor_applied {
            return;
        }
        // SAFETY: SDL video is initialised on this thread, which owns the window.
        let done = unsafe {
            if hidden {
                SDL_HideCursor()
            } else {
                SDL_ShowCursor()
            }
        };
        // A failure is tried again after the next event.
        if done {
            self.cursor_applied = hidden;
        }
    }

    /// A handle that turns typed text on and off.
    pub fn text_input(&self) -> TextInput {
        TextInput {
            wanted: Arc::clone(&self.text_wanted),
        }
    }

    /// Starts or stops SDL text input as [`TextInput`] last asked.
    fn apply_text_input(&mut self) {
        let wanted = self.text_wanted.load(Ordering::Relaxed);
        if wanted == self.text_applied {
            return;
        }
        // SAFETY: SDL video is initialised on this thread, which owns the window.
        let done = unsafe {
            if wanted {
                SDL_StartTextInput(self.window)
            } else {
                SDL_StopTextInput(self.window)
            }
        };
        // A failure is tried again after the next event.
        if done {
            self.text_applied = wanted;
        }
    }

    /// A handle that turns relative mouse mode on and sets its sensitivity.
    pub fn mouse_settings(&self) -> MouseSettings {
        self.mouse_settings.clone()
    }

    /// Applies the relative mode setting and the OS cursor's visibility to
    /// [`RelativeMouse`]. Without the setting SDL captures the mouse while a button is
    /// held (its default); with it, capture is off, since SDL then sends no
    /// enter/leave and relative mode would not come back, as osu-framework's
    /// `MouseAutoCapture`.
    fn apply_mouse(&mut self) {
        let relative = self.mouse_settings.is_relative();
        if relative != self.relative_applied {
            let capture = if relative { c"0" } else { c"1" };
            // SAFETY: valid C strings; hints may be set at any time.
            unsafe { SDL_SetHint(SDL_HINT_MOUSE_AUTO_CAPTURE, capture.as_ptr()) };
            self.relative_applied = relative;
            let change = self.relative.set_enabled(relative);
            self.apply_change(change);
        }
        let change = self.relative.set_cursor_hidden(self.cursor_applied);
        self.apply_change(change);
    }

    /// Does what [`RelativeMouse`] asked: relative mode first, then the OS cursor.
    fn apply_change(&mut self, change: Change) {
        if let Some(on) = change.relative {
            // SAFETY: `window` is valid and this is the thread that owns it.
            let done = unsafe { SDL_SetWindowRelativeMouseMode(self.window, on) };
            if on && !done {
                self.relative.rejected();
            }
        }
        if let Some([x, y]) = change.warp {
            // SAFETY: `window` is valid and this is the thread that owns it.
            unsafe { SDL_WarpMouseInWindow(self.window, x, y) };
        }
    }

    /// Window events [`RelativeMouse`] follows. Returns whether `event` was one.
    fn follow_window(&mut self, event: &SDL_Event) -> bool {
        // SAFETY: `type` is the common prefix of every SDL_Event variant.
        let change = match SDL_EventType(unsafe { event.r#type }) {
            SDL_EVENT_WINDOW_FOCUS_GAINED => self.relative.set_focused(true),
            SDL_EVENT_WINDOW_FOCUS_LOST => self.relative.set_focused(false),
            SDL_EVENT_WINDOW_MOUSE_ENTER => self.relative.set_cursor_in_window(true),
            SDL_EVENT_WINDOW_MOUSE_LEAVE => self.relative.set_cursor_in_window(false),
            SDL_EVENT_WINDOW_ENTER_FULLSCREEN => self.relative.set_confined(true),
            SDL_EVENT_WINDOW_LEAVE_FULLSCREEN => self.relative.set_confined(false),
            SDL_EVENT_WINDOW_RESIZED => {
                // SAFETY: the event type says this is a window event.
                let window = unsafe { event.window };
                self.relative
                    .set_size(window.data1 as f32, window.data2 as f32);
                self.store_scale();
                Change::default()
            }
            _ => return false,
        };
        self.apply_change(change);
        true
    }

    /// A handle that makes [`InputWindow::run`] return as if the window was closed.
    pub fn quit_handle(&self) -> QuitHandle {
        QuitHandle {
            alive: Arc::clone(&self.alive),
        }
    }

    /// A handle that shows the OS folder picker over this window (ADR-0064).
    pub fn folder_dialog(&self) -> FolderDialog {
        FolderDialog::new(Arc::clone(&self.alive), self.dialog_event)
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
            self.apply_cursor();
            self.apply_text_input();
            self.apply_mouse();
            self.apply_window_state();
            // SAFETY: SDL is initialised on this thread; `event` is a valid out pointer.
            if !unsafe { SDL_WaitEvent(&mut event) } {
                return Err(WindowError::WaitEvent(sdl_error()));
            }
            self.counters.record_poll();
            // SAFETY: `type` is the common prefix of every SDL_Event variant.
            if self.dialog_event != 0 && unsafe { event.r#type } == self.dialog_event {
                // SAFETY: only `FolderDialog::open` queues events of this registered type,
                // each handled once here, on the thread that owns SDL and the window.
                unsafe { dialog::show(self.window, &event) };
                continue;
            }
            // SAFETY: `type` is the common prefix of every SDL_Event variant.
            if self.wake_event != 0 && unsafe { event.r#type } == self.wake_event {
                // A WindowSettings wake-up: the new state is applied at the loop's top.
                continue;
            }
            // SAFETY: `type` is the common prefix of every SDL_Event variant.
            match SDL_EventType(unsafe { event.r#type }) {
                SDL_EVENT_QUIT => return Ok(()),
                SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED => {
                    // SAFETY: the event type says this is a window event.
                    let window = unsafe { event.window };
                    self.size.store(window.data1, window.data2);
                    self.store_scale();
                }
                SDL_EVENT_KEY_DOWN if is_mode_key(&event) => {
                    // SAFETY: the event type says this is a keyboard event.
                    let key = unsafe { event.key };
                    // Swallowed, repeats too, as osu-framework handles the binding before
                    // the game sees it: Alt+Enter must not also press Enter.
                    if !key.repeat {
                        let state = self.window_settings.cycle_mode();
                        self.apply_window_state();
                        let queued = sink.push(InputEvent {
                            time_ns: mapper.map(key.timestamp),
                            kind: InputKind::WindowMode(state.mode),
                        });
                        if queued {
                            self.counters.record_queued();
                        }
                    }
                }
                SDL_EVENT_MOUSE_MOTION => {
                    // SAFETY: the event type says this is a motion event.
                    let motion = unsafe { &mut event.motion };
                    let sensitivity = self.mouse_settings.sensitivity();
                    let ([x, y], change) = self.relative.motion(
                        [motion.x, motion.y],
                        [motion.xrel, motion.yrel],
                        sensitivity,
                    );
                    // Consumers see the position the game tracks (ADR-0069).
                    motion.x = x;
                    motion.y = y;
                    // When relative mode turns off, SDL first restores the OS cursor to
                    // its own position (kept inside the window), then the warp below
                    // moves it to ours: one motion event jumps, as in osu-framework.
                    // SAFETY: `event` came from SDL_WaitEvent just now; the next poll is
                    // after this call.
                    if unsafe { dispatch(&event, sink, mapper) } {
                        self.counters.record_queued();
                    }
                    self.apply_change(change);
                }
                _ if self.follow_window(&event) => {}
                _ => {
                    // SAFETY: `event` came from SDL_WaitEvent just now; the next poll is
                    // after this call.
                    if unsafe { dispatch(&event, sink, mapper) } {
                        self.counters.record_queued();
                    }
                }
            }
        }
    }
}

/// Per-event work of [`InputWindow::run`]: convert, stamp in engine time, push.
/// Returns whether an event was queued (for text split into chunks, whether any was).
/// Public for allocation tests.
///
/// # Safety
/// As [`convert_text`]: a text event's pointer must be valid for the call.
#[doc(hidden)]
pub unsafe fn dispatch<E, S>(
    event: &SDL_Event,
    sink: &mut EventSink,
    mapper: &mut TickMapper<E, S>,
) -> bool
where
    E: FnMut() -> u64,
    S: FnMut() -> u64,
{
    if let Some((timestamp, kind)) = convert(event) {
        return sink.push(InputEvent {
            time_ns: mapper.map(timestamp),
            kind,
        });
    }
    // SAFETY: passed on from the caller.
    let Some((timestamp, chunks)) = (unsafe { convert_text(event) }) else {
        return false;
    };
    let time_ns = mapper.map(timestamp);
    let mut queued = false;
    for kind in chunks {
        queued |= sink.push(InputEvent { time_ns, kind });
    }
    queued
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
        // Leave the OS cursor where the game's was, as osu-framework's `Window.Exited`.
        let change = self.relative.set_enabled(false);
        self.apply_change(change);
        // SAFETY: SDL is still initialised on this thread, and no request is shown after
        // `alive` was cleared above.
        if self.dialog_event != 0 {
            unsafe { dialog::fail_queued(self.dialog_event) };
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

/// Whether a key-down event is Alt+Enter or F11 with nothing else held (ADR-0070).
fn is_mode_key(event: &SDL_Event) -> bool {
    // SAFETY: called for key-down events only, so `key` is the variant SDL filled.
    let key = unsafe { event.key };
    let Ok(scancode) = u16::try_from(key.scancode.0) else {
        return false;
    };
    if !cycles_mode(scancode, key.r#mod.0) {
        return false;
    }
    let mut count = 0;
    // SAFETY: SDL video is initialised on this thread (the event came from it). The
    // array is SDL's, valid for the program's life and `count` long.
    let pressed = unsafe {
        let state = SDL_GetKeyboardState(&mut count);
        if state.is_null() {
            return true;
        }
        std::slice::from_raw_parts(state, usize::try_from(count).unwrap_or(0))
    };
    // SAFETY: null positions are allowed; SDL is initialised on this thread.
    let buttons = unsafe { SDL_GetMouseState(ptr::null_mut(), ptr::null_mut()) };
    buttons.0 == 0 && only_held(scancode, pressed)
}

fn video_driver() -> String {
    // SAFETY: SDL video is initialised while a window exists.
    let ptr = unsafe { SDL_GetCurrentVideoDriver() };
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: SDL returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_visibility_handles_share_one_wish() {
        let a = CursorVisibility {
            hidden: Arc::default(),
        };
        let b = a.clone();
        assert!(!a.is_hidden());
        b.set_hidden(true);
        assert!(a.is_hidden());
    }

    #[test]
    fn text_input_handles_share_one_wish() {
        let a = TextInput::default();
        let b = a.clone();
        assert!(!a.is_enabled());
        b.set_enabled(true);
        assert!(a.is_enabled());
        a.set_enabled(false);
        assert!(!b.is_enabled());
    }
}
