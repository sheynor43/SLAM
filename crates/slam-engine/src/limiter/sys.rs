//! Platform timers on the engine clock: `clock_nanosleep` on Linux, high-resolution waitable timer on
//! Windows, `std::thread::sleep` elsewhere.

#[cfg(target_os = "linux")]
mod imp {
    use super::super::Timer;

    /// Engine clock (`CLOCK_MONOTONIC`) with absolute `clock_nanosleep`.
    pub struct SystemTimer(());

    impl SystemTimer {
        pub fn new() -> Self {
            Self(())
        }
    }

    impl Timer for SystemTimer {
        fn now_ns(&self) -> u64 {
            crate::clock::now_ns()
        }

        fn sleep_until(&mut self, deadline_ns: u64) {
            let ts = libc::timespec {
                tv_sec: (deadline_ns / 1_000_000_000) as _,
                tv_nsec: (deadline_ns % 1_000_000_000) as _,
            };
            // An absolute deadline makes a retry after a signal exact, so EINTR just loops.
            loop {
                // SAFETY: `ts` is a valid timespec; the remainder pointer may be null
                // with TIMER_ABSTIME.
                let rc = unsafe {
                    libc::clock_nanosleep(
                        libc::CLOCK_MONOTONIC,
                        libc::TIMER_ABSTIME,
                        &ts,
                        std::ptr::null_mut(),
                    )
                };
                if rc != libc::EINTR {
                    break;
                }
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::super::Timer;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Threading::{
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
        TIMER_ALL_ACCESS, WaitForSingleObject,
    };

    /// Engine clock (QueryPerformanceCounter) with a high-resolution waitable timer.
    pub struct SystemTimer {
        /// Null if no high-resolution timer is available (before Windows 10 1803);
        /// `std::thread::sleep` is used then, with system timer resolution.
        handle: HANDLE,
    }

    // SAFETY: the timer handle is owned exclusively by this value and is only used
    // through `&mut self` or on drop.
    unsafe impl Send for SystemTimer {}

    impl SystemTimer {
        pub fn new() -> Self {
            // SAFETY: null attributes and name are allowed.
            let handle = unsafe {
                CreateWaitableTimerExW(
                    std::ptr::null(),
                    std::ptr::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS,
                )
            };
            Self { handle }
        }
    }

    impl Timer for SystemTimer {
        fn now_ns(&self) -> u64 {
            crate::clock::now_ns()
        }

        fn sleep_until(&mut self, deadline_ns: u64) {
            let remaining = deadline_ns.saturating_sub(self.now_ns());
            if remaining == 0 {
                return;
            }
            if !self.handle.is_null() {
                // Negative due time is relative, in 100 ns units.
                let due = -((remaining / 100).max(1) as i64);
                // SAFETY: `handle` is a valid waitable timer owned by `self`.
                unsafe {
                    if SetWaitableTimer(self.handle, &due, 0, None, std::ptr::null(), 0) != 0 {
                        WaitForSingleObject(self.handle, INFINITE);
                        return;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_nanos(remaining));
        }
    }

    impl Drop for SystemTimer {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                // SAFETY: the handle is valid and closed exactly once.
                unsafe { CloseHandle(self.handle) };
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    use super::super::Timer;
    use std::time::Duration;

    pub struct SystemTimer(());

    impl SystemTimer {
        pub fn new() -> Self {
            Self(())
        }
    }

    impl Timer for SystemTimer {
        fn now_ns(&self) -> u64 {
            crate::clock::now_ns()
        }

        fn sleep_until(&mut self, deadline_ns: u64) {
            let remaining = deadline_ns.saturating_sub(self.now_ns());
            if remaining > 0 {
                std::thread::sleep(Duration::from_nanos(remaining));
            }
        }
    }
}

pub use imp::SystemTimer;

impl Default for SystemTimer {
    fn default() -> Self {
        Self::new()
    }
}
