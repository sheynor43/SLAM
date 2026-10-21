//! The shell's library thread: scans the songs folder ([`slam_library::scan`]) and
//! publishes sorted lists for the song select over an SPSC ring. The update thread only
//! pops and clones `Arc`s; every list is built and freed here, off the hot paths.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use slam_library::scan::{BeatmapSetEntry, ScanEvent, Scanner, sort_sets};
use slam_ui::song_select::{DifficultyRow, SetRow, SongList, SongSelectStatus};

/// Lists waiting for the update thread; a newer list supersedes older ones, so a few do.
const RING: usize = 4;
/// How often a scan in progress publishes what it has found.
const PUBLISH_EVERY: Duration = Duration::from_millis(100);
/// How long the library thread sleeps between polls.
const POLL: Duration = Duration::from_millis(10);

/// A difficulty of a published list, beyond what its row shows.
#[derive(Debug, Clone, PartialEq)]
pub struct SongFile {
    /// The `.osu` file.
    pub path: PathBuf,
    /// The approach rate of the file, without mods.
    pub approach_rate: f32,
}

/// The difficulties of a published list, by set and difficulty, in the list's order.
pub type SongFiles = [Box<[SongFile]>];

/// One state of the library as the update thread sees it.
pub struct Published {
    pub list: Arc<SongList>,
    pub files: Arc<SongFiles>,
    pub status: SongSelectStatus,
    /// The status line while scanning, formatted here: Fluent takes a lock.
    pub scanning_text: Arc<str>,
}

/// Formats the status line of a scan from the sets found so far.
pub type ScanningText = Box<dyn Fn(usize) -> String + Send>;

/// The update thread's end: receives published lists.
pub struct SongsLink {
    updates: rtrb::Consumer<Published>,
}

impl SongsLink {
    /// The newest published state, if any arrived since the last call. Does not allocate:
    /// older states are dropped here, but the library thread still holds their lists.
    pub fn latest(&mut self) -> Option<Published> {
        let mut latest = None;
        while let Ok(published) = self.updates.pop() {
            latest = Some(published);
        }
        latest
    }
}

/// The library thread; stops and joins on drop.
pub struct LibraryThread {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for LibraryThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A link and the end that feeds it: the library thread's, or a test's.
pub fn link() -> (rtrb::Producer<Published>, SongsLink) {
    let (producer, updates) = rtrb::RingBuffer::new(RING);
    (producer, SongsLink { updates })
}

/// Starts scanning `root` on a thread named `slam-library`; `scanning_text` formats the
/// status line as sets are found.
pub fn spawn(
    root: PathBuf,
    scanning_text: ScanningText,
) -> std::io::Result<(SongsLink, LibraryThread)> {
    let (producer, link) = link();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("slam-library".to_owned())
        .spawn(move || run(root, producer, &flag, &scanning_text))?;
    Ok((
        link,
        LibraryThread {
            stop,
            thread: Some(thread),
        },
    ))
}

/// The song select's identity of a set: stable while the folder stays where it is. Lives
/// only in this process: `DefaultHasher` may change between Rust versions, so it is never
/// stored.
fn set_key(dir: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    dir.hash(&mut hasher);
    hasher.finish()
}

/// Builds the published form of sorted `sets`.
pub fn publish(
    sets: &[BeatmapSetEntry],
    status: SongSelectStatus,
    scanning_text: Arc<str>,
) -> Published {
    let list = SongList {
        sets: sets
            .iter()
            .map(|set| SetRow {
                key: set_key(&set.dir),
                artist: set.artist.clone(),
                title: set.title.clone(),
                creator: set.creator.clone(),
                difficulties: set
                    .difficulties
                    .iter()
                    .map(|d| DifficultyRow {
                        version: d.version.clone(),
                    })
                    .collect(),
            })
            .collect(),
    };
    let files = sets
        .iter()
        .map(|set| {
            set.difficulties
                .iter()
                .map(|d| SongFile {
                    path: d.path.clone(),
                    approach_rate: d.approach_rate,
                })
                .collect()
        })
        .collect();
    Published {
        list: Arc::new(list),
        files,
        status,
        scanning_text,
    }
}

fn run(
    root: PathBuf,
    mut producer: rtrb::Producer<Published>,
    stop: &AtomicBool,
    scanning_text: &ScanningText,
) {
    let scanner = match Scanner::spawn(root) {
        Ok(scanner) => Some(scanner),
        Err(e) => {
            tracing::warn!("songs scan: {e}");
            None
        }
    };
    let mut sets: Vec<BeatmapSetEntry> = Vec::new();
    let mut status = match scanner {
        Some(_) => SongSelectStatus::Scanning { sets: 0 },
        None => SongSelectStatus::Failed,
    };
    let mut dirty = true;
    let mut last_publish: Option<Instant> = None;
    // Everything published, until only this thread holds it.
    // Each kept on its own: an `Arc` shared between publications (the empty text) must not
    // keep the others.
    let mut retired_lists: Vec<Arc<SongList>> = Vec::new();
    let mut retired_files: Vec<Arc<SongFiles>> = Vec::new();
    let mut retired_texts: Vec<Arc<str>> = Vec::new();
    // Held for the thread's life: never freed elsewhere.
    let no_text: Arc<str> = Arc::from("");
    while !stop.load(Ordering::Relaxed) {
        let scanning = matches!(status, SongSelectStatus::Scanning { .. });
        while let Some(event) = scanner
            .as_ref()
            .filter(|_| scanning)
            .and_then(Scanner::try_recv)
        {
            match event {
                ScanEvent::Batch(batch) => {
                    sets.extend(batch);
                    status = SongSelectStatus::Scanning { sets: sets.len() };
                }
                ScanEvent::Done(summary) => {
                    tracing::info!(
                        "songs: {} sets, {} difficulties, {} skipped in {:?}",
                        summary.sets,
                        summary.difficulties,
                        summary.skipped_files,
                        summary.elapsed
                    );
                    status = SongSelectStatus::Ready;
                }
                ScanEvent::Failed(e) => {
                    tracing::warn!("songs scan: {e}");
                    status = SongSelectStatus::Failed;
                }
            }
            dirty = true;
            if !matches!(status, SongSelectStatus::Scanning { .. }) {
                break;
            }
        }
        let due = !matches!(status, SongSelectStatus::Scanning { .. })
            || last_publish.is_none_or(|at| at.elapsed() >= PUBLISH_EVERY);
        if dirty && due && producer.slots() > 0 {
            sort_sets(&mut sets);
            let text = match status {
                SongSelectStatus::Scanning { sets } => Arc::from(scanning_text(sets)),
                _ => Arc::clone(&no_text),
            };
            let published = publish(&sets, status, text);
            retired_lists.push(Arc::clone(&published.list));
            retired_files.push(Arc::clone(&published.files));
            if !Arc::ptr_eq(&published.scanning_text, &no_text) {
                retired_texts.push(Arc::clone(&published.scanning_text));
            }
            // A slot is free and this thread is the only producer.
            let _ = producer.push(published);
            dirty = false;
            last_publish = Some(Instant::now());
        }
        retired_lists.retain(|list| Arc::strong_count(list) > 1);
        retired_files.retain(|files| Arc::strong_count(files) > 1);
        retired_texts.retain(|text| Arc::strong_count(text) > 1);
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slam_library::scan::DifficultyEntry;

    fn set(dir: &str, versions: &[&str]) -> BeatmapSetEntry {
        BeatmapSetEntry {
            dir: PathBuf::from(dir),
            artist: format!("{dir} artist"),
            title: format!("{dir} title"),
            creator: "mapper".to_owned(),
            difficulties: versions
                .iter()
                .map(|v| DifficultyEntry {
                    path: PathBuf::from(dir).join(format!("{v}.osu")),
                    version: (*v).to_owned(),
                    overall_difficulty: 5.0,
                    approach_rate: 5.0,
                    circle_size: 4.0,
                    hp: 5.0,
                })
                .collect(),
        }
    }

    #[test]
    fn published_rows_and_files_line_up() {
        let sets = [set("a", &["Easy", "Hard"]), set("b", &["Insane"])];
        let published = publish(&sets, SongSelectStatus::Ready, Arc::from(""));
        let list = &published.list;
        assert_eq!(list.sets.len(), 2);
        assert_eq!(list.sets[0].title, "a title");
        assert_eq!(list.sets[0].difficulties[1].version, "Hard");
        assert_eq!(
            published.files[0][1].path,
            PathBuf::from("a").join("Hard.osu")
        );
        assert_eq!(
            published.files[1][0].path,
            PathBuf::from("b").join("Insane.osu")
        );
        assert_eq!(published.files[1][0].approach_rate, 5.0);
        assert_eq!(published.status, SongSelectStatus::Ready);
    }

    #[test]
    fn keys_follow_the_folder_not_the_position() {
        let a = publish(
            &[set("a", &["x"]), set("b", &["y"])],
            SongSelectStatus::Ready,
            Arc::from(""),
        );
        let b = publish(
            &[set("b", &["y"]), set("a", &["x"])],
            SongSelectStatus::Ready,
            Arc::from(""),
        );
        assert_eq!(a.list.sets[0].key, b.list.sets[1].key);
        assert_ne!(a.list.sets[0].key, a.list.sets[1].key);
    }

    /// A unique empty directory under the system temp directory, removed on drop.
    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn wait_for(link: &mut SongsLink, done: impl Fn(&Published) -> bool) -> Published {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(published) = link.latest()
                && done(&published)
            {
                return published;
            }
            assert!(Instant::now() < deadline, "library thread did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_folder_is_scanned_sorted_and_published() {
        let root =
            TempDir(std::env::temp_dir().join(format!("slam-app-songs-{}", std::process::id())));
        for (dir, artist) in [("1 zed", "Zed"), ("2 alpha", "alpha")] {
            let path = root.0.join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(
                path.join("map.osu"),
                format!(
                    "osu file format v14\n\n[General]\nMode: 0\n\n[Metadata]\nTitle:T\n\
                     Artist:{artist}\nCreator:C\nVersion:Normal\n"
                ),
            )
            .unwrap();
        }
        let (mut link, thread) = spawn(root.0.clone(), Box::new(|n| format!("{n} sets"))).unwrap();
        let published = wait_for(&mut link, |p| p.status == SongSelectStatus::Ready);
        let artists: Vec<_> = published.list.sets.iter().map(|s| &s.artist).collect();
        assert_eq!(artists, ["alpha", "Zed"]);
        assert_eq!(
            published.files[0][0].path,
            root.0.join("2 alpha").join("map.osu")
        );
        // Once nobody else holds the final list, the library thread frees it.
        let (list, files) = (
            Arc::downgrade(&published.list),
            Arc::downgrade(&published.files),
        );
        drop(published);
        let deadline = Instant::now() + Duration::from_secs(5);
        while list.upgrade().is_some() || files.upgrade().is_some() {
            assert!(Instant::now() < deadline, "the final list was not freed");
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(thread);
    }

    #[test]
    fn a_missing_folder_fails() {
        let missing = std::env::temp_dir().join("slam-app-songs-missing-folder");
        let (mut link, _thread) = spawn(missing, Box::new(|n| format!("{n} sets"))).unwrap();
        let published = wait_for(&mut link, |p| {
            p.status != SongSelectStatus::Scanning { sets: 0 }
        });
        assert_eq!(published.status, SongSelectStatus::Failed);
        assert!(published.list.sets.is_empty());
    }
}
