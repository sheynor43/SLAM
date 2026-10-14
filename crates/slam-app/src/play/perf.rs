//! Temporary console report until the in-game overlay is wired into `play`: once a second
//! one line of performance figures goes to stderr. Developer output, not localised
//! (ADR-0021).
//!
//! The update, draw and audio threads only bump counters (and the draw thread keeps its
//! frame-time ring); everything else, including the printing, happens on the reporter
//! thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use slam_audio::{AudioPosition, MixerStats, MusicStats, StreamCounters};
use slam_engine::{EngineCounters, FrameTimes};
use slam_input::InputCounters;

const NS_PER_MS: f64 = 1_000_000.0;
/// Frame times are summarised and published this often, by frame start time.
const FRAME_SUMMARY_NS: u64 = 1_000_000_000;
/// How often the reporter prints.
const REPORT_PERIOD: Duration = Duration::from_secs(1);
/// How often the reporter checks whether the game closed.
const POLL: Duration = Duration::from_millis(50);

/// Frame-time percentiles of the last summary window, nanoseconds, published by the draw
/// thread and read by the reporter. Relaxed: the values may come from neighbouring
/// windows, which is fine for a console line.
#[derive(Debug, Default)]
pub struct SharedFrameTimes {
    p50_ns: AtomicU64,
    p99_ns: AtomicU64,
    max_ns: AtomicU64,
    /// Summaries published so far; 0 until the first one.
    published: AtomicU64,
}

/// The draw thread's frame-time tracking: no allocation after construction, no locks.
pub struct FramePerf {
    times: FrameTimes,
    next_summary_ns: u64,
    shared: Arc<SharedFrameTimes>,
}

impl FramePerf {
    pub fn new(shared: Arc<SharedFrameTimes>) -> Self {
        Self {
            times: FrameTimes::new(FrameTimes::DEFAULT_CAPACITY),
            next_summary_ns: 0,
            shared,
        }
    }

    /// Records the interval since the previous frame start; about once a second
    /// publishes the percentiles of the frames since the last publication.
    pub fn record(&mut self, start_ns: u64) {
        self.times.record_start(start_ns);
        if start_ns < self.next_summary_ns {
            return;
        }
        if self.next_summary_ns != 0
            && let Some(summary) = self.times.summary()
        {
            let s = &self.shared;
            s.p50_ns.store(u64::from(summary.p50_ns), Ordering::Relaxed);
            s.p99_ns.store(u64::from(summary.p99_ns), Ordering::Relaxed);
            s.max_ns.store(u64::from(summary.max_ns), Ordering::Relaxed);
            s.published.fetch_add(1, Ordering::Release);
            // The next summary covers only the next second's frames.
            self.times.clear();
        }
        self.next_summary_ns = start_ns + FRAME_SUMMARY_NS;
    }
}

/// Everything the reporter reads.
pub struct ReportSources {
    pub engine: Arc<EngineCounters>,
    pub input: Arc<InputCounters>,
    pub stream: Arc<StreamCounters>,
    pub position: Arc<AudioPosition>,
    pub mixer: Arc<MixerStats>,
    pub music: Arc<MusicStats>,
    pub frame_times: Arc<SharedFrameTimes>,
    pub buffer_frames: u32,
    pub sample_rate: u32,
}

/// Counter readings at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PerfSample {
    frames: u64,
    ticks: u64,
    polls: u64,
    queued: u64,
    callbacks: u64,
}

/// Figures that are not rates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PerfGauges {
    /// p50, p99 and max frame time in nanoseconds; `None` before the first summary.
    frame_ns: Option<(u64, u64, u64)>,
    buffer_ms: f64,
    /// Output latency of the last audio buffer, nanoseconds.
    latency_ns: Option<u64>,
    missed: u64,
    underruns: u64,
    voices: u32,
}

/// One report line from the counter change `now - before` over `elapsed_ns`. Rates are
/// per second; with no time elapsed they are zero.
fn format_report(
    before: &PerfSample,
    now: &PerfSample,
    elapsed_ns: u64,
    gauges: &PerfGauges,
) -> String {
    let rate = |now: u64, before: u64| {
        if elapsed_ns == 0 {
            0.0
        } else {
            now.saturating_sub(before) as f64 * 1e9 / elapsed_ns as f64
        }
    };
    let ms = |ns: u64| ns as f64 / NS_PER_MS;
    let frame = match gauges.frame_ns {
        Some((p50, p99, max)) => {
            format!("p50 {:.2} p99 {:.2} max {:.2}", ms(p50), ms(p99), ms(max))
        }
        None => "p50 - p99 - max -".to_owned(),
    };
    let latency = gauges
        .latency_ns
        .map_or_else(|| "-".to_owned(), |ns| format!("{:.1}", ms(ns)));
    format!(
        "fps {:.0} | frame ms {frame} | update {:.0} Hz | input {:.0} ev/s {:.0} polls/s | \
         audio {:.0} cb/s, buf {:.1} ms, latency {latency} ms, missed {}, underruns {}, \
         voices {}",
        rate(now.frames, before.frames),
        rate(now.ticks, before.ticks),
        rate(now.queued, before.queued),
        rate(now.polls, before.polls),
        rate(now.callbacks, before.callbacks),
        gauges.buffer_ms,
        gauges.missed,
        gauges.underruns,
        gauges.voices,
    )
}

/// Prints the performance line to stderr once a second until `closed`. Runs on its own
/// thread.
pub fn report(sources: &ReportSources, closed: &AtomicBool) {
    let read = || PerfSample {
        frames: sources.engine.draw_frames(),
        ticks: sources.engine.update_ticks(),
        polls: sources.input.polls(),
        queued: sources.input.queued(),
        callbacks: sources.stream.callbacks.load(Ordering::Relaxed),
    };
    let buffer_ms = f64::from(sources.buffer_frames) * 1000.0 / f64::from(sources.sample_rate);
    let mut before = read();
    let mut last = Instant::now();
    while !closed.load(Ordering::Relaxed) {
        std::thread::sleep(POLL);
        let elapsed = last.elapsed();
        if elapsed < REPORT_PERIOD {
            continue;
        }
        let now = read();
        let shared = &sources.frame_times;
        let gauges = PerfGauges {
            frame_ns: (shared.published.load(Ordering::Acquire) > 0).then(|| {
                (
                    shared.p50_ns.load(Ordering::Relaxed),
                    shared.p99_ns.load(Ordering::Relaxed),
                    shared.max_ns.load(Ordering::Relaxed),
                )
            }),
            buffer_ms,
            latency_ns: sources.position.read().map(|p| p.latency_ns),
            missed: sources.stream.missed_buffers.load(Ordering::Relaxed),
            underruns: sources.music.underruns.load(Ordering::Relaxed),
            voices: sources.mixer.active_voices.load(Ordering::Relaxed),
        };
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        eprintln!("{}", format_report(&before, &now, nanos, &gauges));
        before = now;
        last = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_rates_come_from_deltas_over_the_elapsed_time() {
        let before = PerfSample {
            frames: 1000,
            ticks: 2000,
            polls: 10,
            queued: 5,
            callbacks: 100,
        };
        let now = PerfSample {
            frames: 1500,
            ticks: 3000,
            polls: 160,
            queued: 80,
            callbacks: 287,
        };
        let gauges = PerfGauges {
            frame_ns: Some((1_000_000, 1_210_000, 2_400_000)),
            buffer_ms: 2.7,
            latency_ns: Some(8_100_000),
            missed: 0,
            underruns: 1,
            voices: 3,
        };
        // Half a second: rates are twice the deltas.
        let line = format_report(&before, &now, 500_000_000, &gauges);
        assert_eq!(
            line,
            "fps 1000 | frame ms p50 1.00 p99 1.21 max 2.40 | update 2000 Hz | \
             input 150 ev/s 300 polls/s | audio 374 cb/s, buf 2.7 ms, latency 8.1 ms, \
             missed 0, underruns 1, voices 3"
        );
    }

    #[test]
    fn report_without_data_or_time_is_safe() {
        let zero = PerfSample::default();
        let line = format_report(&zero, &zero, 0, &PerfGauges::default());
        assert!(line.starts_with("fps 0 | frame ms p50 - p99 - max - | update 0 Hz"));
        assert!(line.contains("latency - ms"));
        // A counter that went backwards does not wrap.
        let now = PerfSample { frames: 5, ..zero };
        let before = PerfSample { frames: 9, ..zero };
        let line = format_report(&before, &now, 1_000_000_000, &PerfGauges::default());
        assert!(line.starts_with("fps 0 "));
    }

    #[test]
    fn frame_times_are_published_once_a_second() {
        let shared = Arc::new(SharedFrameTimes::default());
        let mut perf = FramePerf::new(Arc::clone(&shared));
        // 1 ms frames with one 5 ms hiccup, for 2.5 s.
        let mut t = 1_000_000_000u64;
        let mut frames = 0;
        while t < 3_500_000_000 {
            perf.record(t);
            frames += 1;
            t += if frames == 1500 { 5_000_000 } else { 1_000_000 };
        }
        assert_eq!(shared.published.load(Ordering::Relaxed), 2);
        assert_eq!(shared.p50_ns.load(Ordering::Relaxed), 1_000_000);
        assert_eq!(shared.max_ns.load(Ordering::Relaxed), 5_000_000);
    }
}
