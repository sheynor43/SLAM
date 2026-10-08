//! Event loop end to end on SDL's headless `offscreen` video driver.

mod common;

use common::{button, key, motion};
use sdl3_sys::events::{SDL_EVENT_QUIT, SDL_Event, SDL_PushEvent, SDL_QuitEvent};
use sdl3_sys::hints::{SDL_HINT_VIDEO_DRIVER, SDL_SetHint};
use sdl3_sys::scancode::SDL_SCANCODE_Z;
use slam_input::{
    BUTTON_RESERVE, InputKind, InputWindow, MouseButton, TickMapper, WindowError, event_ring,
};

fn push(mut event: SDL_Event) {
    // SAFETY: SDL is initialised; `event` is a valid event.
    assert!(unsafe { SDL_PushEvent(&mut event) });
}

/// The offset is measured on real clocks, so it is exact only to the bracket width.
fn assert_near(actual: u64, expected: u64) {
    assert!(actual.abs_diff(expected) < 10_000, "{actual} vs {expected}");
}

/// Headless driver, so the test runs without a display.
fn use_offscreen_driver() {
    // SAFETY: called before SDL_Init with valid C strings.
    unsafe { SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr()) };
}

// One test per binary: SDL is process-global.
#[test]
fn run_delivers_events_and_stops_on_quit() {
    use_offscreen_driver();
    let mut window = InputWindow::new("test", 320, 240).expect("offscreen window");
    assert!(matches!(
        InputWindow::new("second", 1, 1),
        Err(WindowError::AlreadyActive)
    ));

    assert!(matches!(
        window.create_gl_context(),
        Err(WindowError::NotOpenGl)
    ));

    let mut mapper = TickMapper::new(
        || 1_000_000 + slam_input::sdl_ticks_ns(),
        slam_input::sdl_ticks_ns,
    );
    let (mut sink, mut source) = event_ring(BUTTON_RESERVE + 2);

    let t0 = slam_input::sdl_ticks_ns();
    push(key(t0, SDL_SCANCODE_Z, true, false));
    push(key(t0 + 1, SDL_SCANCODE_Z, true, true)); // repeat: filtered out
    push(button(t0 + 2, 1, true));
    push(motion(t0 + 3, 1.0, 1.0)); // only the button reserve is left: dropped
    push(SDL_Event {
        quit: SDL_QuitEvent {
            r#type: SDL_EVENT_QUIT,
            ..Default::default()
        },
    });

    window.run(&mut sink, &mut mapper).expect("event loop");

    let first = source.pop().expect("key event");
    assert_eq!(
        first.kind,
        InputKind::Key {
            scancode: SDL_SCANCODE_Z.0 as u16,
            down: true
        }
    );
    assert_near(first.time_ns, t0 + 1_000_000);
    let second = source.pop().expect("button event");
    assert_eq!(
        second.kind,
        InputKind::MouseButton {
            button: MouseButton::Left,
            down: true
        }
    );
    assert_near(second.time_ns, t0 + 2 + 1_000_000);
    assert_eq!(source.pop(), None);
    assert_eq!(source.dropped(), 1);

    drop(window);
    // SDL can be brought up again after the window is gone. SDL_Quit resets hints.
    use_offscreen_driver();
    drop(InputWindow::new("again", 1, 1).expect("re-init"));
}
