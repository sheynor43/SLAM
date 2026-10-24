//! The default skin: embedded in the binary and unpacked to the cache on first use, since
//! skins are read from files (ADR-0068).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::settings::Dirs;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/default_skin.rs"));
}

/// The folder with the unpacked default skin; unpacks it on the first call in the process.
pub fn dir() -> Result<PathBuf, String> {
    static DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    DIR.get_or_init(|| {
        let root = cache_root();
        unpack(&root, embedded::FILES, embedded::HASH)
            .map_err(|e| format!("default skin {}: {e}", root.display()))
    })
    .clone()
}

/// Where the skin versions live: `<data>/cache/default-skin`, or the OS temporary folder when
/// there is no data folder.
fn cache_root() -> PathBuf {
    Dirs::detect().map_or_else(
        || std::env::temp_dir().join("slam-default-skin"),
        |dirs| dirs.data.join("cache").join("default-skin"),
    )
}

/// Unpacks `files` into `root/<hash>` unless that folder is already there, then removes the
/// other versions. The folder appears complete or not at all: the files are written into a
/// temporary folder that is renamed at the end.
pub(crate) fn unpack(root: &Path, files: &[(&str, &[u8])], hash: u64) -> io::Result<PathBuf> {
    let name = format!("{hash:016x}");
    let target = root.join(&name);
    if !target.is_dir() {
        std::fs::create_dir_all(root)?;
        let temp = root.join(format!("{name}.tmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let written = write_all(&temp, files).and_then(|()| std::fs::rename(&temp, &target));
        if let Err(e) = written {
            let _ = std::fs::remove_dir_all(&temp);
            // Another process may have unpacked the same version first.
            if !target.is_dir() {
                return Err(e);
            }
        }
    }
    remove_stale(root, &name);
    Ok(target)
}

fn write_all(dir: &Path, files: &[(&str, &[u8])]) -> io::Result<()> {
    std::fs::create_dir(dir)?;
    for (name, bytes) in files {
        std::fs::write(dir.join(name), bytes)?;
    }
    Ok(())
}

/// Removes the folders of other versions and leftover temporary folders. Only names this
/// module creates are touched; failures (a file in use on Windows) are left for next time.
fn remove_stale(root: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name != keep && is_version_name(name) && entry.path().is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// `<16 hex digits>` or `<16 hex digits>.tmp-<pid>`.
fn is_version_name(name: &str) -> bool {
    let (hash, rest) = name.split_at(name.len().min(16));
    let hex = hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    hex && (rest.is_empty()
        || rest
            .strip_prefix(".tmp-")
            .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())))
}

#[cfg(test)]
mod tests;
