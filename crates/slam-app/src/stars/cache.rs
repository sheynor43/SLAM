//! The on-disk cache of star ratings, `cache/stars.bin` (ADR-0134).
//!
//! Key: an FNV-1a hash of a file's path, size and modification time, so an edited file
//! gets a new entry. The header holds a format version and the calculator version
//! ([`slam_osu::stars::CALCULATOR_VERSION`]): a cache of another version is ignored whole.
//! Layout, little-endian: `"SLST"`, `u32` format, `u16` length and bytes of the calculator
//! version, `u32` entry count, then per entry a `u64` key and the `f32` bits of the rating.
//! A damaged or truncated file is ignored and rebuilt: nothing here is worth an error.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::thumbnails::make::{fnv, fnv_start};

const MAGIC: &[u8; 4] = b"SLST";
const FORMAT: u32 = 1;
const ENTRY: usize = 12;

/// The key of the file at `path` as it is now; `None` when it cannot be read.
pub fn file_key(path: &Path) -> Option<u64> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64);
    Some(key_of(path, meta.len(), modified))
}

/// The key of a file by its path, size and modification time in nanoseconds.
pub fn key_of(path: &Path, size: u64, modified_ns: u64) -> u64 {
    let mut hash = fnv(fnv_start(), path.as_os_str().as_encoded_bytes());
    hash = fnv(hash, &[0]);
    hash = fnv(hash, &size.to_le_bytes());
    fnv(hash, &modified_ns.to_le_bytes())
}

/// Ratings by file key, for one calculator version.
#[derive(Debug, Default)]
pub struct StarCache {
    entries: HashMap<u64, f32>,
    dirty: bool,
}

impl StarCache {
    /// Reads the cache at `path` for `version`; empty when the file is missing, damaged
    /// or made by another version.
    pub fn load(path: &Path, version: &str) -> StarCache {
        let entries = fs::read(path)
            .ok()
            .and_then(|bytes| decode(&bytes, version))
            .unwrap_or_default();
        StarCache {
            entries,
            dirty: false,
        }
    }

    pub fn get(&self, key: u64) -> Option<f32> {
        self.entries.get(&key).copied()
    }

    pub fn insert(&mut self, key: u64, stars: f32) {
        if self.entries.insert(key, stars) != Some(stars) {
            self.dirty = true;
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether there is anything to save.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Writes the cache to `path` through a temporary file, so a crash leaves the old one.
    pub fn save(&mut self, path: &Path, version: &str) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("bin.tmp");
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&encode(&self.entries, version))?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        self.dirty = false;
        Ok(())
    }
}

fn encode(entries: &HashMap<u64, f32>, version: &str) -> Vec<u8> {
    let version = &version.as_bytes()[..version.len().min(usize::from(u16::MAX))];
    let mut out = Vec::with_capacity(16 + version.len() + entries.len() * ENTRY);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&(version.len() as u16).to_le_bytes());
    out.extend_from_slice(version);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    // Sorted: the same cache is the same bytes.
    let mut sorted: Vec<_> = entries.iter().collect();
    sorted.sort_unstable_by_key(|(key, _)| **key);
    for (key, stars) in sorted {
        out.extend_from_slice(&key.to_le_bytes());
        out.extend_from_slice(&stars.to_bits().to_le_bytes());
    }
    out
}

fn decode(bytes: &[u8], version: &str) -> Option<HashMap<u64, f32>> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (format, rest) = rest.split_first_chunk::<4>()?;
    if u32::from_le_bytes(*format) != FORMAT {
        return None;
    }
    let (len, rest) = rest.split_first_chunk::<2>()?;
    let (found, rest) = rest.split_at_checked(usize::from(u16::from_le_bytes(*len)))?;
    if found != version.as_bytes() {
        return None;
    }
    let (count, body) = rest.split_first_chunk::<4>()?;
    let count = u32::from_le_bytes(*count) as usize;
    if body.len() != count.checked_mul(ENTRY)? {
        return None;
    }
    let mut entries = HashMap::with_capacity(count);
    let (chunks, _) = body.as_chunks::<ENTRY>();
    for entry in chunks {
        let key = u64::from_le_bytes(entry[..8].try_into().ok()?);
        let stars = f32::from_bits(u32::from_le_bytes(entry[8..].try_into().ok()?));
        if !stars.is_finite() {
            return None;
        }
        entries.insert(key, stars);
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("slam-stars-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("stars.bin")
    }

    #[test]
    fn round_trips() {
        let path = tmp("round");
        let mut cache = StarCache::default();
        cache.insert(1, 4.5);
        cache.insert(u64::MAX, 0.0);
        assert!(cache.is_dirty());
        cache.save(&path, "v1").unwrap();
        assert!(!cache.is_dirty());
        let loaded = StarCache::load(&path, "v1");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get(1), Some(4.5));
        assert_eq!(loaded.get(u64::MAX), Some(0.0));
        assert!(!path.with_extension("bin.tmp").exists());
    }

    #[test]
    fn another_calculator_version_is_ignored() {
        let path = tmp("version");
        let mut cache = StarCache::default();
        cache.insert(1, 4.5);
        cache.save(&path, "v1").unwrap();
        assert!(StarCache::load(&path, "v2").is_empty());
    }

    #[test]
    fn damage_is_ignored() {
        let path = tmp("damage");
        let mut cache = StarCache::default();
        cache.insert(1, 4.5);
        cache.insert(2, 5.5);
        cache.save(&path, "v1").unwrap();
        let good = fs::read(&path).unwrap();
        let mut bad: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"garbage".to_vec(),
            good[..good.len() - 1].to_vec(),
            [good.clone(), vec![0]].concat(),
        ];
        let mut wrong_format = good.clone();
        wrong_format[4] = 9;
        bad.push(wrong_format);
        let mut nan = good;
        let at = nan.len() - 4;
        nan[at..].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
        bad.push(nan);
        for bytes in bad {
            fs::write(&path, bytes).unwrap();
            assert!(StarCache::load(&path, "v1").is_empty());
        }
        assert!(StarCache::load(&path.with_file_name("missing.bin"), "v1").is_empty());
    }

    #[test]
    fn keys_change_with_path_size_and_time() {
        let a = key_of(Path::new("/s/a.osu"), 10, 5);
        assert_eq!(a, key_of(Path::new("/s/a.osu"), 10, 5));
        assert_ne!(a, key_of(Path::new("/s/b.osu"), 10, 5));
        assert_ne!(a, key_of(Path::new("/s/a.osu"), 11, 5));
        assert_ne!(a, key_of(Path::new("/s/a.osu"), 10, 6));
    }
}
