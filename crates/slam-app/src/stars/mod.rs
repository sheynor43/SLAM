//! Star ratings and SS performance of the song list (ADR-0134, ADR-0151).
//!
//! The `slam-stars` thread calculates the nomod rating of every difficulty after the scan:
//! the sets in view first, then the rest in list order, through the on-disk cache
//! ([`cache`]). With mods selected it first rates the sets in view with them, through a
//! cache in memory. It hands each set's ratings over an SPSC ring to the library thread,
//! which owns the list: it sorts the difficulties and publishes the list again, and the
//! update thread, as with every list, only swaps it in. The update thread's whole part is
//! [`StarFocus`]: atomic stores telling where the player is looking and with which mods.

use std::collections::HashMap;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slam_osu::stars::{Rating, RatingMods};
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
/// Ratings with mods kept in memory; past it the cache starts over.
const MOD_CACHE: usize = 1 << 16;
/// How often a worker with nothing left but ratings with mods looks at the focus again.
const WATCH: Duration = Duration::from_millis(20);

/// Calculates the rating of a file with mods. It must have no shared mutable state
/// (interior mutability, locks, caches): a panic is caught with `AssertUnwindSafe`, so a
/// calculator left half-updated by one would be used again.
pub type Calculator = Box<dyn Fn(&Path, &RatingMods) -> io::Result<Rating> + Send>;

/// The calculator of the game: `rosu-pp` through [`slam_osu::stars`].
pub fn rosu_calculator() -> Calculator {
    Box::new(slam_osu::stars::rating_from_path)
}

/// Where the player is looking and with which mods, written by the update thread and read
/// by the worker and the library thread. The place is two relaxed atomics: a torn pair at
/// worst makes the worker look at a wrong place for a moment. The mods are behind a
/// sequence counter, as a torn set would be cached under a wrong key (ADR-0151).
#[derive(Debug)]
pub struct StarFocus {
    key: AtomicU64,
    span: AtomicU64,
    /// A play is on: the worker waits (ADR-0135).
    paused: AtomicBool,
    /// Odd while the mods are being written.
    mods_seq: AtomicU64,
    /// [`RatingMods::key`] of the selected mods.
    mods: [AtomicU64; 4],
}

impl Default for StarFocus {
    fn default() -> StarFocus {
        StarFocus {
            key: AtomicU64::new(0),
            span: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            mods_seq: AtomicU64::new(0),
            mods: RatingMods::NONE.key().map(AtomicU64::new),
        }
    }
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

    /// The selected mods, as far as the ratings go. Called by one thread only (the update
    /// thread); does not allocate and does nothing when they are the same.
    pub fn set_mods(&self, mods: &RatingMods) {
        let key = mods.key();
        if self.mods_key() == key {
            return;
        }
        // A seqlock: odd while writing; the fence keeps the stores after the odd count.
        self.mods_seq.fetch_add(1, Ordering::Relaxed);
        std::sync::atomic::fence(Ordering::Release);
        for (slot, value) in self.mods.iter().zip(key) {
            slot.store(value, Ordering::Relaxed);
        }
        self.mods_seq.fetch_add(1, Ordering::Release);
    }

    /// The selected mods, as [`StarFocus::set_mods`] last wrote them.
    pub fn mods(&self) -> RatingMods {
        RatingMods::from_key(self.mods_key())
    }

    fn mods_key(&self) -> [u64; 4] {
        loop {
            let before = self.mods_seq.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let key = std::array::from_fn(|i| self.mods[i].load(Ordering::Relaxed));
            std::sync::atomic::fence(Ordering::Acquire);
            if self.mods_seq.load(Ordering::Relaxed) == before {
                return key;
            }
        }
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

/// The ratings of one set, by file, for the run `generation`: without mods, or with the
/// mods of the key `mods`.
pub struct StarBatch {
    pub generation: u32,
    pub mods: Option<[u64; 4]>,
    pub stars: Vec<(PathBuf, Rating)>,
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
                    mod_cache: HashMap::new(),
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

    /// Calls `each` with every rating that arrived since the last call, with the key of
    /// its mods (`None` without mods); returns whether there was any.
    pub fn poll(&mut self, mut each: impl FnMut(PathBuf, Rating, Option<[u64; 4]>)) -> bool {
        let mut any = false;
        while let Ok(batch) = self.results.pop() {
            if batch.generation != self.generation {
                continue;
            }
            for (path, rating) in batch.stars {
                any = true;
                each(path, rating, batch.mods);
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
    if let Some(wanted) = next_in_view(done, index, focus) {
        return Some(wanted);
    }
    while *cursor < done.len() && done[*cursor] {
        *cursor += 1;
    }
    (*cursor < done.len()).then_some(*cursor)
}

/// The first set not done among the sets in view and a few after them.
pub fn next_in_view(
    done: &[bool],
    index: &HashMap<u64, usize>,
    focus: (u64, usize),
) -> Option<usize> {
    let &first = index.get(&focus.0)?;
    let end = first.saturating_add(focus.1).saturating_add(FOCUS_MARGIN);
    (first..end.min(done.len())).find(|&i| !done[i])
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
    /// Ratings with mods, by file key and mods key (ADR-0151).
    mod_cache: HashMap<(u64, [u64; 4]), Rating>,
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
            // The sets in view with the selected mods come first (ADR-0151).
            let mods = self.focus.mods();
            if !mods.is_none() {
                let key = mods.key();
                if running.mods_key != key {
                    running.mods_key = key;
                    running.mods_done.fill(false);
                }
                if let Some(set) = next_in_view(&running.mods_done, &running.index, focus) {
                    if self.out.is_full() {
                        std::thread::sleep(POLL);
                        continue;
                    }
                    running.mods_done[set] = true;
                    let stars = self.rate_with(&running.sets[set], &mods);
                    if !stars.is_empty() {
                        // A slot is free and this thread is the only producer.
                        let _ = self.out.push(StarBatch {
                            generation: running.generation,
                            mods: Some(key),
                            stars,
                        });
                    }
                    continue;
                }
            }
            if running.finished {
                // Only ratings with mods are left to do, as the player scrolls or picks
                // mods; without mods, rarely.
                self.save();
                std::thread::park_timeout(if mods.is_none() { IDLE } else { WATCH });
                continue;
            }
            let Some(set) = next_set(&running.done, &running.index, focus, &mut running.cursor)
            else {
                running.finished = true;
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
                    mods: None,
                    stars,
                });
            }
            if self.cache.unsaved() >= SAVE_EVERY && !self.stop.load(Ordering::Relaxed) {
                self.save();
            }
        }
        self.save();
    }

    /// The nomod ratings of `set`'s files that can be had; the rest are skipped.
    fn rate(&mut self, set: &StarSet) -> Vec<(PathBuf, Rating)> {
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
                    if !self.wait_out_play() {
                        break;
                    }
                    // A transient failure is neither cached nor reported: a later run
                    // asks again.
                    let Some(value) = self.calculate(path, &RatingMods::NONE) else {
                        continue;
                    };
                    if let Some(key) = key {
                        self.cache.insert(key, value);
                    }
                    value
                }
            };
            if value.stars >= 0.0 {
                stars.push((path.clone(), value));
            }
        }
        stars
    }

    /// The ratings of `set`'s files with `mods` that can be had. Stops early when the
    /// selected mods change: the batch would be dropped anyway.
    fn rate_with(&mut self, set: &StarSet, mods: &RatingMods) -> Vec<(PathBuf, Rating)> {
        let mods_key = mods.key();
        let mut stars = Vec::with_capacity(set.files.len());
        for path in &set.files {
            if self.stop.load(Ordering::Relaxed) || self.focus.mods().key() != mods_key {
                break;
            }
            let Some(key) = cache::file_key(path) else {
                continue;
            };
            // A file that cannot be rated without mods cannot be with them.
            if self.cache.get(key).is_some_and(|nomod| nomod.stars < 0.0) {
                continue;
            }
            let value = match self.mod_cache.get(&(key, mods_key)) {
                Some(&value) => value,
                None => {
                    if !self.wait_out_play() {
                        break;
                    }
                    // A transient failure skips the file until the mods or the run change.
                    let Some(value) = self.calculate(path, mods) else {
                        continue;
                    };
                    if self.mod_cache.len() >= MOD_CACHE {
                        self.mod_cache.clear();
                    }
                    self.mod_cache.insert((key, mods_key), value);
                    value
                }
            };
            if value.stars >= 0.0 {
                stars.push((path.clone(), value));
            }
        }
        stars
    }

    /// Waits while a play is on; false when the thread is to stop.
    fn wait_out_play(&self) -> bool {
        while self.focus.paused() && !self.stop.load(Ordering::Relaxed) {
            std::thread::park_timeout(PAUSED);
        }
        !self.stop.load(Ordering::Relaxed)
    }

    /// The rating of `path` with `mods`, or [`cache::NOT_COMPUTABLE`] when the file cannot
    /// be rated (a parse failure, `InvalidData`, or a panic): such a file is remembered, not
    /// retried on every start. `None` for any other I/O error (permissions, a sharing
    /// violation, EIO), which may pass: nothing is cached.
    fn calculate(&self, path: &Path, mods: &RatingMods) -> Option<Rating> {
        match catch_unwind(AssertUnwindSafe(|| (self.calculator)(path, mods))) {
            Ok(Ok(value))
                if value.stars.is_finite()
                    && value.stars >= 0.0
                    && value.pp.is_finite()
                    && value.pp >= 0.0 =>
            {
                Some(value)
            }
            Ok(Ok(_)) => Some(cache::NOT_COMPUTABLE),
            Ok(Err(e)) if e.kind() == io::ErrorKind::InvalidData => Some(cache::NOT_COMPUTABLE),
            Ok(Err(e)) => {
                tracing::debug!("stars: cannot rate {}: {e}", path.display());
                None
            }
            Err(_) => {
                tracing::warn!("stars: the calculator panicked on {}", path.display());
                Some(cache::NOT_COMPUTABLE)
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
    /// Every set has its nomod ratings.
    finished: bool,
    /// The sets rated with the mods of `mods_key`.
    mods_done: Vec<bool>,
    mods_key: [u64; 4],
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
            mods_done: vec![false; sets.len()],
            sets,
            index,
            cursor: 0,
            finished: false,
            mods_key: RatingMods::NONE.key(),
        }
    }
}

#[cfg(test)]
mod tests;
