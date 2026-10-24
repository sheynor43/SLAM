use sdl3_sys::events::{
    SDL_EVENT_KEY_DOWN, SDL_EVENT_KEY_UP, SDL_EVENT_MOUSE_BUTTON_DOWN, SDL_EVENT_MOUSE_BUTTON_UP,
    SDL_EVENT_MOUSE_MOTION, SDL_EVENT_MOUSE_WHEEL, SDL_EVENT_TEXT_INPUT, SDL_Event, SDL_EventType,
};
use sdl3_sys::mouse::{
    SDL_BUTTON_LEFT, SDL_BUTTON_MIDDLE, SDL_BUTTON_RIGHT, SDL_BUTTON_X1, SDL_BUTTON_X2,
};

use std::ffi::CStr;

use crate::{InputKind, MouseButton, TextChunk};

/// Converts an SDL event into its SDL timestamp (`SDL_GetTicksNS` time base) and an
/// [`InputKind`]. Returns `None` for events the input pipeline does not carry: unknown
/// buttons, window and system events, and text, which [`convert_text`] splits.
pub fn convert(event: &SDL_Event) -> Option<(u64, InputKind)> {
    // SAFETY: every SDL_Event variant starts with the `type` field, and the variant
    // read below is the one selected by that tag.
    unsafe {
        let ty = SDL_EventType(event.r#type);
        if ty == SDL_EVENT_KEY_DOWN || ty == SDL_EVENT_KEY_UP {
            let key = &event.key;
            let scancode = u16::try_from(key.scancode.0).ok()?;
            if key.repeat {
                // A key-up never repeats; a repeat flag on one would be bogus.
                return key
                    .down
                    .then_some((key.timestamp, InputKind::KeyRepeat { scancode }));
            }
            Some((
                key.timestamp,
                InputKind::Key {
                    scancode,
                    down: key.down,
                },
            ))
        } else if ty == SDL_EVENT_MOUSE_BUTTON_DOWN || ty == SDL_EVENT_MOUSE_BUTTON_UP {
            let button = &event.button;
            let kind = InputKind::MouseButton {
                button: mouse_button(button.button)?,
                down: button.down,
            };
            Some((button.timestamp, kind))
        } else if ty == SDL_EVENT_MOUSE_MOTION {
            let motion = &event.motion;
            Some((
                motion.timestamp,
                InputKind::MouseMove {
                    x: motion.x,
                    y: motion.y,
                },
            ))
        } else if ty == SDL_EVENT_MOUSE_WHEEL {
            let wheel = &event.wheel;
            if wheel.y == 0.0 {
                return None;
            }
            Some((wheel.timestamp, InputKind::MouseWheel { y: wheel.y }))
        } else {
            None
        }
    }
}

/// Converts a text input event into its SDL timestamp and the typed text as
/// [`InputKind::Text`] chunks, split at character boundaries. `None` for other events
/// and for empty or invalid UTF-8 text. The chunks copy the text: SDL frees it on the
/// next event poll.
///
/// # Safety
/// A text input event must come from SDL (or point at a NUL-terminated string that
/// outlives the borrow), with no event poll since: its text pointer is read.
pub unsafe fn convert_text(
    event: &SDL_Event,
) -> Option<(u64, impl Iterator<Item = InputKind> + '_)> {
    // SAFETY: every SDL_Event variant starts with the `type` field, and the variant
    // read below is the one selected by that tag.
    let text = unsafe {
        if SDL_EventType(event.r#type) != SDL_EVENT_TEXT_INPUT {
            return None;
        }
        &event.text
    };
    if text.text.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a NUL-terminated string that outlives the borrow.
    let typed = unsafe { CStr::from_ptr(text.text) }.to_str().ok()?;
    if typed.is_empty() {
        return None;
    }
    Some((text.timestamp, TextChunk::split(typed).map(InputKind::Text)))
}

fn mouse_button(index: u8) -> Option<MouseButton> {
    match i32::from(index) {
        SDL_BUTTON_LEFT => Some(MouseButton::Left),
        SDL_BUTTON_MIDDLE => Some(MouseButton::Middle),
        SDL_BUTTON_RIGHT => Some(MouseButton::Right),
        SDL_BUTTON_X1 => Some(MouseButton::X1),
        SDL_BUTTON_X2 => Some(MouseButton::X2),
        _ => None,
    }
}
