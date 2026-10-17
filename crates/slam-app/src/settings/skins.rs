//! The skins of the stable layout: folders in `Skins`, next to the songs folder (ADR-0051).

use std::path::{Path, PathBuf};

/// The `Skins` folder belonging to the songs folder `songs`.
pub fn skins_root(songs: &Path) -> Option<PathBuf> {
    songs.parent().map(|parent| parent.join("Skins"))
}

/// The folder of the skin `name`, if there is a songs folder to find `Skins` next to.
pub fn skin_path(songs: Option<&Path>, name: Option<&str>) -> Option<PathBuf> {
    Some(skins_root(songs?)?.join(name?))
}

/// The skins next to `songs`: name and folder of each subfolder of `Skins`, sorted by name
/// without regard to case. A missing or unreadable `Skins` folder gives none; files and names
/// that are not UTF-8 are skipped.
pub fn skin_dirs(songs: &Path) -> Vec<(String, PathBuf)> {
    let Some(root) = skins_root(songs) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut skins: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()) || entry.path().is_dir())
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .collect();
    skins.sort_by_cached_key(|(name, _)| (name.to_lowercase(), name.clone()));
    skins
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "slam-skins-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_subfolders_sorted_without_case_and_skips_files() {
        let root = temp_dir("list");
        let songs = root.join("Songs");
        std::fs::create_dir_all(&songs).unwrap();
        let skins = root.join("Skins");
        for name in ["beta", "Alpha", "gamma", "Delta Skin"] {
            std::fs::create_dir_all(skins.join(name)).unwrap();
        }
        std::fs::write(skins.join("readme.txt"), "x").unwrap();
        let found = skin_dirs(&songs);
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Alpha", "beta", "Delta Skin", "gamma"]);
        assert_eq!(found[0].1, skins.join("Alpha"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_skins_folder_gives_no_skins() {
        let root = temp_dir("missing");
        assert!(skin_dirs(&root.join("Songs")).is_empty());
        assert!(skin_dirs(Path::new("/")).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_skin_path_needs_a_songs_folder_and_a_name() {
        let songs = Path::new("/games/osu/Songs");
        assert_eq!(
            skin_path(Some(songs), Some("Cool")),
            Some(PathBuf::from("/games/osu/Skins/Cool"))
        );
        assert_eq!(skin_path(None, Some("Cool")), None);
        assert_eq!(skin_path(Some(songs), None), None);
    }
}
