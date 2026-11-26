//! Files dropped on the window (ADR-0169).

use std::ffi::CStr;
use std::path::PathBuf;

use sdl3_sys::events::SDL_Event;

/// Gets every file dropped on the window. Runs on the window's thread between events: it
/// must not block, or input waits.
pub type DropCallback = Box<dyn FnMut(PathBuf)>;

/// The path of a `SDL_EVENT_DROP_FILE` event, copied; `None` for a null pointer.
///
/// # Safety
/// `event` must be a drop event whose `data` is null or a NUL-terminated string that is
/// valid for the call.
pub(crate) unsafe fn drop_path(event: &SDL_Event) -> Option<PathBuf> {
    // SAFETY: the caller says this is a drop event.
    let data = unsafe { event.drop.data };
    if data.is_null() {
        return None;
    }
    // SAFETY: non-null and NUL-terminated, per the caller.
    let bytes = unsafe { CStr::from_ptr(data) }.to_bytes();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        Some(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl3_sys::events::{SDL_DropEvent, SDL_EVENT_DROP_FILE};

    #[test]
    fn copies_the_dropped_path() {
        let text = c"/tmp/a b.osz";
        let mut event = SDL_Event::default();
        event.drop = SDL_DropEvent {
            r#type: SDL_EVENT_DROP_FILE,
            data: text.as_ptr(),
            ..Default::default()
        };
        // SAFETY: a drop event with a NUL-terminated string.
        let path = unsafe { drop_path(&event) };
        assert_eq!(path, Some(PathBuf::from("/tmp/a b.osz")));
        event.drop.data = std::ptr::null();
        // SAFETY: a drop event with a null string.
        assert_eq!(unsafe { drop_path(&event) }, None);
    }
}
