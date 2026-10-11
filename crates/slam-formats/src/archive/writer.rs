//! Writing zip archives.

use std::collections::HashSet;
use std::io::{Seek, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, System, ZipWriter};

use super::error::ArchiveError;
use super::name::{self, Rejected};

/// Builds a zip archive from in-memory files.
pub struct ArchiveWriter<W: Write + Seek> {
    zip: ZipWriter<W>,
    names: HashSet<String>,
}

impl<W: Write + Seek> std::fmt::Debug for ArchiveWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchiveWriter")
            .field("files", &self.names.len())
            .finish_non_exhaustive()
    }
}

impl<W: Write + Seek> ArchiveWriter<W> {
    /// Starts an archive in `writer`.
    pub fn new(writer: W) -> Self {
        Self {
            zip: ZipWriter::new(writer),
            names: HashSet::new(),
        }
    }

    /// Adds a deflated file. The name is normalized like the reader does; names the reader
    /// would drop (unsafe, empty, directory, system files) and case-insensitive duplicates
    /// are rejected. The content is stored byte for byte.
    pub fn add_file(&mut self, name: &str, data: &[u8]) -> Result<(), ArchiveError> {
        let invalid = |reason| ArchiveError::InvalidName {
            name: name.to_owned(),
            reason,
        };
        if name::is_dir_name(name) {
            return Err(invalid("directory"));
        }
        let normalized = name::normalize(name).map_err(|r| match r {
            Rejected::Unsafe => invalid("absolute path, `:`, `..`, control character, reserved device name or trailing `.`/space"),
            Rejected::Empty => invalid("empty name"),
        })?;
        if name::is_system_file(&normalized) {
            return Err(invalid("system file"));
        }
        let key = name::key(&normalized);
        if self.names.contains(&key) {
            return Err(ArchiveError::DuplicateName(name.to_owned()));
        }
        // Fixed system, permissions, time and level keep the output independent of the host.
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(6))
            .last_modified_time(DateTime::default())
            .system(System::Unix)
            .unix_permissions(0o644);
        self.zip
            .start_file(normalized, options)
            .map_err(ArchiveError::from_zip)?;
        self.names.insert(key);
        self.zip
            .write_all(data)
            .map_err(|e| ArchiveError::Io(e.to_string()))
    }

    /// Writes the central directory and returns the underlying writer.
    pub fn finish(self) -> Result<W, ArchiveError> {
        self.zip.finish().map_err(ArchiveError::from_zip)
    }
}
