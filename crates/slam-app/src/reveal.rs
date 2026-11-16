//! Shows a file in the system's file manager (ADR-0154): selected in its folder where the
//! system can, else the folder opened. Runs on its own thread: the caller never waits.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Shows `path` in the file manager, on a thread named `slam-reveal`.
pub fn reveal(path: PathBuf) {
    // Unit tests click the toast; they must not open the tester's file manager.
    if cfg!(test) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("slam-reveal".to_owned())
        .spawn(move || {
            if let Err(e) = reveal_now(&path) {
                tracing::warn!("cannot show {}: {e}", path.display());
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("reveal: cannot start the thread: {e}");
    }
}

fn reveal_now(path: &Path) -> std::io::Result<()> {
    for mut command in commands(path) {
        let status = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => tracing::debug!("reveal: {command:?} ended with {status}"),
            Err(e) => tracing::debug!("reveal: {command:?}: {e}"),
        }
    }
    Err(std::io::Error::other("no file manager answered"))
}

/// The commands that show `path`, in the order they are tried.
fn commands(path: &Path) -> Vec<Command> {
    let folder = path.parent().unwrap_or(path);
    if cfg!(windows) {
        // `explorer` exits with 1 even when it shows the file: its status is not checked
        // past the first command.
        let mut c = Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        vec![c]
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        vec![c]
    } else {
        // The freedesktop file manager interface selects the file; `xdg-open` on the folder
        // is the fallback.
        let mut show = Command::new("dbus-send");
        show.args([
            "--session",
            "--print-reply",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{}", file_uri(path)))
        .arg("string:");
        let mut open = Command::new("xdg-open");
        open.arg(folder);
        vec![show, open]
    }
}

/// `path` as a `file://` URI, the bytes outside the unreserved set and `/` escaped.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~/".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_escape_spaces_and_non_ascii() {
        assert_eq!(
            file_uri(Path::new("/home/a b/Реплей.osr")),
            "file:///home/a%20b/%D0%A0%D0%B5%D0%BF%D0%BB%D0%B5%D0%B9.osr"
        );
    }

    #[test]
    fn every_system_has_a_command_naming_the_file_or_its_folder() {
        let path = Path::new("/data/replays/x.osr");
        let commands = commands(path);
        assert!(!commands.is_empty());
        let shown = format!("{:?}", commands[0]);
        assert!(shown.contains("x.osr"), "{shown}");
        if !cfg!(windows) && !cfg!(target_os = "macos") {
            assert!(format!("{:?}", commands[1]).contains("/data/replays"));
        }
    }
}
