//! The OS error dialog shown when the game cannot go on (ADR-0143). SDL shows it without
//! `SDL_Init` and without a window, so it works after the window is gone.

use std::ffi::CString;

use sdl3_sys::messagebox::{SDL_MESSAGEBOX_ERROR, SDL_ShowSimpleMessageBox};

use crate::window::sdl_error;

/// Shows a modal error dialog with `title` and `message` and waits until it is closed.
/// Call it from the main thread with no SDL window left open. NUL bytes in the text are
/// dropped. The error is SDL's message when no dialog could be shown (on Linux without a
/// display, for one).
pub fn show_error_box(title: &str, message: &str) -> Result<(), String> {
    let title = c_text(title);
    let message = c_text(message);
    // SAFETY: both pointers are valid NUL-terminated strings that outlive the call; a null
    // parent window is allowed.
    let shown = unsafe {
        SDL_ShowSimpleMessageBox(
            SDL_MESSAGEBOX_ERROR,
            title.as_ptr(),
            message.as_ptr(),
            std::ptr::null_mut(),
        )
    };
    if shown { Ok(()) } else { Err(sdl_error()) }
}

/// `text` as a C string, without the NUL bytes it cannot carry.
fn c_text(text: &str) -> CString {
    let bytes: Vec<u8> = text.bytes().filter(|&b| b != 0).collect();
    // The NUL bytes were just removed.
    CString::new(bytes).expect("no NUL bytes left")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_bytes_are_dropped_from_the_text() {
        assert_eq!(c_text("a\0b").as_bytes(), b"ab");
        assert_eq!(c_text("").as_bytes(), b"");
    }
}
