//! The per-event input path (`dispatch`: convert → map → push) must not allocate.

mod common;

use std::cell::Cell;

use common::{button, key, motion};
use sdl3_sys::scancode::SDL_SCANCODE_Z;
use slam_input::{RECALIBRATE_INTERVAL_NS, TickMapper, dispatch, event_ring};
use slam_testkit::count_allocs;

slam_testkit::install_counting_allocator!();

#[test]
fn event_path_does_not_allocate() {
    let events = [
        key(1, SDL_SCANCODE_Z, true, false),
        key(2, SDL_SCANCODE_Z, true, true),
        button(3, 1, true),
        button(4, 9, true),
        motion(5, 1.0, 2.0),
    ];
    // Fake engine clock: 1 ns per read (brackets are narrow, so recalibrations are
    // accepted), plus a full recalibration interval per round, so `map` recalibrates
    // inside the measured region.
    let engine = Cell::new(0u64);
    let engine_now = || {
        engine.set(engine.get() + 1);
        engine.get()
    };
    let mut mapper = TickMapper::new(engine_now, slam_input::sdl_ticks_ns);
    let (mut sink, mut source) = event_ring(32);

    let ((), stats) = count_allocs(|| {
        // Enough rounds to overflow the ring (moves and buttons) and recalibrate.
        for round in 0..1_000 {
            engine.set(engine.get() + RECALIBRATE_INTERVAL_NS);
            for e in &events {
                dispatch(e, &mut sink, &mut mapper);
            }
            if round % 20 == 0 {
                while source.pop().is_some() {}
            }
        }
    });
    assert_eq!(stats.total(), 0, "{stats}");
    assert!(source.dropped() > 0, "overflow path not exercised");
    assert!(
        source.dropped_buttons() > 0,
        "button overflow not exercised"
    );
    assert!(
        mapper.offset_ns() > 1_000 * RECALIBRATE_INTERVAL_NS as i64 / 2,
        "never recalibrated"
    );
}
