//! The shell's library thread: scans the songs folder ([`slam_library::scan`]) and
//! publishes sorted lists for the song select over an SPSC ring. It owns the folder: on a
//! command of the update thread it shows the OS folder picker, saves the choice and lists
//! the new folder (ADR-0064), with the skins next to it: their names for the update thread
//! and their dropdown for the draw thread. The update thread only pops, swaps and clones
//! `Arc`s; every list is built and freed here, off the hot paths.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use slam_input::{FolderCallback, FolderPick};
use slam_library::scan::{BeatmapSetEntry, ScanEvent, Scanner, sort_sets};
use slam_ui::song_select::{
    DifficultyRow, SetRow, SongFilter, SongList, SongSelectStatus, search_text,
};
use slam_ui::widgets::DropdownView;

/// Lists waiting for the update thread; a newer list supersedes older ones, so a few do.
const RING: usize = 4;
/// How often a scan in progress publishes what it has found.
const PUBLISH_EVERY: Duration = Duration::from_millis(100);
/// How long the library thread sleeps between polls.
const POLL: Duration = Duration::from_millis(10);
/// After this, a folder picker that has not answered no longer blocks a new one: its
/// backend may never answer (a desktop portal that restarted).
const PICK_STALE: Duration = Duration::from_secs(300);

/// A difficulty of a published list, beyond what its row shows.
#[derive(Debug, Clone, PartialEq)]
pub struct SongFile {
    /// The `.osu` file.
    pub path: PathBuf,
    /// The approach rate of the file, without mods.
    pub approach_rate: f32,
    /// The circle size of the file, without mods.
    pub circle_size: f32,
    /// The HP drain rate of the file, without mods.
    pub drain_rate: f32,
    /// The overall difficulty of the file, without mods.
    pub overall_difficulty: f32,
}

/// The difficulties of a published list, by set and difficulty, in the list's order.
pub type SongFiles = [Box<[SongFile]>];

/// One state of the library as the update thread sees it.
pub struct Published {
    pub list: Arc<SongList>,
    /// The search buffers for `list`: the update thread passes them to the song select
    /// and hands the ones it gets back to the library thread ([`SongsLink::retire_filter`]).
    pub filter: SongFilter,
    pub files: Arc<SongFiles>,
    pub status: SongSelectStatus,
    /// The status line while scanning, formatted here: Fluent takes a lock.
    pub scanning_text: Arc<str>,
    /// The songs folder being listed, as the settings screen shows it; empty without one.
    pub folder: Arc<str>,
    /// The player chose another folder: the update thread swaps its values in and hands
    /// the box back with the old ones ([`SongsLink::retire`]).
    pub change: Option<Box<FolderChange>>,
}

/// A new songs folder for the update thread's settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderChange {
    pub songs: Option<PathBuf>,
    /// The folder names in its `Skins`, in the settings screen's order.
    pub skins: Vec<String>,
}

/// The skins next to a songs folder.
pub struct SkinList {
    pub names: Vec<String>,
    /// The settings screen's skin dropdown over them, for the draw thread; `None` in tests.
    pub view: Option<DropdownView>,
}

/// Formats the status line of a scan from the sets found so far.
pub type ScanningText = Box<dyn Fn(usize) -> String + Send>;

/// Commands waiting for the library thread.
const COMMANDS: usize = 4;

/// Search buffers waiting to be freed by the library thread: room for several lists between
/// two polls of the library thread.
const OLD_FILTERS: usize = 16;

/// Skin dropdowns waiting for the draw thread, and old ones waiting to be freed: room for
/// several folder changes between two polls of the library thread.
const SKIN_VIEWS: usize = 4;

/// What the update thread asks of the library thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the OS folder picker; a chosen folder is saved and listed (ADR-0064).
    PickFolder,
    /// Free the old values of a change, off the update thread.
    Retire(Box<FolderChange>),
}

/// Shows the OS folder picker starting at a folder; the callback gets the answer.
pub type OpenFolderPicker = Box<dyn Fn(Option<&Path>, FolderCallback) + Send>;

/// How the library thread changes its folder (ADR-0064).
pub struct Folders {
    pub open: OpenFolderPicker,
    /// The songs folder of a chosen folder: a stable install's own, else the folder.
    pub resolve: fn(&Path) -> PathBuf,
    /// Saves a chosen songs folder for the next start.
    pub save: Box<dyn Fn(&Path) + Send>,
    /// Lists the skins next to a songs folder.
    pub skins: Box<dyn Fn(&Path) -> SkinList + Send>,
}

/// The draw thread's end: new skin dropdowns in, replaced ones back to be freed.
pub struct SkinViews {
    views: rtrb::Consumer<DropdownView>,
    old: rtrb::Producer<DropdownView>,
}

impl SkinViews {
    /// The newest skin dropdown, if one arrived. Does not allocate.
    pub fn take(&mut self) -> Option<DropdownView> {
        let mut latest = None;
        while let Ok(view) = self.views.pop() {
            if let Some(older) = latest.replace(view) {
                self.give_back(older);
            }
        }
        latest
    }

    /// Hands a replaced dropdown to the library thread to free. With its ring full the view
    /// is freed here: more folder changes than the ring holds before the library thread
    /// empties it, which takes 10 ms.
    pub fn give_back(&mut self, view: DropdownView) {
        let _ = self.old.push(view);
    }
}

/// The update thread's end: receives published lists and sends commands.
pub struct SongsLink {
    updates: rtrb::Consumer<Published>,
    commands: rtrb::Producer<Command>,
    old_filters: rtrb::Producer<SongFilter>,
}

impl SongsLink {
    /// The newest published state, if any arrived since the last call, with the newest
    /// folder change among them. Does not allocate: older states are dropped here, but the
    /// library thread still holds their lists, and older changes go back to it.
    pub fn latest(&mut self) -> Option<Published> {
        let mut latest: Option<Published> = None;
        while let Ok(mut published) = self.updates.pop() {
            if let Some(older) = latest.take() {
                self.retire_filter(older.filter);
                if let Some(older) = older.change {
                    match published.change {
                        Some(_) => self.retire(older),
                        None => published.change = Some(older),
                    }
                }
            }
            latest = Some(published);
        }
        latest
    }

    /// Asks the library thread to show the folder picker. Returns whether the command was
    /// queued. Neither blocks nor allocates.
    pub fn pick_folder(&mut self) -> bool {
        self.commands.push(Command::PickFolder).is_ok()
    }

    /// Hands search buffers back to the library thread to free. With the command ring full
    /// they are freed here: more than sixteen lists between two polls of the library thread
    /// (10 ms) do not happen.
    pub fn retire_filter(&mut self, filter: SongFilter) {
        let _ = self.old_filters.push(filter);
    }

    /// Hands a change back with the old values swapped in, for the library thread to free.
    /// With the command ring full the change is freed here; four commands within a tick do
    /// not happen.
    pub fn retire(&mut self, change: Box<FolderChange>) {
        let _ = self.commands.push(Command::Retire(change));
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
    let (producer, link, _) = link_with_commands();
    (producer, link)
}

/// A link, the end that feeds it and the end that reads its commands.
pub fn link_with_commands() -> (
    rtrb::Producer<Published>,
    SongsLink,
    rtrb::Consumer<Command>,
) {
    let (producer, link, commands, _) = link_with_ends();
    (producer, link, commands)
}

/// A link, the end that feeds it, the end that reads its commands and the end that receives
/// the search buffers it hands back.
fn link_with_ends() -> (
    rtrb::Producer<Published>,
    SongsLink,
    rtrb::Consumer<Command>,
    rtrb::Consumer<SongFilter>,
) {
    let (producer, updates) = rtrb::RingBuffer::new(RING);
    let (commands, consumer) = rtrb::RingBuffer::new(COMMANDS);
    let (old_filters, old_filters_rx) = rtrb::RingBuffer::new(OLD_FILTERS);
    (
        producer,
        SongsLink {
            updates,
            commands,
            old_filters,
        },
        consumer,
        old_filters_rx,
    )
}

/// Starts the library thread, named `slam-library`, listing `root` if given; `folders`
/// changes the folder on request, `scanning_text` formats the status line as sets are
/// found.
pub fn spawn(
    root: Option<PathBuf>,
    folders: Folders,
    scanning_text: ScanningText,
) -> std::io::Result<(SongsLink, SkinViews, LibraryThread)> {
    let (producer, link, commands, old_filters) = link_with_ends();
    let (views_tx, views) = rtrb::RingBuffer::new(SKIN_VIEWS);
    let (old, old_rx) = rtrb::RingBuffer::new(SKIN_VIEWS);
    let ends = Ends {
        published: producer,
        commands,
        views: views_tx,
        old_views: old_rx,
        old_filters,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("slam-library".to_owned())
        .spawn(move || run(root, folders, ends, &flag, &scanning_text))?;
    Ok((
        link,
        SkinViews { views, old },
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
                search: search_text(&[
                    &set.creator,
                    &set.artist,
                    &set.artist_unicode,
                    &set.title,
                    &set.title_unicode,
                    &set.source,
                    &set.tags,
                ]),
                difficulties: set
                    .difficulties
                    .iter()
                    .map(|d| DifficultyRow {
                        version: d.version.clone(),
                        search: search_text(&[&d.version]),
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
                    circle_size: d.circle_size,
                    drain_rate: d.hp,
                    overall_difficulty: d.overall_difficulty,
                })
                .collect()
        })
        .collect();
    Published {
        filter: SongFilter::for_list(&list),
        list: Arc::new(list),
        files,
        status,
        scanning_text,
        folder: Arc::from(""),
        change: None,
    }
}

/// The library thread's state between polls.
struct Library {
    folders: Folders,
    /// The songs folder being listed, `None` until one is given or chosen.
    root: Option<PathBuf>,
    scanner: Option<Scanner>,
    sets: Vec<BeatmapSetEntry>,
    status: SongSelectStatus,
    /// The folder as the settings screen shows it; empty without one.
    folder_text: Arc<str>,
    /// Folder texts replaced while the update thread may still hold them.
    retired_folders: Vec<Arc<str>>,
    /// A chosen folder not published yet.
    change: Option<Box<FolderChange>>,
    /// Its skin dropdown, not sent to the draw thread yet.
    skin_view: Option<DropdownView>,
    /// Answers of the folder picker, from whichever thread the OS answers on.
    picks: (Sender<FolderPick>, Receiver<FolderPick>),
    /// When the open folder picker was asked for: further requests are ignored until it
    /// answers, or until [`PICK_STALE`] passes in case it never does.
    picking: Option<Instant>,
    dirty: bool,
}

impl Library {
    fn new(root: Option<PathBuf>, folders: Folders) -> Library {
        let mut library = Library {
            folders,
            root: None,
            scanner: None,
            sets: Vec::new(),
            status: SongSelectStatus::NoFolder,
            folder_text: Arc::from(""),
            retired_folders: Vec::new(),
            change: None,
            skin_view: None,
            picks: channel(),
            picking: None,
            dirty: true,
        };
        if let Some(root) = root {
            library.scan(root);
        }
        library
    }

    /// Starts listing `root` from scratch; a scan of the previous folder is cancelled.
    fn scan(&mut self, root: PathBuf) {
        self.scanner = match Scanner::spawn(root.clone()) {
            Ok(scanner) => Some(scanner),
            Err(e) => {
                tracing::warn!("songs scan: {e}");
                None
            }
        };
        self.status = match self.scanner {
            Some(_) => SongSelectStatus::Scanning { sets: 0 },
            None => SongSelectStatus::Failed,
        };
        self.sets.clear();
        let old = std::mem::replace(&mut self.folder_text, Arc::from(root.display().to_string()));
        self.retired_folders.push(old);
        self.root = Some(root);
        self.dirty = true;
    }

    /// Opens the folder picker, unless one is open.
    fn pick_folder(&mut self) {
        if self.picking.is_some_and(|at| at.elapsed() < PICK_STALE) {
            tracing::info!("songs: a folder picker is already open");
            return;
        }
        self.picking = Some(Instant::now());
        let answers = self.picks.0.clone();
        (self.folders.open)(
            self.root.as_deref(),
            Box::new(move |pick| {
                // The library thread is gone when the window closes: nobody to tell.
                let _ = answers.send(pick);
            }),
        );
    }

    /// Takes the picker's answer, if it came: a chosen folder is saved and listed.
    fn receive_pick(&mut self) {
        let Ok(pick) = self.picks.1.try_recv() else {
            return;
        };
        self.picking = None;
        match pick {
            // Every backend answers with an absolute path; anything else is not saved.
            FolderPick::Chosen(folder) if !folder.is_absolute() => {
                tracing::warn!("songs: ignoring the picked folder {}", folder.display());
            }
            FolderPick::Chosen(folder) => {
                let songs = (self.folders.resolve)(&folder);
                tracing::info!("songs: folder {}", songs.display());
                (self.folders.save)(&songs);
                let skins = (self.folders.skins)(&songs);
                self.change = Some(Box::new(FolderChange {
                    songs: Some(songs.clone()),
                    skins: skins.names,
                }));
                self.skin_view = skins.view;
                self.scan(songs);
            }
            FolderPick::Cancelled => {}
            FolderPick::Failed(e) => tracing::warn!("songs: no folder picker: {e}"),
        }
    }

    /// Takes what the scanner found since the last poll.
    fn receive_scan(&mut self) {
        let Some(scanner) = &self.scanner else {
            return;
        };
        while matches!(self.status, SongSelectStatus::Scanning { .. })
            && let Some(event) = scanner.try_recv()
        {
            match event {
                ScanEvent::Batch(batch) => {
                    self.sets.extend(batch);
                    self.status = SongSelectStatus::Scanning {
                        sets: self.sets.len(),
                    };
                }
                ScanEvent::Done(summary) => {
                    tracing::info!(
                        "songs: {} sets, {} difficulties, {} skipped in {:?}",
                        summary.sets,
                        summary.difficulties,
                        summary.skipped_files,
                        summary.elapsed
                    );
                    self.status = SongSelectStatus::Ready;
                }
                ScanEvent::Failed(e) => {
                    tracing::warn!("songs scan: {e}");
                    self.status = SongSelectStatus::Failed;
                }
            }
            self.dirty = true;
        }
    }
}

/// The library thread's ends of its rings.
struct Ends {
    published: rtrb::Producer<Published>,
    commands: rtrb::Consumer<Command>,
    views: rtrb::Producer<DropdownView>,
    old_views: rtrb::Consumer<DropdownView>,
    old_filters: rtrb::Consumer<SongFilter>,
}

fn run(
    root: Option<PathBuf>,
    folders: Folders,
    mut ends: Ends,
    stop: &AtomicBool,
    scanning_text: &ScanningText,
) {
    let mut library = Library::new(root, folders);
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
        while let Ok(command) = ends.commands.pop() {
            match command {
                Command::PickFolder => library.pick_folder(),
                Command::Retire(old) => drop(old),
            }
        }
        while let Ok(old) = ends.old_views.pop() {
            drop(old);
        }
        while let Ok(old) = ends.old_filters.pop() {
            drop(old);
        }
        library.receive_pick();
        // The dropdown follows the change it belongs to, so the draw thread does not show
        // the new skins with the old choice.
        if library.change.is_none()
            && let Some(view) = library.skin_view.take()
            && let Err(rtrb::PushError::Full(view)) = ends.views.push(view)
        {
            library.skin_view = Some(view);
        }
        library.receive_scan();
        let scanning = matches!(library.status, SongSelectStatus::Scanning { .. });
        let due = !scanning || last_publish.is_none_or(|at| at.elapsed() >= PUBLISH_EVERY);
        if library.dirty && due && ends.published.slots() > 0 {
            sort_sets(&mut library.sets);
            let text = match library.status {
                SongSelectStatus::Scanning { sets } => Arc::from(scanning_text(sets)),
                _ => Arc::clone(&no_text),
            };
            let mut published = publish(&library.sets, library.status, text);
            published.folder = Arc::clone(&library.folder_text);
            published.change = library.change.take();
            retired_lists.push(Arc::clone(&published.list));
            retired_files.push(Arc::clone(&published.files));
            if !Arc::ptr_eq(&published.scanning_text, &no_text) {
                retired_texts.push(Arc::clone(&published.scanning_text));
            }
            // A slot is free and this thread is the only producer.
            let _ = ends.published.push(published);
            library.dirty = false;
            last_publish = Some(Instant::now());
        }
        retired_lists.retain(|list| Arc::strong_count(list) > 1);
        retired_files.retain(|files| Arc::strong_count(files) > 1);
        retired_texts.retain(|text| Arc::strong_count(text) > 1);
        library
            .retired_folders
            .retain(|text| Arc::strong_count(text) > 1);
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
            artist_unicode: String::new(),
            title_unicode: String::new(),
            source: String::new(),
            tags: String::new(),
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
    fn published_rows_carry_lowercased_search_text() {
        let mut entry = set("a", &["Hard Mode"]);
        entry.artist_unicode = "\u{418}\u{43c}\u{44f}".to_owned();
        entry.source = "Some Game".to_owned();
        entry.tags = "Tag One".to_owned();
        let published = publish(&[entry], SongSelectStatus::Ready, Arc::from(""));
        let row = &published.list.sets[0];
        assert_eq!(
            row.search,
            "mapper\na artist\n\u{438}\u{43c}\u{44f}\na title\n\nsome game\ntag one"
        );
        assert_eq!(row.difficulties[0].search, "hard mode");
        // The buffers are made for the list, off the update thread.
        assert_eq!(published.filter.set_count(), 1);
        assert_eq!(published.filter.set_index(0), Some(0));
    }

    #[test]
    fn retired_search_buffers_reach_the_library_thread() {
        let (mut producer, mut link, _, mut old_filters) = link_with_ends();
        let first = publish(&[set("a", &["x"])], SongSelectStatus::Ready, Arc::from(""));
        let second = publish(&[set("b", &["y"])], SongSelectStatus::Ready, Arc::from(""));
        producer.push(first).ok().unwrap();
        producer.push(second).ok().unwrap();
        // The older publication is dropped by `latest`: its buffers go back.
        let latest = link.latest().unwrap();
        assert_eq!(old_filters.pop().unwrap().set_count(), 1);
        assert!(old_filters.pop().is_err());
        link.retire_filter(latest.filter);
        assert!(old_filters.pop().is_ok());
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

    fn no_skins() -> Box<dyn Fn(&Path) -> SkinList + Send> {
        Box::new(|_| SkinList {
            names: Vec::new(),
            view: None,
        })
    }

    /// Folders whose picker always fails and whose saves go nowhere.
    fn no_folders() -> Folders {
        Folders {
            open: Box::new(|_, done| done(FolderPick::Failed("no picker".to_owned()))),
            resolve: Path::to_path_buf,
            save: Box::new(|_| {}),
            skins: no_skins(),
        }
    }

    /// Writes a set folder `dir` under `root` with one difficulty by `artist`.
    fn write_set(root: &Path, dir: &str, artist: &str) {
        let path = root.join(dir);
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

    #[test]
    fn without_a_folder_a_picked_one_is_resolved_saved_and_listed() {
        let root = TempDir(
            std::env::temp_dir().join(format!("slam-app-songs-pick-{}", std::process::id())),
        );
        let songs = root.0.join("Songs");
        write_set(&songs, "1 set", "Picked");
        let (saved_tx, saved_rx) = std::sync::mpsc::channel();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let chosen = root.0.clone();
        let folders = Folders {
            open: Box::new(move |start, done| {
                started_tx.send(start.map(Path::to_path_buf)).unwrap();
                done(FolderPick::Chosen(chosen.clone()));
            }),
            // Stands in for the stable install lookup.
            resolve: |folder| folder.join("Songs"),
            save: Box::new(move |path| saved_tx.send(path.to_path_buf()).unwrap()),
            skins: Box::new(|songs| SkinList {
                names: vec![format!("skin of {}", songs.display())],
                view: None,
            }),
        };
        let (mut link, _views, _thread) =
            spawn(None, folders, Box::new(|n| format!("{n} sets"))).unwrap();
        let first = wait_for(&mut link, |_| true);
        assert_eq!(first.status, SongSelectStatus::NoFolder);
        assert_eq!(&*first.folder, "");
        assert!(first.change.is_none());

        assert!(link.pick_folder());
        // The change comes once, with the first publication of the new folder.
        let mut published = wait_for(&mut link, |p| p.change.is_some());
        let change = published.change.take().unwrap();
        assert_eq!(change.songs.as_deref(), Some(songs.as_path()));
        assert_eq!(change.skins, [format!("skin of {}", songs.display())]);
        link.retire(change);
        if published.status != SongSelectStatus::Ready {
            published = wait_for(&mut link, |p| p.status == SongSelectStatus::Ready);
            assert!(published.change.is_none());
        }
        assert_eq!(published.list.sets[0].artist, "Picked");
        assert_eq!(&*published.folder, songs.display().to_string());
        assert_eq!(started_rx.try_recv().unwrap(), None);
        assert_eq!(saved_rx.try_recv().unwrap(), songs);

        // The next picker starts at the folder being listed.
        assert!(link.pick_folder());
        let deadline = Instant::now() + Duration::from_secs(10);
        let start = loop {
            if let Ok(start) = started_rx.try_recv() {
                break start;
            }
            assert!(Instant::now() < deadline, "the picker was not opened again");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(start, Some(songs));
    }

    /// Folders whose picker answers `pick` at once and whose saves are counted.
    fn answering(pick: FolderPick, saves: Arc<std::sync::atomic::AtomicUsize>) -> Folders {
        Folders {
            open: Box::new(move |_, done| done(pick.clone())),
            resolve: Path::to_path_buf,
            save: Box::new(move |_| {
                saves.fetch_add(1, Ordering::Relaxed);
            }),
            skins: no_skins(),
        }
    }

    fn dropdown(options: usize) -> DropdownView {
        let names: Vec<String> = (0..options).map(|i| i.to_string()).collect();
        DropdownView::new(
            Arc::new(slam_text::FontRegistry::with_builtin()),
            slam_ui::theme::Theme::default(),
            &names,
        )
    }

    fn skin_views() -> (
        rtrb::Producer<DropdownView>,
        SkinViews,
        rtrb::Consumer<DropdownView>,
    ) {
        let (producer, views) = rtrb::RingBuffer::new(SKIN_VIEWS);
        let (old, returned) = rtrb::RingBuffer::new(SKIN_VIEWS);
        (producer, SkinViews { views, old }, returned)
    }

    #[test]
    fn skin_views_take_the_newest_and_give_back_the_older() {
        let (mut producer, mut views, mut returned) = skin_views();
        assert!(views.take().is_none());
        for options in [1, 2, 3] {
            producer.push(dropdown(options)).ok().unwrap();
        }
        let newest = views.take().unwrap();
        assert!(format!("{newest:?}").contains("options: 3"));
        for options in ["options: 1", "options: 2"] {
            let old = returned.pop().unwrap();
            assert!(format!("{old:?}").contains(options));
        }
        assert!(returned.pop().is_err());
        assert!(views.take().is_none());
        views.give_back(newest);
        assert!(returned.pop().is_ok());
    }

    #[test]
    fn latest_keeps_a_change_of_an_older_publication() {
        let (mut producer, mut link, mut commands) = link_with_commands();
        let change = |songs: &str| {
            Some(Box::new(FolderChange {
                songs: Some(PathBuf::from(songs)),
                skins: Vec::new(),
            }))
        };
        let mut first = publish(&[], SongSelectStatus::Scanning { sets: 0 }, Arc::from(""));
        first.change = change("/a");
        producer.push(first).ok().unwrap();
        producer
            .push(publish(&[], SongSelectStatus::Ready, Arc::from("")))
            .ok()
            .unwrap();
        let latest = link.latest().unwrap();
        assert_eq!(latest.status, SongSelectStatus::Ready);
        assert_eq!(latest.change, change("/a"));

        // Of two changes the newer wins; the older goes back to be freed.
        let mut first = publish(&[], SongSelectStatus::Ready, Arc::from(""));
        first.change = change("/a");
        let mut second = publish(&[], SongSelectStatus::Ready, Arc::from(""));
        second.change = change("/b");
        producer.push(first).ok().unwrap();
        producer.push(second).ok().unwrap();
        assert_eq!(link.latest().unwrap().change, change("/b"));
        assert_eq!(
            commands.pop().ok(),
            Some(Command::Retire(change("/a").unwrap()))
        );
    }

    #[test]
    fn a_cancelled_failed_or_relative_pick_keeps_the_folder() {
        for pick in [
            FolderPick::Cancelled,
            FolderPick::Failed("no picker".to_owned()),
            FolderPick::Chosen(PathBuf::new()),
            FolderPick::Chosen(PathBuf::from("relative")),
        ] {
            let saves = Arc::default();
            let mut library = Library::new(None, answering(pick.clone(), Arc::clone(&saves)));
            library.pick_folder();
            library.receive_pick();
            assert!(library.picking.is_none(), "{pick:?}");
            assert_eq!(library.status, SongSelectStatus::NoFolder, "{pick:?}");
            assert!(library.root.is_none(), "{pick:?}");
            assert_eq!(saves.load(Ordering::Relaxed), 0, "{pick:?}");
        }
    }

    #[test]
    fn a_new_folder_retires_the_old_folder_text() {
        let saves = Arc::default();
        let picked = std::env::temp_dir().join("slam-app-songs-retire-missing");
        let mut library = Library::new(
            Some(PathBuf::from("/old")),
            answering(FolderPick::Chosen(picked.clone()), saves),
        );
        let old = Arc::downgrade(&library.folder_text);
        library.pick_folder();
        library.receive_pick();
        assert_eq!(&*library.folder_text, picked.display().to_string());
        assert!(
            old.upgrade().is_some(),
            "kept while the update thread may hold it"
        );
        library.retired_folders.retain(|t| Arc::strong_count(t) > 1);
        assert!(old.upgrade().is_none());
    }

    #[test]
    fn a_second_request_while_the_picker_is_open_is_ignored() {
        let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&opened);
        let folders = Folders {
            // Never answers, like a dialog still open.
            open: Box::new(move |_, done| {
                count.fetch_add(1, Ordering::Relaxed);
                std::mem::forget(done);
            }),
            resolve: Path::to_path_buf,
            save: Box::new(|_| {}),
            skins: no_skins(),
        };
        let mut library = Library::new(None, folders);
        library.pick_folder();
        library.pick_folder();
        assert_eq!(opened.load(Ordering::Relaxed), 1);
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
            write_set(&root.0, dir, artist);
        }
        let (mut link, _views, thread) = spawn(
            Some(root.0.clone()),
            no_folders(),
            Box::new(|n| format!("{n} sets")),
        )
        .unwrap();
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
        let (mut link, _views, _thread) = spawn(
            Some(missing),
            no_folders(),
            Box::new(|n| format!("{n} sets")),
        )
        .unwrap();
        let published = wait_for(&mut link, |p| {
            p.status != SongSelectStatus::Scanning { sets: 0 }
        });
        assert_eq!(published.status, SongSelectStatus::Failed);
        assert!(published.list.sets.is_empty());
    }
}
