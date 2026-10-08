//! SDL event → `InputKind` conversion.

mod common;

use common::{button, key, motion};
use sdl3_sys::events::{
    SDL_EVENT_QUIT, SDL_EVENT_WINDOW_FOCUS_LOST, SDL_Event, SDL_QuitEvent, SDL_WindowEvent,
};
use sdl3_sys::scancode::{SDL_SCANCODE_X, SDL_SCANCODE_Z, SDL_Scancode};
use slam_input::{InputKind, MouseButton, convert};

#[test]
fn key_down_and_up_keep_scancode_and_timestamp() {
    assert_eq!(
        convert(&key(1_000, SDL_SCANCODE_Z, true, false)),
        Some((
            1_000,
            InputKind::Key {
                scancode: SDL_SCANCODE_Z.0 as u16,
                down: true
            }
        ))
    );
    assert_eq!(
        convert(&key(2_500, SDL_SCANCODE_X, false, false)),
        Some((
            2_500,
            InputKind::Key {
                scancode: SDL_SCANCODE_X.0 as u16,
                down: false
            }
        ))
    );
}

#[test]
fn key_repeat_is_dropped() {
    assert_eq!(convert(&key(1_000, SDL_SCANCODE_Z, true, true)), None);
}

#[test]
fn scancode_out_of_range_is_dropped() {
    assert_eq!(convert(&key(1, SDL_Scancode(-1), true, false)), None);
    assert_eq!(convert(&key(1, SDL_Scancode(70_000), true, false)), None);
}

#[test]
fn mouse_buttons_map_by_sdl_index() {
    let expected = [
        (1, MouseButton::Left),
        (2, MouseButton::Middle),
        (3, MouseButton::Right),
        (4, MouseButton::X1),
        (5, MouseButton::X2),
    ];
    for (index, mapped) in expected {
        for down in [true, false] {
            assert_eq!(
                convert(&button(7, index, down)),
                Some((
                    7,
                    InputKind::MouseButton {
                        button: mapped,
                        down
                    }
                )),
                "button {index} down={down}"
            );
        }
    }
}

#[test]
fn unknown_mouse_button_is_dropped() {
    assert_eq!(convert(&button(7, 0, true)), None);
    assert_eq!(convert(&button(7, 6, true)), None);
}

#[test]
fn motion_keeps_window_coordinates() {
    assert_eq!(
        convert(&motion(42, 640.5, 360.25)),
        Some((
            42,
            InputKind::MouseMove {
                x: 640.5,
                y: 360.25
            }
        ))
    );
}

#[test]
fn other_events_are_ignored() {
    let quit = SDL_Event {
        quit: SDL_QuitEvent {
            r#type: SDL_EVENT_QUIT,
            timestamp: 1,
            ..Default::default()
        },
    };
    let focus = SDL_Event {
        window: SDL_WindowEvent {
            r#type: SDL_EVENT_WINDOW_FOCUS_LOST,
            timestamp: 1,
            ..Default::default()
        },
    };
    assert_eq!(convert(&quit), None);
    assert_eq!(convert(&focus), None);
}
