//! The store of HUD layouts: built-in presets plus user layouts in `<data>/hud-layouts/`.
//!
//! A user layout is `<root>/<id>/layout.json` with a `sprites/` folder next to it
//! (ADR-0109). Everything here is cold-path file work. Not wired into the settings UI yet;
//! layout selection is a follow-up.

pub mod migrate;

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use slam_ui::hud_layout::{self, Builtin, HudLayout, LAYOUT_VERSION, LayoutError};

use crate::settings::Dirs;

const LAYOUT_FILE: &str = "layout.json";
const SPRITES_DIR: &str = "sprites";
const MAX_ID_LEN: usize = 64;
const MAX_SLUG_LEN: usize = 48;

/// A layout id: `builtin:*` or `[a-z0-9_-]{1,64}`. The latter has no `:` and no separators,
/// so it never collides with a built-in and never leaves the store root.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LayoutId(String);

impl LayoutId {
    pub fn parse(s: &str) -> Option<LayoutId> {
        if Builtin::from_id(s).is_some() {
            return Some(LayoutId(s.to_owned()));
        }
        let ok = !s.is_empty()
            && s.len() <= MAX_ID_LEN
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        ok.then(|| LayoutId(s.to_owned()))
    }

    pub fn is_builtin(&self) -> bool {
        Builtin::from_id(&self.0).is_some()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn builtin(&self) -> Option<Builtin> {
        Builtin::from_id(&self.0)
    }
}

impl fmt::Display for LayoutId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a store operation failed.
#[derive(Debug)]
pub enum StoreError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Layout(LayoutError),
    NotFound(LayoutId),
    ReadOnly(LayoutId),
    InvalidId,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Layout(e) => write!(f, "{e}"),
            Self::NotFound(id) => write!(f, "layout `{id}` not found"),
            Self::ReadOnly(id) => write!(f, "layout `{id}` is built in and read-only"),
            Self::InvalidId => write!(f, "invalid layout id"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Layout(e) => Some(e),
            _ => None,
        }
    }
}

impl From<LayoutError> for StoreError {
    fn from(e: LayoutError) -> Self {
        Self::Layout(e)
    }
}

type Result<T> = std::result::Result<T, StoreError>;

fn io<T>(path: &Path, r: std::io::Result<T>) -> Result<T> {
    r.map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// One row of [`LayoutStore::list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutEntry {
    pub id: LayoutId,
    pub name: String,
    pub builtin: bool,
}

/// The layouts folder.
#[derive(Debug, Clone)]
pub struct LayoutStore {
    root: PathBuf,
}

/// The id for a new layout named `name`: lowercase ASCII alphanumerics, other runs become
/// one `-`, trimmed, at most 48 chars, `layout` when nothing is left.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(MAX_SLUG_LEN);
    let out = out.trim_end_matches('-');
    if out.is_empty() {
        "layout".to_owned()
    } else {
        out.to_owned()
    }
}

impl LayoutStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn in_data_dir(dirs: &Dirs) -> Self {
        Self::new(dirs.data.join("hud-layouts"))
    }

    fn dir(&self, id: &LayoutId) -> PathBuf {
        self.root.join(id.as_str())
    }

    fn file(&self, id: &LayoutId) -> PathBuf {
        self.dir(id).join(LAYOUT_FILE)
    }

    /// Builtins first, then user layouts by name (case-insensitive) and id.
    pub fn list(&self) -> Vec<LayoutEntry> {
        let mut out: Vec<LayoutEntry> = Builtin::ALL
            .iter()
            .map(|b| LayoutEntry {
                id: LayoutId(b.id().to_owned()),
                name: b.layout().name,
                builtin: true,
            })
            .collect();
        let Ok(read) = std::fs::read_dir(&self.root) else {
            return out;
        };
        let mut users = Vec::new();
        for entry in read.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let file_name = entry.file_name();
            let Some(id) = file_name.to_str().and_then(LayoutId::parse) else {
                tracing::warn!(dir = %entry.path().display(), "skipping layout folder with an invalid id");
                continue;
            };
            if id.is_builtin() {
                tracing::warn!(dir = %entry.path().display(), "skipping layout folder with a built-in id");
                continue;
            }
            match self.read_user(&id) {
                Ok(layout) => users.push(LayoutEntry {
                    id,
                    name: layout.name,
                    builtin: false,
                }),
                Err(e) => tracing::warn!(%id, "skipping unreadable layout: {e}"),
            }
        }
        users.sort_by_cached_key(|e| (e.name.to_lowercase(), e.id.0.clone()));
        out.extend(users);
        out
    }

    fn read_user(&self, id: &LayoutId) -> Result<HudLayout> {
        let path = self.file(id);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(hud_layout::from_json(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(StoreError::NotFound(id.clone()))
            }
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    pub fn load(&self, id: &LayoutId) -> Result<HudLayout> {
        match id.builtin() {
            Some(b) => Ok(b.layout()),
            None => self.read_user(id),
        }
    }

    pub fn save(&self, id: &LayoutId, layout: &HudLayout) -> Result<()> {
        if id.is_builtin() {
            return Err(StoreError::ReadOnly(id.clone()));
        }
        if !self.dir(id).is_dir() {
            return Err(StoreError::NotFound(id.clone()));
        }
        let mut layout = layout.clone();
        layout.version = LAYOUT_VERSION;
        write_atomic(&self.file(id), hud_layout::to_json(&layout).as_bytes())
    }

    /// Copies any layout, built-in included, into a new user layout named `new_name`.
    pub fn duplicate(&self, id: &LayoutId, new_name: &str) -> Result<LayoutId> {
        let mut layout = self.load(id)?;
        layout.name = new_name.to_owned();
        layout.version = LAYOUT_VERSION;
        let base = slug(new_name);
        io(&self.root, std::fs::create_dir_all(&self.root))?;
        // `create_dir` is atomic, so the first free id is claimed without a race.
        let (new_id, dir) = (1u32..)
            .find_map(|n| {
                let candidate = if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                };
                let dir = self.root.join(&candidate);
                match std::fs::create_dir(&dir) {
                    Ok(()) => Some(Ok((candidate, dir))),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(source) => Some(Err(StoreError::Io { path: dir, source })),
                }
            })
            .expect("an unbounded range always yields")?;
        let new_id = LayoutId(new_id);
        let result = self.fill_copy(id, &dir, &layout);
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        result.map(|()| new_id)
    }

    fn fill_copy(&self, from: &LayoutId, dir: &Path, layout: &HudLayout) -> Result<()> {
        let sprites = dir.join(SPRITES_DIR);
        io(&sprites, std::fs::create_dir(&sprites))?;
        if let Some(src) = self.sprites_dir(from)
            && let Ok(read) = std::fs::read_dir(&src)
        {
            for entry in read.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_file()) {
                    let to = sprites.join(entry.file_name());
                    io(&to, std::fs::copy(entry.path(), &to))?;
                }
            }
        }
        write_atomic(
            &dir.join(LAYOUT_FILE),
            hud_layout::to_json(layout).as_bytes(),
        )
    }

    /// Changes the display name; the id stays.
    pub fn rename(&self, id: &LayoutId, new_name: &str) -> Result<()> {
        if id.is_builtin() {
            return Err(StoreError::ReadOnly(id.clone()));
        }
        let mut layout = self.read_user(id)?;
        layout.name = new_name.to_owned();
        self.save(id, &layout)
    }

    pub fn delete(&self, id: &LayoutId) -> Result<()> {
        if id.is_builtin() {
            return Err(StoreError::ReadOnly(id.clone()));
        }
        let dir = self.dir(id);
        if !dir.is_dir() {
            return Err(StoreError::NotFound(id.clone()));
        }
        io(&dir, std::fs::remove_dir_all(&dir))
    }

    /// The sprites folder of a user layout; `None` for built-ins.
    pub fn sprites_dir(&self, id: &LayoutId) -> Option<PathBuf> {
        (!id.is_builtin()).then(|| self.dir(id).join(SPRITES_DIR))
    }
}

/// Writes through a temp file in the same folder and renames it over `path`.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    io(path, written)
}

#[cfg(test)]
mod tests;
