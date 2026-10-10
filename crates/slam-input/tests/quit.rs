//! `QuitHandle` stops the event loop from another thread.

use std::time::Duration;

use sdl3_sys::hints::{SDL_HINT_VIDEO_DRIVER, SDL_SetHint};
use slam_input::{InputWindow, TickMapper, event_ring};

// One test per binary: SDL is process-global.
#[test]
fn quit_handle_stops_run_and_outlives_the_window() {
    // SAFETY: called before SDL_Init with valid C strings.
    unsafe { SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr()) };
    let mut window = InputWindow::new("test", 320, 240).expect("offscreen window");
    let handle = window.quit_handle();
    let mut mapper = TickMapper::new(slam_input::sdl_ticks_ns, slam_input::sdl_ticks_ns);
    let (mut sink, _source) = event_ring(1024);

    let remote = handle.clone();
    let thread = std::thread::spawn(move || {
        // Queued before or during the wait: `run` returns either way.
        std::thread::sleep(Duration::from_millis(20));
        if !remote.request() {
            // `run` would wait forever: fail loudly instead of hanging.
            eprintln!("quit request was not queued");
            std::process::abort();
        }
    });
    window.run(&mut sink, &mut mapper).expect("event loop");
    thread.join().expect("quit thread");

    drop(window);
    assert!(!handle.request(), "no SDL after the window is dropped");
}
