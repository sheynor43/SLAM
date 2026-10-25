//! The window reports sizes the player gives it (ADR-0073), on SDL's headless `offscreen`
//! video driver.

use sdl3_sys::events::{
    SDL_EVENT_QUIT, SDL_EVENT_WINDOW_MOVED, SDL_EVENT_WINDOW_RESIZED, SDL_Event, SDL_EventType,
    SDL_PushEvent, SDL_QuitEvent, SDL_WindowEvent,
};
use sdl3_sys::hints::{SDL_HINT_VIDEO_DRIVER, SDL_SetHint};
use sdl3_sys::rect::SDL_Rect;
use sdl3_sys::stdinc::SDL_free;
use sdl3_sys::video::{
    SDL_GetDisplayBounds, SDL_GetDisplayForWindow, SDL_GetWindowPosition, SDL_GetWindows,
    SDL_Window,
};
use slam_input::{InputWindow, TickMapper, WindowMode, WindowState, event_ring};

fn push(mut event: SDL_Event) {
    // SAFETY: SDL is initialised; `event` is a valid event.
    assert!(unsafe { SDL_PushEvent(&mut event) });
}

fn window_event(r#type: SDL_EventType, data1: i32, data2: i32) -> SDL_Event {
    SDL_Event {
        window: SDL_WindowEvent {
            r#type,
            data1,
            data2,
            ..Default::default()
        },
    }
}

fn resized(w: i32, h: i32) -> SDL_Event {
    window_event(SDL_EVENT_WINDOW_RESIZED, w, h)
}

/// The only window SDL has: the test's.
fn sdl_window() -> *mut SDL_Window {
    let mut count = 0;
    // SAFETY: SDL video is initialised; `count` is a valid out pointer. Freed below.
    let list = unsafe { SDL_GetWindows(&mut count) };
    assert_eq!(count, 1);
    // SAFETY: SDL returned an array of one window.
    let window = unsafe { *list };
    // SAFETY: SDL allocated the array and leaves it to us.
    unsafe { SDL_free(list.cast()) };
    window
}

fn quit() -> SDL_Event {
    SDL_Event {
        quit: SDL_QuitEvent {
            r#type: SDL_EVENT_QUIT,
            ..Default::default()
        },
    }
}

// One test per binary: SDL is process-global.
#[test]
fn a_resized_windowed_window_keeps_its_size_as_the_wish() {
    // SAFETY: called before SDL_Init with valid C strings.
    unsafe { SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr()) };
    let mut window = InputWindow::new("test", 800, 600).expect("offscreen window");
    let settings = window.window_settings();
    let mut mapper = TickMapper::new(slam_input::sdl_ticks_ns, slam_input::sdl_ticks_ns);
    let (mut sink, mut source) = event_ring(64);

    // Resized by the player: the wish takes the size, within the smallest windowed size.
    push(resized(1000, 700));
    push(quit());
    window.run(&mut sink, &mut mapper).expect("event loop");
    assert_eq!(settings.get().windowed_size, [1000, 700]);
    push(resized(100, 900));
    push(quit());
    window.run(&mut sink, &mut mapper).expect("event loop");
    assert_eq!(settings.get().windowed_size, [640, 900]);
    // Reported, not wished: the window does not apply it back.
    assert_eq!(settings.get().mode, WindowMode::Windowed);

    // Moved by the player (the offscreen driver cannot move windows, so SDL's own
    // position is reported): the place is kept as fractions of the display's free space.
    let window_ptr = sdl_window();
    let mut bounds = SDL_Rect::default();
    let (mut x, mut y) = (0, 0);
    // SAFETY: `window_ptr` is the live test window; the out pointers are valid.
    unsafe {
        assert!(SDL_GetDisplayBounds(
            SDL_GetDisplayForWindow(window_ptr),
            &mut bounds
        ));
        assert!(SDL_GetWindowPosition(window_ptr, &mut x, &mut y));
    }
    push(window_event(SDL_EVENT_WINDOW_MOVED, x, y));
    push(quit());
    window.run(&mut sink, &mut mapper).expect("event loop");
    let fraction = |at: i32, origin: i32, display: i32, size: i32| {
        if display > size {
            (at - origin) as f32 / (display - size) as f32
        } else {
            0.0
        }
    };
    assert_eq!(
        settings.position(),
        [
            fraction(x, bounds.x, bounds.w, 640),
            fraction(y, bounds.y, bounds.h, 900),
        ]
        .map(|p| p.clamp(-0.5, 1.5))
    );
    assert_ne!(
        settings.position(),
        [0.5, 0.5],
        "a 640×900 window is not centred"
    );

    // Outside windowed mode a resize is the mode's, not the player's.
    settings.set(WindowState {
        mode: WindowMode::Borderless,
        ..settings.get()
    });
    // Applied first, so the resize below is newer than the mode change.
    push(quit());
    window.run(&mut sink, &mut mapper).expect("event loop");
    push(resized(1234, 567));
    push(quit());
    window.run(&mut sink, &mut mapper).expect("event loop");
    assert_eq!(settings.get().windowed_size, [640, 900]);
    // Resizes are window events: none reaches the ring.
    assert_eq!(source.pop(), None);
}
