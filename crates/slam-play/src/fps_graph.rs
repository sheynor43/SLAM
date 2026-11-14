//! A ring of recent frame times for the FPS counter's graphs (ADR-0138): fixed-size, so
//! recording never allocates.

/// Bars of a graph.
pub const GRAPH_LEN: usize = 120;

/// Milliseconds one bar spans; the graph covers `GRAPH_LEN * BUCKET_MS` (3 s). A bar shows
/// the longest frame in its span, so that a stall stays visible at any frame rate.
pub const BUCKET_MS: f64 = 25.0;

/// The longest frame time of each of the last [`GRAPH_LEN`] spans of [`BUCKET_MS`].
#[derive(Debug, Clone)]
pub struct FrameGraph {
    /// Finished spans, a ring; `head` is the oldest.
    bars: [f32; GRAPH_LEN],
    head: usize,
    /// End of the span being filled, in the caller's milliseconds; `None` before the first
    /// sample.
    bucket_end: Option<f64>,
    /// The longest sample of the span being filled.
    current: f32,
}

impl Default for FrameGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameGraph {
    pub fn new() -> FrameGraph {
        FrameGraph {
            bars: [0.0; GRAPH_LEN],
            head: 0,
            bucket_end: None,
            current: 0.0,
        }
    }

    /// Records a frame of `ms` milliseconds seen at time `time_ms`. Time only moves
    /// forward; an earlier one counts as the current span's.
    pub fn record(&mut self, time_ms: f64, ms: f32) {
        let end = *self.bucket_end.get_or_insert(time_ms + BUCKET_MS);
        if time_ms >= end {
            // The finished span, then empty ones for any gap; a gap longer than the
            // graph clears it.
            let spans = (((time_ms - end) / BUCKET_MS) as usize).saturating_add(1);
            self.push_bar(self.current);
            for _ in 1..spans.min(GRAPH_LEN) {
                self.push_bar(0.0);
            }
            self.current = 0.0;
            self.bucket_end = Some(end + spans as f64 * BUCKET_MS);
        }
        if ms.is_finite() && ms > self.current {
            self.current = ms;
        }
    }

    fn push_bar(&mut self, value: f32) {
        self.bars[self.head] = value;
        self.head = (self.head + 1) % GRAPH_LEN;
    }

    /// Bar `i` from the oldest (0) to the span being filled (`GRAPH_LEN - 1`), in
    /// milliseconds; 0 where nothing was recorded.
    pub fn bar(&self, i: usize) -> f32 {
        match i {
            i if i + 1 == GRAPH_LEN => self.current,
            i if i + 1 < GRAPH_LEN => self.bars[(self.head + i + 1) % GRAPH_LEN],
            _ => 0.0,
        }
    }

    /// The longest bar, in milliseconds.
    pub fn max(&self) -> f32 {
        (0..GRAPH_LEN).map(|i| self.bar(i)).fold(0.0, f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_holds_the_longest_frame_of_its_span() {
        let mut g = FrameGraph::new();
        g.record(0.0, 1.0);
        g.record(5.0, 9.0);
        g.record(10.0, 2.0);
        // Still the span being filled: the last bar.
        assert_eq!(g.bar(GRAPH_LEN - 1), 9.0);
        g.record(BUCKET_MS + 1.0, 3.0);
        assert_eq!(g.bar(GRAPH_LEN - 2), 9.0);
        assert_eq!(g.bar(GRAPH_LEN - 1), 3.0);
        assert_eq!(g.max(), 9.0);
    }

    #[test]
    fn old_bars_fall_off_after_the_graph_length() {
        let mut g = FrameGraph::new();
        g.record(0.0, 50.0);
        for i in 1..=GRAPH_LEN + 1 {
            g.record(i as f64 * BUCKET_MS, 1.0);
        }
        assert_eq!(g.max(), 1.0);
    }

    #[test]
    fn a_gap_leaves_empty_bars_and_a_long_one_clears_the_graph() {
        let mut g = FrameGraph::new();
        g.record(0.0, 7.0);
        g.record(3.5 * BUCKET_MS, 1.0);
        // 7 ms, two empty spans, then the current one.
        assert_eq!(g.bar(GRAPH_LEN - 4), 7.0);
        assert_eq!(g.bar(GRAPH_LEN - 3), 0.0);
        assert_eq!(g.bar(GRAPH_LEN - 2), 0.0);
        assert_eq!(g.bar(GRAPH_LEN - 1), 1.0);
        g.record(1e9, 2.0);
        assert_eq!(g.max(), 2.0);
    }

    #[test]
    fn bad_samples_and_out_of_range_bars_are_harmless() {
        let mut g = FrameGraph::new();
        g.record(0.0, f32::NAN);
        g.record(1.0, f32::INFINITY);
        assert_eq!(g.max(), 0.0);
        assert_eq!(g.bar(GRAPH_LEN), 0.0);
        // Time going backwards stays in the current span.
        g.record(-100.0, 4.0);
        assert_eq!(g.bar(GRAPH_LEN - 1), 4.0);
    }
}
