//! Runs the input, update and draw threads together. Prints once per second the
//! update and draw rates and the delay from an input event's timestamp to the tick
//! that received it.
//!
//! Run: `cargo run --release -p slam-engine --example pipeline [update_hz] [draw_hz]`
//! (defaults 1000 and 240; draw_hz 0 = unlimited). Close the window to exit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use slam_engine::clock::now_ns;
use slam_engine::limiter::LimiterMode;
use slam_engine::{Draw, Engine, EngineConfig, FrameInfo, TickInfo, Update};
use slam_input::{InputEvent, InputKind, InputWindow, TickMapper, event_ring, sdl_ticks_ns};

#[derive(Default)]
struct Counters {
    ticks: AtomicU64,
    input_ticks: AtomicU64,
    frames: AtomicU64,
    events: AtomicU64,
    max_delay_ns: AtomicU64,
}

#[derive(Clone, Copy, Default)]
struct Snap {
    cursor: (f32, f32),
}

struct Game {
    counters: Arc<Counters>,
    cursor: (f32, f32),
}

impl Update for Game {
    type Snapshot = Snap;

    fn tick(&mut self, info: &TickInfo, events: &[InputEvent], snapshot: &mut Snap) {
        let c = &self.counters;
        c.ticks.fetch_add(1, Ordering::Relaxed);
        if info.by_input {
            c.input_ticks.fetch_add(1, Ordering::Relaxed);
        }
        for e in events {
            if let InputKind::MouseMove { x, y } = e.kind {
                self.cursor = (x, y);
            }
            c.events.fetch_add(1, Ordering::Relaxed);
            c.max_delay_ns
                .fetch_max(info.now_ns.saturating_sub(e.time_ns), Ordering::Relaxed);
        }
        snapshot.cursor = self.cursor;
    }
}

struct Frames(Arc<Counters>);

impl Draw<Snap> for Frames {
    fn frame(&mut self, _: &FrameInfo, snapshot: &Snap) {
        std::hint::black_box(snapshot.cursor);
        self.0.frames.fetch_add(1, Ordering::Relaxed);
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let update_hz: f64 = args
        .next()
        .map_or(1000.0, |a| a.parse().expect("update_hz"));
    let draw_hz: f64 = args.next().map_or(240.0, |a| a.parse().expect("draw_hz"));
    let draw = if draw_hz == 0.0 {
        LimiterMode::Unlimited
    } else {
        LimiterMode::Hz(draw_hz)
    };

    let counters = Arc::new(Counters::default());
    let (mut sink, source) = event_ring(1024);
    let game = Game {
        counters: Arc::clone(&counters),
        cursor: (0.0, 0.0),
    };
    let config = EngineConfig {
        update_hz,
        draw,
        ..EngineConfig::default()
    };
    let engine = Engine::spawn(
        config,
        game,
        Frames(Arc::clone(&counters)),
        Snap::default(),
        source,
    )
    .expect("engine");
    sink.set_notify(engine.waker().as_notify());

    let done = Arc::new(AtomicBool::new(false));
    let reporter = {
        let (c, done) = (Arc::clone(&counters), Arc::clone(&done));
        std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_secs(1));
                println!(
                    "update {:>5}/s (by input {:>5})  draw {:>5}/s  events {:>5}  max input→tick {:>7.1} us",
                    c.ticks.swap(0, Ordering::Relaxed),
                    c.input_ticks.swap(0, Ordering::Relaxed),
                    c.frames.swap(0, Ordering::Relaxed),
                    c.events.swap(0, Ordering::Relaxed),
                    c.max_delay_ns.swap(0, Ordering::Relaxed) as f64 / 1e3,
                );
            }
        })
    };

    let mut window = InputWindow::new("SLAM pipeline", 1280, 720).expect("window");
    // No renderer yet; without a frame a Wayland window is never shown.
    window.fill_placeholder(20, 20, 30).expect("fill");
    let mut mapper = TickMapper::new(now_ns, sdl_ticks_ns);
    window.run(&mut sink, &mut mapper).expect("event loop");
    drop(window);

    let stopped = engine.shutdown();
    done.store(true, Ordering::Release);
    reporter.join().unwrap();
    println!(
        "ticks {}, by input {}, resyncs {}, reordered {}, frames {}",
        stopped.update_stats.ticks,
        stopped.update_stats.input_ticks,
        stopped.update_stats.resyncs,
        stopped.update_stats.reordered,
        stopped.frames
    );
}
