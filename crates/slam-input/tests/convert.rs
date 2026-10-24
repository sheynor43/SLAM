//! SDL event → `InputKind` conversion.

mod common;

use common::{button, key, motion, text};
use sdl3_sys::events::{
    SDL_EVENT_MOUSE_WHEEL, SDL_EVENT_QUIT, SDL_EVENT_WINDOW_FOCUS_LOST, SDL_Event,
    SDL_MouseWheelEvent, SDL_QuitEvent, SDL_WindowEvent,
};
use sdl3_sys::scancode::{SDL_SCANCODE_X, SDL_SCANCODE_Z, SDL_Scancode};
use slam_input::{InputKind, MouseButton, TextChunk, convert, convert_text};

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
fn key_repeat_is_its_own_kind() {
    assert_eq!(
        convert(&key(1_000, SDL_SCANCODE_Z, true, true)),
        Some((
            1_000,
            InputKind::KeyRepeat {
                scancode: SDL_SCANCODE_Z.0 as u16
            }
        ))
    );
    // A repeat flag on a release is not a repeat.
    assert_eq!(convert(&key(1_000, SDL_SCANCODE_Z, false, true)), None);
}

/// `convert_text` on events whose text the test owns.
fn text_of(event: &SDL_Event) -> Option<(u64, impl Iterator<Item = InputKind> + '_)> {
    // SAFETY: every event here points at a C string literal.
    unsafe { convert_text(event) }
}

fn texts(event: &SDL_Event) -> Option<(u64, Vec<String>)> {
    let (timestamp, chunks) = text_of(event)?;
    let chunks = chunks
        .map(|kind| match kind {
            InputKind::Text(chunk) => chunk.as_str().to_owned(),
            other => panic!("not text: {other:?}"),
        })
        .collect();
    Some((timestamp, chunks))
}

#[test]
fn text_keeps_timestamp_and_utf8() {
    assert_eq!(texts(&text(7, c"a")), Some((7, vec!["a".to_owned()])));
    assert_eq!(texts(&text(8, c"ж")), Some((8, vec!["ж".to_owned()])));
    // Text is not a key event, and keys are not text.
    assert_eq!(convert(&text(7, c"a")), None);
    assert!(text_of(&key(1, SDL_SCANCODE_Z, true, false)).is_none());
}

#[test]
fn long_text_splits_at_character_boundaries() {
    // 9 Cyrillic letters, 2 bytes each: 7 fit in 15 bytes, then 2.
    let (_, chunks) = texts(&text(1, c"абвгдежзи")).unwrap();
    assert_eq!(chunks, ["абвгдеж", "зи"]);
    // 3-byte characters: 5 per chunk.
    let (_, chunks) = texts(&text(1, c"日本語のテキストです")).unwrap();
    assert_eq!(chunks, ["日本語のテ", "キストです"]);
    assert!(chunks.iter().all(|c| c.len() <= TextChunk::CAPACITY));
}

#[test]
fn empty_and_invalid_text_is_ignored() {
    assert!(text_of(&text(1, c"")).is_none());
    assert!(text_of(&text(1, c"\xff\xfe")).is_none());
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

fn wheel(timestamp: u64, y: f32) -> SDL_Event {
    SDL_Event {
        wheel: SDL_MouseWheelEvent {
            r#type: SDL_EVENT_MOUSE_WHEEL,
            timestamp,
            y,
            ..Default::default()
        },
    }
}

#[test]
fn wheel_keeps_vertical_notches() {
    assert_eq!(
        convert(&wheel(9, -1.0)),
        Some((9, InputKind::MouseWheel { y: -1.0 }))
    );
    assert_eq!(
        convert(&wheel(9, 0.25)),
        Some((9, InputKind::MouseWheel { y: 0.25 }))
    );
    // Horizontal-only scrolls carry no vertical motion.
    assert_eq!(convert(&wheel(9, 0.0)), None);
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
