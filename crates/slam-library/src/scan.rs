//! Scan of a stable-style `Songs` folder: one subdirectory per beatmap set, each holding
//! `.osu` files. Only headers are decoded, and only osu!standard difficulties are kept.

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use slam_formats::osu::{DecodeOptions, decode_with};

/// Maximum number of sets in one [`ScanEvent::Batch`].
pub const BATCH_SIZE: usize = 64;

const MODE_STANDARD: i32 = 0;

/// One scanned beatmap set (a directory).
#[derive(Debug, Clone, PartialEq)]
pub struct BeatmapSetEntry {
    /// The set directory.
    pub dir: PathBuf,
    /// Romanised artist of the first difficulty.
    pub artist: String,
    /// Romanised title of the first difficulty.
    pub title: String,
    /// Mapper of the first difficulty.
    pub creator: String,
    /// Artist in the original script of the first difficulty; empty when the file has none.
    pub artist_unicode: String,
    /// Title in the original script of the first difficulty; empty when the file has none.
    pub title_unicode: String,
    /// Source (the game, anime or album) of the first difficulty.
    pub source: String,
    /// Space-separated tags of the first difficulty.
    pub tags: String,
    /// osu!standard difficulties, sorted by version (case-insensitive), then path.
    pub difficulties: Vec<DifficultyEntry>,
}

/// One scanned difficulty file.
#[derive(Debug, Clone, PartialEq)]
pub struct DifficultyEntry {
    /// Path of the `.osu` file.
    pub path: PathBuf,
    /// Difficulty name (`Version`).
    pub version: String,
    /// `OverallDifficulty`.
    pub overall_difficulty: f32,
    /// `ApproachRate`.
    pub approach_rate: f32,
    /// `CircleSize`.
    pub circle_size: f32,
    /// `HPDrainRate`.
    pub hp: f32,
    /// The background image named in `[Events]`, relative to the set directory and with
    /// `/` separators, as the file writes it; its case may not match the file on disk.
    pub background: Option<String>,
}

/// Why a set could not be scanned.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// The set directory could not be listed.
    #[error("cannot read set directory: {0}")]
    Io(#[from] io::Error),
}

/// Result of scanning one set.
#[derive(Debug)]
struct SetScan {
    entry: Option<BeatmapSetEntry>,
    skipped: usize,
}

/// The set-level metadata of one difficulty file.
struct SetMeta {
    artist: String,
    title: String,
    creator: String,
    artist_unicode: String,
    title_unicode: String,
    source: String,
    tags: String,
}

fn scan_set_counted(dir: &Path) -> Result<SetScan, ScanError> {
    let options = DecodeOptions {
        headers_only: true,
        ..DecodeOptions::default()
    };
    let mut skipped = 0;
    let mut difficulties = Vec::new();
    let mut meta: Vec<SetMeta> = Vec::new();

    for item in fs::read_dir(dir)? {
        let Ok(item) = item else {
            skipped += 1;
            continue;
        };
        let path = item.path();
        let is_osu = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("osu"));
        if !is_osu || !path.is_file() {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            skipped += 1;
            continue;
        };
        let Ok(decoded) = decode_with(&bytes, &options) else {
            skipped += 1;
            continue;
        };
        let b = decoded.beatmap;
        if b.general.mode != MODE_STANDARD {
            continue;
        }
        meta.push(SetMeta {
            artist: b.metadata.artist,
            title: b.metadata.title,
            creator: b.metadata.creator,
            artist_unicode: b.metadata.artist_unicode,
            title_unicode: b.metadata.title_unicode,
            source: b.metadata.source,
            tags: b.metadata.tags,
        });
        difficulties.push(DifficultyEntry {
            path,
            version: b.metadata.version,
            overall_difficulty: b.difficulty.overall_difficulty,
            approach_rate: b.difficulty.approach_rate,
            circle_size: b.difficulty.circle_size,
            hp: b.difficulty.drain_rate,
            background: b.events.background,
        });
    }

    // Sort difficulties and their metadata together so the first one is deterministic.
    let mut order: Vec<usize> = (0..difficulties.len()).collect();
    order.sort_by(|&a, &b| cmp_difficulty(&difficulties[a], &difficulties[b]));
    let Some(&first) = order.first() else {
        return Ok(SetScan {
            entry: None,
            skipped,
        });
    };
    let meta = meta.swap_remove(first);
    let mut slots: Vec<Option<DifficultyEntry>> = difficulties.into_iter().map(Some).collect();
    let difficulties = order.into_iter().filter_map(|i| slots[i].take()).collect();

    Ok(SetScan {
        entry: Some(BeatmapSetEntry {
            dir: dir.to_owned(),
            artist: meta.artist,
            title: meta.title,
            creator: meta.creator,
            artist_unicode: meta.artist_unicode,
            title_unicode: meta.title_unicode,
            source: meta.source,
            tags: meta.tags,
            difficulties,
        }),
        skipped,
    })
}

fn cmp_difficulty(a: &DifficultyEntry, b: &DifficultyEntry) -> Ordering {
    a.version
        .to_lowercase()
        .cmp(&b.version.to_lowercase())
        .then_with(|| a.path.cmp(&b.path))
}

/// Scans one set directory (non-recursive). Returns `None` when it holds no osu!standard
/// difficulty. Files that fail to decode are skipped.
pub fn scan_set(dir: &Path) -> Result<Option<BeatmapSetEntry>, ScanError> {
    scan_set_counted(dir).map(|s| s.entry)
}

/// Sorts sets by artist, then title (case-insensitive), then directory.
pub fn sort_sets(sets: &mut [BeatmapSetEntry]) {
    sets.sort_by_cached_key(|set| {
        (
            set.artist.to_lowercase(),
            set.title.to_lowercase(),
            set.dir.clone(),
        )
    });
}

/// Totals of a finished scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanSummary {
    /// Sets found.
    pub sets: usize,
    /// Standard difficulties found.
    pub difficulties: usize,
    /// Files or directories that could not be read or decoded.
    pub skipped_files: usize,
    /// Wall time of the scan.
    pub elapsed: Duration,
}

/// A message from the scan thread.
#[derive(Debug)]
pub enum ScanEvent {
    /// Up to [`BATCH_SIZE`] scanned sets, in directory-name order within the batch.
    Batch(Vec<BeatmapSetEntry>),
    /// The scan finished.
    Done(ScanSummary),
    /// The root directory could not be read; no further events follow.
    Failed(io::Error),
}

/// Handle to a background scan. Dropping it cancels the scan: the worker thread is detached
/// and stops at the next set boundary, so drop never blocks.
#[derive(Debug)]
pub struct Scanner {
    rx: Receiver<ScanEvent>,
    cancel: Arc<AtomicBool>,
}

impl Scanner {
    /// Starts scanning `root` in a thread named `slam-library-scan`.
    pub fn spawn(root: PathBuf) -> io::Result<Scanner> {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        thread::Builder::new()
            .name("slam-library-scan".to_owned())
            .spawn(move || run(&root, &tx, &flag))?;
        Ok(Scanner { rx, cancel })
    }

    /// Next pending event, without blocking.
    pub fn try_recv(&self) -> Option<ScanEvent> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        self.cancel.store(true, AtomicOrdering::Relaxed);
    }
}

fn run(root: &Path, tx: &Sender<ScanEvent>, cancel: &AtomicBool) {
    let start = Instant::now();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut skipped = 0;
    match fs::read_dir(root) {
        Ok(entries) => {
            for e in entries {
                match e {
                    Ok(e) if e.path().is_dir() => dirs.push(e.path()),
                    Ok(_) => {}
                    Err(_) => skipped += 1,
                }
            }
        }
        Err(e) => {
            let _ = tx.send(ScanEvent::Failed(e));
            return;
        }
    }
    dirs.sort();

    let (mut sets, mut difficulties) = (0, 0);
    let mut batch = Vec::with_capacity(BATCH_SIZE);
    for dir in dirs {
        if cancel.load(AtomicOrdering::Relaxed) {
            return;
        }
        match scan_set_counted(&dir) {
            Ok(scan) => {
                skipped += scan.skipped;
                if let Some(entry) = scan.entry {
                    sets += 1;
                    difficulties += entry.difficulties.len();
                    batch.push(entry);
                }
            }
            Err(_) => skipped += 1,
        }
        if batch.len() >= BATCH_SIZE {
            let full = std::mem::replace(&mut batch, Vec::with_capacity(BATCH_SIZE));
            if tx.send(ScanEvent::Batch(full)).is_err() {
                return;
            }
        }
    }
    if !batch.is_empty() && tx.send(ScanEvent::Batch(batch)).is_err() {
        return;
    }
    let _ = tx.send(ScanEvent::Done(ScanSummary {
        sets,
        difficulties,
        skipped_files: skipped,
        elapsed: start.elapsed(),
    }));
}
