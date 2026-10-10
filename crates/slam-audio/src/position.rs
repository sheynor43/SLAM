//! Playback position published by the audio thread for the update thread (ADR-0022).
//!
//! A single-writer seqlock over atomics: neither side blocks, the reader retries while a
//! write is in progress. All fields are atomics, so a torn read is detected, never UB.
//!
//! The reader is lock-free, not wait-free: if the writer is preempted between its two
//! sequence stores (no real-time priority), a reader spins until it resumes. The
//! stress test runs on x86 (TSO) only and cannot catch an ordering regression on
//! weaker memory models; there is no loom model yet.

use std::sync::atomic::{AtomicU64, Ordering, fence};

/// One consistent position report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionSnapshot {
    /// Stream frame index of the first frame of the last buffer handed to the device.
    pub frame: u64,
    /// When the report was taken, in the engine clock base (`CLOCK_MONOTONIC` on Linux,
    /// QPC on Windows), nanoseconds.
    pub timestamp_ns: u64,
    /// Time from `timestamp_ns` until `frame` leaves the device, nanoseconds.
    pub latency_ns: u64,
    /// Stream sample rate, frames per second.
    pub sample_rate: u32,
}

impl PositionSnapshot {
    /// Stream frame (fractional) leaving the device at engine time `now_ns`, extrapolated
    /// linearly from the report. Negative before the first reported frame is audible.
    pub fn output_frame_at(&self, now_ns: u64) -> f64 {
        // Subtract in integers: nanosecond timestamps exceed `f64` precision after
        // ~104 days of uptime.
        let since_out_ns =
            i128::from(now_ns) - i128::from(self.timestamp_ns) - i128::from(self.latency_ns);
        self.frame as f64 + since_out_ns as f64 * f64::from(self.sample_rate) / 1e9
    }
}

/// Seqlock-protected [`PositionSnapshot`]. Writer: the audio callback. Readers: any thread.
#[derive(Debug, Default)]
pub struct AudioPosition {
    /// Even: stable; odd: write in progress; zero: nothing published yet.
    seq: AtomicU64,
    frame: AtomicU64,
    timestamp_ns: AtomicU64,
    latency_ns: AtomicU64,
    sample_rate: AtomicU64,
}

impl AudioPosition {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes a report. Allocation-free, lock-free, wait-free.
    ///
    /// Must only be called from one thread at a time (the audio callback); concurrent
    /// writers corrupt the sequence.
    pub fn publish(&self, snapshot: PositionSnapshot) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        // Orders the odd sequence before the field stores below.
        fence(Ordering::Release);
        self.frame.store(snapshot.frame, Ordering::Relaxed);
        self.timestamp_ns
            .store(snapshot.timestamp_ns, Ordering::Relaxed);
        self.latency_ns
            .store(snapshot.latency_ns, Ordering::Relaxed);
        self.sample_rate
            .store(u64::from(snapshot.sample_rate), Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// Latest consistent report, or `None` before the first one. Allocation-free and
    /// lock-free; spins only while a write is in progress (a few stores).
    pub fn read(&self) -> Option<PositionSnapshot> {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            if before == 0 {
                return None;
            }
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let snapshot = PositionSnapshot {
                frame: self.frame.load(Ordering::Relaxed),
                timestamp_ns: self.timestamp_ns.load(Ordering::Relaxed),
                latency_ns: self.latency_ns.load(Ordering::Relaxed),
                sample_rate: self.sample_rate.load(Ordering::Relaxed) as u32,
            };
            // Orders the field loads before the sequence re-check.
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                return Some(snapshot);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn empty_until_first_publish() {
        let pos = AudioPosition::new();
        assert_eq!(pos.read(), None);
        let s = PositionSnapshot {
            frame: 480,
            timestamp_ns: 1_000,
            latency_ns: 2_000,
            sample_rate: 48_000,
        };
        pos.publish(s);
        assert_eq!(pos.read(), Some(s));
    }

    #[test]
    fn output_frame_extrapolates() {
        let s = PositionSnapshot {
            frame: 48_000,
            timestamp_ns: 10_000_000_000,
            latency_ns: 5_000_000,
            sample_rate: 48_000,
        };
        // Frame 48000 leaves the device 5 ms after the report.
        assert_eq!(s.output_frame_at(10_005_000_000), 48_000.0);
        // 10 ms later, 480 more frames have played.
        assert_eq!(s.output_frame_at(10_015_000_000), 48_480.0);
        // At the report time the output is still 240 frames behind.
        assert_eq!(s.output_frame_at(10_000_000_000), 47_760.0);
        // Before the stream's first frame is audible the result is negative.
        let start = PositionSnapshot { frame: 0, ..s };
        assert_eq!(start.output_frame_at(10_000_000_000), -240.0);
        // Exact at timestamps beyond f64 integer precision (2^53 ns ≈ 104 days).
        let late = PositionSnapshot {
            timestamp_ns: (1 << 60) + 1,
            ..s
        };
        assert_eq!(
            late.output_frame_at((1 << 60) + 1 + 5_000_000 + 1_000_000),
            48_048.0
        );
    }

    /// Every field of a published report is derived from `frame`; a torn read would
    /// mix two reports and break the relation.
    #[test]
    fn concurrent_reads_are_never_torn() {
        let pos = Arc::new(AudioPosition::new());
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let pos = Arc::clone(&pos);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut frame = 1u64;
                while !stop.load(Ordering::Relaxed) {
                    pos.publish(PositionSnapshot {
                        frame,
                        timestamp_ns: frame * 3,
                        latency_ns: frame * 5,
                        sample_rate: (frame % 1_000_000) as u32,
                    });
                    frame += 1;
                }
            })
        };
        let mut seen = 0u64;
        let mut last = 0u64;
        while seen < 200_000 {
            if let Some(s) = pos.read() {
                assert_eq!(s.timestamp_ns, s.frame * 3);
                assert_eq!(s.latency_ns, s.frame * 5);
                assert_eq!(u64::from(s.sample_rate), s.frame % 1_000_000);
                assert!(s.frame >= last, "position went backwards");
                last = s.frame;
                seen += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
    }
}
