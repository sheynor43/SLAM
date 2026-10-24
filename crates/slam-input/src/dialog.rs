//! The OS folder picker (ADR-0064). SDL shows it only from the main thread, so a
//! [`FolderDialog`] handle queues a request as an SDL user event and
//! [`crate::InputWindow::run`] opens the dialog when it gets there. SDL answers on any
//! thread; the answer goes to the request's callback, not through the event ring: a path
//! is allocated and not `Copy`.
//!
//! On Linux SDL prefers the desktop portal. A broken portal (its file chooser backend
//! crashing, common outside GNOME and KDE) answers a request like a cancel, after seconds,
//! and SDL does not try zenity then; so zenity is used whenever it is installed.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use sdl3_sys::dialog::SDL_ShowOpenFolderDialog;
use sdl3_sys::events::{SDL_Event, SDL_GETEVENT, SDL_PeepEvents, SDL_PushEvent, SDL_UserEvent};
use sdl3_sys::hints::{SDL_HINT_FILE_DIALOG_DRIVER, SDL_SetHint};
use sdl3_sys::video::SDL_Window;

use crate::window::sdl_error;

/// How a folder pick ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderPick {
    Chosen(PathBuf),
    /// The player closed the dialog without choosing.
    Cancelled,
    /// No dialog could be shown (on Linux: neither the desktop portal nor zenity), or it
    /// failed; SDL's message.
    Failed(String),
}

/// Receives the answer of a dialog, once, on whichever thread SDL answers on. Must not
/// panic: it runs inside a C callback, where a panic aborts.
pub type FolderCallback = Box<dyn FnOnce(FolderPick) + Send>;

/// Whether SDL chooses between the desktop portal and zenity.
const PORTAL_OR_ZENITY: bool = cfg!(all(unix, not(target_os = "macos")));

/// What a queued user event carries to the main thread.
struct Request {
    start: Option<CString>,
    done: FolderCallback,
}

/// Opens the OS folder picker from any thread. Outliving the window is harmless: requests
/// after the window is dropped fail at once. The default handle belongs to no window
/// (for tests): every request fails.
#[derive(Clone, Default)]
pub struct FolderDialog {
    alive: Arc<Mutex<bool>>,
    /// The registered SDL event type of requests; 0 when SDL had none left.
    event_type: u32,
}

impl FolderDialog {
    pub(crate) fn new(alive: Arc<Mutex<bool>>, event_type: u32) -> Self {
        Self { alive, event_type }
    }

    /// Asks the window to show a folder picker starting at `start` (the OS default when
    /// `None`). `done` gets the answer; when the request cannot be queued it gets
    /// [`FolderPick::Failed`] before this returns. Requests from several threads may show
    /// several dialogs. Allocates and takes a lock: not for hot paths. If the window
    /// closes before the request is shown, `done` gets [`FolderPick::Failed`] then.
    pub fn open(&self, start: Option<&Path>, done: FolderCallback) {
        // A start with a NUL byte cannot be passed to SDL: start at the OS default.
        let start = start.and_then(|path| CString::new(path_bytes(path)).ok());
        let request = Box::into_raw(Box::new(Request { start, done }));
        let alive = self.alive.lock().unwrap_or_else(PoisonError::into_inner);
        let queued = *alive && self.event_type != 0 && {
            let mut event = SDL_Event {
                user: SDL_UserEvent {
                    r#type: self.event_type,
                    data1: request.cast(),
                    ..Default::default()
                },
            };
            // SAFETY: SDL is initialised while `alive` is set, and the window's drop waits
            // for this lock before shutting SDL down. SDL_PushEvent is thread-safe.
            unsafe { SDL_PushEvent(&mut event) }
        };
        drop(alive);
        if !queued {
            // SAFETY: the event carrying the pointer was not queued, so it is still ours.
            let request = unsafe { Box::from_raw(request) };
            let reason = if self.event_type == 0 {
                "no SDL event type for dialog requests".to_owned()
            } else {
                "the window is closed or SDL's event queue is full".to_owned()
            };
            (request.done)(FolderPick::Failed(reason));
        }
    }
}

/// Shows the dialog a request event asks for, parented to `window`. Main thread only.
///
/// # Safety
/// `event` must be a user event queued by [`FolderDialog::open`], handled once; SDL video
/// must be initialised on this thread and `window` valid.
pub(crate) unsafe fn show(window: *mut SDL_Window, event: &SDL_Event) {
    // Looked up on every request: `SDL_Quit` resets hints, and zenity may be installed
    // meanwhile.
    if PORTAL_OR_ZENITY {
        let path = std::env::var_os("PATH").unwrap_or_default();
        if on_path(&path, "zenity", &|p| p.is_file()) {
            // SAFETY: valid C strings, on the thread that owns SDL.
            unsafe { SDL_SetHint(SDL_HINT_FILE_DIALOG_DRIVER, c"zenity".as_ptr()) };
        }
    }
    // SAFETY: by the contract, `data1` is the `Request` that `open` leaked for this event.
    let request = unsafe { Box::from_raw(event.user.data1.cast::<Request>()) };
    let Request { start, done } = *request;
    let start = start.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
    let userdata = Box::into_raw(Box::new(done));
    // SAFETY: main thread with SDL video initialised and a valid window (the contract).
    // SDL copies `start` before returning and calls `answer` exactly once with
    // `userdata`, which `answer` frees.
    unsafe { SDL_ShowOpenFolderDialog(Some(answer), userdata.cast(), window, start, false) };
}

/// Whether an executable `name` is in one of the directories of `path` (a `PATH` value).
fn on_path(path: &std::ffi::OsStr, name: &str, is_file: &dyn Fn(&Path) -> bool) -> bool {
    std::env::split_paths(path).any(|dir| is_file(&dir.join(name)))
}

/// Fails the requests still queued, so their callbacks are freed. Main thread only.
///
/// # Safety
/// SDL must be initialised on this thread, and `event_type` registered for
/// [`FolderDialog`] requests, none of which is handled after this.
pub(crate) unsafe fn fail_queued(event_type: u32) {
    let mut event = SDL_Event::default();
    // SAFETY: SDL is initialised on this thread; `event` is a valid out pointer for one.
    while unsafe { SDL_PeepEvents(&mut event, 1, SDL_GETEVENT, event_type, event_type) } == 1 {
        // SAFETY: events of this type carry the `Request` that `open` leaked, taken once.
        let request = unsafe { Box::from_raw(event.user.data1.cast::<Request>()) };
        (request.done)(FolderPick::Failed("the window was closed".to_owned()));
    }
}

/// SDL's callback: hands the pick to the request's callback and frees it.
unsafe extern "C" fn answer(userdata: *mut c_void, filelist: *const *const c_char, _: c_int) {
    // SAFETY: `userdata` is the boxed callback `show` leaked; SDL calls this once.
    let done = unsafe { Box::from_raw(userdata.cast::<FolderCallback>()) };
    // SAFETY: SDL passes NULL or a NULL-terminated list of NUL-terminated strings, valid
    // during this call.
    let pick = unsafe { folder_pick(filelist) }.unwrap_or_else(|| FolderPick::Failed(sdl_error()));
    done(pick);
}

/// The pick of an SDL file list; `None` when SDL reports an error (a NULL list).
///
/// # Safety
/// `filelist` is NULL or points to a NULL-terminated array of valid C strings.
unsafe fn folder_pick(filelist: *const *const c_char) -> Option<FolderPick> {
    if filelist.is_null() {
        return None;
    }
    // SAFETY: a non-NULL list has at least its terminating entry.
    let first = unsafe { *filelist };
    if first.is_null() {
        return Some(FolderPick::Cancelled);
    }
    // SAFETY: entries before the terminator are valid C strings.
    let bytes = unsafe { CStr::from_ptr(first) }.to_bytes();
    // SDL's zenity backend answers a cancel with one empty entry.
    if bytes.is_empty() {
        return Some(FolderPick::Cancelled);
    }
    Some(FolderPick::Chosen(path_from_bytes(bytes)))
}

/// SDL's paths are UTF-8 on Windows and raw bytes on Unix.
#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_null_list_is_an_error_and_an_empty_one_a_cancel() {
        // SAFETY: NULL and a list with only its terminator are valid inputs.
        unsafe {
            assert_eq!(folder_pick(std::ptr::null()), None);
            let empty = [std::ptr::null::<c_char>()];
            assert_eq!(folder_pick(empty.as_ptr()), Some(FolderPick::Cancelled));
            let zenity_cancel = [c"".as_ptr(), std::ptr::null()];
            assert_eq!(
                folder_pick(zenity_cancel.as_ptr()),
                Some(FolderPick::Cancelled)
            );
        }
    }

    #[test]
    fn zenity_is_looked_up_in_every_path_directory() {
        let path = std::env::join_paths(["/usr/local/bin", "/usr/bin"]).unwrap();
        let installed = |p: &Path| p == Path::new("/usr/bin/zenity");
        assert!(on_path(&path, "zenity", &installed));
        assert!(!on_path(&path, "zenity", &|_| false));
        assert!(!on_path(std::ffi::OsStr::new(""), "zenity", &installed));
    }

    #[test]
    fn the_first_entry_is_the_choice() {
        let path = c"/home/player/osu!/Songs";
        let list = [path.as_ptr(), std::ptr::null()];
        // SAFETY: a NULL-terminated list of valid C strings.
        let pick = unsafe { folder_pick(list.as_ptr()) };
        assert_eq!(
            pick,
            Some(FolderPick::Chosen(PathBuf::from("/home/player/osu!/Songs")))
        );
    }

    #[test]
    fn a_handle_without_a_window_fails_at_once() {
        let (tx, rx) = mpsc::channel();
        FolderDialog::default().open(
            Some(Path::new("/tmp")),
            Box::new(move |pick| tx.send(pick).unwrap()),
        );
        assert!(matches!(rx.try_recv(), Ok(FolderPick::Failed(_))));
    }

    #[test]
    fn a_closed_window_fails_at_once() {
        let (tx, rx) = mpsc::channel();
        FolderDialog::new(Arc::new(Mutex::new(false)), 0x8000)
            .open(None, Box::new(move |pick| tx.send(pick).unwrap()));
        assert!(matches!(rx.try_recv(), Ok(FolderPick::Failed(_))));
    }
}
