//! Saving `slam.toml` off the update thread: the thread hands a copy of the settings to a
//! saver thread through a wait-free ring and wakes it; the file is written there.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{JoinHandle, Thread};
use std::time::Duration;

use super::file::{self, LoadStatus};
use super::model::Settings;

/// Requests that wait for the saver; a request that finds the ring full is handed back.
const RING: usize = 4;
/// How long the saver sleeps without a wake-up before looking at the ring again.
const POLL: Duration = Duration::from_millis(200);

/// The update thread's end: sends settings to be saved. Never blocks or allocates.
pub struct SaveLink {
    requests: rtrb::Producer<Box<Settings>>,
    saver: Thread,
}

impl SaveLink {
    /// Queues `settings` to be written; hands them back if the saver is behind.
    pub fn request(&mut self, settings: Box<Settings>) -> Result<(), Box<Settings>> {
        match self.requests.push(settings) {
            Ok(()) => {
                self.saver.unpark();
                Ok(())
            }
            Err(rtrb::PushError::Full(settings)) => Err(settings),
        }
    }
}

/// A link whose requests the caller reads, for tests of the sending side.
#[cfg(test)]
pub fn detached(capacity: usize) -> (SaveLink, rtrb::Consumer<Box<Settings>>) {
    let (requests, consumer) = rtrb::RingBuffer::new(capacity);
    let link = SaveLink {
        requests,
        saver: std::thread::current(),
    };
    (link, consumer)
}

/// The saver thread; [`Saver::finish`] writes what is queued and joins it.
pub struct Saver {
    thread: JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

impl Saver {
    /// Stops the thread after it has written every queued request.
    pub fn finish(self) {
        self.stop.store(true, Ordering::Release);
        self.thread.thread().unpark();
        let _ = self.thread.join();
    }
}

/// Starts a saver writing to `path`.
pub fn spawn(path: PathBuf) -> std::io::Result<(SaveLink, Saver)> {
    let (producer, mut consumer) = rtrb::RingBuffer::<Box<Settings>>::new(RING);
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("slam-settings".into())
            .spawn(move || {
                loop {
                    // Read the flag first: what was queued before it was set is written.
                    let stopping = stop.load(Ordering::Acquire) || consumer.is_abandoned();
                    while let Ok(settings) = consumer.pop() {
                        if let Err(e) = write(&path, *settings) {
                            tracing::warn!("settings: {e}");
                        }
                    }
                    // The link going away (an early return of the window) also ends it.
                    if stopping {
                        return;
                    }
                    std::thread::park_timeout(POLL);
                }
            })?
    };
    let link = SaveLink {
        requests: producer,
        saver: thread.thread().clone(),
    };
    Ok((link, Saver { thread, stop }))
}

/// Writes `settings` to `path`. The songs folder is the file's own: one given on the command
/// line is not saved. A file that is not TOML is left alone.
pub fn write(path: &Path, mut settings: Settings) -> Result<(), String> {
    let on_disk = file::load(path);
    if on_disk.status == LoadStatus::Broken {
        return Err(format!(
            "{} is not valid TOML, not overwriting it",
            path.display()
        ));
    }
    settings.paths = on_disk.settings.paths;
    file::save(path, &settings).map_err(|e| format!("cannot write {}: {e}", path.display()))
}
