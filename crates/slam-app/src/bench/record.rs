//! Per-frame measurements of the benchmark scene and the report built from them.

use std::fmt;

use slam_engine::limiter::FrameWait;
use slam_render::sprite::SpriteStats;

/// Upper bound on the frame rate the recorder sizes its buffers for. Frames past
/// the buffers are counted but not recorded, and the report says so.
pub const MAX_RECORDED_FPS: u64 = 50_000;

/// When one frame's work happened on the draw thread, in engine time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameWork {
    /// The batch was built and drawn; `present` is about to be called.
    pub drawn_ns: u64,
    /// `present` returned.
    pub presented_ns: u64,
}

/// Collects frame intervals, limiter lateness and batcher counts for frames that
/// start inside `[start_ns, end_ns)`. Buffers are allocated up front: [`record`]
/// does not allocate.
///
/// [`record`]: Recorder::record
pub struct Recorder {
    start_ns: u64,
    end_ns: u64,
    /// Start of the previous frame, measured or not.
    last_ns: Option<u64>,
    /// End of the previous frame's work.
    last_end_ns: u64,
    first_measured_ns: Option<u64>,
    last_measured_ns: u64,
    frames: u64,
    unrecorded: u64,
    /// Frame starts of the recorded frames, parallel to `draw` and `present`.
    starts: Vec<u64>,
    intervals: Vec<u32>,
    draw: Vec<u32>,
    present: Vec<u32>,
    /// Only for frames whose predecessor finished before their deadline: lateness
    /// after an overrun is the overrun, not the limiter.
    lateness: Vec<u32>,
    overruns: u64,
    resyncs: u64,
    errors: u64,
    sprites: u64,
    batches: u64,
    draw_calls: u64,
    max_draw_calls: u32,
}

impl Recorder {
    /// A recorder for the window `[start_ns, end_ns)` of engine time, with room for
    /// `capacity` frames.
    pub fn new(start_ns: u64, end_ns: u64, capacity: usize) -> Self {
        Self {
            start_ns,
            end_ns,
            last_ns: None,
            last_end_ns: 0,
            first_measured_ns: None,
            last_measured_ns: 0,
            frames: 0,
            unrecorded: 0,
            starts: Vec::with_capacity(capacity),
            intervals: Vec::with_capacity(capacity),
            draw: Vec::with_capacity(capacity),
            present: Vec::with_capacity(capacity),
            lateness: Vec::with_capacity(capacity),
            overruns: 0,
            resyncs: 0,
            errors: 0,
            sprites: 0,
            batches: 0,
            draw_calls: 0,
            max_draw_calls: 0,
        }
    }

    /// Room for `seconds` at [`MAX_RECORDED_FPS`].
    pub fn capacity_for(seconds: f64) -> usize {
        (seconds.max(0.0) * MAX_RECORDED_FPS as f64).ceil() as usize
    }

    /// Records one frame: when the limiter released it, when its work finished, and
    /// what the batcher drew (`None` if drawing or presenting failed). The frame
    /// time of a frame is the interval from the previous frame's start to its own.
    pub fn record(&mut self, wait: &FrameWait, work: FrameWork, stats: Option<SpriteStats>) {
        let start = wait.woke_ns;
        let previous = self.last_ns.replace(start);
        let previous_end = std::mem::replace(&mut self.last_end_ns, work.presented_ns);
        if start < self.start_ns || start >= self.end_ns {
            return;
        }
        self.first_measured_ns.get_or_insert(start);
        self.last_measured_ns = start;
        self.frames += 1;
        if self.draw.len() == self.draw.capacity() {
            self.unrecorded += 1;
        } else {
            if let Some(previous) = previous {
                self.intervals
                    .push(saturate(start.saturating_sub(previous)));
            }
            self.starts.push(start);
            self.draw
                .push(saturate(work.drawn_ns.saturating_sub(start)));
            self.present
                .push(saturate(work.presented_ns.saturating_sub(work.drawn_ns)));
            if previous_end <= wait.deadline_ns {
                self.lateness.push(saturate(wait.lateness_ns()));
            }
        }
        self.overruns += u64::from(previous_end > wait.deadline_ns);
        self.resyncs += u64::from(wait.resynced);
        match stats {
            Some(stats) => {
                self.sprites += u64::from(stats.sprites);
                self.batches += u64::from(stats.batches);
                self.draw_calls += u64::from(stats.draw_calls);
                self.max_draw_calls = self.max_draw_calls.max(stats.draw_calls);
            }
            None => self.errors += 1,
        }
    }

    /// Writes the recorded frames as CSV: start relative to the first recorded frame,
    /// draw and present time, all in nanoseconds. Call before [`Recorder::report`],
    /// which reorders the samples.
    pub fn write_csv<W: std::io::Write>(&self, mut out: W) -> std::io::Result<()> {
        writeln!(out, "start_ns,draw_ns,present_ns")?;
        let first = self.starts.first().copied().unwrap_or(0);
        for ((start, draw), present) in self.starts.iter().zip(&self.draw).zip(&self.present) {
            writeln!(out, "{},{draw},{present}", start - first)?;
        }
        out.flush()
    }

    /// Frames whose work (draw and present) took longer than `threshold_ns`, and the
    /// gaps between the starts of consecutive such frames.
    fn stalls(&self, threshold_ns: u32) -> Stalls {
        let mut count = 0;
        let mut in_draw = 0;
        let mut gaps = Vec::new();
        let mut last = None;
        for ((&start, &draw), &present) in self.starts.iter().zip(&self.draw).zip(&self.present) {
            if draw.saturating_add(present) <= threshold_ns {
                continue;
            }
            count += 1;
            in_draw += u64::from(draw > present);
            if let Some(last) = last.replace(start) {
                gaps.push(saturate(start - last));
            }
        }
        let span_ns = match (self.starts.first(), self.starts.last()) {
            (Some(first), Some(last)) => last - first,
            _ => 0,
        };
        Stalls {
            threshold_ns,
            count,
            in_draw,
            per_second: if span_ns == 0 {
                0.0
            } else {
                count as f64 * 1e9 / span_ns as f64
            },
            gap: Percentiles::of(&mut gaps),
        }
    }

    /// Summarises the measured frames; a frame whose work takes longer than
    /// `stall_ns` counts as a stall. Sorts the recorded samples in place.
    pub fn report(&mut self, stall_ns: u32) -> Report {
        let stalls = self.stalls(stall_ns);
        let span_ns = self
            .first_measured_ns
            .map_or(0, |first| self.last_measured_ns - first);
        let per_frame = |sum: u64| {
            if self.frames == 0 {
                0.0
            } else {
                sum as f64 / self.frames as f64
            }
        };
        Report {
            frames: self.frames,
            unrecorded: self.unrecorded,
            // Frames between the first and the last measured start.
            fps: if span_ns == 0 {
                0.0
            } else {
                (self.frames - 1) as f64 * 1e9 / span_ns as f64
            },
            frame_time: Percentiles::of(&mut self.intervals),
            draw: Percentiles::of(&mut self.draw),
            present: Percentiles::of(&mut self.present),
            lateness: Percentiles::of(&mut self.lateness),
            overruns: self.overruns,
            resyncs: self.resyncs,
            errors: self.errors,
            sprites: per_frame(self.sprites),
            batches: per_frame(self.batches),
            draw_calls: per_frame(self.draw_calls),
            max_draw_calls: self.max_draw_calls,
            stalls,
        }
    }
}

fn saturate(ns: u64) -> u32 {
    u32::try_from(ns).unwrap_or(u32::MAX)
}

/// Nearest-rank percentiles of a sample set, in nanoseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Percentiles {
    pub samples: usize,
    pub p10_ns: u32,
    pub p50_ns: u32,
    pub p90_ns: u32,
    pub p99_ns: u32,
    pub p999_ns: u32,
    pub max_ns: u32,
}

impl Percentiles {
    /// Sorts `samples` in place. All zero for an empty set.
    pub fn of(samples: &mut [u32]) -> Self {
        samples.sort_unstable();
        let at = |per_mille: usize| {
            let rank = (samples.len() * per_mille).div_ceil(1000).max(1) - 1;
            samples[rank]
        };
        if samples.is_empty() {
            return Self::default();
        }
        Self {
            samples: samples.len(),
            p10_ns: at(100),
            p50_ns: at(500),
            p90_ns: at(900),
            p99_ns: at(990),
            p999_ns: at(999),
            max_ns: samples[samples.len() - 1],
        }
    }
}

impl fmt::Display for Percentiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let us = |ns: u32| f64::from(ns) / 1000.0;
        write!(
            f,
            "p50 {:.1}, p99 {:.1}, p99.9 {:.1}, max {:.1}",
            us(self.p50_ns),
            us(self.p99_ns),
            us(self.p999_ns),
            us(self.max_ns)
        )
    }
}

/// Frames whose work (draw and present) exceeded a threshold.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stalls {
    pub threshold_ns: u32,
    pub count: u64,
    /// Stalls that spent more time drawing than presenting: on GL the driver may
    /// block in a draw call instead of the swap.
    pub in_draw: u64,
    /// Over the recorded frames.
    pub per_second: f64,
    /// Between the starts of consecutive stalls.
    pub gap: Percentiles,
}

/// Benchmark results for frames inside the measurement window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub frames: u64,
    /// Measured frames that did not fit the sample buffers: counted in `frames` and
    /// `fps`, missing from the percentiles.
    pub unrecorded: u64,
    pub fps: f64,
    pub frame_time: Percentiles,
    /// From the frame's start to the call to `present`: building and drawing the
    /// batch.
    pub draw: Percentiles,
    /// Time inside `present`.
    pub present: Percentiles,
    /// How late the limiter released frames whose predecessor finished in time;
    /// zero without a frame-rate limit.
    pub lateness: Percentiles,
    /// Frames whose predecessor's work ran past their deadline (always zero
    /// without a limit, where the deadline is the start).
    pub overruns: u64,
    pub resyncs: u64,
    /// Frames whose draw or present failed.
    pub errors: u64,
    /// Means per measured frame; frames that failed to draw count as zero.
    pub sprites: f64,
    pub batches: f64,
    pub draw_calls: f64,
    pub max_draw_calls: u32,
    pub stalls: Stalls,
}

impl fmt::Display for Report {
    // Developer tool output, not UI: not localised.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "frames: {}, mean FPS: {:.0}", self.frames, self.fps)?;
        writeln!(f, "frame time, us: {}", self.frame_time)?;
        writeln!(f, "  draw, us:     {}", self.draw)?;
        writeln!(f, "  present, us:  {}", self.present)?;
        writeln!(
            f,
            "limiter lateness, us: {}; frames after an overrun {}, resyncs {}",
            self.lateness, self.overruns, self.resyncs
        )?;
        writeln!(
            f,
            "per frame: {:.1} sprites, {:.2} batches, {:.2} draw calls (max {})",
            self.sprites, self.batches, self.draw_calls, self.max_draw_calls
        )?;
        let s = &self.stalls;
        let ms = |ns: u32| f64::from(ns) / 1e6;
        writeln!(
            f,
            "stalls (work > {:.0} us): {} ({:.1}/s, {} mostly in draw); gap between stalls, ms: \
             p10 {:.2}, p50 {:.2}, p90 {:.2}",
            f64::from(s.threshold_ns) / 1000.0,
            s.count,
            s.per_second,
            s.in_draw,
            ms(s.gap.p10_ns),
            ms(s.gap.p50_ns),
            ms(s.gap.p90_ns),
        )?;
        if self.errors > 0 {
            writeln!(f, "frames that failed to draw or present: {}", self.errors)?;
        }
        if self.unrecorded > 0 {
            writeln!(
                f,
                "warning: {} frames over {MAX_RECORDED_FPS} FPS were not recorded",
                self.unrecorded
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(woke_ns: u64, deadline_ns: u64) -> FrameWait {
        FrameWait {
            deadline_ns,
            woke_ns,
            resynced: false,
        }
    }

    /// Draws for 10 ns, presents for 5 ns.
    fn work(woke_ns: u64) -> FrameWork {
        FrameWork {
            drawn_ns: woke_ns + 10,
            presented_ns: woke_ns + 15,
        }
    }

    fn record(r: &mut Recorder, w: FrameWait, stats: Option<SpriteStats>) {
        r.record(&w, work(w.woke_ns), stats);
    }

    const STATS: SpriteStats = SpriteStats {
        sprites: 500,
        batches: 1,
        draw_calls: 1,
    };

    const STALL_NS: u32 = 1_000;

    #[test]
    fn stalls_are_counted_with_gaps() {
        let mut r = Recorder::new(0, u64::MAX, 16);
        for t in (0..10).map(|i| i * 1_000_000) {
            let slow = t == 2_000_000 || t == 5_000_000 || t == 9_000_000;
            let (draw, present) = match t {
                9_000_000 => (2_000, 0),
                _ if slow => (10, 2_000),
                _ => (10, 5),
            };
            r.record(
                &wait(t, t),
                FrameWork {
                    drawn_ns: t + draw,
                    presented_ns: t + draw + present,
                },
                Some(STATS),
            );
        }
        let mut csv = Vec::new();
        r.write_csv(&mut csv).unwrap();
        let csv = String::from_utf8(csv).unwrap();
        assert_eq!(csv.lines().count(), 11);
        assert_eq!(csv.lines().nth(3), Some("2000000,10,2000"));
        let s = r.report(STALL_NS).stalls;
        assert_eq!(s.count, 3);
        assert_eq!(s.in_draw, 1);
        // 3 stalls over 9 ms of frame starts.
        assert!((s.per_second - 3e9 / 9e6).abs() < 1e-6, "{}", s.per_second);
        assert_eq!(s.gap.samples, 2);
        assert_eq!(s.gap.p10_ns, 3_000_000);
        assert_eq!(s.gap.max_ns, 4_000_000);
    }

    #[test]
    fn stalls_of_empty_and_threshold_frames() {
        let mut r = Recorder::new(0, u64::MAX, 4);
        let mut csv = Vec::new();
        r.write_csv(&mut csv).unwrap();
        assert_eq!(csv, b"start_ns,draw_ns,present_ns\n");
        assert_eq!(r.report(STALL_NS).stalls.count, 0);

        let mut r = Recorder::new(0, u64::MAX, 4);
        let at = |t, work| FrameWork {
            drawn_ns: t,
            presented_ns: t + work,
        };
        // Exactly at the threshold is not a stall; one stall has no gap.
        r.record(&wait(0, 0), at(0, u64::from(STALL_NS)), Some(STATS));
        r.record(&wait(10_000, 10_000), at(10_000, 5_000), Some(STATS));
        let s = r.report(STALL_NS).stalls;
        assert_eq!(s.count, 1);
        assert_eq!(s.gap.samples, 0);
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let mut samples: Vec<u32> = (1..=1000).rev().collect();
        let p = Percentiles::of(&mut samples);
        assert_eq!(
            p,
            Percentiles {
                samples: 1000,
                p10_ns: 100,
                p50_ns: 500,
                p90_ns: 900,
                p99_ns: 990,
                p999_ns: 999,
                max_ns: 1000,
            }
        );
        assert_eq!(Percentiles::of(&mut [7]).p50_ns, 7);
        assert_eq!(Percentiles::of(&mut []), Percentiles::default());
    }

    #[test]
    fn only_frames_inside_the_window_count() {
        let mut r = Recorder::new(1_000, 2_000, 16);
        // Warm-up frame: not measured, but the next frame's interval starts there.
        record(&mut r, wait(900, 900), Some(STATS));
        for t in [1_000, 1_250, 1_500, 1_750] {
            record(&mut r, wait(t, t), Some(STATS));
        }
        record(&mut r, wait(2_000, 2_000), Some(STATS)); // past the end
        let report = r.report(STALL_NS);
        assert_eq!(report.frames, 4);
        assert_eq!(report.frame_time.samples, 4);
        assert_eq!(report.frame_time.p50_ns, 250);
        assert_eq!(report.frame_time.max_ns, 250);
        assert_eq!(report.draw.max_ns, 10);
        assert_eq!(report.present.max_ns, 5);
        // 3 intervals over 750 ns.
        assert!((report.fps - 4e6).abs() < 1e-6, "{}", report.fps);
        assert_eq!(report.sprites, 500.0);
        assert_eq!(report.draw_calls, 1.0);
        assert_eq!(report.unrecorded, 0);
        assert_eq!(report.overruns, 0);
    }

    #[test]
    fn first_frame_without_predecessor_has_no_interval() {
        let mut r = Recorder::new(0, 100, 4);
        record(&mut r, wait(10, 10), Some(STATS));
        let report = r.report(STALL_NS);
        assert_eq!(report.frames, 1);
        assert_eq!(report.frame_time.samples, 0);
        assert_eq!(report.draw.samples, 1);
        assert_eq!(report.fps, 0.0);
    }

    #[test]
    fn lateness_resyncs_and_failures_are_recorded() {
        let mut r = Recorder::new(0, 1_000_000, 8);
        record(&mut r, wait(100, 90), Some(STATS));
        record(&mut r, wait(200, 200), None);
        let mut late = wait(330, 300);
        late.resynced = true;
        record(&mut r, late, Some(STATS));
        let report = r.report(STALL_NS);
        assert_eq!(report.lateness.samples, 3);
        assert_eq!(report.lateness.max_ns, 30);
        assert_eq!(report.lateness.p50_ns, 10);
        assert_eq!(report.resyncs, 1);
        assert_eq!(report.errors, 1);
        // Means over all measured frames; the failed frame drew nothing.
        assert!((report.sprites - 1000.0 / 3.0).abs() < 1e-9);
        assert_eq!(report.max_draw_calls, 1);
    }

    #[test]
    fn lateness_after_an_overrun_is_not_the_limiter() {
        let mut r = Recorder::new(0, 1_000_000, 8);
        r.record(
            &wait(100, 100),
            FrameWork {
                drawn_ns: 150,
                presented_ns: 400, // past the next deadline
            },
            Some(STATS),
        );
        record(&mut r, wait(400, 200), Some(STATS));
        record(&mut r, wait(502, 500), Some(STATS));
        let report = r.report(STALL_NS);
        assert_eq!(report.overruns, 1);
        assert_eq!(report.lateness.samples, 2);
        assert_eq!(report.lateness.max_ns, 2);
        assert_eq!(report.present.max_ns, 250);
    }

    #[test]
    fn frames_past_capacity_are_counted_not_recorded() {
        let mut r = Recorder::new(0, u64::MAX, 2);
        for t in [10, 20, 30, 40] {
            record(&mut r, wait(t, t), Some(STATS));
        }
        let report = r.report(STALL_NS);
        assert_eq!(report.frames, 4);
        assert_eq!(report.unrecorded, 2);
        // 15 ns of work every 10 ns: each frame after the first follows an
        // overrun, recorded or not.
        assert_eq!(report.overruns, 3);
        assert_eq!(report.draw.samples, 2);
        assert_eq!(report.frame_time.samples, 1);
        assert!(report.to_string().contains("were not recorded"));
    }

    #[test]
    fn long_gaps_saturate() {
        let mut r = Recorder::new(0, u64::MAX, 2);
        record(&mut r, wait(0, 0), Some(STATS));
        record(&mut r, wait(10_000_000_000, 10_000_000_000), Some(STATS));
        assert_eq!(r.report(STALL_NS).frame_time.max_ns, u32::MAX);
    }

    #[test]
    fn capacity_scales_with_duration() {
        assert_eq!(Recorder::capacity_for(10.0), 500_000);
        assert_eq!(Recorder::capacity_for(-1.0), 0);
    }
}
