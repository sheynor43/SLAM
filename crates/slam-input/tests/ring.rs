//! Input ring: order, overflow accounting, button reserve, never blocking.

use slam_input::{BUTTON_RESERVE, InputEvent, InputKind, MouseButton, event_ring};

const CAPACITY: usize = BUTTON_RESERVE + 4;

fn mv(time_ns: u64) -> InputEvent {
    InputEvent {
        time_ns,
        kind: InputKind::MouseMove { x: 0.0, y: 0.0 },
    }
}

fn btn(time_ns: u64, down: bool) -> InputEvent {
    InputEvent {
        time_ns,
        kind: InputKind::MouseButton {
            button: MouseButton::Left,
            down,
        },
    }
}

#[test]
fn events_come_out_in_push_order() {
    let (mut sink, mut source) = event_ring(CAPACITY);
    for t in 0..4 {
        assert!(sink.push(mv(t)));
    }
    assert_eq!(source.len(), 4);
    for t in 0..4 {
        assert_eq!(source.pop(), Some(mv(t)));
    }
    assert!(source.is_empty());
    assert_eq!(source.pop(), None);
}

#[test]
fn moves_stop_at_the_reserve_buttons_fill_it() {
    let (mut sink, source) = event_ring(CAPACITY);
    // Moves fill the ring only up to the reserve.
    for t in 0..10 {
        sink.push(mv(t));
    }
    assert_eq!(source.len(), CAPACITY - BUTTON_RESERVE);
    assert_eq!(source.dropped(), 10 - (CAPACITY - BUTTON_RESERVE) as u64);
    assert_eq!(source.dropped_buttons(), 0);

    // Buttons still get through: the whole reserve is theirs.
    for t in 0..BUTTON_RESERVE as u64 {
        assert!(sink.push(btn(100 + t, t % 2 == 0)), "button {t}");
    }
    assert_eq!(source.len(), CAPACITY);
}

#[test]
fn full_ring_drops_newest_and_counts() {
    let (mut sink, mut source) = event_ring(CAPACITY);
    for t in 0..CAPACITY as u64 {
        assert!(sink.push(btn(t, true)));
    }
    let before = source.dropped();
    // Full: further pushes return immediately and are counted.
    assert!(!sink.push(btn(1_000, false)));
    assert!(!sink.push(mv(1_001)));
    assert_eq!(source.dropped(), before + 2);
    assert_eq!(source.dropped_buttons(), 1);
    // The queued events are the oldest ones, untouched.
    for t in 0..CAPACITY as u64 {
        assert_eq!(source.pop(), Some(btn(t, true)));
    }
    // Space freed: pushing works again, the counter stays.
    assert!(sink.push(btn(2_000, false)));
    assert_eq!(source.pop(), Some(btn(2_000, false)));
    assert_eq!(source.dropped(), before + 2);
}

#[test]
#[should_panic(expected = "BUTTON_RESERVE")]
fn capacity_must_exceed_reserve() {
    let _ = event_ring(BUTTON_RESERVE);
}

#[test]
fn producer_and_consumer_on_different_threads() {
    const N: u64 = 100_000;
    let (mut sink, mut source) = event_ring(64);
    let producer = std::thread::spawn(move || {
        for t in 0..N {
            sink.push(mv(t));
        }
    });
    let mut received = 0u64;
    let mut last = None;
    while !producer.is_finished() || !source.is_empty() {
        if let Some(e) = source.pop() {
            assert!(last.is_none_or(|l| e.time_ns > l), "order broken");
            last = Some(e.time_ns);
            received += 1;
        }
    }
    producer.join().unwrap();
    assert_eq!(received + source.dropped(), N);
}
