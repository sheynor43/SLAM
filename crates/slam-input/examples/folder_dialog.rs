//! Manual check of the folder picker (ADR-0064): opens a window and the picker at once,
//! prints the answer and closes.

use slam_input::{EventSink, InputWindow, TickMapper, event_ring, sdl_ticks_ns};

fn main() {
    let mut window = InputWindow::new_opengl("folder dialog", 640, 360).expect("window");
    let quit = window.quit_handle();
    window.folder_dialog().open(
        None,
        Box::new(move |pick| {
            eprintln!("answer: {pick:?}");
            quit.request();
        }),
    );
    eprintln!("requested");
    let (mut sink, _source): (EventSink, _) = event_ring(64);
    let start = std::time::Instant::now();
    let mut mapper = TickMapper::new(move || start.elapsed().as_nanos() as u64, sdl_ticks_ns);
    let ran = window.run(&mut sink, &mut mapper);
    eprintln!("run: {ran:?}");
}
