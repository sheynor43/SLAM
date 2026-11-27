//! Saved tablet presets (ADR-0176): snapshots of a profile's behaviour (output mode, areas,
//! relative settings, filters), kept in SLAM's data folder, not in OTD's files. The file is
//! read and written on short-lived threads, never on the update thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One saved preset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    /// The profile subtree from `Settings::profile_preset`.
    pub profile: Value,
}

#[derive(Serialize, Deserialize)]
struct File {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    presets: Vec<Preset>,
}

/// Where the presets file lives under the data folder.
pub fn file_in(data_dir: &Path) -> PathBuf {
    data_dir.join("otd-presets.json")
}

/// Reads the presets; a missing or broken file is an empty list (a broken one is logged).
pub fn read(path: &Path) -> Vec<Preset> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("OTD presets {}: {e}", path.display());
            }
            return Vec::new();
        }
    };
    match serde_json::from_slice::<File>(&bytes) {
        Ok(file) => file.presets,
        Err(e) => {
            tracing::warn!("OTD presets {}: {e}", path.display());
            Vec::new()
        }
    }
}

/// Writes the presets atomically.
pub fn write(path: &Path, presets: &[Preset]) -> std::io::Result<()> {
    let file = File {
        version: 1,
        presets: presets.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::atomic_file::write(path, &bytes)
}

/// Reads the file on a new thread; the list comes over the returned channel.
pub fn read_in_background(path: PathBuf) -> Receiver<Vec<Preset>> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new()
        .name("otd-presets-read".into())
        .spawn(move || {
            let _ = tx.send(read(&path));
        });
    if let Err(e) = spawned {
        tracing::warn!("OTD presets: {e}");
    }
    rx
}

/// Writes the file on a new thread (the list is moved in).
pub fn write_in_background(path: PathBuf, presets: Vec<Preset>) {
    let spawned = std::thread::Builder::new()
        .name("otd-presets-write".into())
        .spawn(move || {
            if let Err(e) = write(&path, &presets) {
                tracing::warn!("OTD presets {}: {e}", path.display());
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("OTD presets: {e}");
    }
}

/// The first "Preset N" not taken by a name in `presets`.
pub fn auto_name(presets: &[Preset]) -> String {
    (1..)
        .map(|n| format!("Preset {n}"))
        .find(|name| presets.iter().all(|p| &p.name != name))
        .unwrap_or_default()
}
