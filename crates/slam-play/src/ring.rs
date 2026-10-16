//! A fixed-size ring of events in the snapshot: the update side appends, the draw side reads
//! what it has not seen by a running count (ADR-0042, ADR-0043).

/// The last `N` events, newest last, with a count of all ever written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EventRing<T, const N: usize> {
    entries: [T; N],
    written: u64,
    /// Events written before the current attempt (a restart).
    attempt_start: u64,
}

impl<T: Copy + Default, const N: usize> Default for EventRing<T, N> {
    fn default() -> Self {
        EventRing {
            entries: [T::default(); N],
            written: 0,
            attempt_start: 0,
        }
    }
}

impl<T: Copy, const N: usize> EventRing<T, N> {
    /// Appends an event, overwriting the oldest one when full.
    pub fn push(&mut self, event: T) {
        self.entries[(self.written % N as u64) as usize] = event;
        self.written += 1;
    }

    /// Events ever written.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Starts a new attempt: the events written so far are of the previous one and are no
    /// longer read.
    pub fn restart(&mut self) {
        self.attempt_start = self.written;
    }

    /// The events of the current attempt written after the first `seen`, oldest first. Those
    /// overwritten already are skipped.
    pub fn since(&self, seen: u64) -> impl Iterator<Item = &T> + '_ {
        let start = seen
            .max(self.attempt_start)
            .max(self.written.saturating_sub(N as u64));
        (start..self.written).map(|i| &self.entries[(i % N as u64) as usize])
    }

    /// How many events of the current attempt written after the first `seen` were overwritten
    /// before [`EventRing::since`] could return them.
    pub fn skipped(&self, seen: u64) -> u64 {
        self.written
            .saturating_sub(N as u64)
            .saturating_sub(seen.max(self.attempt_start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_was_not_seen_of_the_attempt() {
        let mut ring = EventRing::<u32, 4>::default();
        for i in 0..6 {
            ring.push(i);
        }
        assert_eq!(ring.since(0).copied().collect::<Vec<_>>(), [2, 3, 4, 5]);
        assert_eq!(ring.skipped(0), 2);
        assert_eq!(ring.since(5).copied().collect::<Vec<_>>(), [5]);
        assert_eq!(ring.skipped(5), 0);
        ring.restart();
        ring.push(6);
        assert_eq!(ring.since(0).copied().collect::<Vec<_>>(), [6]);
        assert_eq!(ring.skipped(0), 0);
    }
}
