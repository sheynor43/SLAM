//! Playback position published by the audio thread for the update thread (ADR-0022),
//! through a single-writer seqlock (see `seqlock`).

use crate::seqlock::SeqLock;

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
/// Readers never block; they spin only while a write is in progress.
#[derive(Debug, Default)]
pub struct AudioPosition {
    lock: SeqLock<4>,
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
        self.lock.write([
            snapshot.frame,
            snapshot.timestamp_ns,
            snapshot.latency_ns,
            u64::from(snapshot.sample_rate),
        ]);
    }

    /// Latest consistent report, or `None` before the first one. Allocation-free and
    /// lock-free; spins only while a write is in progress (a few stores).
    pub fn read(&self) -> Option<PositionSnapshot> {
        self.lock
            .read()
            .map(|[frame, timestamp_ns, latency_ns, rate]| PositionSnapshot {
                frame,
                timestamp_ns,
                latency_ns,
                sample_rate: rate as u32,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

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
