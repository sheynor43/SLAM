//! Reading zip archives.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

use zip::ZipArchive;
use zip::result::ZipError;

use super::error::{ArchiveError, Warning, WarningKind, WarningSink};
use super::name::{self, Rejected};

/// Maximum number of entries in an archive.
pub const MAX_ENTRIES: usize = 65_535;

/// Maximum size of a decompressed entry, as in osu!lazer.
pub const MAX_ENTRY_SIZE: u64 = 100 * 1024 * 1024;

/// Largest buffer reserved up front; larger entries grow while being read.
const MAX_PREALLOCATION: u64 = 1024 * 1024;

/// A file in an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Normalized name (`/` separators), in its original case.
    pub name: String,
    /// Uncompressed size declared in the header. Informational; not trusted.
    pub size: u64,
    index: usize,
}

/// Reads the files of a zip archive.
#[derive(Debug)]
pub struct ArchiveReader<R> {
    zip: ZipArchive<R>,
    entries: Vec<Entry>,
    lookup: HashMap<String, usize>,
    warnings: Vec<Warning>,
    suppressed: usize,
}

impl<R: Read + Seek> ArchiveReader<R> {
    /// Opens an archive, reads its central directory and filters the entries.
    pub fn new(reader: R) -> Result<Self, ArchiveError> {
        let mut reader = reader;
        let declared = declared_entry_count(&mut reader)?;
        check_entry_count(declared.max_count, MAX_ENTRIES as u64)?;
        let mut zip = ZipArchive::new(reader).map_err(open_error)?;
        check_entry_count(zip.len() as u64, MAX_ENTRIES as u64)?;
        let mut sink = WarningSink::default();
        // The zip crate keeps one entry per raw name (the last one's data), silently. Count
        // the lost ones when the crate used the same central directory as the pre-scan.
        if zip.central_directory_start() == declared.cd_offset {
            let collapsed = declared.count.saturating_sub(zip.len() as u64);
            if collapsed > 0 {
                sink.push(WarningKind::CollapsedDuplicates { count: collapsed }, "");
            }
        }
        let mut entries = Vec::new();
        let mut lookup = HashMap::new();
        for index in 0..zip.len() {
            let file = zip.by_index_raw(index).map_err(open_error)?;
            let raw = file.name_raw();
            let (decoded, lossy) = name::decode_name(raw);
            if lossy {
                sink.push(WarningKind::UndecodableName, &decoded);
            }
            if name::is_dir_name(&decoded) {
                continue;
            }
            let normalized = match name::normalize(&decoded) {
                Ok(n) => n,
                Err(Rejected::Unsafe) => {
                    sink.push(WarningKind::UnsafePath, &decoded);
                    continue;
                }
                Err(Rejected::Empty) => {
                    // `.` and `./` are skipped like directories; only a truly empty name warns.
                    if decoded.is_empty() {
                        sink.push(WarningKind::EmptyName, &decoded);
                    }
                    continue;
                }
            };
            if name::is_system_file(&normalized) {
                continue;
            }
            let key = name::key(&normalized);
            if lookup.contains_key(&key) {
                sink.push(WarningKind::DuplicateName, &decoded);
                continue;
            }
            lookup.insert(key, entries.len());
            entries.push(Entry {
                name: normalized,
                size: file.size(),
                index,
            });
        }
        Ok(Self {
            zip,
            entries,
            lookup,
            warnings: sink.list,
            suppressed: sink.suppressed,
        })
    }

    /// The files of the archive in archive order, after filtering.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Problems found while opening the archive.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Number of warnings dropped after the first [`MAX_WARNINGS`](super::MAX_WARNINGS).
    pub fn suppressed_warnings(&self) -> usize {
        self.suppressed
    }

    /// Finds an entry by name, ignoring case and path separator style.
    pub fn find(&self, name: &str) -> Option<&Entry> {
        let i = *self
            .lookup
            .get(&super::name::key(&super::name::clean(name)))?;
        self.entries.get(i)
    }

    /// Reads the file with the given name, or `None` if there is none.
    pub fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, ArchiveError> {
        let Some(entry) = self.find(name).cloned() else {
            return Ok(None);
        };
        self.read_entry(&entry).map(Some)
    }

    /// Reads an entry of this archive, failing if it is larger than [`MAX_ENTRY_SIZE`].
    ///
    /// The entry must come from this reader; an entry of another archive is rejected with
    /// [`ArchiveError::ForeignEntry`].
    pub fn read_entry(&mut self, entry: &Entry) -> Result<Vec<u8>, ArchiveError> {
        self.read_entry_limited(entry, MAX_ENTRY_SIZE)
    }

    fn read_entry_limited(&mut self, entry: &Entry, limit: u64) -> Result<Vec<u8>, ArchiveError> {
        let own = self
            .lookup
            .get(&name::key(&entry.name))
            .and_then(|&i| self.entries.get(i))
            .is_some_and(|e| e == entry);
        if !own {
            return Err(ArchiveError::ForeignEntry(entry.name.clone()));
        }
        let file = self
            .zip
            .by_index(entry.index)
            .map_err(|e| entry_error(&entry.name, e))?;
        read_limited(file, entry.size, limit, &entry.name)
    }
}

/// Fails if an archive has more than `max` entries.
fn check_entry_count(count: u64, max: u64) -> Result<(), ArchiveError> {
    if count > max {
        return Err(ArchiveError::TooManyEntries {
            count: usize::try_from(count).unwrap_or(usize::MAX),
            limit: usize::try_from(max).unwrap_or(usize::MAX),
        });
    }
    Ok(())
}

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const EOCD_SIZE: usize = 22;
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const ZIP64_LOCATOR_SIZE: u64 = 20;
const ZIP64_EOCD_SIGNATURE: u32 = 0x0606_4b50;
/// End record plus the longest possible comment.
const EOCD_SEARCH: u64 = EOCD_SIZE as u64 + u16::MAX as u64;

fn le_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn le_u64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        b.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

/// What the end records of an archive declare.
struct Declared {
    /// Entry count the `zip` crate will use: "entries on this disk" of the end record, or
    /// the total of the zip64 end record.
    count: u64,
    /// Offset of the central directory from the same record.
    cd_offset: u64,
    /// Largest count of any end record found in the tail, for the limit check.
    max_count: u64,
}

/// Reads the entry count of the central directory from the end record (and the zip64 end
/// record, if there is one) before the `zip` crate allocates for it.
///
/// Stricter than the `zip` crate: the end record must be in the last 65 557 bytes and a zip64
/// end record must be exactly where its locator points, otherwise the archive is invalid.
fn declared_entry_count<R: Read + Seek>(reader: &mut R) -> Result<Declared, ArchiveError> {
    let io = |e: std::io::Error| ArchiveError::Io(e.to_string());
    let len = reader.seek(SeekFrom::End(0)).map_err(io)?;
    let start = len.saturating_sub(EOCD_SEARCH);
    reader.seek(SeekFrom::Start(start)).map_err(io)?;
    let mut tail = Vec::new();
    reader
        .by_ref()
        .take(len - start)
        .read_to_end(&mut tail)
        .map_err(io)?;
    let invalid = |m: &str| ArchiveError::InvalidArchive(m.to_owned());
    let Some(last) = tail.len().checked_sub(EOCD_SIZE) else {
        return Err(invalid("no end of central directory record"));
    };
    let mut primary: Option<Declared> = None;
    let mut max_count = 0;
    for i in (0..=last).rev() {
        if le_u32(&tail, i) != Some(EOCD_SIGNATURE) {
            continue;
        }
        let comment = usize::from(le_u16(&tail, i + 20).unwrap_or(0));
        if i + EOCD_SIZE + comment > tail.len() {
            continue;
        }
        let info = eocd_info(reader, &tail, start, i);
        if primary.is_none() {
            // The nearest end record to the end of the file is the one that must be sound.
            let d = info?;
            max_count = d.max_count;
            primary = Some(d);
        } else if let Ok(d) = info {
            max_count = max_count.max(d.count).max(d.max_count);
        }
    }
    reader.seek(SeekFrom::Start(0)).map_err(io)?;
    let mut declared = primary.ok_or_else(|| invalid("no end of central directory record"))?;
    declared.max_count = max_count.max(declared.count);
    Ok(declared)
}

/// Parses the end record at `tail[i..]`, following the zip64 locator like the `zip` crate.
fn eocd_info<R: Read + Seek>(
    reader: &mut R,
    tail: &[u8],
    start: u64,
    i: usize,
) -> Result<Declared, ArchiveError> {
    let io = |e: std::io::Error| ArchiveError::Io(e.to_string());
    let field = |v: Option<u64>| v.unwrap_or(0);
    let this_disk = field(le_u16(tail, i + 8).map(u64::from));
    let total = field(le_u16(tail, i + 10).map(u64::from));
    let cd_size = field(le_u32(tail, i + 12).map(u64::from));
    let cd_offset = field(le_u32(tail, i + 16).map(u64::from));
    let zip32 = Declared {
        count: this_disk,
        cd_offset,
        max_count: this_disk.max(total),
    };
    let may_be_zip64 = total == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF;
    let eocd_pos = start + i as u64;
    let Some(locator_pos) = eocd_pos
        .checked_sub(ZIP64_LOCATOR_SIZE)
        .filter(|_| may_be_zip64)
    else {
        return Ok(zip32);
    };
    reader.seek(SeekFrom::Start(locator_pos)).map_err(io)?;
    let mut locator = [0u8; ZIP64_LOCATOR_SIZE as usize];
    if reader.read_exact(&mut locator).is_err()
        || le_u32(&locator, 0) != Some(ZIP64_LOCATOR_SIGNATURE)
    {
        // The zip crate falls back to the 32-bit record here.
        return Ok(zip32);
    }
    let bad = || {
        ArchiveError::InvalidArchive("zip64 end record is not at the locator's offset".to_owned())
    };
    let record_pos = le_u64(&locator, 8).ok_or_else(bad)?;
    reader.seek(SeekFrom::Start(record_pos)).map_err(io)?;
    let mut record = [0u8; 56];
    if reader.read_exact(&mut record).is_err() || le_u32(&record, 0) != Some(ZIP64_EOCD_SIGNATURE) {
        return Err(bad());
    }
    let count = le_u64(&record, 32).ok_or_else(bad)?;
    let cd_offset = le_u64(&record, 48).ok_or_else(bad)?;
    Ok(Declared {
        count,
        cd_offset,
        max_count: count.max(this_disk).max(total),
    })
}

/// Reads all of `reader`, failing if it yields more than `limit` bytes.
fn read_limited(
    reader: impl Read,
    declared: u64,
    limit: u64,
    name: &str,
) -> Result<Vec<u8>, ArchiveError> {
    let capacity = declared.min(limit).min(MAX_PREALLOCATION);
    let mut data = Vec::with_capacity(usize::try_from(capacity).unwrap_or(0));
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|e| ArchiveError::CorruptEntry {
            name: name.to_owned(),
            message: e.to_string(),
        })?;
    if data.len() as u64 > limit {
        return Err(ArchiveError::EntryTooLarge {
            name: name.to_owned(),
            limit,
        });
    }
    Ok(data)
}

fn open_error(e: ZipError) -> ArchiveError {
    ArchiveError::from_zip(e)
}

fn entry_error(name: &str, e: ZipError) -> ArchiveError {
    let name = name.to_owned();
    match e {
        ZipError::UnsupportedArchive(reason) => ArchiveError::Unsupported {
            name,
            reason: reason.to_owned(),
        },
        ZipError::CompressionMethodNotSupported(m) => ArchiveError::Unsupported {
            name,
            reason: format!("compression method {m}"),
        },
        ZipError::InvalidPassword => ArchiveError::Unsupported {
            name,
            reason: "encrypted".to_owned(),
        },
        other => ArchiveError::CorruptEntry {
            name,
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_is_checked_on_bytes_read() {
        let data = vec![7u8; 10];
        // The declared size lies (0), the real data is checked.
        assert_eq!(read_limited(&data[..], 0, 10, "a").unwrap(), data);
        assert_eq!(
            read_limited(&data[..], 0, 9, "a"),
            Err(ArchiveError::EntryTooLarge {
                name: "a".into(),
                limit: 9
            })
        );
        // A huge declared size does not preallocate.
        assert_eq!(read_limited(&data[..], u64::MAX, 100, "a").unwrap(), data);
    }

    #[test]
    fn entry_count_limit() {
        assert!(check_entry_count(MAX_ENTRIES as u64, MAX_ENTRIES as u64).is_ok());
        assert_eq!(
            check_entry_count(MAX_ENTRIES as u64 + 1, MAX_ENTRIES as u64),
            Err(ArchiveError::TooManyEntries {
                count: MAX_ENTRIES + 1,
                limit: MAX_ENTRIES
            })
        );
    }

    #[test]
    fn compression_bomb_is_stopped_by_the_limit() {
        use std::io::{Cursor, Write};
        use zip::write::SimpleFileOptions;
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        w.start_file("zeros.bin", options).unwrap();
        w.write_all(&[0u8; 4096]).unwrap();
        let bytes = w.finish().unwrap().into_inner();
        assert!(bytes.len() < 1024);
        let mut r = ArchiveReader::new(Cursor::new(bytes)).unwrap();
        let entry = r.entries()[0].clone();
        assert_eq!(
            r.read_entry_limited(&entry, 1024),
            Err(ArchiveError::EntryTooLarge {
                name: "zeros.bin".into(),
                limit: 1024
            })
        );
        assert_eq!(r.read_entry_limited(&entry, 4096).unwrap().len(), 4096);
    }
}
