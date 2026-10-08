//! Opens a window and prints every input event from a consumer thread, with the
//! delay between the event timestamp and its arrival at the consumer.
//!
//! Run: `cargo run --release -p slam-engine --example input_echo`. Close the window
//! to exit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use slam_engine::clock::now_ns;
use slam_engine::gl::SdlGlSurface;
use slam_input::{InputWindow, TickMapper, event_ring, sdl_ticks_ns};
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::{Color, Device};

fn main() {
    let (mut sink, mut source) = event_ring(1024);
    let done = Arc::new(AtomicBool::new(false));

    let consumer = {
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                while let Some(event) = source.pop() {
                    let delay_us = now_ns().saturating_sub(event.time_ns) as f64 / 1e3;
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

    let mut window = InputWindow::new_opengl("SLAM input echo", 1280, 720).expect("window");
    // Without a frame a Wayland window is never shown: present one cleared frame.
    let surface = SdlGlSurface(window.create_gl_context().expect("GL context"));
    let mut device = GlDevice::new(surface, GlDebug::Off).expect("GL device");
    device.begin_frame(Color::rgb(0.08, 0.08, 0.12));
    device.present().expect("present");
    let surface = device.into_surface().expect("release GL context");
    let mut mapper = TickMapper::new(now_ns, sdl_ticks_ns);
    println!(
        "clock offset {} ns, uncertainty {} ns",
        mapper.offset_ns(),
        mapper.uncertainty_ns()
    );
    window.run(&mut sink, &mut mapper).expect("event loop");
    drop(surface);
    drop(window);

    done.store(true, Ordering::Release);
    consumer.join().unwrap();
}
