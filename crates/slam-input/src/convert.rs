use sdl3_sys::events::{
    SDL_EVENT_KEY_DOWN, SDL_EVENT_KEY_UP, SDL_EVENT_MOUSE_BUTTON_DOWN, SDL_EVENT_MOUSE_BUTTON_UP,
    SDL_EVENT_MOUSE_MOTION, SDL_Event, SDL_EventType,
};
use sdl3_sys::mouse::{
    SDL_BUTTON_LEFT, SDL_BUTTON_MIDDLE, SDL_BUTTON_RIGHT, SDL_BUTTON_X1, SDL_BUTTON_X2,
};

use crate::{InputKind, MouseButton};

/// Converts an SDL event into its SDL timestamp (`SDL_GetTicksNS` time base) and an
/// [`InputKind`]. Returns `None` for events the input pipeline does not carry: key
/// repeats, unknown buttons, window and system events.
pub fn convert(event: &SDL_Event) -> Option<(u64, InputKind)> {
    // SAFETY: every SDL_Event variant starts with the `type` field, and the variant
    // read below is the one selected by that tag.
    unsafe {
        let ty = SDL_EventType(event.r#type);
        if ty == SDL_EVENT_KEY_DOWN || ty == SDL_EVENT_KEY_UP {
            let key = &event.key;
            if key.repeat {
                return None;
            }
            let scancode = u16::try_from(key.scancode.0).ok()?;
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
        } else {
            None
        }
    }
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
