//! Where the settings and the user data live.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The platform family that decides the directory layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// `%APPDATA%\SLAM`.
    Windows,
    /// XDG directories.
    Unix,
}

impl Os {
    /// The OS this build runs on.
    pub fn current() -> Os {
        if cfg!(windows) { Os::Windows } else { Os::Unix }
    }
}

/// The resolved directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// Holds `slam.toml`.
    pub config: PathBuf,
    /// Holds the user data.
    pub data: PathBuf,
    /// Both are next to the executable (a `portable` file is there).
    pub portable: bool,
}

/// Name of the marker file that makes an installation portable.
pub const PORTABLE_MARKER: &str = "portable";

/// An absolute, non-empty path from the environment variable `name`, per the XDG rule that
/// empty and relative values are ignored.
fn absolute_env(env: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    let value = env(name)?;
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

/// Resolves the directories from the given environment; touches nothing itself.
pub fn resolve(
    env: &dyn Fn(&str) -> Option<OsString>,
    exe_dir: Option<&Path>,
    exists: &dyn Fn(&Path) -> bool,
    os: Os,
) -> Option<Dirs> {
    if let Some(dir) = exe_dir
        && exists(&dir.join(PORTABLE_MARKER))
    {
        return Some(Dirs {
            config: dir.to_path_buf(),
            data: dir.to_path_buf(),
            portable: true,
        });
    }
    match os {
        Os::Windows => {
            let base = env("APPDATA").filter(|v| !v.is_empty())?;
            let dir = PathBuf::from(base).join("SLAM");
            Some(Dirs {
                config: dir.clone(),
                data: dir,
                portable: false,
            })
        }
        Os::Unix => {
            let home = absolute_env(env, "HOME");
            let config = absolute_env(env, "XDG_CONFIG_HOME")
                .or_else(|| home.as_ref().map(|h| h.join(".config")))?;
            let data = absolute_env(env, "XDG_DATA_HOME")
                .or_else(|| home.as_ref().map(|h| h.join(".local").join("share")))?;
            Some(Dirs {
                config: config.join("slam"),
                data: data.join("slam"),
                portable: false,
            })
        }
    }
}

impl Dirs {
    /// Resolves from the real environment, executable location and file system.
    pub fn detect() -> Option<Dirs> {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        resolve(
            &|name| std::env::var_os(name),
            exe_dir.as_deref(),
            &|path| path.exists(),
            Os::current(),
        )
    }

    /// The settings file.
    pub fn settings_file(&self) -> PathBuf {
        self.config.join(super::FILE_NAME)
    }

    /// Where replays are saved (ARCHITECTURE §17).
    pub fn replays(&self) -> PathBuf {
        self.data.join("replays")
    }
}
