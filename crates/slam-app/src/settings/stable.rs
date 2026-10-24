//! Finding the songs folder of an osu!stable installation (ADR-0064).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::dirs::{Os, absolute_env};

/// The file system queries the lookup needs; a fake in tests.
pub trait Fs {
    /// Whether `path` is an existing file.
    fn is_file(&self, path: &Path) -> bool;
    /// Whether `path` is an existing directory.
    fn is_dir(&self, path: &Path) -> bool;
    /// The whole file as text, `None` when it cannot be read.
    fn read_to_string(&self, path: &Path) -> Option<String>;
    /// Full paths of the entries of a directory, in any order.
    fn list_dir(&self, path: &Path) -> Vec<PathBuf>;
}

/// The real file system.
pub struct RealFs;

impl Fs for RealFs {
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read_to_string(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn list_dir(&self, path: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(path)
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default()
    }
}

/// Whether `folder` looks like an osu!stable installation.
fn is_install(folder: &Path, fs: &dyn Fs) -> bool {
    fs.is_file(&folder.join("osu!.exe")) || fs.is_dir(&folder.join("Songs"))
}

/// `BeatmapDirectory` from the first `osu!.<user>.cfg` of the installation, if any.
fn beatmap_directory(folder: &Path, fs: &dyn Fs) -> Option<PathBuf> {
    const PREFIX: &str = "osu!.";
    const SUFFIX: &str = ".cfg";
    let mut configs: Vec<PathBuf> = fs
        .list_dir(folder)
        .into_iter()
        .filter(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.len() > PREFIX.len() + SUFFIX.len()
                && name.starts_with(PREFIX)
                && name.ends_with(SUFFIX)
                && fs.is_file(path)
        })
        .collect();
    configs.sort();
    let text = fs.read_to_string(configs.first()?)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        let value = value.trim();
        (key.trim().eq_ignore_ascii_case("BeatmapDirectory") && !value.is_empty())
            .then(|| PathBuf::from(value))
    })
}

/// The songs folder for a folder the player chose: an installation resolves to its configured
/// or default `Songs`, anything else is taken as is.
pub fn songs_of(folder: &Path, fs: &dyn Fs) -> PathBuf {
    if !is_install(folder, fs) {
        return folder.to_path_buf();
    }
    if let Some(dir) = beatmap_directory(folder, fs) {
        let dir = if dir.is_absolute() {
            dir
        } else {
            folder.join(dir)
        };
        if fs.is_dir(&dir) {
            return dir;
        }
    }
    let songs = folder.join("Songs");
    if fs.is_dir(&songs) {
        songs
    } else {
        folder.to_path_buf()
    }
}

/// Candidate installation folders, most likely first.
fn candidates(env: &dyn Fn(&str) -> Option<OsString>, fs: &dyn Fs, os: Os) -> Vec<PathBuf> {
    match os {
        Os::Windows => env("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(|base| vec![PathBuf::from(base).join("osu!")])
            .unwrap_or_default(),
        Os::Unix => {
            let home = absolute_env(env, "HOME");
            let mut list = Vec::new();
            let data = absolute_env(env, "XDG_DATA_HOME")
                .or_else(|| home.as_ref().map(|h| h.join(".local").join("share")));
            if let Some(data) = data {
                list.push(data.join("osu-wine").join("osu!"));
                list.push(data.join("osu-wine").join("OSU"));
            }
            let prefixes = absolute_env(env, "WINEPREFIX")
                .into_iter()
                .chain(home.map(|h| h.join(".wine")));
            for prefix in prefixes {
                let mut users = fs.list_dir(&prefix.join("drive_c").join("users"));
                users.sort();
                for user in users {
                    if user.file_name().is_some_and(|n| n == "Public") || !fs.is_dir(&user) {
                        continue;
                    }
                    list.push(user.join("AppData").join("Local").join("osu!"));
                }
            }
            list
        }
    }
}

/// Looks for an osu!stable installation in the usual places and returns its songs folder.
pub fn find_stable_songs(
    env: &dyn Fn(&str) -> Option<OsString>,
    fs: &dyn Fs,
    os: Os,
) -> Option<PathBuf> {
    candidates(env, fs, os).into_iter().find_map(|folder| {
        if !is_install(&folder, fs) {
            return None;
        }
        let songs = songs_of(&folder, fs);
        (songs != folder && fs.is_dir(&songs)).then_some(songs)
    })
}

/// [`find_stable_songs`] on the real environment and file system.
pub fn detect() -> Option<PathBuf> {
    find_stable_songs(&|name| std::env::var_os(name), &RealFs, Os::current())
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;

    #[derive(Default)]
    struct FakeFs {
        files: HashSet<PathBuf>,
        dirs: HashSet<PathBuf>,
        contents: HashMap<PathBuf, String>,
    }

    impl FakeFs {
        fn dir(mut self, path: &str) -> Self {
            let mut p = Some(Path::new(path));
            while let Some(d) = p {
                self.dirs.insert(d.to_path_buf());
                p = d.parent();
            }
            self
        }

        fn file(mut self, path: &str, text: &str) -> Self {
            let path = PathBuf::from(path);
            self = self.dir(path.parent().unwrap().to_str().unwrap());
            self.files.insert(path.clone());
            self.contents.insert(path, text.to_owned());
            self
        }
    }

    impl Fs for FakeFs {
        fn is_file(&self, path: &Path) -> bool {
            self.files.contains(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.dirs.contains(path)
        }

        fn read_to_string(&self, path: &Path) -> Option<String> {
            self.contents.get(path).cloned()
        }

        fn list_dir(&self, path: &Path) -> Vec<PathBuf> {
            self.files
                .iter()
                .chain(self.dirs.iter())
                .filter(|p| p.parent() == Some(path))
                .cloned()
                .collect()
        }
    }

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).map(OsString::from)
    }

    #[test]
    fn relative_beatmap_directory() {
        let fs = FakeFs::default()
            .file("/o/osu!.exe", "")
            .file("/o/osu!.alice.cfg", "Foo = 1\nBeatmapDirectory = Maps\n")
            .dir("/o/Maps")
            .dir("/o/Songs");
        assert_eq!(songs_of(Path::new("/o"), &fs), PathBuf::from("/o/Maps"));
    }

    #[test]
    fn absolute_beatmap_directory() {
        let fs = FakeFs::default()
            .file("/o/osu!.exe", "")
            .file("/o/osu!.alice.cfg", "beatmapdirectory=/data/songs\n")
            .dir("/data/songs");
        assert_eq!(songs_of(Path::new("/o"), &fs), PathBuf::from("/data/songs"));
    }

    #[test]
    fn bom_is_stripped() {
        let fs = FakeFs::default()
            .file("/o/osu!.alice.cfg", "\u{feff}BeatmapDirectory = Maps\r\n")
            .dir("/o/Maps")
            .dir("/o/Songs");
        assert_eq!(songs_of(Path::new("/o"), &fs), PathBuf::from("/o/Maps"));
    }

    #[test]
    fn missing_key_falls_back_to_songs() {
        let fs = FakeFs::default()
            .file("/o/osu!.exe", "")
            .file("/o/osu!.alice.cfg", "Foo = 1\n")
            .file("/o/osu!.cfg", "BeatmapDirectory = Wrong\n")
            .dir("/o/Songs")
            .dir("/o/Wrong");
        assert_eq!(songs_of(Path::new("/o"), &fs), PathBuf::from("/o/Songs"));
    }

    #[test]
    fn missing_configured_dir_falls_back_to_songs() {
        let fs = FakeFs::default()
            .file("/o/osu!.alice.cfg", "BeatmapDirectory = Gone\n")
            .dir("/o/Songs");
        assert_eq!(songs_of(Path::new("/o"), &fs), PathBuf::from("/o/Songs"));
    }

    #[test]
    fn songs_folder_itself_is_unchanged() {
        let fs = FakeFs::default()
            .dir("/o/Songs")
            .file("/o/Songs/a/a.osu", "");
        assert_eq!(
            songs_of(Path::new("/o/Songs"), &fs),
            PathBuf::from("/o/Songs")
        );
    }

    #[test]
    fn exe_without_songs_or_cfg_gives_folder() {
        let fs = FakeFs::default().file("/o/osu!.exe", "");
        assert_eq!(songs_of(Path::new("/o"), &fs), PathBuf::from("/o"));
    }

    #[test]
    fn windows_localappdata() {
        let fs = FakeFs::default().dir("/local/osu!/Songs");
        let env = env_of(&[("LOCALAPPDATA", "/local")]);
        assert_eq!(
            find_stable_songs(&env, &fs, Os::Windows),
            Some(PathBuf::from("/local/osu!/Songs"))
        );
    }

    #[test]
    fn unix_osu_winello() {
        let fs = FakeFs::default()
            .file("/h/.local/share/osu-wine/OSU/osu!.exe", "")
            .dir("/h/.local/share/osu-wine/OSU/Songs");
        let env = env_of(&[("HOME", "/h")]);
        assert_eq!(
            find_stable_songs(&env, &fs, Os::Unix),
            Some(PathBuf::from("/h/.local/share/osu-wine/OSU/Songs"))
        );
    }

    #[test]
    fn wine_prefix_scans_users() {
        let fs = FakeFs::default()
            .dir("/pfx/drive_c/users/Public")
            .dir("/pfx/drive_c/users/alice/AppData/Local/Other")
            .dir("/pfx/drive_c/users/bob/AppData/Local/osu!/Songs");
        let env = env_of(&[("HOME", "/h"), ("WINEPREFIX", "/pfx")]);
        assert_eq!(
            find_stable_songs(&env, &fs, Os::Unix),
            Some(PathBuf::from(
                "/pfx/drive_c/users/bob/AppData/Local/osu!/Songs"
            ))
        );
    }

    #[test]
    fn nothing_found() {
        let env = env_of(&[("HOME", "/h")]);
        assert_eq!(find_stable_songs(&env, &FakeFs::default(), Os::Unix), None);
        assert_eq!(
            find_stable_songs(&env, &FakeFs::default(), Os::Windows),
            None
        );
    }

    #[test]
    fn install_without_songs_dir_is_skipped() {
        let fs = FakeFs::default()
            .file("/h/.local/share/osu-wine/osu!/osu!.exe", "")
            .dir("/h/.wine/drive_c/users/me/AppData/Local/osu!/Songs");
        let env = env_of(&[("HOME", "/h")]);
        assert_eq!(
            find_stable_songs(&env, &fs, Os::Unix),
            Some(PathBuf::from(
                "/h/.wine/drive_c/users/me/AppData/Local/osu!/Songs"
            ))
        );
    }
}
