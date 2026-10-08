//! Update and draw threads: input order and wake-up, draw never waiting for update,
//! shutdown.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use slam_engine::clock::now_ns;
use slam_engine::limiter::LimiterMode;
use slam_engine::{Draw, Engine, EngineConfig, FrameInfo, TickInfo, Update};
use slam_input::{InputEvent, InputKind, MouseButton, event_ring};

/// Snapshot: index of the tick that wrote it.
type Snap = u64;

#[derive(Default)]
struct Recorder {
    ticks: u64,
    /// (event time, engine time of the tick that saw it, woken by input)
    seen: Vec<(u64, u64, bool)>,
    /// While set, `tick` blocks.
    stall: Arc<AtomicBool>,
}

impl Update for Recorder {
    type Snapshot = Snap;

    fn tick(&mut self, info: &TickInfo, events: &[InputEvent], snapshot: &mut Snap) {
        self.ticks += 1;
        for e in events {
            self.seen.push((e.time_ns, info.now_ns, info.by_input));
        }
        while self.stall.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_micros(100));
        }
        *snapshot = self.ticks;
    }
}

#[derive(Default)]
struct Counter {
    frames: Arc<AtomicU64>,
    last_snapshot: Snap,
    fresh: u64,
}

impl Draw<Snap> for Counter {
    fn frame(&mut self, info: &FrameInfo, snapshot: &Snap) {
        assert!(*snapshot >= self.last_snapshot, "snapshot went back");
        self.last_snapshot = *snapshot;
        self.fresh += u64::from(info.fresh);
        self.frames.fetch_add(1, Ordering::Relaxed);
    }
}

fn press(time_ns: u64) -> InputEvent {
    InputEvent {
        time_ns,
        kind: InputKind::MouseButton {
            button: MouseButton::Left,
            down: true,
        },
    }
}

fn config(update_hz: f64, draw: LimiterMode) -> EngineConfig {
    EngineConfig {
        update_hz,
        draw,
        ..EngineConfig::default()
    }
}

#[test]
fn draw_keeps_running_while_update_is_stalled() {
    let (_sink, source) = event_ring(64);
    let stall = Arc::new(AtomicBool::new(true));
    let frames = Arc::new(AtomicU64::new(0));
    let update = Recorder {
        stall: Arc::clone(&stall),
        ..Recorder::default()
    };
    let draw = Counter {
        frames: Arc::clone(&frames),
        ..Counter::default()
    };
    let engine = Engine::spawn(
        config(1000.0, LimiterMode::Hz(1000.0)),
        update,
        draw,
        0,
        source,
    )
    .unwrap();

    // Update blocks in its first tick; draw goes on at its own rate.
    std::thread::sleep(Duration::from_millis(50));
    let before = frames.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(200));
    let during = frames.load(Ordering::Relaxed) - before;
    assert!(
        during >= 100,
        "only {during} frames in 200 ms of stalled update"
    );

    stall.store(false, Ordering::Release);
    std::thread::sleep(Duration::from_millis(50));
    let stopped = engine.shutdown();
    assert!(stopped.update.ticks > 1);
    assert!(stopped.draw.fresh > 0);
    assert!(stopped.frames >= during);
}

#[test]
fn update_sees_events_in_timestamp_order() {
    let (mut sink, source) = event_ring(256);
    let mut update = Recorder::default();
    update.seen.reserve(1024);
    // Slow rate: every tick below is woken by input.
    let engine = Engine::spawn(
        config(1.0, LimiterMode::Hz(100.0)),
        update,
        Counter::default(),
        0,
        source,
    )
    .unwrap();

    // Two devices interleaved out of order inside a batch. Queued before the
    // notifier is set, so update takes them in one tick.
    let base = now_ns();
    for t in [5, 1, 4, 2, 3, 3, 0] {
        assert!(sink.push(press(base + t)));
    }
    sink.set_notify(engine.waker().as_notify());
    engine.waker().notify();
    std::thread::sleep(Duration::from_millis(20));
    // Arrives after newer events were delivered.
    assert!(sink.push(press(base + 2)));
    assert!(sink.push(press(base + 6)));
    std::thread::sleep(Duration::from_millis(20));

    let stopped = engine.shutdown();
    let times: Vec<u64> = stopped.update.seen.iter().map(|s| s.0 - base).collect();
    assert_eq!(times, [0, 1, 2, 3, 3, 4, 5, 5, 6]);
    assert_eq!(stopped.update_stats.reordered, 1);
    assert!(
        stopped.update.seen.iter().all(|s| s.2),
        "not woken by input"
    );
}

#[test]
fn input_wakes_update_long_before_its_deadline() {
    let (mut sink, source) = event_ring(64);
    let mut update = Recorder::default();
    update.seen.reserve(64);
    let engine = Engine::spawn(
        config(1.0, LimiterMode::Hz(100.0)),
        update,
        Counter::default(),
        0,
        source,
    )
    .unwrap();
    sink.set_notify(engine.waker().as_notify());

    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(5));
        sink.push(press(now_ns()));
    }
    std::thread::sleep(Duration::from_millis(20));
    let stopped = engine.shutdown();
    assert_eq!(stopped.update.seen.len(), 20);
    for &(event, tick, by_input) in &stopped.update.seen {
        assert!(by_input);
        // Generous bound for loaded CI machines; typical is tens of microseconds.
        assert!(
            tick - event < 50_000_000,
            "woke {} ns after input",
            tick - event
        );
    }
    assert_eq!(stopped.update_stats.input_ticks, 20);
}

#[test]
fn shutdown_does_not_wait_for_the_next_update_deadline() {
    let (_sink, source) = event_ring(64);
    let engine = Engine::spawn(
        config(1.0, LimiterMode::Hz(10.0)),
        Recorder::default(),
        Counter::default(),
        0,
        source,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let start = Instant::now();
    let _ = engine.shutdown();
    // Update sleeps 1 s per tick; draw finishes its 100 ms frame.
    assert!(start.elapsed() < Duration::from_millis(500));
}

#[test]
fn dropping_the_engine_joins_its_threads() {
    let (_sink, source) = event_ring(64);
    let frames = Arc::new(AtomicU64::new(0));
    let draw = Counter {
        frames: Arc::clone(&frames),
        ..Counter::default()
    };
    let engine = Engine::spawn(
        config(1.0, LimiterMode::Hz(1000.0)),
        Recorder::default(),
        draw,
        0,
        source,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(10));
    drop(engine);
    // The draw state, holding the other `frames` handle, is gone with its thread.
    assert_eq!(Arc::strong_count(&frames), 1);
}

struct Panics;

impl Update for Panics {
    type Snapshot = Snap;

    fn tick(&mut self, _: &TickInfo, _: &[InputEvent], _: &mut Snap) {
        panic!("tick failed");
    }
}

#[test]
#[should_panic(expected = "tick failed")]
fn shutdown_resumes_an_update_panic() {
    let (_sink, source) = event_ring(64);
    let engine = Engine::spawn(
        config(1000.0, LimiterMode::Hz(1000.0)),
        Panics,
        Counter::default(),
        0,
        source,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let _ = engine.shutdown();
}

#[test]
fn invalid_rates_are_rejected() {
    for hz in [0.0, 0.5, 5e-324, -1.0, 8000.5, f64::NAN, f64::INFINITY] {
        let (_sink, source) = event_ring(64);
        let result = Engine::spawn(
            config(hz, LimiterMode::Unlimited),
            Recorder::default(),
            Counter::default(),
            0,
            source,
        );
        assert!(result.is_err(), "{hz} accepted");
    }
}

#[test]
fn rates_change_while_running() {
    let (_sink, source) = event_ring(64);
    let frames = Arc::new(AtomicU64::new(0));
    let draw = Counter {
        frames: Arc::clone(&frames),
        ..Counter::default()
    };
    let mut update = Recorder::default();
    update.seen.reserve(16);
    let engine =
        Engine::spawn(config(1.0, LimiterMode::Hz(10.0)), update, draw, 0, source).unwrap();
    assert_eq!(engine.update_hz(), 1.0);
    assert_eq!(engine.draw_mode(), LimiterMode::Hz(10.0));

    // Update sleeps a second per tick; the change must wake it.
    std::thread::sleep(Duration::from_millis(20));
    let frames_before = frames.load(Ordering::Relaxed);
    engine.set_update_hz(1000.0).unwrap();
    engine.set_draw_mode(LimiterMode::Hz(1000.0)).unwrap();
    // Draw picks the change up after its current 100 ms frame.
    std::thread::sleep(Duration::from_millis(400));
    let drawn = frames.load(Ordering::Relaxed) - frames_before;
    assert_eq!(engine.update_hz(), 1000.0);
    assert_eq!(engine.draw_mode(), LimiterMode::Hz(1000.0));

    let stopped = engine.shutdown();
    // At 1 Hz there would be no tick yet; at 1000 Hz about 400.
    assert!(stopped.update.ticks > 100, "{} ticks", stopped.update.ticks);
    assert!(drawn > 100, "{drawn} frames");
}

#[test]
fn invalid_rate_changes_are_rejected_and_ignored() {
    let (_sink, source) = event_ring(64);
    let engine = Engine::spawn(
        config(1000.0, LimiterMode::Unlimited),
        Recorder::default(),
        Counter::default(),
        0,
        source,
    )
    .unwrap();
    for hz in [
        0.0,
        -0.0,
        1e-11,
        5e-324,
        -1.0,
        8000.5,
        f64::NAN,
        f64::INFINITY,
    ] {
        assert!(engine.set_update_hz(hz).is_err(), "{hz} accepted");
        assert!(
            engine.set_draw_mode(LimiterMode::Hz(hz)).is_err(),
            "{hz} accepted"
        );
    }
    assert_eq!(engine.update_hz(), 1000.0);
    assert_eq!(engine.draw_mode(), LimiterMode::Unlimited);
    for hz in slam_engine::RATE_PRESETS {
        engine.set_update_hz(hz).unwrap();
        engine.set_draw_mode(LimiterMode::Hz(hz)).unwrap();
    }
    engine.set_draw_mode(LimiterMode::Unlimited).unwrap();
    assert_eq!(engine.draw_mode(), LimiterMode::Unlimited);
    let _ = engine.shutdown();
}

#[test]
fn a_rate_set_right_after_spawn_is_applied() {
    let (_sink, source) = event_ring(64);
    let frames = Arc::new(AtomicU64::new(0));
    let draw = Counter {
        frames: Arc::clone(&frames),
        ..Counter::default()
    };
    let engine = Engine::spawn(
        config(1.0, LimiterMode::Hz(1.0)),
        Recorder::default(),
        draw,
        0,
        source,
    )
    .unwrap();
    // Before either thread has necessarily run.
    engine.set_update_hz(1000.0).unwrap();
    engine.set_draw_mode(LimiterMode::Hz(1000.0)).unwrap();
    // At 1 Hz neither would tick or draw more than once in this time; draw may first
    // finish a one-second frame it is already waiting for.
    std::thread::sleep(Duration::from_millis(1300));
    let drawn = frames.load(Ordering::Relaxed);
    let stopped = engine.shutdown();
    assert!(stopped.update.ticks > 300, "{} ticks", stopped.update.ticks);
    assert!(drawn > 100, "{drawn} frames");
}
