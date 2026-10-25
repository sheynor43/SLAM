//! Window mode keys and wishes end to end on SDL's headless `offscreen` video driver.

mod common;

use common::key;
use sdl3_sys::events::{SDL_EVENT_QUIT, SDL_Event, SDL_PushEvent, SDL_QuitEvent};
use sdl3_sys::hints::{SDL_HINT_VIDEO_DRIVER, SDL_SetHint};
use sdl3_sys::keycode::{SDL_KMOD_LALT, SDL_KMOD_LCTRL};
use sdl3_sys::scancode::{SDL_SCANCODE_F11, SDL_SCANCODE_RETURN};
use slam_input::{InputKind, InputWindow, TickMapper, WindowMode, WindowState, event_ring};

fn push(mut event: SDL_Event) {
    // SAFETY: SDL is initialised; `event` is a valid event.
    assert!(unsafe { SDL_PushEvent(&mut event) });
}

fn with_mods(mut event: SDL_Event, mods: u16) -> SDL_Event {
    event.key.r#mod.0 = mods;
    event
}

// One test per binary: SDL is process-global.
#[test]
fn mode_keys_cycle_the_mode_and_are_swallowed() {
    // SAFETY: called before SDL_Init with valid C strings.
    unsafe { SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr()) };
    let mut window = InputWindow::new("test", 320, 240).expect("offscreen window");
    let settings = window.window_settings();
    // A window made by size starts windowed at that size.
    assert_eq!(
        settings.get(),
        WindowState {
            mode: WindowMode::Windowed,
            windowed_size: [320, 240],
            ..WindowState::default()
        }
    );
    assert!(window.window_scale().pixels_per_point() > 0.0);

    let mut mapper = TickMapper::new(slam_input::sdl_ticks_ns, slam_input::sdl_ticks_ns);
    let (mut sink, mut source) = event_ring(64);
    let t0 = slam_input::sdl_ticks_ns();
    let alt = SDL_KMOD_LALT.0;
    // Alt+Enter: swallowed and reported; its repeat swallowed; its release passes.
    push(with_mods(key(t0, SDL_SCANCODE_RETURN, true, false), alt));
    push(with_mods(key(t0 + 1, SDL_SCANCODE_RETURN, true, true), alt));
    push(with_mods(
        key(t0 + 2, SDL_SCANCODE_RETURN, false, false),
        alt,
    ));
    // Ctrl+Alt+Enter and Ctrl+F11 are ordinary keys.
    let ctrl_alt = alt | SDL_KMOD_LCTRL.0;
    push(with_mods(
        key(t0 + 3, SDL_SCANCODE_RETURN, true, false),
        ctrl_alt,
    ));
    push(with_mods(
        key(t0 + 4, SDL_SCANCODE_F11, true, false),
        SDL_KMOD_LCTRL.0,
    ));
    // F11 alone cycles again.
    push(key(t0 + 5, SDL_SCANCODE_F11, true, false));
    push(SDL_Event {
        quit: SDL_QuitEvent {
            r#type: SDL_EVENT_QUIT,
            ..Default::default()
        },
    });

    window.run(&mut sink, &mut mapper).expect("event loop");

    let enter = SDL_SCANCODE_RETURN.0 as u16;
    let f11 = SDL_SCANCODE_F11.0 as u16;
    let kinds: Vec<InputKind> = std::iter::from_fn(|| source.pop().map(|e| e.kind)).collect();
    assert_eq!(
        kinds,
        [
            InputKind::WindowMode(WindowMode::Borderless),
            InputKind::Key {
                scancode: enter,
                down: false
            },
            InputKind::Key {
                scancode: enter,
                down: true
            },
            InputKind::Key {
                scancode: f11,
                down: true
            },
            InputKind::WindowMode(WindowMode::Fullscreen),
        ]
    );
    // The wish follows the switches, so the next apply keeps them.
    assert_eq!(settings.get().mode, WindowMode::Fullscreen);
    assert_eq!(settings.get().windowed_size, [320, 240]);

    // A wish from another thread wakes the window waiting for events: it is applied
    // before the quit that follows, which ends the loop at once.
    let other = std::thread::spawn({
        let settings = settings.clone();
        move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            settings.set(WindowState {
                mode: WindowMode::Windowed,
                windowed_size: [640, 480],
                ..settings.get()
            });
            std::thread::sleep(std::time::Duration::from_millis(100));
            push(SDL_Event {
                quit: SDL_QuitEvent {
                    r#type: SDL_EVENT_QUIT,
                    ..Default::default()
                },
            });
        }
    });
    window.run(&mut sink, &mut mapper).expect("event loop");
    other.join().unwrap();
    assert_eq!(window.size_in_points(), (640, 480));
    // The wake-up does not reach the ring.
    assert_eq!(source.pop(), None);
}
