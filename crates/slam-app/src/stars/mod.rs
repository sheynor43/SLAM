//! Star ratings of the song list (ADR-0134).
//!
//! The `slam-stars` thread calculates the nomod rating of every difficulty after the scan:
//! the sets in view first, then the rest in list order, through the on-disk cache
//! ([`cache`]). It hands each set's ratings over an SPSC ring to the library thread, which
//! owns the list: it sorts the difficulties and publishes the list again, and the update
//! thread, as with every list, only swaps it in. The update thread's whole part is
//! [`StarFocus::set`]: two atomic stores telling where the player is looking.

use std::collections::HashMap;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

pub mod cache;

use cache::StarCache;

/// Batches waiting for the library thread: one per set, so a few hundred sets of room.
const RING: usize = 256;
/// How long an idle worker parks at most: commands unpark it, so this is a safety net.
const IDLE: Duration = Duration::from_millis(500);
/// How long a worker with a full ring waits for the library thread.
const POLL: Duration = Duration::from_millis(5);
/// How often a worker paused for a play looks again; unparking ends the wait early.
const PAUSED: Duration = Duration::from_millis(50);
/// New ratings between two saves of the cache, so a quit does not lose minutes of work.
const SAVE_EVERY: usize = 256;
/// Sets after the visible ones that are calculated before the rest.
const FOCUS_MARGIN: usize = 4;

/// Calculates the rating of a file.
pub type Calculator = Box<dyn Fn(&Path) -> io::Result<f32> + Send>;

/// The calculator of the game: `rosu-pp` through [`slam_osu::stars`].
pub fn rosu_calculator() -> Calculator {
    Box::new(slam_osu::stars::stars_from_path)
}

/// Where the player is looking, written by the update thread and read by the worker.
/// Two relaxed atomics: a torn pair at worst makes the worker look at a wrong place for a
/// moment.
#[derive(Debug, Default)]
pub struct StarFocus {
    key: AtomicU64,
    span: AtomicU64,
    /// A play is on: the worker waits (ADR-0135).
    paused: AtomicBool,
}

impl StarFocus {
    /// The set in view first (a list's set key) and how many sets the view holds. Does not
    /// allocate.
    pub fn set(&self, key: u64, span: usize) {
        self.key.store(key, Ordering::Relaxed);
        self.span.store(span as u64, Ordering::Relaxed);
    }

    /// Pauses the worker while a play is on, and the republishing of the list for ratings.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    fn get(&self) -> (u64, usize) {
        (
            self.key.load(Ordering::Relaxed),
            self.span.load(Ordering::Relaxed) as usize,
        )
    }
}

/// One set to calculate: its key in the published list and its difficulty files.
pub struct StarSet {
    pub key: u64,
    pub files: Vec<PathBuf>,
}

/// The ratings of one set, by file, for the run `generation`.
pub struct StarBatch {
    pub generation: u32,
    pub stars: Vec<(PathBuf, f32)>,
}

enum Command {
    /// Calculate these sets, in list order; replaces the run in progress.
    Start { generation: u32, sets: Vec<StarSet> },
}

/// The library thread's end of the worker. The thread ends and is joined on drop.
pub struct StarsLink {
    commands: Sender<Command>,
    results: rtrb::Consumer<StarBatch>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    generation: u32,
}

impl StarsLink {
    /// Starts the thread, named `slam-stars`. `cache` is the file of the on-disk cache;
    /// `None` caches nothing.
    pub fn spawn(
        cache: Option<PathBuf>,
        calculator: Calculator,
        version: &'static str,
        focus: Arc<StarFocus>,
    ) -> io::Result<StarsLink> {
        let (commands, receiver) = channel();
        let (out, results) = rtrb::RingBuffer::new(RING);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("slam-stars".to_owned())
            .spawn(move || {
                let mut worker = Worker {
                    commands: receiver,
                    out,
                    stop: flag,
                    focus,
                    calculator,
                    version,
                    cache_path: cache,
                    cache: StarCache::default(),
                };
                worker.run();
            })?;
        Ok(StarsLink {
            commands,
            results,
            stop,
            thread: Some(thread),
            generation: 0,
        })
    }

    /// Calculates `sets` (in list order), dropping the run in progress. Ratings of the
    /// earlier run that are still in the ring are ignored by [`StarsLink::poll`].
    pub fn start(&mut self, sets: Vec<StarSet>) {
        self.generation = self.generation.wrapping_add(1);
        let _ = self.commands.send(Command::Start {
            generation: self.generation,
            sets,
        });
        self.wake();
    }

    /// Drops the run in progress, as when the songs folder changes (ADR-0135): the worker
    /// stops after the set it is on, and its ratings are ignored by [`StarsLink::poll`].
    pub fn cancel(&mut self) {
        self.start(Vec::new());
    }

    /// Calls `each` with every rating that arrived since the last call; returns whether
    /// there was any.
    pub fn poll(&mut self, mut each: impl FnMut(PathBuf, f32)) -> bool {
        let mut any = false;
        while let Ok(batch) = self.results.pop() {
            if batch.generation != self.generation {
                continue;
            }
            for (path, stars) in batch.stars {
                any = true;
                each(path, stars);
            }
        }
        any
    }

    fn wake(&self) {
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
}

impl Drop for StarsLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The next set to calculate: the first one not done among the sets in view and a few
/// after them, else the first one not done from `cursor` on (which moves past the done
/// ones). `index` maps set keys to positions in `done`.
pub fn next_set(
    done: &[bool],
    index: &HashMap<u64, usize>,
    focus: (u64, usize),
    cursor: &mut usize,
) -> Option<usize> {
    if let Some(&first) = index.get(&focus.0) {
        let end = first.saturating_add(focus.1).saturating_add(FOCUS_MARGIN);
        let wanted = (first..end.min(done.len())).find(|&i| !done[i]);
        if wanted.is_some() {
            return wanted;
        }
    }
    while *cursor < done.len() && done[*cursor] {
        *cursor += 1;
    }
    (*cursor < done.len()).then_some(*cursor)
}

struct Worker {
    commands: Receiver<Command>,
    out: rtrb::Producer<StarBatch>,
    stop: Arc<AtomicBool>,
    focus: Arc<StarFocus>,
    calculator: Calculator,
    version: &'static str,
    cache_path: Option<PathBuf>,
    cache: StarCache,
}

impl Worker {
    fn run(&mut self) {
        if let Some(path) = &self.cache_path {
            self.cache = StarCache::load(path, self.version);
        }
        let mut job: Option<Job> = None;
        while !self.stop.load(Ordering::Relaxed) {
            // Only the newest command counts.
            while let Ok(Command::Start { generation, sets }) = self.commands.try_recv() {
                job = Some(Job::new(generation, sets));
            }
            let Some(running) = &mut job else {
                self.save();
                std::thread::park_timeout(IDLE);
                continue;
            };
            if self.focus.paused() {
                // A play is on: the ratings wait for the way out (ADR-0135).
                std::thread::park_timeout(PAUSED);
                continue;
            }
            let focus = self.focus.get();
            let Some(set) = next_set(&running.done, &running.index, focus, &mut running.cursor)
            else {
                job = None;
                self.save();
                continue;
            };
            if self.out.is_full() {
                std::thread::sleep(POLL);
                continue;
            }
            running.done[set] = true;
            let stars = self.rate(&running.sets[set]);
            if !stars.is_empty() {
                // A slot is free and this thread is the only producer.
                let _ = self.out.push(StarBatch {
                    generation: running.generation,
                    stars,
                });
            }
            if self.cache.unsaved() >= SAVE_EVERY && !self.stop.load(Ordering::Relaxed) {
                self.save();
            }
        }
        self.save();
    }

    /// The ratings of `set`'s files that can be had; the rest are skipped.
    fn rate(&mut self, set: &StarSet) -> Vec<(PathBuf, f32)> {
        let mut stars = Vec::with_capacity(set.files.len());
        for path in &set.files {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            let key = cache::file_key(path);
            let cached = key.and_then(|key| self.cache.get(key));
            let value = match cached {
                Some(value) => value,
                None => {
                    while self.focus.paused() && !self.stop.load(Ordering::Relaxed) {
                        std::thread::park_timeout(PAUSED);
                    }
                    if self.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let value = self.calculate(path);
                    if let Some(key) = key {
                        self.cache.insert(key, value);
                    }
                    value
                }
            };
            if value >= 0.0 {
                stars.push((path.clone(), value));
            }
        }
        stars
    }

    /// The rating of `path`, or [`cache::NOT_COMPUTABLE`] when the calculator fails or
    /// panics: such a file is remembered, not retried on every start.
    fn calculate(&self, path: &Path) -> f32 {
        match catch_unwind(AssertUnwindSafe(|| (self.calculator)(path))) {
            Ok(Ok(value)) if value.is_finite() && value >= 0.0 => value,
            Ok(_) => cache::NOT_COMPUTABLE,
            Err(_) => {
                tracing::warn!("stars: the calculator panicked on {}", path.display());
                cache::NOT_COMPUTABLE
            }
        }
    }

    fn save(&mut self) {
        if !self.cache.is_dirty() {
            return;
        }
        if let Some(path) = &self.cache_path
            && let Err(e) = self.cache.save(path, self.version)
        {
            tracing::warn!("stars: cannot save the cache: {e}");
        }
    }
}

struct Job {
    generation: u32,
    sets: Vec<StarSet>,
    index: HashMap<u64, usize>,
    done: Vec<bool>,
    cursor: usize,
}

impl Job {
    fn new(generation: u32, sets: Vec<StarSet>) -> Job {
        let index = sets
            .iter()
            .enumerate()
            .map(|(i, set)| (set.key, i))
            .collect();
        Job {
            generation,
            done: vec![false; sets.len()],
            sets,
            index,
            cursor: 0,
        }
    }
}

#[cfg(test)]
mod tests;
