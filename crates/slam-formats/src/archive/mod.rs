//! Generic zip archive reader and writer shared by `.osz` and `.osk`.
//!
//! [`ArchiveReader`] lists the files of an archive and reads them into memory;
//! [`ArchiveWriter`] builds an archive from in-memory files. Both treat the archive as
//! untrusted input.
//!
//! Names are decoded from the raw bytes of the zip header rather than by the `zip` crate
//! (which reads names without the UTF-8 flag as CP437). A name that is valid UTF-8 (including
//! pure ASCII) is used as is; otherwise it is decoded as Shift-JIS (CP932), the encoding of
//! stable. The `zip` crate does not expose the UTF-8 flag, but the rule covers a flagged name
//! as well, except that a flagged name with invalid UTF-8 is decoded as CP932 instead of
//! lossily. If CP932 has errors too, the name is decoded lossily with a [`Warning`].
//!
//! Names are normalized: `\` becomes `/`, empty and `.` components are removed. Entries with
//! a `..` component, an absolute path (leading `/` or `\`), a `:` in any component (drive
//! prefix, NTFS stream), a control character, a reserved Windows device name (`CON`, `NUL`,
//! `COM1`, ... with or without extension) or a component ending in `.` or a space (Windows
//! strips those) are rejected with a warning, as are an empty name and case-insensitive
//! duplicates of an earlier entry. Normalized names are therefore safe to join onto a base
//! directory on Windows and Unix. Directories (also `.` and `./`), `__MACOSX` content,
//! `.DS_Store`, `Thumbs.db` and `desktop.ini` are skipped silently. Lookup with
//! [`ArchiveReader::find`] normalizes the query the same way and ignores case.
//!
//! Limits: at most [`MAX_ENTRIES`] entries ([`ArchiveError::TooManyEntries`]) and at most
//! [`MAX_ENTRY_SIZE`] bytes per file, counted on the bytes actually produced by decompression
//! rather than on the header ([`ArchiveError::EntryTooLarge`]). At most [`MAX_WARNINGS`]
//! warnings are stored, the rest is counted in [`ArchiveReader::suppressed_warnings`].
//!
//! If several entries have byte-identical names, the `zip` crate keeps only the last one's
//! data (at the first one's position); the reader reports the lost entries in one
//! [`WarningKind::CollapsedDuplicates`] warning with an empty name.
//!
//! The entry count is read from the end records before the `zip` crate allocates for it. This
//! is stricter than the crate: the end record must lie in the last 65 557 bytes of the input
//! (the crate accepts arbitrary trailing data) and a zip64 end record must be exactly where
//! its locator points; otherwise the archive is [`ArchiveError::InvalidArchive`].
//!
//! The writer deflates every file (level 6, Unix system, mode 0644), writes names as UTF-8 (the `zip` crate sets the UTF-8 flag
//! for non-ASCII names) and uses a fixed modification time, so equal input gives equal bytes
//! on every host. Byte identity holds for the same versions of `zip` and `zlib-rs`.
//!
//! Deviations from osu!lazer: `ZipArchiveReader` decodes every name without the UTF-8 flag as
//! CP932; here a name that is valid UTF-8 stays UTF-8 (ADR-0030). A CP932 name that happens to be
//! valid UTF-8 (e.g. pairs of half-width katakana) is therefore read as UTF-8. Lazer skips
//! files whose whole path contains `__MACOSX`, `.DS_Store` or `Thumbs.db` as a case-insensitive
//! substring and does not skip `desktop.ini`; here a path component must match exactly, and
//! `desktop.ini` is skipped too. Lazer's
//! `LegacyBeatmapExporter` writes CP932 names and rewrites `.osu` files; the writer here
//! stores UTF-8 names and keeps file content byte for byte. Encrypted entries and compression
//! methods other than stored, deflate and deflate64 are not supported.

mod error;
mod name;
mod reader;
mod writer;

pub use error::{ArchiveError, MAX_WARNINGS, Warning, WarningKind};
pub use reader::{ArchiveReader, Entry, MAX_ENTRIES, MAX_ENTRY_SIZE};
pub use writer::ArchiveWriter;
