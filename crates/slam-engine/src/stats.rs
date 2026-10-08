//! Frame-time statistics and thread rates for the overlay. Allocation-free after
//! construction.

use std::sync::atomic::{AtomicU64, Ordering};

const NS_PER_SEC: f64 = 1_000_000_000.0;

/// Progress of the update and draw threads, published by [`Engine`](crate::Engine)
/// after every step. Each counter has a single writer; reads are relaxed and may lag.
#[derive(Debug, Default)]
pub struct EngineCounters {
    update_ticks: Padded,
    draw_frames: Padded,
}

/// A counter on its own cache line pair (adjacent-line prefetch), so the update and
/// draw threads do not write to a shared line.
#[derive(Debug, Default)]
#[repr(align(128))]
struct Padded(AtomicU64);

impl EngineCounters {
    /// Ticks run by the update thread since start.
    pub fn update_ticks(&self) -> u64 {
        self.update_ticks.0.load(Ordering::Relaxed)
    }

    /// Frames drawn by the draw thread since start.
    pub fn draw_frames(&self) -> u64 {
        self.draw_frames.0.load(Ordering::Relaxed)
    }

    pub(crate) fn set_update_ticks(&self, ticks: u64) {
        self.update_ticks.0.store(ticks, Ordering::Relaxed);
    }

    pub(crate) fn set_draw_frames(&self, frames: u64) {
        self.draw_frames.0.store(frames, Ordering::Relaxed);
    }
}

/// Percentiles of the frame times held by [`FrameTimes`], in nanoseconds. Nearest
/// rank: the smallest sample with at least `p` of all samples at or below it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameSummary {
    /// Samples summarised.
    pub samples: usize,
    pub p50_ns: u32,
    pub p99_ns: u32,
    pub p999_ns: u32,
    pub max_ns: u32,
    pub mean_ns: u32,
}

/// The last `capacity` frame times (intervals between frame starts) in a ring.
/// Samples are stored as `u32` nanoseconds, saturating at about 4.3 s.
#[derive(Debug)]
pub struct FrameTimes {
    ring: Box<[u32]>,
    /// Slot of the next sample.
    next: usize,
    len: usize,
    /// Copy of the ring reordered by [`FrameTimes::summary`]; kept to reuse its memory.
    scratch: Box<[u32]>,
    last_start_ns: Option<u64>,
}

impl FrameTimes {
    /// Enough samples for a stable p99.9: a thousand frames over the 99.9th
    /// percentile is four samples.
    pub const DEFAULT_CAPACITY: usize = 4096;

    /// # Panics
    /// If `capacity` is zero.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "frame time ring needs at least one slot");
        Self {
            ring: vec![0; capacity].into_boxed_slice(),
            next: 0,
            len: 0,
            scratch: vec![0; capacity].into_boxed_slice(),
            last_start_ns: None,
        }
    }

    pub fn capacity(&self) -> usize {
        self.ring.len()
    }

    /// Samples held, at most the capacity.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Records the interval since the previous frame start. The first frame, or one
    /// starting before the previous one, records nothing.
    pub fn record_start(&mut self, start_ns: u64) {
        if let Some(last) = self.last_start_ns
            && start_ns >= last
        {
            self.record(start_ns - last);
        }
        self.last_start_ns = Some(start_ns);
    }

    /// Adds one frame time, overwriting the oldest once full.
    pub fn record(&mut self, ns: u64) {
        self.ring[self.next] = u32::try_from(ns).unwrap_or(u32::MAX);
        self.next = (self.next + 1) % self.ring.len();
        self.len = (self.len + 1).min(self.ring.len());
    }

    /// Forgets all samples and the previous frame start.
    pub fn clear(&mut self) {
        self.next = 0;
        self.len = 0;
        self.last_start_ns = None;
    }

    /// The newest `n` samples (fewer if not held), oldest first.
    pub fn latest(&self, n: usize) -> impl DoubleEndedIterator<Item = u32> + '_ {
        let n = n.min(self.len);
        let cap = self.ring.len();
        let start = (self.next + cap - n) % cap;
        // Up to two contiguous runs: to the end of the ring, then from its start.
        let first = (cap - start).min(n);
        self.ring[start..start + first]
            .iter()
            .chain(&self.ring[..n - first])
            .copied()
    }

    /// Percentiles of all held samples, `None` when empty. Linear in the number of
    /// samples (selection, not sorting).
    pub fn summary(&mut self) -> Option<FrameSummary> {
        let n = self.len;
        if n == 0 {
            return None;
        }
        // Order does not matter; the held samples are a prefix once the ring is full
        // or before it wraps.
        let samples = &mut self.scratch[..n];
        samples.copy_from_slice(&self.ring[..n]);
        let sum: u64 = samples.iter().map(|&s| u64::from(s)).sum();

        let (i50, i99, i999) = (rank(n, 500), rank(n, 990), rank(n, 999));
        // Each selection leaves everything after its index no smaller, so the next,
        // higher rank is selected within that tail only. A selection reorders its
        // tail, so each value is read before the next one.
        let p50 = select(samples, i50, 0);
        let p99 = select(samples, i99, i50);
        let p999 = select(samples, i999, i99);
        let max = samples[i999..].iter().copied().max().unwrap_or(p999);
        Some(FrameSummary {
            samples: n,
            p50_ns: p50,
            p99_ns: p99,
            p999_ns: p999,
            max_ns: max,
            mean_ns: (sum / n as u64) as u32,
        })
    }
}

/// Puts the value of zero-based rank `rank` at `samples[rank]` and returns it.
/// `samples[from..]` must already hold every rank from `from` up.
fn select(samples: &mut [u32], rank: usize, from: usize) -> u32 {
    *samples[from..].select_nth_unstable(rank - from).1
}

/// Zero-based index of the nearest-rank percentile `per_mille / 1000` of `n > 0`
/// sorted samples: rank `ceil(n * p)`, at least 1.
fn rank(n: usize, per_mille: usize) -> usize {
    (n * per_mille).div_ceil(1000).max(1) - 1
}

/// Rate of a growing counter, averaged over windows of at least `window_ns`.
#[derive(Clone, Copy, Debug)]
pub struct RateMeter {
    window_ns: u64,
    /// Count and time at the start of the current window.
    start: Option<(u64, u64)>,
    hz: Option<f64>,
}

impl RateMeter {
    pub fn new(window_ns: u64) -> Self {
        Self {
            window_ns: window_ns.max(1),
            start: None,
            hz: None,
        }
    }

    /// Feeds the counter's value at `now_ns`. Once a window has passed, computes the
    /// rate over it, starts the next window and returns `true`. A counter that went
    /// backwards (restarted) or a clock that did starts a new window.
    pub fn sample(&mut self, count: u64, now_ns: u64) -> bool {
        let Some((count0, time0)) = self.start else {
            self.start = Some((count, now_ns));
            return false;
        };
        if count < count0 || now_ns < time0 {
            self.start = Some((count, now_ns));
            return false;
        }
        let elapsed = now_ns - time0;
        if elapsed < self.window_ns {
            return false;
        }
        self.hz = Some((count - count0) as f64 * NS_PER_SEC / elapsed as f64);
        self.start = Some((count, now_ns));
        true
    }

    /// Rate over the last complete window, in Hz.
    pub fn hz(&self) -> Option<f64> {
        self.hz
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference nearest-rank percentile by sorting.
    fn reference(samples: &[u32], per_mille: usize) -> u32 {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let rank = (samples.len() * per_mille).div_ceil(1000).max(1);
        sorted[rank - 1]
    }

    fn filled(samples: &[u32]) -> FrameTimes {
        let mut t = FrameTimes::new(samples.len().max(1));
        for &s in samples {
            t.record(u64::from(s));
        }
        t
    }

    #[test]
    fn empty_has_no_summary() {
        assert_eq!(FrameTimes::new(8).summary(), None);
    }

    #[test]
    fn one_sample_is_every_percentile() {
        let s = filled(&[1234]).summary().unwrap();
        assert_eq!(
            s,
            FrameSummary {
                samples: 1,
                p50_ns: 1234,
                p99_ns: 1234,
                p999_ns: 1234,
                max_ns: 1234,
                mean_ns: 1234,
            }
        );
    }

    #[test]
    fn nearest_rank_of_1_to_1000() {
        // Shuffled 1..=1000: percentile p is exactly 1000 * p.
        let samples: Vec<u32> = (0..1000).map(|i| (i * 337 % 1000) + 1).collect();
        let s = filled(&samples).summary().unwrap();
        assert_eq!(
            (s.p50_ns, s.p99_ns, s.p999_ns, s.max_ns),
            (500, 990, 999, 1000)
        );
        assert_eq!(s.mean_ns, 500); // 500.5 truncated
    }

    #[test]
    fn small_counts_round_rank_up() {
        // n = 10: p50 is rank 5, p99 and p99.9 are rank 10.
        let samples = [10, 9, 8, 7, 6, 5, 4, 3, 2, 1];
        let s = filled(&samples).summary().unwrap();
        assert_eq!((s.p50_ns, s.p99_ns, s.p999_ns), (5, 10, 10));
        // n = 2: p50 is rank 1.
        let s = filled(&[7, 3]).summary().unwrap();
        assert_eq!((s.p50_ns, s.p99_ns), (3, 7));
    }

    #[test]
    fn matches_sorting_on_spiky_data() {
        // Pseudo-random frame times with rare large spikes, many duplicates.
        let mut x = 0x2545_f491_u32;
        let samples: Vec<u32> = (0..3000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                if x.is_multiple_of(500) {
                    20_000_000 + x % 1000
                } else {
                    1_000_000 + x % 50
                }
            })
            .collect();
        let s = filled(&samples).summary().unwrap();
        assert_eq!(s.p50_ns, reference(&samples, 500));
        assert_eq!(s.p99_ns, reference(&samples, 990));
        assert_eq!(s.p999_ns, reference(&samples, 999));
        assert_eq!(s.max_ns, *samples.iter().max().unwrap());
    }

    #[test]
    fn ring_keeps_only_the_newest() {
        let mut t = FrameTimes::new(4);
        for ns in [100, 1, 2, 3, 4] {
            t.record(ns);
        }
        assert_eq!(t.len(), 4);
        assert_eq!(t.latest(10).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert_eq!(t.latest(2).collect::<Vec<_>>(), [3, 4]);
        // The overwritten 100 is gone from the statistics.
        assert_eq!(t.summary().unwrap().max_ns, 4);
    }

    #[test]
    fn latest_before_wrap() {
        let mut t = FrameTimes::new(8);
        for ns in [5, 6, 7] {
            t.record(ns);
        }
        assert_eq!(t.latest(8).collect::<Vec<_>>(), [5, 6, 7]);
        assert_eq!(t.latest(0).count(), 0);
    }

    #[test]
    fn records_intervals_between_starts() {
        let mut t = FrameTimes::new(8);
        t.record_start(1_000);
        assert!(t.is_empty());
        t.record_start(3_000);
        t.record_start(3_500);
        // A start before the previous one (clock went back) is skipped.
        t.record_start(3_200);
        t.record_start(4_200);
        assert_eq!(t.latest(8).collect::<Vec<_>>(), [2_000, 500, 1_000]);
        t.clear();
        t.record_start(10_000);
        assert!(t.is_empty());
    }

    #[test]
    fn saturates_long_frames() {
        let s = filled(&[]).summary();
        assert_eq!(s, None);
        let mut t = FrameTimes::new(2);
        t.record(10_000_000_000);
        assert_eq!(t.summary().unwrap().max_ns, u32::MAX);
    }

    #[test]
    fn summary_does_not_reorder_the_ring() {
        let mut t = filled(&[3, 1, 2]);
        t.summary();
        assert_eq!(t.latest(3).collect::<Vec<_>>(), [3, 1, 2]);
    }

    #[test]
    fn rate_over_a_window() {
        let mut r = RateMeter::new(1_000_000_000);
        assert!(!r.sample(0, 0));
        assert_eq!(r.hz(), None);
        assert!(!r.sample(500, 500_000_000));
        assert!(r.sample(1_000, 1_000_000_000));
        assert_eq!(r.hz(), Some(1_000.0));
        // The next window starts where the last ended.
        assert!(r.sample(1_000 + 3_000, 2_000_000_000));
        assert_eq!(r.hz(), Some(3_000.0));
    }

    #[test]
    fn rate_uses_the_actual_elapsed_time() {
        let mut r = RateMeter::new(250_000_000);
        r.sample(10, 0);
        assert!(r.sample(510, 500_000_000));
        assert_eq!(r.hz(), Some(1_000.0));
    }

    #[test]
    fn rate_restarts_when_the_counter_goes_back() {
        let mut r = RateMeter::new(100);
        r.sample(1_000, 0);
        assert!(!r.sample(5, 200));
        assert!(r.sample(105, 300));
        assert_eq!(r.hz(), Some(1_000_000_000.0));
    }

    #[test]
    fn engine_counters_store() {
        let c = EngineCounters::default();
        c.set_update_ticks(7);
        c.set_draw_frames(9);
        assert_eq!((c.update_ticks(), c.draw_frames()), (7, 9));
    }
}
