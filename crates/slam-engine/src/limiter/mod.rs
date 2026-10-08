//! Frame limiter with absolute deadlines.
//!
//! Deadlines are computed from the frame index (`epoch + n * period`), so rounding
//! never accumulates and the schedule does not drift. Waiting is a coarse
//! high-resolution sleep followed by a busy-wait for the last `spin_threshold`.
//! A deadline missed by a whole period or more restarts the schedule from the
//! current time instead of bursting frames to catch up.

mod sys;

pub use sys::SystemTimer;

/// Highest supported frame rate.
pub const MAX_HZ: f64 = 8000.0;

/// Lowest supported rate. Lower rates make deadlines overflow and a running loop
/// unresponsive to rate changes for too long.
pub const MIN_HZ: f64 = 1.0;

/// Whether `hz` is a valid frame or update rate: finite, in `[MIN_HZ, MAX_HZ]`.
pub fn is_valid_hz(hz: f64) -> bool {
    hz.is_finite() && (MIN_HZ..=MAX_HZ).contains(&hz)
}

/// Default length of the busy-wait before each deadline.
pub const DEFAULT_SPIN_THRESHOLD_NS: u64 = 200_000;

const NS_PER_SEC: f64 = 1_000_000_000.0;

/// Monotonic time source with a coarse sleep. Both use the same time base.
pub trait Timer {
    /// Current monotonic time in nanoseconds.
    fn now_ns(&self) -> u64;

    /// Sleeps until roughly `deadline_ns`. May wake early or late.
    fn sleep_until(&mut self, deadline_ns: u64);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LimiterMode {
    /// `wait` returns immediately.
    Unlimited,
    /// Fixed rate in frames per second, `MIN_HZ <= hz <= MAX_HZ`.
    Hz(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("frame rate must be finite and in [{MIN_HZ}, {MAX_HZ}]")]
pub struct InvalidRate;

/// Result of a single [`FrameLimiter::wait`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameWait {
    /// Deadline of this frame. Equals `woke_ns` in unlimited mode.
    pub deadline_ns: u64,
    /// Time at which `wait` returned.
    pub woke_ns: u64,
    /// The deadline was missed by at least one period and the schedule restarted.
    pub resynced: bool,
}

impl FrameWait {
    /// How late `wait` returned relative to the deadline.
    pub fn lateness_ns(&self) -> u64 {
        self.woke_ns.saturating_sub(self.deadline_ns)
    }
}

pub struct FrameLimiter<T: Timer = SystemTimer> {
    timer: T,
    mode: LimiterMode,
    /// Zero in unlimited mode.
    period_ns: f64,
    spin_threshold_ns: u64,
    epoch_ns: u64,
    frame: u64,
    started: bool,
}

impl FrameLimiter<SystemTimer> {
    pub fn new(mode: LimiterMode) -> Result<Self, InvalidRate> {
        Self::with_timer(SystemTimer::new(), mode)
    }
}

impl<T: Timer> FrameLimiter<T> {
    pub fn with_timer(timer: T, mode: LimiterMode) -> Result<Self, InvalidRate> {
        let mut limiter = Self {
            timer,
            mode: LimiterMode::Unlimited,
            period_ns: 0.0,
            spin_threshold_ns: DEFAULT_SPIN_THRESHOLD_NS,
            epoch_ns: 0,
            frame: 0,
            started: false,
        };
        limiter.set_mode(mode)?;
        Ok(limiter)
    }

    /// Changes the mode. The schedule restarts on the next `wait`.
    pub fn set_mode(&mut self, mode: LimiterMode) -> Result<(), InvalidRate> {
        self.period_ns = match mode {
            LimiterMode::Unlimited => 0.0,
            LimiterMode::Hz(hz) if is_valid_hz(hz) => NS_PER_SEC / hz,
            LimiterMode::Hz(_) => return Err(InvalidRate),
        };
        self.mode = mode;
        self.started = false;
        Ok(())
    }

    pub fn mode(&self) -> LimiterMode {
        self.mode
    }

    /// Sets how long before each deadline the limiter stops sleeping and busy-waits.
    /// Zero sleeps right up to the deadline; a threshold longer than the period
    /// never sleeps and spins for the whole frame.
    pub fn set_spin_threshold_ns(&mut self, ns: u64) {
        self.spin_threshold_ns = ns;
    }

    pub fn spin_threshold_ns(&self) -> u64 {
        self.spin_threshold_ns
    }

    pub fn timer(&self) -> &T {
        &self.timer
    }

    /// Blocks until the next frame deadline. Allocation-free.
    pub fn wait(&mut self) -> FrameWait {
        if self.period_ns == 0.0 {
            let now = self.timer.now_ns();
            return FrameWait {
                deadline_ns: now,
                woke_ns: now,
                resynced: false,
            };
        }

        if !self.started {
            self.epoch_ns = self.timer.now_ns();
            self.frame = 0;
            self.started = true;
        }
        self.frame += 1;
        let deadline = self.epoch_ns + (self.frame as f64 * self.period_ns).round() as u64;

        let mut now = self.timer.now_ns();
        if now >= deadline {
            let resynced = (now - deadline) as f64 >= self.period_ns;
            if resynced {
                self.epoch_ns = now;
                self.frame = 0;
            }
            return FrameWait {
                deadline_ns: deadline,
                woke_ns: now,
                resynced,
            };
        }

        if deadline - now > self.spin_threshold_ns {
            self.timer.sleep_until(deadline - self.spin_threshold_ns);
            now = self.timer.now_ns();
        }
        while now < deadline {
            std::hint::spin_loop();
            now = self.timer.now_ns();
        }

        FrameWait {
            deadline_ns: deadline,
            woke_ns: now,
            resynced: false,
        }
    }
}

// The limiter is created on one thread and moved to the draw thread.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<FrameLimiter<SystemTimer>>();
};

#[cfg(test)]
mod tests;
