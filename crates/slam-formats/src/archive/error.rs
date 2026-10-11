//! Errors and warnings of the archive layer.

use thiserror::Error;

/// A fatal problem while reading or writing an archive.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ArchiveError {
    /// The input is not a zip archive or its central directory is corrupt.
    #[error("not a valid zip archive: {0}")]
    InvalidArchive(String),
    /// The data of an entry is corrupt (checksum mismatch, bad compressed stream, truncation).
    #[error("corrupt data in entry {name}: {message}")]
    CorruptEntry {
        /// Normalized name of the entry.
        name: String,
        /// What went wrong.
        message: String,
    },
    /// The entry is encrypted or uses an unsupported compression method.
    #[error("unsupported entry {name}: {reason}")]
    Unsupported {
        /// Normalized name of the entry.
        name: String,
        /// Why the entry cannot be read.
        reason: String,
    },
    /// The central directory lists more than [`MAX_ENTRIES`](super::MAX_ENTRIES) entries.
    #[error("archive has {count} entries, the limit is {limit}")]
    TooManyEntries {
        /// Number of entries in the archive.
        count: usize,
        /// The limit that was exceeded.
        limit: usize,
    },
    /// An entry is larger than [`MAX_ENTRY_SIZE`](super::MAX_ENTRY_SIZE) when decompressed.
    #[error("entry {name} exceeds the {limit} byte limit")]
    EntryTooLarge {
        /// Normalized name of the entry.
        name: String,
        /// The limit that was exceeded.
        limit: u64,
    },
    /// An I/O error of the underlying reader or writer.
    #[error("I/O error: {0}")]
    Io(String),
    /// The entry passed to a read does not belong to this reader.
    #[error("entry {0} does not belong to this archive")]
    ForeignEntry(String),
    /// The writer was given a name that cannot be stored.
    #[error("invalid entry name {name:?}: {reason}")]
    InvalidName {
        /// The rejected name.
        name: String,
        /// Why it was rejected.
        reason: &'static str,
    },
    /// The writer was given a name that already exists (compared case-insensitively).
    #[error("duplicate entry name {0:?}")]
    DuplicateName(String),
}

impl ArchiveError {
    /// Maps an error of the `zip` crate that is not tied to one entry.
    pub(super) fn from_zip(e: zip::result::ZipError) -> Self {
        match e {
            zip::result::ZipError::Io(e) => Self::Io(e.to_string()),
            other => Self::InvalidArchive(other.to_string()),
        }
    }
}

/// Kind of a non-fatal problem found while opening an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// A name was neither valid UTF-8 nor valid CP932 and was decoded lossily.
    UndecodableName,
    /// An entry with an unsafe path was dropped: absolute, `..`, `:` in a component, control
    /// character, reserved Windows device name or a component ending in `.` or a space.
    UnsafePath,
    /// An entry with an empty name was dropped.
    EmptyName,
    /// An entry whose name differs from an earlier one only by case was dropped.
    DuplicateName,
    /// Entries with byte-identical names were collapsed by the `zip` crate, which keeps one
    /// of them. `count` is the number of lost entries; the warning's name is empty.
    CollapsedDuplicates {
        /// Number of entries that were lost.
        count: u64,
    },
}

/// A non-fatal problem found while opening an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// What went wrong.
    pub kind: WarningKind,
    /// The (decoded, not normalized) entry name.
    pub name: String,
}

/// Maximum number of warnings stored; further ones are only counted.
pub const MAX_WARNINGS: usize = 100;

/// Collects warnings up to [`MAX_WARNINGS`] and counts the rest.
#[derive(Debug, Default)]
pub(super) struct WarningSink {
    pub(super) list: Vec<Warning>,
    pub(super) suppressed: usize,
}

impl WarningSink {
    pub(super) fn push(&mut self, kind: WarningKind, name: &str) {
        if self.list.len() < MAX_WARNINGS {
            self.list.push(Warning {
                kind,
                name: name.to_owned(),
            });
        } else {
            self.suppressed += 1;
        }
    }
}
