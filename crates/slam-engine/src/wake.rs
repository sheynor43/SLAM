//! Wake-up for the update thread: sleep until a deadline unless input arrives first.
//!
//! [`Waker::notify`] is called by the input thread after every queued event; it costs
//! one atomic swap, plus a system call only while the update thread is actually
//! asleep. [`Parker::wait_until`] sleeps until shortly before the deadline (futex
//! with an absolute `CLOCK_MONOTONIC` timeout on Linux; an event and a
//! high-resolution waitable timer on Windows; `thread::park_timeout` elsewhere), then
//! busy-waits the rest, returning as soon as a notification is seen.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::clock;

/// No pending notification, the parker is not asleep.
const EMPTY: u32 = 0;
/// A notification is pending.
const NOTIFIED: u32 = 1;
/// The parker is asleep (or about to be) and must be woken by a system call.
const PARKED: u32 = 2;

pub(crate) struct Inner {
    state: AtomicU32,
    sys: imp::Shared,
}

impl Inner {
    fn notify(&self) {
        if self.state.swap(NOTIFIED, Ordering::Release) == PARKED {
            self.sys.wake(&self.state);
        }
    }

    /// Consumes a pending notification.
    fn take(&self) -> bool {
        self.state
            .compare_exchange(NOTIFIED, EMPTY, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
}

impl slam_input::Notify for Inner {
    fn notify(&self) {
        Inner::notify(self);
    }
}

/// Creates a connected waker and parker.
pub fn wake_pair() -> (Waker, Parker) {
    let inner = Arc::new(Inner {
        state: AtomicU32::new(EMPTY),
        sys: imp::Shared::new(),
    });
    (
        Waker {
            inner: Arc::clone(&inner),
        },
        Parker {
            inner,
            sys: imp::Local::new(),
        },
    )
}

/// Notifying end; may be cloned and used from any thread.
#[derive(Clone)]
pub struct Waker {
    inner: Arc<Inner>,
}

impl Waker {
    /// Wakes the parker, or makes its next wait return immediately. Lock-free,
    /// allocation-free. Notifications are not counted: several before one wait
    /// count as one.
    pub fn notify(&self) {
        self.inner.notify();
    }

    /// The waker as an input ring notifier for [`slam_input::EventSink::set_notify`].
    pub fn as_notify(&self) -> Arc<dyn slam_input::Notify> {
        self.inner.clone()
    }
}

/// Waiting end, owned by one thread.
pub struct Parker {
    inner: Arc<Inner>,
    sys: imp::Local,
}

impl Parker {
    /// Blocks until notified or until the engine clock reaches `deadline_ns`.
    /// Sleeps until `spin_ns` before the deadline, then busy-waits. Returns `true` if
    /// a notification ended the wait (also when one was pending on entry). A
    /// notification that arrives during the busy-wait is seen within one clock read.
    /// Allocation-free.
    pub fn wait_until(&mut self, deadline_ns: u64, spin_ns: u64) -> bool {
        let inner = &*self.inner;
        if inner.take() {
            return true;
        }
        let sleep_until = deadline_ns.saturating_sub(spin_ns);
        self.sys.prepare(&inner.sys);
        while clock::now_ns() < sleep_until {
            if inner
                .state
                .compare_exchange(EMPTY, PARKED, Ordering::Acquire, Ordering::Acquire)
                .is_err()
            {
                // Only the parker leaves NOTIFIED, so the state is NOTIFIED here.
                inner.state.store(EMPTY, Ordering::Relaxed);
                return true;
            }
            self.sys.park(&inner.sys, &inner.state, sleep_until);
            // PARKED on timeout or a spurious wake-up, NOTIFIED otherwise.
            if inner.state.swap(EMPTY, Ordering::Acquire) == NOTIFIED {
                return true;
            }
        }
        loop {
            if inner.take() {
                return true;
            }
            if clock::now_ns() >= deadline_ns {
                return false;
            }
            std::hint::spin_loop();
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::atomic::AtomicU32;

    pub(super) struct Shared;

    impl Shared {
        pub(super) fn new() -> Self {
            Self
        }

        pub(super) fn wake(&self, state: &AtomicU32) {
            // SAFETY: `state` is a valid aligned u32 that outlives the call.
            unsafe {
                libc::syscall(
                    libc::SYS_futex,
                    state.as_ptr(),
                    libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG,
                    1,
                );
            }
        }
    }

    pub(super) struct Local;

    impl Local {
        pub(super) fn new() -> Self {
            Self
        }

        pub(super) fn prepare(&mut self, _: &Shared) {}

        /// Sleeps while `state` is PARKED, until `deadline_ns` on `CLOCK_MONOTONIC`.
        /// May return early (signal, spurious wake-up).
        pub(super) fn park(&mut self, _: &Shared, state: &AtomicU32, deadline_ns: u64) {
            let ts = libc::timespec {
                tv_sec: (deadline_ns / 1_000_000_000) as _,
                tv_nsec: (deadline_ns % 1_000_000_000) as _,
            };
            // FUTEX_WAIT_BITSET takes an absolute timeout on CLOCK_MONOTONIC (no
            // FUTEX_CLOCK_REALTIME), the engine clock.
            // SAFETY: `state` is a valid aligned u32 and `ts` a valid timespec, both
            // outliving the call; the second address is unused by this operation.
            unsafe {
                libc::syscall(
                    libc::SYS_futex,
                    state.as_ptr(),
                    libc::FUTEX_WAIT_BITSET | libc::FUTEX_PRIVATE_FLAG,
                    super::PARKED,
                    &ts as *const libc::timespec,
                    std::ptr::null::<u32>(),
                    libc::FUTEX_BITSET_MATCH_ANY,
                );
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::AtomicU32;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Threading::{
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateEventW, CreateWaitableTimerExW, INFINITE,
        SetEvent, SetWaitableTimer, TIMER_ALL_ACCESS, WaitForMultipleObjects, WaitForSingleObject,
    };

    /// Auto-reset event signalled by the waker.
    pub(super) struct Shared {
        event: HANDLE,
    }

    // SAFETY: event handles may be signalled and waited on from any thread.
    unsafe impl Send for Shared {}
    // SAFETY: as above; `SetEvent` and waits are thread-safe.
    unsafe impl Sync for Shared {}

    impl Shared {
        pub(super) fn new() -> Self {
            // SAFETY: null attributes and name are allowed; auto-reset, not signalled.
            let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
            assert!(!event.is_null(), "CreateEventW failed");
            Self { event }
        }

        pub(super) fn wake(&self, _: &AtomicU32) {
            // SAFETY: `event` is a valid event handle owned by `self`.
            unsafe { SetEvent(self.event) };
        }
    }

    impl Drop for Shared {
        fn drop(&mut self) {
            // SAFETY: the handle is valid and closed exactly once.
            unsafe { CloseHandle(self.event) };
        }
    }

    /// High-resolution waitable timer of the waiting thread. Null before Windows 10
    /// 1803; the event wait then times out with millisecond resolution.
    pub(super) struct Local {
        timer: HANDLE,
    }

    // SAFETY: the timer handle is owned exclusively by this value.
    unsafe impl Send for Local {}

    impl Local {
        pub(super) fn new() -> Self {
            // SAFETY: null attributes and name are allowed.
            let timer = unsafe {
                CreateWaitableTimerExW(
                    std::ptr::null(),
                    std::ptr::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS,
                )
            };
            Self { timer }
        }

        pub(super) fn prepare(&mut self, _: &Shared) {}

        /// Sleeps until the event is signalled or `deadline_ns` (engine clock). May
        /// return early: a signal left over from an earlier notification.
        pub(super) fn park(&mut self, shared: &Shared, _: &AtomicU32, deadline_ns: u64) {
            let remaining = deadline_ns.saturating_sub(crate::clock::now_ns());
            if remaining == 0 {
                return;
            }
            if !self.timer.is_null() {
                // Negative due time is relative, in 100 ns units.
                let due = -((remaining / 100).max(1) as i64);
                let handles = [shared.event, self.timer];
                // SAFETY: both handles are valid for the duration of the wait.
                unsafe {
                    if SetWaitableTimer(self.timer, &due, 0, None, std::ptr::null(), 0) != 0 {
                        WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE);
                        return;
                    }
                }
            }
            let ms = remaining.div_ceil(1_000_000).min(u64::from(INFINITE - 1)) as u32;
            // SAFETY: `event` is a valid event handle.
            unsafe { WaitForSingleObject(shared.event, ms) };
        }
    }

    impl Drop for Local {
        fn drop(&mut self) {
            if !self.timer.is_null() {
                // SAFETY: the handle is valid and closed exactly once.
                unsafe { CloseHandle(self.timer) };
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    use std::sync::OnceLock;
    use std::sync::atomic::AtomicU32;
    use std::thread::Thread;
    use std::time::Duration;

    /// The parked thread, registered on its first wait.
    pub(super) struct Shared {
        thread: OnceLock<Thread>,
    }

    impl Shared {
        pub(super) fn new() -> Self {
            Self {
                thread: OnceLock::new(),
            }
        }

        pub(super) fn wake(&self, _: &AtomicU32) {
            if let Some(thread) = self.thread.get() {
                thread.unpark();
            }
        }
    }

    pub(super) struct Local;

    impl Local {
        pub(super) fn new() -> Self {
            Self
        }

        /// Registers the waiting thread before it can be seen as PARKED, so no
        /// notification misses it.
        pub(super) fn prepare(&mut self, shared: &Shared) {
            shared.thread.get_or_init(std::thread::current);
        }

        pub(super) fn park(&mut self, _: &Shared, _: &AtomicU32, deadline_ns: u64) {
            let remaining = deadline_ns.saturating_sub(crate::clock::now_ns());
            std::thread::park_timeout(Duration::from_nanos(remaining));
        }
    }
}
