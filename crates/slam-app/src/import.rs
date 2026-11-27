//! Importing files dropped on the window (ADR-0169): `.osz` and `.olz` beatmap sets are
//! extracted into `Songs`, `.osk` skins into `Skins` next to it, `.osr` replays are copied
//! into SLAM's replay folder. The work runs on its own thread; the update thread only hands
//! it paths and polls for the outcome. A file is deleted once its import succeeded.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use slam_formats::archive::{ArchiveError, ArchiveReader};

use crate::settings::skins::skins_root;

/// What a dropped file is, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `.osz` (stable) or `.olz` (lazer): a beatmap set.
    Beatmap,
    /// `.osk`: a skin.
    Skin,
    /// `.osr`: a replay.
    Replay,
}

/// The kind of file `path` is, `None` for an extension that is not imported.
pub fn classify(path: &Path) -> Option<Kind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "osz" | "olz" => Some(Kind::Beatmap),
        "osk" => Some(Kind::Skin),
        "osr" => Some(Kind::Replay),
        _ => None,
    }
}

/// Why an import failed.
#[derive(Debug)]
pub enum ImportError {
    Unsupported,
    NoFolder,
    /// The archive has no files.
    Empty,
    /// An entry name that would leave the target folder.
    UnsafeName(String),
    Archive(ArchiveError),
    Io(io::Error),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Unsupported => write!(f, "unsupported file type"),
            ImportError::NoFolder => write!(f, "no folder to import into"),
            ImportError::Empty => write!(f, "the archive has no files"),
            ImportError::UnsafeName(name) => write!(f, "unsafe name in the archive: {name}"),
            ImportError::Archive(e) => write!(f, "archive: {e}"),
            ImportError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<ArchiveError> for ImportError {
    fn from(e: ArchiveError) -> Self {
        ImportError::Archive(e)
    }
}

impl From<io::Error> for ImportError {
    fn from(e: io::Error) -> Self {
        ImportError::Io(e)
    }
}

/// Where imports go. `None` where the folder is not known.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Targets {
    /// The songs folder; the skins go to the `Skins` folder next to it.
    pub songs: Option<PathBuf>,
    pub replays: Option<PathBuf>,
}

/// The outcome of one dropped file.
#[derive(Debug)]
pub struct Report {
    pub path: PathBuf,
    pub result: Result<Kind, ImportError>,
}

/// A file to import, with where it goes.
#[derive(Debug)]
pub struct Job {
    pub path: PathBuf,
    pub targets: Targets,
}

/// Imports the file at `path`; on success the file is deleted, on failure it stays.
pub fn import(path: &Path, targets: &Targets) -> Result<Kind, ImportError> {
    let kind = classify(path).ok_or(ImportError::Unsupported)?;
    match kind {
        Kind::Beatmap => {
            let songs = targets.songs.as_deref().ok_or(ImportError::NoFolder)?;
            extract(path, songs)?;
        }
        Kind::Skin => {
            let skins = targets
                .songs
                .as_deref()
                .and_then(skins_root)
                .ok_or(ImportError::NoFolder)?;
            extract(path, &skins)?;
        }
        Kind::Replay => {
            let replays = targets.replays.as_deref().ok_or(ImportError::NoFolder)?;
            copy_unique(path, replays)?;
        }
    }
    if let Err(e) = fs::remove_file(path) {
        tracing::warn!("import: cannot delete {}: {e}", path.display());
    }
    Ok(kind)
}

/// The name of the folder an archive extracts into: its file name without the extension.
fn stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("import")
        .to_owned()
}

/// Creates `parent/<stem>`, or `parent/<stem> (2)`, `(3)`, ... when that exists. The
/// creation itself decides, so a concurrent import cannot take the same folder.
fn create_unique_dir(parent: &Path, stem: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(parent)?;
    for n in 1..=10_000u32 {
        let name = if n == 1 {
            stem.to_owned()
        } else {
            format!("{stem} ({n})")
        };
        let dir = parent.join(name);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free folder name",
    ))
}

/// Extracts the zip `archive` into a new folder in `parent`; returns the folder. The names
/// come from [`ArchiveReader`] (ADR-0030), which drops unsafe ones; each is checked again
/// before it is joined onto the folder. A failed extraction removes the folder.
fn extract(archive: &Path, parent: &Path) -> Result<PathBuf, ImportError> {
    let mut reader = ArchiveReader::new(BufReader::new(File::open(archive)?))?;
    let entries = reader.entries().to_vec();
    if entries.is_empty() {
        return Err(ImportError::Empty);
    }
    for entry in &entries {
        let safe = Path::new(&entry.name)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
        if !safe {
            return Err(ImportError::UnsafeName(entry.name.clone()));
        }
    }
    let dir = create_unique_dir(parent, &stem_of(archive))?;
    let result = (|| -> Result<(), ImportError> {
        for entry in &entries {
            let dest = dir.join(&entry.name);
            if let Some(folder) = dest.parent() {
                fs::create_dir_all(folder)?;
            }
            fs::write(&dest, reader.read_entry(entry)?)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(dir),
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// Copies `file` into `dir`, under its own name or `<stem> (2).<ext>` when that exists.
fn copy_unique(file: &Path, dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let stem = stem_of(file);
    let extension = file.extension().and_then(|e| e.to_str()).unwrap_or("osr");
    for n in 1..=10_000u32 {
        let name = if n == 1 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem} ({n}).{extension}")
        };
        let dest = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&dest) {
            Ok(mut out) => {
                let copied = File::open(file).and_then(|mut src| io::copy(&mut src, &mut out));
                if let Err(e) = copied {
                    drop(out);
                    let _ = fs::remove_file(&dest);
                    return Err(e);
                }
                return Ok(dest);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free file name",
    ))
}

/// The toast entries, by [`slam_ui::toast::ToastFrame::variant`]: Fluent keys of the title
/// and of the hint line.
pub fn toast_keys() -> &'static [(&'static str, Option<&'static str>)] {
    &[
        ("toast-replay-saved", Some("toast-replay-saved-hint")),
        ("import-beatmap-imported", None),
        ("import-skin-imported", None),
        ("import-replay-imported", None),
        ("import-failed", None),
        ("import-unsupported", None),
        ("settings-row-pin", Some("settings-row-pin-hint")),
        ("settings-row-unpin", None),
        ("settings-row-pin-full", None),
        // The self-updater's (ADR-0185), `shell::updates::TOAST_*`.
        (
            "update-toast-available",
            Some("update-toast-available-hint"),
        ),
        ("update-toast-ready", Some("update-toast-ready-hint")),
    ]
}

/// The toast variants of the toolbar's pins (ADR-0168), after the import ones.
pub const TOAST_PINNED: u8 = 6;
pub const TOAST_UNPINNED: u8 = 7;
pub const TOAST_PIN_FULL: u8 = 8;

/// The toast variant telling the outcome of an import.
pub fn variant(result: &Result<Kind, ImportError>) -> u8 {
    match result {
        Ok(Kind::Beatmap) => 1,
        Ok(Kind::Skin) => 2,
        Ok(Kind::Replay) => 3,
        Err(ImportError::Unsupported) => 5,
        Err(_) => 4,
    }
}

/// The import thread's handle: jobs in, reports out. The thread ends when this is dropped.
pub struct Importer {
    jobs: Sender<Job>,
    reports: Receiver<Report>,
}

impl Importer {
    /// Starts the thread, named `slam-import`.
    pub fn spawn() -> io::Result<Importer> {
        let (jobs, job_rx) = channel::<Job>();
        let (report_tx, reports) = channel();
        std::thread::Builder::new()
            .name("slam-import".to_owned())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    let result = import(&job.path, &job.targets);
                    match &result {
                        Ok(kind) => tracing::info!("import: {kind:?} {}", job.path.display()),
                        Err(e) => tracing::warn!("import: {}: {e}", job.path.display()),
                    }
                    let report = Report {
                        path: job.path,
                        result,
                    };
                    if report_tx.send(report).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Importer { jobs, reports })
    }

    /// Queues a file; false when the thread is gone.
    pub fn submit(&self, job: Job) -> bool {
        self.jobs.send(job).is_ok()
    }

    /// The next finished import, if any. Does not allocate when there is none.
    pub fn poll(&self) -> Option<Report> {
        self.reports.try_recv().ok()
    }
}

/// The paths of files dropped on the window: the window thread's callback sends, the update
/// thread polls.
pub struct Drops(Receiver<PathBuf>);

/// A drop queue and the callback that fills it, for
/// [`slam_input::InputWindow::set_drop_callback`].
pub fn drop_queue() -> (Drops, Box<dyn FnMut(PathBuf)>) {
    let (tx, rx) = channel();
    (
        Drops(rx),
        Box::new(move |path| {
            // The receiver is gone only when the shell is.
            let _ = tx.send(path);
        }),
    )
}

impl Drops {
    /// The next dropped path, if any. Does not allocate when there is none.
    pub fn poll(&self) -> Option<PathBuf> {
        self.0.try_recv().ok()
    }
}

/// The importer with the drop queue: what the shell holds.
pub struct Imports {
    pub drops: Drops,
    pub importer: Importer,
    /// Where dropped replays go, found once at start: the update tick touches no files.
    pub replays: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir =
                std::env::temp_dir().join(format!("slam-import-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        !crc
    }

    /// A zip of stored entries with the names as given, without any name checks.
    fn raw_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let offset = out.len() as u32;
            let mut common = Vec::new();
            common.extend(20u16.to_le_bytes()); // version needed
            common.extend(0u16.to_le_bytes()); // flags
            common.extend(0u16.to_le_bytes()); // stored
            common.extend(0u16.to_le_bytes()); // time
            common.extend(0x21u16.to_le_bytes()); // date
            common.extend(crc32(data).to_le_bytes());
            common.extend((data.len() as u32).to_le_bytes());
            common.extend((data.len() as u32).to_le_bytes());
            common.extend((name.len() as u16).to_le_bytes());
            common.extend(0u16.to_le_bytes()); // extra
            out.extend(b"PK\x03\x04");
            out.extend(&common);
            out.extend(name.as_bytes());
            out.extend(*data);
            central.extend(b"PK\x01\x02");
            central.extend(20u16.to_le_bytes()); // made by
            central.extend(&common);
            central.extend(0u16.to_le_bytes()); // comment
            central.extend(0u16.to_le_bytes()); // disk
            central.extend(0u16.to_le_bytes()); // internal attrs
            central.extend(0u32.to_le_bytes()); // external attrs
            central.extend(offset.to_le_bytes());
            central.extend(name.as_bytes());
        }
        let start = out.len() as u32;
        out.extend(&central);
        out.extend(b"PK\x05\x06");
        out.extend([0u8; 4]);
        out.extend((entries.len() as u16).to_le_bytes());
        out.extend((entries.len() as u16).to_le_bytes());
        out.extend((central.len() as u32).to_le_bytes());
        out.extend(start.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }

    fn targets(root: &Path) -> Targets {
        Targets {
            songs: Some(root.join("osu").join("Songs")),
            replays: Some(root.join("replays")),
        }
    }

    #[test]
    fn extensions_pick_the_kind() {
        assert_eq!(classify(Path::new("a.osz")), Some(Kind::Beatmap));
        assert_eq!(classify(Path::new("a.OLZ")), Some(Kind::Beatmap));
        assert_eq!(classify(Path::new("/x/a b.osk")), Some(Kind::Skin));
        assert_eq!(classify(Path::new("a.Osr")), Some(Kind::Replay));
        assert_eq!(classify(Path::new("a.zip")), None);
        assert_eq!(classify(Path::new("osz")), None);
    }

    #[test]
    fn a_beatmap_is_extracted_and_its_archive_deleted() {
        let temp = Temp::new("osz");
        let archive = temp.0.join("123 Artist - Title.osz");
        fs::write(&archive, raw_zip(&[("a.osu", b"x"), ("sub/b.wav", b"yy")])).unwrap();
        let t = targets(&temp.0);
        assert!(matches!(import(&archive, &t), Ok(Kind::Beatmap)));
        let dir = t.songs.unwrap().join("123 Artist - Title");
        assert_eq!(fs::read(dir.join("a.osu")).unwrap(), b"x");
        assert_eq!(fs::read(dir.join("sub/b.wav")).unwrap(), b"yy");
        assert!(!archive.exists());
    }

    #[test]
    fn a_taken_folder_name_gets_a_suffix() {
        let temp = Temp::new("collide");
        let t = targets(&temp.0);
        let songs = t.songs.clone().unwrap();
        for _ in 0..3 {
            let archive = temp.0.join("set.olz");
            fs::write(&archive, raw_zip(&[("a.osu", b"x")])).unwrap();
            import(&archive, &t).unwrap();
        }
        assert!(songs.join("set/a.osu").exists());
        assert!(songs.join("set (2)/a.osu").exists());
        assert!(songs.join("set (3)/a.osu").exists());
    }

    #[test]
    fn a_skin_goes_to_skins_next_to_songs() {
        let temp = Temp::new("osk");
        let archive = temp.0.join("Cool.osk");
        fs::write(&archive, raw_zip(&[("skin.ini", b"[General]")])).unwrap();
        let t = targets(&temp.0);
        assert!(matches!(import(&archive, &t), Ok(Kind::Skin)));
        assert!(temp.0.join("osu/Skins/Cool/skin.ini").exists());
    }

    #[test]
    fn a_replay_is_copied_without_overwriting() {
        let temp = Temp::new("osr");
        let t = targets(&temp.0);
        let replays = t.replays.clone().unwrap();
        for _ in 0..2 {
            let file = temp.0.join("r.osr");
            fs::write(&file, b"replay").unwrap();
            assert!(matches!(import(&file, &t), Ok(Kind::Replay)));
            assert!(!file.exists());
        }
        assert_eq!(fs::read(replays.join("r.osr")).unwrap(), b"replay");
        assert_eq!(fs::read(replays.join("r (2).osr")).unwrap(), b"replay");
    }

    #[test]
    fn traversing_names_never_leave_the_folder() {
        let temp = Temp::new("slip");
        let archive = temp.0.join("evil.osz");
        let entries: [(&str, &[u8]); 3] = [
            ("../evil.txt", b"no"),
            ("..\\evil2.txt", b"no"),
            ("ok.osu", b"y"),
        ];
        fs::write(&archive, raw_zip(&entries)).unwrap();
        let t = targets(&temp.0);
        let _ = import(&archive, &t);
        let songs = t.songs.unwrap();
        assert!(!songs.join("evil.txt").exists());
        assert!(!songs.parent().unwrap().join("evil.txt").exists());
        assert!(!songs.parent().unwrap().join("evil2.txt").exists());
        assert!(!temp.0.join("evil.txt").exists());
        // Whatever the reader dropped, only the safe file may be written.
        if let Ok(data) = fs::read(songs.join("evil/ok.osu")) {
            assert_eq!(data, b"y");
        }
    }

    #[test]
    fn a_failed_import_keeps_the_source() {
        let temp = Temp::new("fail");
        let t = targets(&temp.0);
        let bad = temp.0.join("bad.osz");
        fs::write(&bad, b"not a zip").unwrap();
        assert!(import(&bad, &t).is_err());
        assert!(bad.exists());
        // Nothing is left half extracted.
        assert!(!t.songs.clone().unwrap().join("bad").exists());
        let empty = temp.0.join("empty.osz");
        fs::write(&empty, raw_zip(&[])).unwrap();
        assert!(matches!(import(&empty, &t), Err(ImportError::Empty)));
        assert!(empty.exists());
        let other = temp.0.join("notes.txt");
        fs::write(&other, b"x").unwrap();
        assert!(matches!(import(&other, &t), Err(ImportError::Unsupported)));
        assert!(other.exists());
        let map = temp.0.join("m.osz");
        fs::write(&map, raw_zip(&[("a.osu", b"x")])).unwrap();
        assert!(matches!(
            import(&map, &Targets::default()),
            Err(ImportError::NoFolder)
        ));
        assert!(map.exists());
    }

    #[test]
    fn the_thread_reports_each_file() {
        let temp = Temp::new("thread");
        let importer = Importer::spawn().unwrap();
        let archive = temp.0.join("t.osz");
        fs::write(&archive, raw_zip(&[("a.osu", b"x")])).unwrap();
        assert!(importer.submit(Job {
            path: archive,
            targets: targets(&temp.0),
        }));
        let report = loop {
            if let Some(report) = importer.poll() {
                break report;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(matches!(report.result, Ok(Kind::Beatmap)));
        assert_eq!(variant(&report.result), 1);
    }

    #[test]
    fn every_variant_has_toast_keys() {
        assert_eq!(toast_keys().len(), 11);
        assert_eq!(usize::from(TOAST_PIN_FULL), 8);
        assert_eq!(
            usize::from(crate::shell::TOAST_UPDATE_READY),
            toast_keys().len() - 1
        );
        assert_eq!(variant(&Err(ImportError::Unsupported)), 5);
        assert_eq!(variant(&Err(ImportError::Empty)), 4);
    }
}
