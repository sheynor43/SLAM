//! Engine clock: monotonic nanoseconds shared by every thread, the frame limiter, the
//! update loop and input timestamps. `CLOCK_MONOTONIC` on Linux,
//! `QueryPerformanceCounter` on Windows, a process-wide `Instant` origin elsewhere.

/// Current engine time in nanoseconds. Allocation-free, lock-free.
#[cfg(target_os = "linux")]
pub fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    debug_assert_eq!(rc, 0);
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Current engine time in nanoseconds. Allocation-free, lock-free.
#[cfg(windows)]
pub fn now_ns() -> u64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    use windows_sys::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };

    // The frequency is fixed at boot; zero means "not read yet".
    static FREQUENCY: AtomicI64 = AtomicI64::new(0);
    let mut freq = FREQUENCY.load(Ordering::Relaxed);
    if freq == 0 {
        // SAFETY: `freq` is a valid out pointer. Never fails on Windows XP and later.
        unsafe { QueryPerformanceFrequency(&mut freq) };
        FREQUENCY.store(freq, Ordering::Relaxed);
    }
    let mut ticks = 0i64;
    // SAFETY: `ticks` is a valid out pointer. Never fails on Windows XP and later.
    unsafe { QueryPerformanceCounter(&mut ticks) };
    (ticks as u128 * 1_000_000_000 / freq as u128) as u64
}

/// Current engine time in nanoseconds. Allocation-free after the first call.
#[cfg(not(any(target_os = "linux", windows)))]
pub fn now_ns() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64
}
