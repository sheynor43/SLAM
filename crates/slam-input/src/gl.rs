//! The window's OpenGL context, handed to the draw thread (ADR-0018).

use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, ThreadId};

use sdl3_sys::video::{
    SDL_EGL_GetCurrentDisplay, SDL_EGL_GetProcAddress, SDL_EGL_SetAttributeCallbacks,
    SDL_EGLConfig, SDL_EGLDisplay, SDL_EGLint, SDL_GL_CONTEXT_DEBUG_FLAG, SDL_GL_CONTEXT_FLAGS,
    SDL_GL_CONTEXT_MAJOR_VERSION, SDL_GL_CONTEXT_MINOR_VERSION, SDL_GL_CONTEXT_PROFILE_CORE,
    SDL_GL_CONTEXT_PROFILE_MASK, SDL_GL_CreateContext, SDL_GL_DEPTH_SIZE, SDL_GL_DOUBLEBUFFER,
    SDL_GL_DestroyContext, SDL_GL_GetProcAddress, SDL_GL_MakeCurrent, SDL_GL_SetAttribute,
    SDL_GL_SetSwapInterval, SDL_GL_SwapWindow, SDL_GLContext, SDL_Window,
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

// EGL_IMG_context_priority.
const EGL_CONTEXT_PRIORITY_LEVEL_IMG: SDL_EGLint = 0x3100;
const EGL_CONTEXT_PRIORITY_HIGH_IMG: SDL_EGLint = 0x3101;
const EGL_CONTEXT_PRIORITY_MEDIUM_IMG: SDL_EGLint = 0x3102;
const EGL_CONTEXT_PRIORITY_LOW_IMG: SDL_EGLint = 0x3103;
const EGL_EXTENSIONS: SDL_EGLint = 0x3055;
const EGL_NONE: SDL_EGLint = 0x3038;
const PRIORITY_EXTENSION: &[u8] = b"EGL_IMG_context_priority";

/// GPU scheduling priority of a context, as granted by the driver
/// (`EGL_IMG_context_priority`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextPriority {
    Low,
    Medium,
    High,
}

impl ContextPriority {
    fn from_egl(value: SDL_EGLint) -> Option<Self> {
        match value {
            EGL_CONTEXT_PRIORITY_LOW_IMG => Some(Self::Low),
            EGL_CONTEXT_PRIORITY_MEDIUM_IMG => Some(Self::Medium),
            EGL_CONTEXT_PRIORITY_HIGH_IMG => Some(Self::High),
            _ => None,
        }
    }
}

/// GPU priority to ask for when creating a context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriorityRequest {
    /// High priority where the driver supports it (ADR-0075).
    High,
    /// The driver default; for comparisons.
    Default,
}

/// How the context's GPU priority was set up (ADR-0075).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PriorityOutcome {
    /// Not an EGL context or no `EGL_IMG_context_priority`: the driver default.
    Unsupported,
    /// The level the driver reports, which the process is allowed to get.
    Granted(ContextPriority),
    /// The driver reports high priority, but the process lacks `CAP_SYS_NICE`:
    /// amdgpu and i915 run it at normal priority all the same.
    NoPermission,
    /// Creating the context with high priority failed (the error is kept), so it
    /// was created without the attribute.
    Refused(String),
}

impl PriorityOutcome {
    fn new(refused: Option<String>, reported: Option<ContextPriority>, may_raise: bool) -> Self {
        match (refused, reported) {
            (Some(err), _) => Self::Refused(err),
            (None, Some(ContextPriority::High)) if !may_raise => Self::NoPermission,
            (None, Some(level)) => Self::Granted(level),
            (None, None) => Self::Unsupported,
        }
    }
}

const CAP_SYS_NICE: u32 = 23;

/// Whether the effective capability set in a `/proc/<pid>/status` text has
/// `CAP_SYS_NICE`. A missing or malformed line counts as no.
fn status_has_sys_nice(status: &str) -> bool {
    status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:"))
        .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
        .is_some_and(|caps| caps >> CAP_SYS_NICE & 1 == 1)
}

/// Whether the kernel checks `CAP_SYS_NICE` for a raised GPU priority under this GL
/// driver: Mesa's on Linux (amdgpu, i915 and xe do), where the driver reports the
/// requested level all the same. Other drivers are trusted.
fn kernel_checks_priority(linux: bool, gl_version: &[u8]) -> bool {
    linux && gl_version.windows(4).any(|w| w == b"Mesa")
}

/// Whether the process may get the priority the current context's driver reports.
///
/// # Safety
/// A GL context is current on this thread.
unsafe fn may_raise_priority() -> bool {
    type GlGetString = unsafe extern "C" fn(u32) -> *const c_char;
    const GL_VERSION: u32 = 0x1F02;
    // SAFETY: valid C string; the signature matches glGetString.
    let version = unsafe { SDL_GL_GetProcAddress(c"glGetString".as_ptr()) }
        .map(|f| unsafe { std::mem::transmute_copy::<_, GlGetString>(&f)(GL_VERSION) });
    let Some(version) = version.filter(|v| !v.is_null()) else {
        return true;
    };
    // SAFETY: glGetString returns a static NUL-terminated string.
    let version = unsafe { CStr::from_ptr(version) }.to_bytes();
    !kernel_checks_priority(cfg!(target_os = "linux"), version)
        || std::fs::read_to_string("/proc/self/status").is_ok_and(|s| status_has_sys_nice(&s))
}

/// Whether a space-separated extension list names `name`.
fn has_extension(list: &[u8], name: &[u8]) -> bool {
    list.split(|&b| b == b' ').any(|ext| ext == name)
}

type EglQueryString = unsafe extern "C" fn(SDL_EGLDisplay, SDL_EGLint) -> *const c_char;
type EglGetCurrentContext = unsafe extern "C" fn() -> *mut c_void;
type EglQueryContext =
    unsafe extern "C" fn(SDL_EGLDisplay, *mut c_void, SDL_EGLint, *mut SDL_EGLint) -> c_int;

/// An EGL function by name, or `None` when EGL is not loaded.
///
/// # Safety
/// `F` is the function's real signature.
unsafe fn egl_fn<F: Copy>(name: &CStr) -> Option<F> {
    // SAFETY: valid C string; returns null when EGL is not in use.
    let f = unsafe { SDL_EGL_GetProcAddress(name.as_ptr()) }?;
    // SAFETY: function pointers have one size; the signature is the caller's promise.
    Some(unsafe { std::mem::transmute_copy::<_, F>(&f) })
}

/// Whether `display` supports `EGL_IMG_context_priority`.
///
/// # Safety
/// `display` is an initialised EGL display.
unsafe fn display_supports_priority(display: SDL_EGLDisplay) -> bool {
    // SAFETY: the signature matches eglQueryString.
    let Some(query) = (unsafe { egl_fn::<EglQueryString>(c"eglQueryString") }) else {
        return false;
    };
    // SAFETY: `display` is initialised; EGL returns a static string or null.
    let list = unsafe { query(display, EGL_EXTENSIONS) };
    // SAFETY: non-null strings from EGL are NUL-terminated.
    !list.is_null()
        && has_extension(
            unsafe { CStr::from_ptr(list) }.to_bytes(),
            PRIORITY_EXTENSION,
        )
}

/// SDL's context attribute callback: asks for high priority when the display
/// supports it and records in `userdata` (a `Cell<bool>`) that it did. SDL frees the
/// returned array with SDL_free.
unsafe extern "C" fn context_attributes(
    userdata: *mut c_void,
    display: SDL_EGLDisplay,
    _config: SDL_EGLConfig,
) -> *mut SDL_EGLint {
    // SAFETY: SDL calls this with its initialised display.
    let high = unsafe { display_supports_priority(display) };
    // SAFETY: `create` passes a `Cell<bool>` that outlives the callback.
    unsafe { (*userdata.cast::<Cell<bool>>()).set(high) };
    let attribs: &[SDL_EGLint] = if high {
        &[
            EGL_CONTEXT_PRIORITY_LEVEL_IMG,
            EGL_CONTEXT_PRIORITY_HIGH_IMG,
            EGL_NONE,
        ]
    } else {
        &[EGL_NONE]
    };
    // SAFETY: plain allocation; SDL frees it. Null makes context creation fail.
    let out = unsafe { sdl3_sys::stdinc::SDL_malloc(size_of_val(attribs)) }.cast::<SDL_EGLint>();
    if !out.is_null() {
        // SAFETY: `out` holds `attribs.len()` values.
        unsafe { std::ptr::copy_nonoverlapping(attribs.as_ptr(), out, attribs.len()) };
    }
    out
}

/// Priority of the current EGL context, if the driver reports one.
///
/// # Safety
/// SDL video is initialised; a context was just made current on this thread.
unsafe fn current_priority() -> Option<ContextPriority> {
    // SAFETY: SDL video is initialised; null when not EGL.
    let display = unsafe { SDL_EGL_GetCurrentDisplay() };
    if display.is_null() || !unsafe { display_supports_priority(display) } {
        return None;
    }
    // SAFETY: the signatures match the EGL functions.
    let current = unsafe { egl_fn::<EglGetCurrentContext>(c"eglGetCurrentContext") }?;
    let query = unsafe { egl_fn::<EglQueryContext>(c"eglQueryContext") }?;
    let mut value = 0;
    // SAFETY: the display is initialised and the context is current.
    let ok = unsafe {
        query(
            display,
            current(),
            EGL_CONTEXT_PRIORITY_LEVEL_IMG,
            &mut value,
        )
    };
    if ok == 0 {
        return None;
    }
    ContextPriority::from_egl(value)
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
    priority: PriorityOutcome,
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
        request: PriorityRequest,
    ) -> Result<Self, String> {
        // SAFETY: guaranteed by the caller. CreateContext makes the context current on
        // this thread; the swap interval applies to it; then it is released.
        unsafe {
            // High priority via the context attribute callback (ADR-0075).
            let asked = Cell::new(false);
            if request == PriorityRequest::High {
                let userdata = (&raw const asked).cast_mut().cast();
                SDL_EGL_SetAttributeCallbacks(None, None, Some(context_attributes), userdata);
            }
            let mut context = SDL_GL_CreateContext(window);
            let mut refused = None;
            if context.is_null() && asked.get() {
                refused = Some(sdl_error());
                SDL_EGL_SetAttributeCallbacks(None, None, None, std::ptr::null_mut());
                context = SDL_GL_CreateContext(window);
            }
            SDL_EGL_SetAttributeCallbacks(None, None, None, std::ptr::null_mut());
            if context.is_null() {
                return Err(sdl_error());
            }
            let priority = PriorityOutcome::new(refused, current_priority(), may_raise_priority());
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
                priority,
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

    /// How the context's GPU priority was set up.
    pub fn priority(&self) -> &PriorityOutcome {
        &self.priority
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
    fn extension_list_matches_whole_names() {
        let list = b"EGL_KHR_image EGL_IMG_context_priority EGL_EXT_x";
        assert!(has_extension(list, PRIORITY_EXTENSION));
        assert!(!has_extension(
            b"EGL_IMG_context_priority_x",
            PRIORITY_EXTENSION
        ));
        assert!(!has_extension(b"", PRIORITY_EXTENSION));
    }

    #[test]
    fn priority_levels_map_from_egl() {
        assert_eq!(
            ContextPriority::from_egl(0x3101),
            Some(ContextPriority::High)
        );
        assert_eq!(
            ContextPriority::from_egl(0x3102),
            Some(ContextPriority::Medium)
        );
        assert_eq!(
            ContextPriority::from_egl(0x3103),
            Some(ContextPriority::Low)
        );
        assert_eq!(ContextPriority::from_egl(0), None);
    }

    #[test]
    fn sys_nice_is_read_from_effective_caps() {
        let status =
            |eff: &str| format!("Name:\tslam\nCapPrm:\t0000000000800000\nCapEff:\t{eff}\n");
        assert!(status_has_sys_nice(&status("0000000000800000")));
        assert!(status_has_sys_nice(&status("000001ffffffffff")));
        assert!(!status_has_sys_nice(&status("0000000000000000")));
        assert!(!status_has_sys_nice(&status("0000000000400000")));
        assert!(!status_has_sys_nice(&status("zz")));
        assert!(!status_has_sys_nice("Name:\tslam\n"));
    }

    #[test]
    fn only_mesa_on_linux_is_checked_by_the_kernel() {
        let mesa = b"4.6 (Core Profile) Mesa 26.2.5-arch3.1";
        assert!(kernel_checks_priority(true, mesa));
        assert!(!kernel_checks_priority(false, mesa));
        assert!(!kernel_checks_priority(true, b"4.6.0 NVIDIA 580.95.05"));
    }

    #[test]
    fn high_priority_without_permission_is_not_trusted() {
        use ContextPriority::*;
        let outcome = PriorityOutcome::new;
        assert_eq!(
            outcome(None, Some(High), false),
            PriorityOutcome::NoPermission
        );
        assert_eq!(
            outcome(None, Some(High), true),
            PriorityOutcome::Granted(High)
        );
        assert_eq!(
            outcome(None, Some(Medium), false),
            PriorityOutcome::Granted(Medium)
        );
        assert_eq!(outcome(None, None, true), PriorityOutcome::Unsupported);
        assert_eq!(
            outcome(Some("no".into()), Some(Medium), true),
            PriorityOutcome::Refused("no".into())
        );
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
            priority: PriorityOutcome::Unsupported,
        });
        assert!(
            Arc::strong_count(&size) > 1,
            "the window must see a live context"
        );
    }
}
