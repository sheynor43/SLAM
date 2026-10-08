//! Translation of SDL event timestamps into the engine clock.
//!
//! SDL stamps events in its own time base (`SDL_GetTicksNS`: nanoseconds since SDL
//! initialisation; `CLOCK_MONOTONIC_RAW` on Linux, `QueryPerformanceCounter` on
//! Windows). The engine runs on its own monotonic clock (`CLOCK_MONOTONIC` on Linux),
//! which NTP may slew relative to the raw clock by up to a few hundred ppm. The
//! mapper keeps an offset between the two, measured by bracketing an SDL clock read
//! between two engine clock reads, and re-measures it periodically so drift never
//! exceeds a few microseconds.

/// How often [`TickMapper::map`] re-measures the clock offset, in engine nanoseconds.
pub const RECALIBRATE_INTERVAL_NS: u64 = 1_000_000_000;

/// Bracketing attempts per calibration; the tightest bracket wins.
const CALIBRATION_SAMPLES: usize = 3;

/// A calibration whose tightest bracket is wider than this (the thread was preempted
/// mid-measurement) is discarded in favour of the previous offset and retried on the
/// next [`TickMapper::map`].
pub const MAX_BRACKET_NS: u64 = 50_000;

/// Minimum engine time between retries after a rejected recalibration, so a noisy
/// host does not pay for a full measurement on every event.
pub const RETRY_SPACING_NS: u64 = 10_000_000;

/// After this many consecutive rejections the tightest bracket seen across them is
/// accepted anyway: on a host where every bracket is wide (VM, slow clocksource) a
/// noisy offset beats one that never follows the drift.
pub const MAX_REJECTIONS: u32 = 8;

/// Current SDL tick time in nanoseconds, the time base of SDL event timestamps.
///
/// SDL re-bases this clock to zero on every `SDL_Init`, so an offset measured against
/// it is valid only within one SDL lifetime (see [`TickMapper::calibrate`]).
pub fn sdl_ticks_ns() -> u64 {
    // SAFETY: SDL initialises its tick base lazily on first use. Concurrent calls
    // from another thread during `SDL_Init`/`SDL_Quit` race on that base, so the
    // input pipeline reads this clock only from the thread that owns SDL.
    unsafe { sdl3_sys::timer::SDL_GetTicksNS() }
}

/// Maps SDL timestamps to engine time. `E` reads the engine clock, `S` the SDL clock.
pub struct TickMapper<E, S> {
    engine_now: E,
    sdl_now: S,
    /// `engine - sdl` at the last calibration.
    offset_ns: i64,
    /// Engine time of the last calibration.
    calibrated_at_ns: u64,
    /// Width of the bracket of the last calibration: the offset's uncertainty.
    uncertainty_ns: u64,
    /// Engine time before which no recalibration is attempted after a rejection.
    retry_at_ns: u64,
    /// Consecutive rejected recalibrations.
    rejections: u32,
    /// Tightest rejected measurement since the last accepted one: (width, offset).
    best_rejected: Option<(u64, i64)>,
}

impl<E: FnMut() -> u64, S: FnMut() -> u64> TickMapper<E, S> {
    pub fn new(engine_now: E, sdl_now: S) -> Self {
        let mut mapper = Self {
            engine_now,
            sdl_now,
            offset_ns: 0,
            calibrated_at_ns: 0,
            uncertainty_ns: 0,
            retry_at_ns: 0,
            rejections: 0,
            best_rejected: None,
        };
        mapper.measure(true);
        mapper
    }

    /// Re-measures the offset unconditionally. Call after (re)initialising SDL, which
    /// resets its tick base.
    pub fn calibrate(&mut self) {
        self.measure(true);
    }

    /// Measures the offset. Unless `force`d, a measurement with a bracket wider than
    /// [`MAX_BRACKET_NS`] is rejected (the previous offset stays, a retry is scheduled
    /// after [`RETRY_SPACING_NS`]) until [`MAX_REJECTIONS`] in a row, after which the
    /// tightest one seen is accepted.
    fn measure(&mut self, force: bool) {
        let mut best: Option<(u64, u64, u64)> = None; // (bracket width, engine midpoint, sdl)
        for _ in 0..CALIBRATION_SAMPLES {
            let before = (self.engine_now)();
            let sdl = (self.sdl_now)();
            let after = (self.engine_now)();
            let width = after.saturating_sub(before);
            if best.is_none_or(|(w, _, _)| width < w) {
                best = Some((width, before + width / 2, sdl));
            }
        }
        let (mut width, engine, sdl) = best.expect("at least one sample");
        let mut offset = engine.wrapping_sub(sdl) as i64;
        if !force && width > MAX_BRACKET_NS {
            if let Some((w, o)) = self.best_rejected
                && w <= width
            {
                (width, offset) = (w, o);
            }
            self.rejections += 1;
            if self.rejections < MAX_REJECTIONS {
                self.best_rejected = Some((width, offset));
                self.retry_at_ns = engine.saturating_add(RETRY_SPACING_NS);
                return;
            }
        }
        self.offset_ns = offset;
        self.calibrated_at_ns = engine;
        self.uncertainty_ns = width;
        self.retry_at_ns = 0;
        self.rejections = 0;
        self.best_rejected = None;
    }

    /// Translates an SDL timestamp into engine time, re-measuring the offset first if
    /// it is older than [`RECALIBRATE_INTERVAL_NS`].
    pub fn map(&mut self, sdl_timestamp_ns: u64) -> u64 {
        // One extra engine clock read per event (a vDSO call, ~20 ns) keeps the
        // recalibration schedule without a timer.
        let now = (self.engine_now)();
        if now.saturating_sub(self.calibrated_at_ns) >= RECALIBRATE_INTERVAL_NS
            && now >= self.retry_at_ns
        {
            self.measure(false);
        }
        self.to_engine(sdl_timestamp_ns)
    }

    /// Translates with the current offset, without touching the clocks.
    pub fn to_engine(&self, sdl_timestamp_ns: u64) -> u64 {
        sdl_timestamp_ns.saturating_add_signed(self.offset_ns)
    }

    /// `engine - sdl` in nanoseconds.
    pub fn offset_ns(&self) -> i64 {
        self.offset_ns
    }

    /// Uncertainty of the current offset in nanoseconds.
    pub fn uncertainty_ns(&self) -> u64 {
        self.uncertainty_ns
    }
}
