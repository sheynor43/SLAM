//! `UpdateLoop` scheduling: resync after a stall, early input ticks, batching.

use std::time::Duration;

use slam_engine::clock::now_ns;
use slam_engine::{InvalidUpdateConfig, TickInfo, Update, UpdateLoop, triple_buffer, wake_pair};
use slam_input::{EventSink, InputEvent, InputKind, event_ring};

#[derive(Default)]
struct Log {
    ticks: Vec<(TickInfo, Vec<InputEvent>)>,
    stall_next: Option<Duration>,
}

impl Update for Log {
    type Snapshot = u64;

    fn tick(&mut self, info: &TickInfo, events: &[InputEvent], snapshot: &mut u64) {
        if let Some(stall) = self.stall_next.take() {
            std::thread::sleep(stall);
        }
        self.ticks.push((*info, events.to_vec()));
        *snapshot = self.ticks.len() as u64;
    }
}

fn mv(time_ns: u64) -> InputEvent {
    InputEvent {
        time_ns,
        kind: InputKind::MouseMove { x: 0.0, y: 0.0 },
    }
}

fn make(hz: f64, batch: usize) -> (EventSink, UpdateLoop<Log>) {
    let (mut sink, source) = event_ring(256);
    let (waker, parker) = wake_pair();
    sink.set_notify(waker.as_notify());
    let (writer, _reader) = triple_buffer(0);
    let update = UpdateLoop::new(Log::default(), source, parker, writer, hz, batch, 50_000);
    (sink, update.unwrap())
}

#[test]
fn a_stalled_tick_restarts_the_schedule() {
    let (_sink, mut update) = make(1000.0, 16);
    for _ in 0..5 {
        assert!(update.step());
    }
    assert_eq!(update.stats().resyncs, 0);
    update.state_mut().stall_next = Some(Duration::from_millis(10));
    assert!(update.step());
    assert!(update.step());
    assert_eq!(update.stats().resyncs, 1);
    // No burst of catch-up ticks: deadlines are a period apart again.
    let start = now_ns();
    for _ in 0..5 {
        assert!(update.step());
    }
    assert!(now_ns() - start >= 4_000_000);
    let ticks = &update.state().ticks;
    for pair in ticks.windows(2) {
        assert!(pair[1].0.deadline_ns > pair[0].0.deadline_ns);
    }
}

#[test]
fn input_ticks_before_the_deadline() {
    let (mut sink, mut update) = make(1.0, 16);
    // The first step starts the schedule; its deadline is a second away.
    let start = now_ns();
    sink.push(mv(start));
    assert!(update.step());
    assert!(now_ns() - start < 100_000_000);
    let (info, events) = &update.state().ticks[0];
    assert!(info.by_input);
    assert!(info.deadline_ns > info.now_ns, "{info:?}");
    assert_eq!(events, &[mv(start)]);
    assert_eq!(update.stats().input_ticks, 1);
}

#[test]
fn a_full_ring_is_split_into_batches_in_order() {
    let (mut sink, mut update) = make(1.0, 4);
    let base = now_ns() - 1_000_000;
    for t in [9, 3, 7, 1, 8, 2, 6, 0, 5, 4] {
        sink.push(mv(base + t));
    }
    assert!(update.step());
    let ticks = &update.state().ticks;
    let sizes: Vec<usize> = ticks.iter().map(|t| t.1.len()).collect();
    assert_eq!(sizes, [4, 4, 2]);
    let mut last = 0;
    for (info, events) in ticks {
        for e in events {
            assert!(e.time_ns >= last);
            assert!(e.time_ns <= info.now_ns, "event newer than its tick");
            last = e.time_ns;
        }
    }
    // Sorted within each batch only; later batches are raised to keep the order.
    assert!(update.stats().reordered > 0);
}

#[test]
fn a_notification_without_input_does_not_tick() {
    let (_sink, source) = event_ring(64);
    let (waker, parker) = wake_pair();
    let (writer, _reader) = triple_buffer(0);
    let mut update =
        UpdateLoop::new(Log::default(), source, parker, writer, 1.0, 4, 50_000).unwrap();
    // As after a stop request, or an event consumed by the tick before its wake-up.
    waker.notify();
    let start = now_ns();
    assert!(!update.step());
    assert!(now_ns() - start < 100_000_000);
    assert_eq!(update.stats().ticks, 0);
}

#[test]
fn invalid_config_is_rejected() {
    let make_with = |hz, batch| {
        let (_sink, source) = event_ring(64);
        let (_waker, parker) = wake_pair();
        let (writer, _reader) = triple_buffer(0);
        UpdateLoop::new(Log::default(), source, parker, writer, hz, batch, 0).err()
    };
    assert_eq!(make_with(1000.0, 0), Some(InvalidUpdateConfig::EmptyBatch));
    assert_eq!(make_with(0.0, 16), Some(InvalidUpdateConfig::Rate));
    assert_eq!(make_with(8000.0, 1), None);
}

#[test]
fn a_new_rate_restarts_the_schedule() {
    let (_sink, mut update) = make(1000.0, 4);
    assert!(update.step());
    assert_eq!(update.set_hz(0.0), Err(InvalidUpdateConfig::Rate));
    assert_eq!(update.set_hz(1e-11), Err(InvalidUpdateConfig::Rate));
    assert_eq!(update.hz(), 1000.0);
    update.set_hz(100.0).unwrap();
    assert_eq!(update.hz(), 100.0);
    let start = now_ns();
    assert!(update.step());
    let waited = now_ns() - start;
    // One 10 ms period from the restart, not the 1 ms one.
    assert!(waited >= 9_000_000, "{waited} ns");
    assert_eq!(update.stats().resyncs, 0);
}

#[test]
fn the_scheduled_interval_skips_input_ticks() {
    // 50 ms period, so that the input tick surely comes before the deadline.
    let (mut sink, mut update) = make(20.0, 16);
    assert!(update.step());
    // One scheduled tick so far: no interval yet.
    assert_eq!(update.stats().scheduled_interval_ns, 0);
    assert!(update.step());
    let interval = update.stats().scheduled_interval_ns;
    // Generous upper bound for a loaded machine.
    assert!((45_000_000..250_000_000).contains(&interval), "{interval}");
    sink.push(mv(now_ns()));
    assert!(update.step());
    assert!(update.state().ticks.last().unwrap().0.by_input);
    assert_eq!(update.stats().scheduled_interval_ns, interval);
}

#[test]
fn the_scheduled_interval_spans_a_stall() {
    let (_sink, mut update) = make(1000.0, 16);
    for _ in 0..3 {
        assert!(update.step());
    }
    update.state_mut().stall_next = Some(Duration::from_millis(30));
    assert!(update.step());
    assert!(update.step());
    assert!(update.stats().scheduled_interval_ns >= 30_000_000);
}

#[test]
fn a_rate_change_restarts_the_scheduled_interval() {
    let (_sink, mut update) = make(1000.0, 16);
    for _ in 0..3 {
        assert!(update.step());
    }
    assert!(update.stats().scheduled_interval_ns > 0);
    update.set_hz(500.0).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    assert!(update.step());
    assert_eq!(update.stats().scheduled_interval_ns, 0);
    assert!(update.step());
    let interval = update.stats().scheduled_interval_ns;
    assert!((1_500_000..25_000_000).contains(&interval), "{interval}");
}
