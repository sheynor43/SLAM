//! Opens a window and prints every input event from a consumer thread, with the
//! delay between the event timestamp and its arrival at the consumer.
//!
//! Run: `cargo run --release -p slam-engine --example input_echo`. Close the window
//! to exit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use slam_input::{InputWindow, TickMapper, event_ring, sdl_ticks_ns};

fn main() {
    // Engine clock: monotonic nanoseconds from a shared origin.
    let origin = Instant::now();
    let engine_now = move || origin.elapsed().as_nanos() as u64;

    let (mut sink, mut source) = event_ring(1024);
    let done = Arc::new(AtomicBool::new(false));

    let consumer = {
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                while let Some(event) = source.pop() {
                    let delay_us = engine_now().saturating_sub(event.time_ns) as f64 / 1e3;
                    println!(
                        "{:>12.3} ms  +{delay_us:>8.1} us  {:?}",
                        event.time_ns as f64 / 1e6,
                        event.kind
                    );
                }
                std::thread::sleep(Duration::from_micros(200));
            }
            println!("dropped: {}", source.dropped());
        })
    };

    let mut window = InputWindow::new("SLAM input echo", 1280, 720).expect("window");
    // No renderer yet; without a frame a Wayland window is never shown.
    window.fill_placeholder(20, 20, 30).expect("fill");
    let mut mapper = TickMapper::new(engine_now, sdl_ticks_ns);
    println!(
        "clock offset {} ns, uncertainty {} ns",
        mapper.offset_ns(),
        mapper.uncertainty_ns()
    );
    window.run(&mut sink, &mut mapper).expect("event loop");
    drop(window);

    done.store(true, Ordering::Release);
    consumer.join().unwrap();
}
