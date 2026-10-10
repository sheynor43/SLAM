//! Stream checks shared by the real-device backend tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use slam_audio::{AudioCallback, CallbackInfo, OutputStream, SineTone};
use slam_testkit::AllocStats;

/// Callbacks skipped before checking: the first ones run while the device settles.
pub const WARMUP: u64 = 16;
/// Checked callbacks.
pub const CHECKED: u64 = 500;

#[derive(Default)]
pub struct Shared {
    pub callbacks: AtomicU64,
    /// Callback counts whose preceding interval allocated (0 = none).
    alloc_at: AtomicU64,
    wrong_thread: AtomicBool,
    frame_gap: AtomicBool,
    no_timing: AtomicBool,
}

/// Plays a tone and checks, on the audio thread, that nothing between the starts of two
/// consecutive callbacks touched the Rust allocator: this covers the callback and the
/// backend's whole per-buffer path (buffer acquisition, timing, position publish,
/// release).
pub struct Checker {
    tone: SineTone,
    shared: Arc<Shared>,
    last: Option<AllocStats>,
    thread: Option<std::thread::ThreadId>,
    next_frame: Option<u64>,
}

impl Checker {
    pub fn new() -> (Self, Arc<Shared>) {
        let shared = Arc::new(Shared::default());
        let checker = Self {
            tone: SineTone::new(440.0, 0.05),
            shared: Arc::clone(&shared),
            last: None,
            thread: None,
            next_frame: None,
        };
        (checker, shared)
    }
}

impl AudioCallback for Checker {
    fn process(&mut self, out: &mut [f32], info: &CallbackInfo) {
        let now = AllocStats::current();
        let n = self.shared.callbacks.fetch_add(1, Ordering::Relaxed) + 1;
        // `ThreadId` of a foreign (backend) thread: `thread::current()` may allocate on
        // first use, so it is taken during warm-up only.
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

/// Engine clock base of the backends (ADR-0022): `CLOCK_MONOTONIC` on Linux.
#[cfg(target_os = "linux")]
pub fn engine_clock_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Engine clock base of the backends (ADR-0022): QPC in nanoseconds on Windows.
#[cfg(windows)]
pub fn engine_clock_ns() -> u64 {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut ticks, mut freq) = (0i64, 0i64);
    // SAFETY: valid out pointers; never fails on Windows XP and later.
    unsafe {
        let _ = QueryPerformanceFrequency(&mut freq);
        let _ = QueryPerformanceCounter(&mut ticks);
    }
    (ticks as u128 * 1_000_000_000 / freq.max(1) as u128) as u64
}

/// Waits for the checked callbacks, closes the stream and asserts every check.
pub fn play_and_check(stream: OutputStream, shared: &Shared) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while shared.callbacks.load(Ordering::Relaxed) < WARMUP + CHECKED {
        assert!(Instant::now() < deadline, "callbacks stalled");
        std::thread::sleep(Duration::from_millis(10));
    }

    let rate = stream.config().sample_rate;
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
    assert_eq!(position.sample_rate, rate);
    // Same clock base as the engine: the extrapolated output time is the stream's
    // age, well under the 10 s budget and not wildly negative.
    assert!((-0.1..10.0).contains(&output_s), "output at {output_s} s");
    assert_eq!(missed, 0, "missed buffers");
    assert!(!failed, "stream failed while playing");
}
