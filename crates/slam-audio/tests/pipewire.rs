//! Real PipeWire output stream. Needs a running PipeWire server and plays a quiet tone
//! for about a second, so it is ignored by default:
//! `cargo test -p slam-audio --test pipewire -- --ignored`.
#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use slam_audio::{AudioCallback, Backend, CallbackInfo, SineTone, StreamConfig};
use slam_testkit::AllocStats;

slam_testkit::install_counting_allocator!();

/// Callbacks skipped before checking: the first ones run while the graph settles.
const WARMUP: u64 = 16;
/// Checked callbacks.
const CHECKED: u64 = 500;

#[derive(Default)]
struct Shared {
    callbacks: AtomicU64,
    /// Callback counts whose preceding interval allocated (0 = none).
    alloc_at: AtomicU64,
    wrong_thread: AtomicBool,
    frame_gap: AtomicBool,
    no_timing: AtomicBool,
}

/// Plays a tone and checks, on the audio thread, that nothing between the starts of two
/// consecutive callbacks touched the Rust allocator: this covers the callback and the
/// backend's whole process path (dequeue, buffer mapping, timing, position publish,
/// queue).
struct Checker {
    tone: SineTone,
    shared: Arc<Shared>,
    last: Option<AllocStats>,
    thread: Option<std::thread::ThreadId>,
    next_frame: Option<u64>,
}

impl AudioCallback for Checker {
    fn process(&mut self, out: &mut [f32], info: &CallbackInfo) {
        let now = AllocStats::current();
        let n = self.shared.callbacks.fetch_add(1, Ordering::Relaxed) + 1;
        // `ThreadId` of a foreign (PipeWire) thread: `thread::current()` may allocate
        // on first use, so it is taken during warm-up only.
        if n <= WARMUP {
            let id = std::thread::current().id();
            if *self.thread.get_or_insert(id) != id {
                self.shared.wrong_thread.store(true, Ordering::Relaxed);
            }
        } else if n > WARMUP + 1
            && let Some(last) = self.last
            && now.total() != last.total()
        {
            let _ =
                self.shared
                    .alloc_at
                    .compare_exchange(0, n, Ordering::Relaxed, Ordering::Relaxed);
        }
        if self.next_frame.is_some_and(|f| f != info.frame) {
            self.shared.frame_gap.store(true, Ordering::Relaxed);
        }
        // Null or dummy sinks may report zero latency; only the timestamp must exist.
        if n > WARMUP && info.timestamp_ns == 0 {
            self.shared.no_timing.store(true, Ordering::Relaxed);
        }
        self.next_frame = Some(info.frame + (out.len() / usize::from(info.channels)) as u64);
        self.tone.process(out, info);
        self.last = Some(now);
    }
}

fn engine_clock_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[test]
#[ignore = "needs a running PipeWire server"]
fn pipewire_stream_plays_without_allocating() {
    let devices = slam_audio::devices(Backend::PipeWire).expect("listing devices");
    assert!(!devices.is_empty(), "no Audio/Sink nodes");

    let shared = Arc::new(Shared::default());
    let checker = Checker {
        tone: SineTone::new(440.0, 0.05),
        shared: Arc::clone(&shared),
        last: None,
        thread: None,
        next_frame: None,
    };
    let stream = slam_audio::open_output(
        Backend::PipeWire,
        StreamConfig {
            name: "SLAM test".to_owned(),
            ..StreamConfig::default()
        },
        Box::new(checker),
    )
    .expect("opening the stream");

    let deadline = Instant::now() + Duration::from_secs(10);
    while shared.callbacks.load(Ordering::Relaxed) < WARMUP + CHECKED {
        assert!(Instant::now() < deadline, "callbacks stalled");
        std::thread::sleep(Duration::from_millis(10));
    }

    let position = stream.position().read().expect("position published");
    let output_s = position.output_frame_at(engine_clock_ns()) / f64::from(position.sample_rate);
    let counters = stream.counters();
    let missed = counters.missed_buffers.load(Ordering::Relaxed);
    let failed = counters.failed.load(Ordering::Relaxed);
    drop(stream);

    assert_eq!(
        shared.alloc_at.load(Ordering::Relaxed),
        0,
        "process path allocated before that callback"
    );
    assert!(
        !shared.wrong_thread.load(Ordering::Relaxed),
        "callback moved threads"
    );
    assert!(
        !shared.frame_gap.load(Ordering::Relaxed),
        "frame index skipped"
    );
    assert!(
        !shared.no_timing.load(Ordering::Relaxed),
        "missing timing report"
    );
    assert_eq!(position.sample_rate, 48_000);
    // Same clock base as the engine: the extrapolated output time is the stream's
    // age, well under the 10 s budget and not wildly negative.
    assert!((-0.1..10.0).contains(&output_s), "output at {output_s} s");
    assert_eq!(missed, 0, "missed buffers");
    assert!(!failed, "stream failed while playing");
}
