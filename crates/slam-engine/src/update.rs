//! Update loop: ticks at a fixed rate and as soon as input arrives, writes snapshots.

use slam_input::{EventSource, InputEvent};

use crate::clock;
use crate::snapshot::SnapshotWriter;
use crate::wake::Parker;

/// Highest supported update rate.
pub const MAX_UPDATE_HZ: f64 = crate::limiter::MAX_HZ;

const NS_PER_SEC: f64 = 1_000_000_000.0;

/// Game logic run by the update thread.
pub trait Update: Send + 'static {
    /// State handed to the draw thread after every tick.
    type Snapshot: Send + 'static;

    /// One step of game logic. Must not allocate, lock or do I/O.
    ///
    /// `events` holds the input that arrived since the previous tick, in
    /// non-decreasing `time_ns` order, also across ticks. `snapshot` holds an older
    /// snapshot, not necessarily the previous one: overwrite everything draw reads.
    fn tick(&mut self, info: &TickInfo, events: &[InputEvent], snapshot: &mut Self::Snapshot);
}

/// Why and when a tick runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickInfo {
    /// Engine time right after the tick's events were taken from the ring: no event
    /// passed to the tick is newer.
    pub now_ns: u64,
    /// The rate deadline this tick serves; for a tick woken early by input, the next
    /// deadline, still ahead.
    pub deadline_ns: u64,
    /// Woken by input before the deadline.
    pub by_input: bool,
}

/// Counters of the update loop since start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdateStats {
    pub ticks: u64,
    /// Ticks woken by input before their deadline.
    pub input_ticks: u64,
    /// Deadlines missed by a whole period or more; the schedule restarted.
    pub resyncs: u64,
    /// Events whose timestamp was older than an event already delivered in an earlier
    /// tick (different devices, delivered late); raised to that event's time to keep
    /// the order.
    pub reordered: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvalidUpdateConfig {
    #[error(
        "update rate must be finite and in [{}, {MAX_UPDATE_HZ}]",
        crate::limiter::MIN_HZ
    )]
    Rate,
    #[error("event batch must hold at least one event")]
    EmptyBatch,
}

/// The update thread's state. [`UpdateLoop::step`] is one iteration: wait, then tick.
pub struct UpdateLoop<U: Update> {
    state: U,
    source: EventSource,
    parker: Parker,
    writer: SnapshotWriter<U::Snapshot>,
    /// Preallocated; never grows.
    events: Vec<InputEvent>,
    hz: f64,
    period_ns: f64,
    spin_ns: u64,
    epoch_ns: u64,
    deadline_index: u64,
    started: bool,
    last_event_ns: u64,
    stats: UpdateStats,
}

impl<U: Update> UpdateLoop<U> {
    /// `event_batch` is the most events passed to one tick; more are split into
    /// several ticks.
    pub fn new(
        state: U,
        source: EventSource,
        parker: Parker,
        writer: SnapshotWriter<U::Snapshot>,
        hz: f64,
        event_batch: usize,
        spin_ns: u64,
    ) -> Result<Self, InvalidUpdateConfig> {
        validate_hz(hz)?;
        if event_batch == 0 {
            return Err(InvalidUpdateConfig::EmptyBatch);
        }
        Ok(Self {
            state,
            source,
            parker,
            writer,
            events: Vec::with_capacity(event_batch),
            hz,
            period_ns: NS_PER_SEC / hz,
            spin_ns,
            epoch_ns: 0,
            deadline_index: 0,
            started: false,
            last_event_ns: 0,
            stats: UpdateStats::default(),
        })
    }

    /// Changes the update rate. The schedule restarts on the next step.
    pub fn set_hz(&mut self, hz: f64) -> Result<(), InvalidUpdateConfig> {
        validate_hz(hz)?;
        self.hz = hz;
        self.period_ns = NS_PER_SEC / hz;
        self.started = false;
        Ok(())
    }

    pub fn hz(&self) -> f64 {
        self.hz
    }

    pub fn state(&self) -> &U {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut U {
        &mut self.state
    }

    pub fn stats(&self) -> UpdateStats {
        self.stats
    }

    pub fn into_state(self) -> U {
        self.state
    }

    fn deadline(&self) -> u64 {
        self.epoch_ns + (self.deadline_index as f64 * self.period_ns).round() as u64
    }

    /// Waits for the next deadline or a notification, then ticks and publishes a
    /// snapshot. A notification with no queued input (already consumed by an earlier
    /// tick, or a stop request) returns without ticking. Returns whether it ticked.
    /// Allocation-free.
    pub fn step(&mut self) -> bool {
        if !self.started {
            self.epoch_ns = clock::now_ns();
            self.deadline_index = 1;
            self.started = true;
        }
        let deadline = self.deadline();
        self.parker.wait_until(deadline, self.spin_ns);
        let now = clock::now_ns();
        let due = now >= deadline;
        if !due && self.source.is_empty() {
            return false;
        }
        let by_input = !due;
        if due {
            if (now - deadline) as f64 >= self.period_ns {
                self.epoch_ns = now;
                self.deadline_index = 1;
                self.stats.resyncs += 1;
            } else {
                self.deadline_index += 1;
            }
        }
        let mut info = TickInfo {
            now_ns: now,
            deadline_ns: if due { deadline } else { self.deadline() },
            by_input,
        };

        // At least one tick even without input; more while the ring holds more than
        // a batch.
        loop {
            crate::zone!("update tick");
            self.drain();
            // After the drain, so no event is newer than the tick.
            info.now_ns = clock::now_ns();
            self.state.tick(&info, &self.events, self.writer.slot());
            self.writer.publish();
            self.stats.ticks += 1;
            if by_input {
                self.stats.input_ticks += 1;
            }
            if self.events.len() < self.events.capacity() || self.source.is_empty() {
                break;
            }
        }
        true
    }

    /// Fills `events` from the ring (up to its capacity), sorted by time and never
    /// older than an event delivered before.
    fn drain(&mut self) {
        self.events.clear();
        while self.events.len() < self.events.capacity() {
            match self.source.pop() {
                Some(event) => self.events.push(event),
                None => break,
            }
        }
        insertion_sort_by_time(&mut self.events);
        for event in &mut self.events {
            if event.time_ns < self.last_event_ns {
                event.time_ns = self.last_event_ns;
                self.stats.reordered += 1;
            } else {
                self.last_event_ns = event.time_ns;
            }
        }
    }
}

pub(crate) fn validate_hz(hz: f64) -> Result<(), InvalidUpdateConfig> {
    if crate::limiter::is_valid_hz(hz) {
        Ok(())
    } else {
        Err(InvalidUpdateConfig::Rate)
    }
}

/// Stable, in place, linear on already sorted input (the usual case: the ring is
/// out of order only across devices).
fn insertion_sort_by_time(events: &mut [InputEvent]) {
    for i in 1..events.len() {
        let mut j = i;
        while j > 0 && events[j - 1].time_ns > events[j].time_ns {
            events.swap(j - 1, j);
            j -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slam_input::InputKind;

    fn ev(time_ns: u64, x: f32) -> InputEvent {
        InputEvent {
            time_ns,
            kind: InputKind::MouseMove { x, y: 0.0 },
        }
    }

    #[test]
    fn sort_is_stable() {
        let mut events = [ev(3, 0.0), ev(1, 1.0), ev(3, 2.0), ev(2, 3.0), ev(1, 4.0)];
        insertion_sort_by_time(&mut events);
        assert_eq!(
            events,
            [ev(1, 1.0), ev(1, 4.0), ev(2, 3.0), ev(3, 0.0), ev(3, 2.0)]
        );
    }
}
