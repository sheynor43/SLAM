//! Writing a file so that no reader ever sees it half-written.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The temporary file `write` fills before renaming it over `path`: `path` with `.tmp`
/// appended, in the same directory so the rename stays on one file system.
pub fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Writes `bytes` to `path` atomically: into a temporary file next to it, flushed to disk,
/// then renamed over `path`. A reader sees either the old file (or none) or the whole new
/// one. On failure the temporary file is removed.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = temporary_path(path);
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("slam-atomic-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_the_whole_file_and_leaves_no_temporary() {
        let dir = scratch("write");
        let path = dir.join("a.osr");
        write(&path, b"first").unwrap();
        write(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["a.osr"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_leaves_no_temporary() {
        let dir = scratch("fail");
        // Renaming a file over a non-empty directory fails.
        let path = dir.join("taken");
        std::fs::create_dir_all(path.join("inside")).unwrap();
        assert!(write(&path, b"bytes").is_err());
        assert!(!temporary_path(&path).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_temporary_sits_next_to_the_file() {
        assert_eq!(
            temporary_path(Path::new("dir/map (1).osr")),
            Path::new("dir/map (1).osr.tmp")
        );
    }
}
