//! The Discord IPC wire format and transport (ADR-0170).
//!
//! A frame is a little-endian `u32` opcode, a little-endian `u32` payload length and the
//! payload, which is JSON. Discord listens on a Unix socket `discord-ipc-N` (N in 0..10)
//! or, on Windows, on the named pipe `\\.\pipe\discord-ipc-N`.

use std::io::{self, Read, Write};
use std::path::Path;
#[cfg(any(unix, test))]
use std::path::PathBuf;
#[cfg(unix)]
use std::time::Duration;

/// First frame of a connection: `{"v":1,"client_id":...}`.
pub const OP_HANDSHAKE: u32 = 0;
/// A command or an event.
pub const OP_FRAME: u32 = 1;
/// Discord closes the connection.
pub const OP_CLOSE: u32 = 2;
pub const OP_PING: u32 = 3;
pub const OP_PONG: u32 = 4;

/// Largest payload accepted from Discord; a longer frame means the stream is not Discord's.
pub const MAX_PAYLOAD: usize = 64 * 1024;
/// How many `discord-ipc-N` endpoints Discord tries.
pub const ENDPOINTS: u32 = 10;
/// Read/write timeout of one operation (Unix only).
#[cfg(unix)]
pub const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// Subdirectories of a runtime directory where the flatpak and snap builds put their socket.
#[cfg(any(unix, test))]
const SUBDIRS: [&str; 3] = ["", "app/com.discordapp.Discord", "snap.discord"];

/// The bytes of a frame.
pub fn encode(op: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Reads one frame: its opcode and payload.
pub fn read_frame(reader: &mut impl Read) -> io::Result<(u32, Vec<u8>)> {
    let mut head = [0u8; 8];
    reader.read_exact(&mut head)?;
    let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a Discord frame is too long",
        ));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    Ok((op, payload))
}

/// Writes one frame.
pub fn write_frame(writer: &mut impl Write, op: u32, payload: &[u8]) -> io::Result<()> {
    writer.write_all(&encode(op, payload))?;
    writer.flush()
}

/// The directories sockets may lie in, without repeats: the runtime directory, the temp
/// directory and `/tmp`.
#[cfg(any(unix, test))]
pub fn base_dirs(runtime: Option<PathBuf>, temp: Option<PathBuf>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for dir in runtime
        .into_iter()
        .chain(temp)
        .chain([PathBuf::from("/tmp")])
    {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Every Unix socket path to try, lowest endpoint number first.
#[cfg(any(unix, test))]
pub fn socket_candidates(bases: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for n in 0..ENDPOINTS {
        for base in bases {
            for sub in SUBDIRS {
                out.push(base.join(sub).join(format!("discord-ipc-{n}")));
            }
        }
    }
    out
}

/// The named pipe of endpoint `n` on Windows.
#[cfg(any(windows, test))]
pub fn pipe_name(n: u32) -> String {
    format!(r"\\.\pipe\discord-ipc-{n}")
}

#[cfg(unix)]
fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The stream type of a connection on this platform.
#[cfg(unix)]
pub type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
pub type Stream = std::fs::File;

/// Connects to the first Discord endpoint that answers; `NotFound` when none does.
#[cfg(unix)]
pub fn connect() -> io::Result<Stream> {
    let bases = base_dirs(env_dir("XDG_RUNTIME_DIR"), env_dir("TMPDIR"));
    for path in socket_candidates(&bases) {
        if let Ok(stream) = connect_to(&path) {
            return Ok(stream);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Discord is not running",
    ))
}

/// Connects to the Unix socket at `path`.
#[cfg(unix)]
pub fn connect_to(path: &Path) -> io::Result<Stream> {
    let stream = std::os::unix::net::UnixStream::connect(path)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    Ok(stream)
}

/// Connects to the first Discord pipe that opens; `NotFound` when none does.
#[cfg(windows)]
pub fn connect() -> io::Result<Stream> {
    for n in 0..ENDPOINTS {
        if let Ok(stream) = connect_to(Path::new(&pipe_name(n))) {
            return Ok(stream);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Discord is not running",
    ))
}

/// Opens the named pipe at `path`.
#[cfg(windows)]
pub fn connect_to(path: &Path) -> io::Result<Stream> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_opcode_length_and_payload_little_endian() {
        let bytes = encode(OP_FRAME, b"{}");
        assert_eq!(bytes, [1, 0, 0, 0, 2, 0, 0, 0, b'{', b'}']);
    }

    #[test]
    fn a_frame_reads_back() {
        let bytes = encode(OP_HANDSHAKE, b"{\"v\":1}");
        let (op, payload) = read_frame(&mut bytes.as_slice()).unwrap();
        assert_eq!((op, payload.as_slice()), (OP_HANDSHAKE, &b"{\"v\":1}"[..]));
    }

    #[test]
    fn a_short_frame_is_an_error() {
        let mut bytes = encode(OP_FRAME, b"abcd");
        bytes.truncate(10);
        assert!(read_frame(&mut bytes.as_slice()).is_err());
    }

    #[test]
    fn an_overlong_frame_is_refused() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&OP_FRAME.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        let e = read_frame(&mut bytes.as_slice()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn candidates_cover_the_runtime_dir_and_the_packaged_builds() {
        let bases = base_dirs(
            Some(PathBuf::from("/run/user/1000")),
            Some(PathBuf::from("/tmp")),
        );
        assert_eq!(
            bases,
            [PathBuf::from("/run/user/1000"), PathBuf::from("/tmp")]
        );
        let paths = socket_candidates(&bases);
        assert_eq!(paths.len(), 10 * 2 * 3);
        assert_eq!(paths[0], PathBuf::from("/run/user/1000/discord-ipc-0"));
        assert!(paths.contains(&PathBuf::from(
            "/run/user/1000/app/com.discordapp.Discord/discord-ipc-0"
        )));
        assert!(paths.contains(&PathBuf::from("/tmp/snap.discord/discord-ipc-9")));
        // Lower endpoints first.
        assert!(
            paths.iter().position(|p| p.ends_with("discord-ipc-0"))
                < paths.iter().position(|p| p.ends_with("discord-ipc-1"))
        );
    }

    #[test]
    fn the_windows_pipe_is_named_by_endpoint() {
        assert_eq!(pipe_name(3), r"\\.\pipe\discord-ipc-3");
    }
}
