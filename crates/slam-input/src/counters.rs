use std::sync::atomic::{AtomicU64, Ordering};

/// Activity of the input thread since the window was created, for the frame-time
/// overlay. Updated by [`InputWindow::run`](crate::InputWindow::run), read from any
/// thread; values are relaxed and may lag.
#[derive(Debug, Default)]
pub struct InputCounters {
    polls: AtomicU64,
    queued: AtomicU64,
}

impl InputCounters {
    /// Returns of the event wait: every OS event the main thread woke up for, input or
    /// not.
    pub fn polls(&self) -> u64 {
        self.polls.load(Ordering::Relaxed)
    }

    /// Input events pushed into the ring (dropped ones not included).
    pub fn queued(&self) -> u64 {
        self.queued.load(Ordering::Relaxed)
    }

    /// Counts a return of the event wait.
    pub fn record_poll(&self) {
        self.polls.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts a queued event.
    pub fn record_queued(&self) {
        self.queued.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_from_zero() {
        let c = InputCounters::default();
        assert_eq!((c.polls(), c.queued()), (0, 0));
        c.record_poll();
        c.record_poll();
        c.record_queued();
        assert_eq!((c.polls(), c.queued()), (2, 1));
    }
}
