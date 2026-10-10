//! Scheduling of the draw thread for benchmark variants: CPU affinity, nice level
//! and real-time priority. Linux only; elsewhere the settings are rejected.

/// How to schedule the calling thread; `None` fields keep the inherited setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreadSched {
    /// Pin to this logical CPU.
    pub cpu: Option<usize>,
    /// `SCHED_FIFO` with this priority (1..=99); needs `RLIMIT_RTPRIO` or a capability.
    pub fifo: Option<i32>,
    /// Nice level of the thread (-20..=19) under `SCHED_OTHER`.
    pub nice: Option<i32>,
}

impl ThreadSched {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Applies the settings to the calling thread. Threads it spawns afterwards
    /// inherit the affinity and nice level, but not `SCHED_FIFO`.
    #[cfg(target_os = "linux")]
    pub fn apply(&self) -> Result<(), String> {
        let os_error = |what: &str| format!("{what}: {}", std::io::Error::last_os_error());
        if let Some(cpu) = self.cpu {
            if cpu >= libc::CPU_SETSIZE as usize {
                return Err(format!("cpu {cpu} out of range"));
            }
            // SAFETY: a zeroed cpu_set_t is a valid empty set.
            let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
            // SAFETY: `cpu` is below CPU_SETSIZE (checked above).
            unsafe { libc::CPU_SET(cpu, &mut set) };
            // SAFETY: pid 0 is the calling thread; `set` is a valid cpu_set_t.
            if unsafe { libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &set) } != 0 {
                return Err(os_error("sched_setaffinity"));
            }
        }
        if let Some(nice) = self.nice {
            // SAFETY: plain syscall; on Linux PRIO_PROCESS with the thread id sets the
            // nice level of that thread only.
            let tid = unsafe { libc::gettid() };
            if unsafe { libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, nice) } != 0 {
                return Err(os_error("setpriority"));
            }
        }
        if let Some(priority) = self.fifo {
            let param = libc::sched_param {
                sched_priority: priority,
            };
            // Threads the driver spawns later start under SCHED_OTHER instead of
            // inheriting FIFO and starving behind the draw thread.
            let policy = libc::SCHED_FIFO | libc::SCHED_RESET_ON_FORK;
            // SAFETY: pid 0 is the calling thread; `param` is valid.
            if unsafe { libc::sched_setscheduler(0, policy, &param) } != 0 {
                return Err(os_error("sched_setscheduler(SCHED_FIFO)"));
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn apply(&self) -> Result<(), String> {
        if self.is_default() {
            Ok(())
        } else {
            Err("thread scheduling options are only supported on Linux".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_changes_nothing() {
        assert!(ThreadSched::default().is_default());
        assert_eq!(ThreadSched::default().apply(), Ok(()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn out_of_range_cpu_is_rejected() {
        let sched = ThreadSched {
            cpu: Some(libc::CPU_SETSIZE as usize),
            ..ThreadSched::default()
        };
        assert!(sched.apply().unwrap_err().contains("out of range"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pins_the_calling_thread() {
        std::thread::spawn(|| {
            let sched = ThreadSched {
                cpu: Some(0),
                ..ThreadSched::default()
            };
            sched.apply().unwrap();
            // SAFETY: plain syscall without arguments.
            assert_eq!(unsafe { libc::sched_getcpu() }, 0);
        })
        .join()
        .unwrap();
    }
}
