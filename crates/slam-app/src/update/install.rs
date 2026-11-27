//! Putting a verified download in place (ADR-0185). Linux replaces the running AppImage
//! atomically; Windows renames the running `slam.exe` to `slam.exe.old` (a running file can be
//! renamed but not overwritten), unpacks the zip over its folder, and deletes the `.old` file
//! at the next start. All of it runs on the update thread.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

/// What gets replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The AppImage file this process runs from.
    AppImage(PathBuf),
    /// The executable of a portable folder.
    Portable(PathBuf),
}

impl Target {
    /// The file to start again after the install.
    pub fn executable(&self) -> &Path {
        match self {
            Target::AppImage(p) | Target::Portable(p) => p,
        }
    }

    /// The install this process can update, `None` for a build run from `cargo run`, an
    /// unpacked package or another system.
    pub fn detect() -> Option<Target> {
        if cfg!(target_os = "linux") {
            let path = PathBuf::from(std::env::var_os("APPIMAGE")?);
            return path.is_file().then_some(Target::AppImage(path));
        }
        if cfg!(windows) {
            return std::env::current_exe().ok().map(Target::Portable);
        }
        None
    }
}

/// Puts the verified download `new` in place of `target`.
pub fn install(new: &Path, target: &Target) -> io::Result<()> {
    match target {
        Target::AppImage(path) => replace_file(new, path),
        Target::Portable(exe) => install_zip(new, exe),
    }
}

fn sibling(path: &Path, prefix: &str, suffix: &str) -> PathBuf {
    let mut name = OsString::from(prefix);
    name.push(path.file_name().unwrap_or_default());
    name.push(suffix);
    path.with_file_name(name)
}

/// Replaces `target` with a copy of `new`: the copy is written next to it, flushed to disk
/// and renamed over it, so a crash leaves the old or the new file, never half of one.
pub fn replace_file(new: &Path, target: &Path) -> io::Result<()> {
    let staged = sibling(target, ".", ".new");
    let result = stage(new, &staged).and_then(|()| fs::rename(&staged, target));
    if result.is_err() {
        let _ = fs::remove_file(&staged);
        return result;
    }
    // The rename itself reaches the disk with the folder.
    #[cfg(unix)]
    if let Some(dir) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        File::open(dir)?.sync_all()?;
    }
    Ok(())
}

fn stage(new: &Path, staged: &Path) -> io::Result<()> {
    let mut source = File::open(new)?;
    let mut out = File::create(staged)?;
    io::copy(&mut source, &mut out)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        out.set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    out.flush()?;
    out.sync_all()
}

/// `slam.exe` renamed to make room for the new one.
pub fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Deletes the `.old` executable a previous update left. Nothing to do when there is none.
pub fn clean_old(exe: &Path) {
    let old = old_path(exe);
    if old.exists()
        && let Err(e) = fs::remove_file(&old)
    {
        tracing::warn!("update: cannot delete {}: {e}", old.display());
    }
}

/// Unpacks the release zip, whose files sit in one top folder, over the folder of `exe`. The
/// running `exe` is renamed to `.old` first; when the unpacking fails it is put back.
pub fn install_zip(zip_path: &Path, exe: &Path) -> io::Result<()> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let mut archive = zip::ZipArchive::new(io::BufReader::new(File::open(zip_path)?))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    // Names are checked before anything is touched.
    let mut files = Vec::new();
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if entry.is_dir() {
            continue;
        }
        let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
        let relative = unpack_name(&name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsafe name in the archive: {name}"),
            )
        })?;
        files.push((i, relative));
    }
    if files.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty archive"));
    }
    let old = old_path(exe);
    if old.exists() {
        fs::remove_file(&old)?;
    }
    fs::rename(exe, &old)?;
    let unpacked = (|| {
        for (i, relative) in &files {
            let mut entry = archive
                .by_index(*i)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            let path = dir.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&path)?;
            io::copy(&mut entry, &mut out)?;
            out.sync_all()?;
        }
        Ok(())
    })();
    if unpacked.is_err() {
        // Best effort: a half-unpacked folder still needs an executable.
        let _ = fs::remove_file(exe);
        let _ = fs::rename(&old, exe);
    }
    unpacked
}

/// The path inside the install folder of the zip entry `name`: without the top folder;
/// `None` for a name that would leave the folder.
fn unpack_name(name: &str) -> Option<PathBuf> {
    let mut parts = Path::new(name).components();
    let mut relative = PathBuf::new();
    let mut seen = 0;
    for part in &mut parts {
        match part {
            Component::Normal(p) => {
                seen += 1;
                // The first one is the release's folder.
                if seen > 1 {
                    relative.push(p);
                }
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (seen > 1).then_some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir =
                std::env::temp_dir().join(format!("slam-install-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn zip_of(path: &Path, entries: &[(&str, &[u8])]) {
        use zip::write::SimpleFileOptions;
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, data) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn replacing_a_file_swaps_the_content_and_leaves_no_stage() {
        let t = Temp::new("replace");
        let (new, target) = (t.0.join("new"), t.0.join("SLAM.AppImage"));
        fs::write(&new, b"new build").unwrap();
        fs::write(&target, b"old build").unwrap();
        replace_file(&new, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new build");
        assert!(!t.0.join(".SLAM.AppImage.new").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111);
        }
    }

    #[test]
    fn a_failed_replace_keeps_the_old_file() {
        let t = Temp::new("failed");
        let target = t.0.join("SLAM.AppImage");
        fs::write(&target, b"old build").unwrap();
        assert!(replace_file(&t.0.join("missing"), &target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old build");
        assert!(!t.0.join(".SLAM.AppImage.new").exists());
    }

    #[test]
    fn the_zip_goes_over_the_folder_and_the_old_exe_waits_for_cleanup() {
        let t = Temp::new("zip");
        let exe = t.0.join("slam.exe");
        fs::write(&exe, b"old exe").unwrap();
        fs::write(t.0.join("README.md"), b"old readme").unwrap();
        let zip = t.0.join("update.zip");
        zip_of(
            &zip,
            &[
                ("SLAM-v1-windows-x64/slam.exe", b"new exe"),
                ("SLAM-v1-windows-x64/README.md", b"new readme"),
                ("SLAM-v1-windows-x64/sub/data.bin", b"data"),
            ],
        );
        install_zip(&zip, &exe).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new exe");
        assert_eq!(fs::read(t.0.join("README.md")).unwrap(), b"new readme");
        assert_eq!(fs::read(t.0.join("sub/data.bin")).unwrap(), b"data");
        assert_eq!(fs::read(old_path(&exe)).unwrap(), b"old exe");
        clean_old(&exe);
        assert!(!old_path(&exe).exists());
        // Nothing left to clean is fine.
        clean_old(&exe);
    }

    #[test]
    fn an_archive_with_an_unsafe_name_changes_nothing() {
        let t = Temp::new("unsafe");
        let exe = t.0.join("slam.exe");
        fs::write(&exe, b"old exe").unwrap();
        let zip = t.0.join("update.zip");
        zip_of(
            &zip,
            &[("top/slam.exe", b"new"), ("top/../../evil.txt", b"x")],
        );
        assert!(install_zip(&zip, &exe).is_err());
        assert_eq!(fs::read(&exe).unwrap(), b"old exe");
        assert!(!old_path(&exe).exists());
        assert!(!t.0.parent().unwrap().join("evil.txt").exists());
    }

    #[test]
    fn unpack_names_drop_the_top_folder_and_refuse_escapes() {
        assert_eq!(unpack_name("top/a/b.txt"), Some(PathBuf::from("a/b.txt")));
        assert_eq!(unpack_name("top/../x"), None);
        assert_eq!(unpack_name("/abs/x"), None);
        assert_eq!(unpack_name("loose.txt"), None);
    }

    #[test]
    fn the_old_name_appends_a_suffix() {
        assert_eq!(
            old_path(Path::new("/a/slam.exe")),
            PathBuf::from("/a/slam.exe.old")
        );
    }
}
