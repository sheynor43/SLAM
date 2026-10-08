//! SDL → engine timestamp mapping on fake clocks.

use std::cell::Cell;
use std::rc::Rc;

use slam_input::{
    MAX_BRACKET_NS, MAX_REJECTIONS, RECALIBRATE_INTERVAL_NS, RETRY_SPACING_NS, TickMapper,
};

/// Clock that returns a fixed value, set from outside.
fn fixed(cell: &Rc<Cell<u64>>) -> impl FnMut() -> u64 + use<> {
    let cell = Rc::clone(cell);
    move || cell.get()
}

#[test]
fn constant_offset_maps_exactly() {
    let engine = Rc::new(Cell::new(5_000_000_000));
    let sdl = Rc::new(Cell::new(1_000));
    let mut mapper = TickMapper::new(fixed(&engine), fixed(&sdl));
    assert_eq!(mapper.offset_ns(), 5_000_000_000 - 1_000);
    assert_eq!(mapper.uncertainty_ns(), 0);
    // An event stamped 500 ns before the calibration point.
    assert_eq!(mapper.map(500), 4_999_999_500);
}

#[test]
fn engine_clock_behind_sdl_gives_negative_offset() {
    let engine = Rc::new(Cell::new(100));
    let sdl = Rc::new(Cell::new(1_000));
    let mut mapper = TickMapper::new(fixed(&engine), fixed(&sdl));
    assert_eq!(mapper.offset_ns(), -900);
    assert_eq!(mapper.map(1_500), 600);
    // Stamps before the engine epoch saturate at 0 instead of wrapping.
    assert_eq!(mapper.map(10), 0);
}

#[test]
fn drift_is_corrected_after_the_interval() {
    let engine = Rc::new(Cell::new(1_000_000));
    let sdl = Rc::new(Cell::new(0));
    let mut mapper = TickMapper::new(fixed(&engine), fixed(&sdl));
    assert_eq!(mapper.offset_ns(), 1_000_000);

    // SDL's raw clock runs 100 ppm slower than the engine clock.
    let advance = |engine_ns: u64| {
        engine.set(1_000_000 + engine_ns);
        sdl.set(engine_ns - engine_ns / 10_000);
    };

    // Just before the interval: the old offset is kept.
    advance(RECALIBRATE_INTERVAL_NS - 1);
    mapper.map(0);
    assert_eq!(mapper.offset_ns(), 1_000_000);

    // At the interval: re-measured, so the current SDL time maps to current engine time.
    advance(RECALIBRATE_INTERVAL_NS);
    let sdl_now = sdl.get();
    assert_eq!(mapper.map(sdl_now), engine.get());
    assert_eq!(
        mapper.offset_ns(),
        1_000_000 + (RECALIBRATE_INTERVAL_NS / 10_000) as i64
    );
}

#[test]
fn tightest_bracket_wins() {
    // Engine reads: (0, 1000) then (2000, 2100) then (3000, 3500): the second bracket
    // (100 ns) is the tightest; its midpoint is 2050. SDL reads 10, 20, 30.
    let engine_reads = [0u64, 1_000, 2_000, 2_100, 3_000, 3_500];
    let sdl_reads = [10u64, 20, 30];
    let mut e = engine_reads.into_iter();
    let mut s = sdl_reads.into_iter();
    let mapper = TickMapper::new(move || e.next().unwrap(), move || s.next().unwrap());
    assert_eq!(mapper.offset_ns(), 2_050 - 20);
    assert_eq!(mapper.uncertainty_ns(), 100);
}

#[test]
fn real_clocks_agree_to_within_a_millisecond() {
    let start = std::time::Instant::now();
    let engine = move || start.elapsed().as_nanos() as u64;
    let mut mapper = TickMapper::new(engine, slam_input::sdl_ticks_ns);
    let sdl_now = slam_input::sdl_ticks_ns();
    let engine_now = engine();
    let mapped = mapper.map(sdl_now);
    assert!(
        mapped.abs_diff(engine_now) < 1_000_000,
        "{mapped} vs {engine_now}"
    );
}

#[test]
fn noisy_recalibration_keeps_the_previous_offset() {
    // Engine clock that jumps by `step` on every read: brackets are `step` wide.
    let engine = Rc::new(Cell::new(0u64));
    let step = Rc::new(Cell::new(0u64));
    let engine_clock = {
        let (engine, step) = (Rc::clone(&engine), Rc::clone(&step));
        move || {
            engine.set(engine.get() + step.get());
            engine.get()
        }
    };
    let sdl = Rc::new(Cell::new(0));
    let mut mapper = TickMapper::new(engine_clock, fixed(&sdl));
    assert_eq!(mapper.offset_ns(), 0);

    // Past the interval, but every bracket is wider than the limit: offset kept.
    engine.set(RECALIBRATE_INTERVAL_NS);
    step.set(MAX_BRACKET_NS + 1);
    sdl.set(RECALIBRATE_INTERVAL_NS - 777);
    mapper.map(0);
    assert_eq!(mapper.offset_ns(), 0);

    // An event within the retry spacing does not measure again.
    step.set(0);
    engine.set(engine.get() + RETRY_SPACING_NS / 2);
    sdl.set(0);
    mapper.map(0);
    assert_eq!(mapper.offset_ns(), 0);

    // After the spacing the retry runs; with a quiet thread it succeeds.
    engine.set(engine.get() + RETRY_SPACING_NS);
    sdl.set(engine.get() - 777);
    mapper.map(0);
    assert_eq!(mapper.offset_ns(), engine.get() as i64 - sdl.get() as i64);
}

#[test]
fn explicit_calibrate_accepts_any_bracket() {
    let reads = [
        0u64,
        MAX_BRACKET_NS * 10,
        0,
        MAX_BRACKET_NS * 10,
        0,
        MAX_BRACKET_NS * 10,
    ];
    let mut e = reads.into_iter().chain(std::iter::repeat(0));
    let mapper = TickMapper::new(move || e.next().unwrap(), || 0);
    assert_eq!(mapper.offset_ns(), (MAX_BRACKET_NS * 5) as i64);
    assert_eq!(mapper.uncertainty_ns(), MAX_BRACKET_NS * 10);
}

#[test]
fn engine_clock_going_backwards_gives_zero_width() {
    let reads = [1_000u64, 900, 1_000, 900, 1_000, 900];
    let mut e = reads.into_iter();
    let mapper = TickMapper::new(move || e.next().unwrap(), || 100);
    assert_eq!(mapper.uncertainty_ns(), 0);
    assert_eq!(mapper.offset_ns(), 900);
}

#[test]
fn persistent_noise_accepts_the_tightest_after_max_rejections() {
    // Every read advances the engine clock by `step`, so each bracket is `step` wide
    // and the SDL clock (fixed) yields offset = midpoint.
    let engine = Rc::new(Cell::new(0u64));
    let step = Rc::new(Cell::new(0u64));
    let engine_clock = {
        let (engine, step) = (Rc::clone(&engine), Rc::clone(&step));
        move || {
            engine.set(engine.get() + step.get());
            engine.get()
        }
    };
    let mut mapper = TickMapper::new(engine_clock, || 0);
    let initial = mapper.offset_ns();

    engine.set(RECALIBRATE_INTERVAL_NS);
    for attempt in 0..MAX_REJECTIONS {
        // The tightest bracket comes on the third attempt.
        step.set(if attempt == 2 {
            MAX_BRACKET_NS * 2
        } else {
            MAX_BRACKET_NS * 5
        });
        mapper.map(0);
        if attempt + 1 < MAX_REJECTIONS {
            assert_eq!(mapper.offset_ns(), initial, "attempt {attempt}");
        }
        step.set(0);
        engine.set(engine.get() + RETRY_SPACING_NS);
    }
    // Accepted: the uncertainty is the tightest bracket across the attempts.
    assert_ne!(mapper.offset_ns(), initial);
    assert_eq!(mapper.uncertainty_ns(), MAX_BRACKET_NS * 2);
}
