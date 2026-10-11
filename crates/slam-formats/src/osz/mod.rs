//! Reading and writing of `.osz` beatmap archives.
//!
//! An `.osz` is a zip archive of `.osu` files and their assets; this is a thin layer over
//! [`crate::archive`], which documents name decoding, normalization, limits and deviations from
//! osu!lazer (UTF-8 names and byte-identical files on export instead of the CP932 names and
//! rewritten `.osu` files of `LegacyBeatmapExporter`).
//!
//! ```
//! use std::io::Cursor;
//! use slam_formats::osz::{write, OszReader};
//!
//! let osu = b"osu file format v14\n\n[General]\nAudioFilename: a.mp3\n";
//! let bytes = write(Cursor::new(Vec::new()), [("a.osu", &osu[..])]).unwrap().into_inner();
//! let mut osz = OszReader::new(Cursor::new(bytes)).unwrap();
//! assert_eq!(osz.beatmap_files().count(), 1);
//! assert!(osz.read_beatmap("A.OSU").unwrap().is_some());
//! ```

use std::io::{Read, Seek, Write};

use thiserror::Error;

use crate::archive::{ArchiveError, ArchiveReader, ArchiveWriter, Entry};
use crate::osu::{self, DecodeError};

/// A problem while reading a beatmap from an `.osz`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OszError {
    /// The archive could not be read.
    #[error(transparent)]
    Archive(#[from] ArchiveError),
    /// The `.osu` file inside could not be decoded.
    #[error("invalid beatmap {name}: {source}")]
    Beatmap {
        /// Normalized name of the `.osu` file.
        name: String,
        /// The decoding error.
        source: DecodeError,
    },
}

/// Reads an `.osz` archive.
#[derive(Debug)]
pub struct OszReader<R> {
    archive: ArchiveReader<R>,
}

impl<R: Read + Seek> OszReader<R> {
    /// Opens an `.osz` archive.
    pub fn new(reader: R) -> Result<Self, ArchiveError> {
        Ok(Self {
            archive: ArchiveReader::new(reader)?,
        })
    }

    /// The underlying archive.
    pub fn archive(&self) -> &ArchiveReader<R> {
        &self.archive
    }

    /// The underlying archive, for reading files.
    pub fn archive_mut(&mut self) -> &mut ArchiveReader<R> {
        &mut self.archive
    }

    /// The `.osu` files of the archive (extension compared case-insensitively).
    pub fn beatmap_files(&self) -> impl Iterator<Item = &Entry> {
        self.archive.entries().iter().filter(|e| {
            e.name
                .len()
                .checked_sub(4)
                .and_then(|i| e.name.as_bytes().get(i..))
                .is_some_and(|ext| ext.eq_ignore_ascii_case(b".osu"))
        })
    }

    /// Reads and decodes an `.osu` file, or `None` if there is no such file.
    pub fn read_beatmap(&mut self, name: &str) -> Result<Option<osu::Decoded>, OszError> {
        let Some(bytes) = self.archive.read(name)? else {
            return Ok(None);
        };
        let name = self
            .archive
            .find(name)
            .map_or_else(|| name.to_owned(), |e| e.name.clone());
        osu::decode(&bytes)
            .map(Some)
            .map_err(|source| OszError::Beatmap { name, source })
    }
}

/// Writes an `.osz` archive of the given `(name, content)` files and returns the writer.
pub fn write<'a, W: Write + Seek>(
    writer: W,
    files: impl IntoIterator<Item = (&'a str, &'a [u8])>,
) -> Result<W, ArchiveError> {
    let mut archive = ArchiveWriter::new(writer);
    for (name, data) in files {
        archive.add_file(name, data)?;
    }
    archive.finish()
}
