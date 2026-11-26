//! Connection to the daemon's .NET named pipe `OpenTabletDriver.Daemon`.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// .NET pipe name used by `OpenTabletDriver.Daemon` 0.6.x.
pub const PIPE_NAME: &str = "OpenTabletDriver.Daemon";

/// Read/write timeout for one daemon operation (Unix only; see ADR-0166).
pub const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// The stream type of a daemon connection on this platform.
#[cfg(unix)]
pub type DaemonStream = std::os::unix::net::UnixStream;
#[cfg(windows)]
pub type DaemonStream = std::fs::File;

/// Where the daemon's pipe lives. On Unix .NET maps a pipe name to the socket
/// `<Path.GetTempPath()>/CoreFxPipe_<name>`; `GetTempPath` is `$TMPDIR` or `/tmp`.
pub fn pipe_path() -> PathBuf {
    #[cfg(unix)]
    {
        let tmp = std::env::var_os("TMPDIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        unix_pipe_path(&tmp)
    }
    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\{PIPE_NAME}"))
    }
}

/// Unix socket path for the pipe under the given temp directory.
pub fn unix_pipe_path(tmp: &std::path::Path) -> PathBuf {
    tmp.join(format!("CoreFxPipe_{PIPE_NAME}"))
}

/// Opens a connection to the daemon. `NotFound`/`ConnectionRefused` mean "not running".
pub fn connect() -> io::Result<DaemonStream> {
    connect_to(&pipe_path())
}

/// Opens a connection to the daemon at `path`.
#[cfg(unix)]
pub fn connect_to(path: &std::path::Path) -> io::Result<DaemonStream> {
    check_owner(path)?;
    let s = std::os::unix::net::UnixStream::connect(path)?;
    s.set_read_timeout(Some(IO_TIMEOUT))?;
    s.set_write_timeout(Some(IO_TIMEOUT))?;
    Ok(s)
}

/// Opens a connection to the daemon at `path`. All server instances may be
/// momentarily busy (`ERROR_PIPE_BUSY`), so busy opens are retried for ~1 s.
#[cfg(windows)]
pub fn connect_to(path: &std::path::Path) -> io::Result<DaemonStream> {
    const ERROR_PIPE_BUSY: i32 = 231;
    let mut attempts = 0;
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
        {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempts < 20 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

/// On Linux the socket lies in the shared `/tmp`: only one the player owns is trusted (the
/// daemon names the settings file SLAM writes). Elsewhere the temp directory is the
/// user's own.
#[cfg(unix)]
fn check_owner(path: &std::path::Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let socket = std::fs::metadata(path)?.uid();
        let me = std::fs::metadata("/proc/self")?.uid();
        if socket != me {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the daemon's socket belongs to another user",
            ));
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = path;
    Ok(())
}

/// Whether a connect error means the daemon is simply not running.
pub fn is_not_running(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_socket_name_matches_dotnet() {
        assert_eq!(
            unix_pipe_path(std::path::Path::new("/tmp")),
            PathBuf::from("/tmp/CoreFxPipe_OpenTabletDriver.Daemon")
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_socket_is_not_running() {
        let dir = std::env::temp_dir().join(format!("slam-otd-test-{}", std::process::id()));
        let err = connect_to(&unix_pipe_path(&dir)).unwrap_err();
        assert!(is_not_running(&err), "{err:?}");
    }
}
