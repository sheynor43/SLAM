//! The `slam-discord` thread: keeps a connection to Discord and shows the latest presence.

use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use super::Presence;
use super::ipc::{self, OP_CLOSE, OP_FRAME, OP_HANDSHAKE, OP_PING, OP_PONG};

/// What the update side tells the thread.
pub enum Msg {
    /// Show this presence; `None` clears it.
    Set(Option<Presence>),
    /// The app is going: clear the presence and end.
    Stop,
}

/// Pause between attempts to reach Discord while it is not running.
pub const RETRY: Duration = Duration::from_secs(15);

/// Sleep while there is nothing to do.
const IDLE: Duration = Duration::from_secs(3600);

/// Runs until [`Msg::Stop`] or until the sender is dropped. `connect` opens a stream to
/// Discord; it is tried again every [`RETRY`] while it fails.
pub fn run<S, C>(app_id: &str, pid: u32, connect: C, rx: &Receiver<Msg>)
where
    S: Read + Write,
    C: Fn() -> io::Result<S>,
{
    let mut wanted: Option<Presence> = None;
    let mut conn: Option<S> = None;
    let mut dirty = false;
    let mut nonce = 0u64;
    let mut next_try = Instant::now();
    loop {
        let wait = if conn.is_none() && wanted.is_some() {
            next_try.saturating_duration_since(Instant::now())
        } else {
            IDLE
        };
        match rx.recv_timeout(wait) {
            Ok(Msg::Set(presence)) => {
                wanted = presence;
                dirty = true;
            }
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                if let Some(stream) = &mut conn {
                    nonce += 1;
                    let _ = set_activity(stream, pid, None, nonce);
                }
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if conn.is_none() && wanted.is_some() && Instant::now() >= next_try {
            match connect().and_then(|mut stream| handshake(&mut stream, app_id).map(|()| stream)) {
                Ok(stream) => {
                    tracing::debug!("discord: connected");
                    conn = Some(stream);
                    dirty = true;
                }
                Err(e) => {
                    tracing::trace!("discord: not connected: {e}");
                    next_try = Instant::now() + RETRY;
                }
            }
        }
        if dirty && let Some(stream) = &mut conn {
            nonce += 1;
            match set_activity(stream, pid, wanted.as_ref(), nonce) {
                Ok(()) => dirty = false,
                Err(e) => {
                    tracing::debug!("discord: connection lost: {e}");
                    conn = None;
                    next_try = Instant::now() + RETRY;
                }
            }
        }
    }
}

/// Sends the handshake and waits for Discord's `READY`.
pub fn handshake<S: Read + Write>(stream: &mut S, app_id: &str) -> io::Result<()> {
    write_json(stream, OP_HANDSHAKE, &super::handshake_payload(app_id))?;
    read_response(stream)
}

/// Sets (or, with `None`, clears) the activity and reads Discord's answer.
pub fn set_activity<S: Read + Write>(
    stream: &mut S,
    pid: u32,
    presence: Option<&Presence>,
    nonce: u64,
) -> io::Result<()> {
    let payload = super::set_activity_payload(pid, presence, nonce);
    write_json(stream, OP_FRAME, &payload)?;
    read_response(stream)
}

fn write_json<S: Write>(stream: &mut S, op: u32, payload: &serde_json::Value) -> io::Result<()> {
    ipc::write_frame(stream, op, payload.to_string().as_bytes())
}

/// Reads frames until a response comes; answers pings. A close is an error. The content of
/// a response is of no interest: a rejected activity just does not show.
fn read_response<S: Read + Write>(stream: &mut S) -> io::Result<()> {
    loop {
        let (op, payload) = ipc::read_frame(stream)?;
        match op {
            OP_FRAME => return Ok(()),
            OP_PING => ipc::write_frame(stream, OP_PONG, &payload)?,
            OP_CLOSE => {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "Discord closed the connection",
                ));
            }
            _ => {}
        }
    }
}

/// Starts the thread; the sender is its inbox.
pub fn spawn(app_id: &'static str) -> io::Result<mpsc::Sender<Msg>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("slam-discord".to_owned())
        .spawn(move || run(app_id, std::process::id(), ipc::connect, &rx))?;
    Ok(tx)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;

    fn presence(details: &str) -> Presence {
        Presence {
            details: details.to_owned(),
            state: None,
            start: None,
        }
    }

    #[test]
    fn the_thread_handshakes_sets_and_clears_the_activity() {
        let (client, mut server) = UnixStream::pair().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let client = Mutex::new(Some(client));
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            run(
                "123",
                7,
                || {
                    client
                        .lock()
                        .unwrap()
                        .take()
                        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
                },
                &rx,
            );
        });
        tx.send(Msg::Set(Some(presence("In the menu")))).unwrap();

        let (op, body) = ipc::read_frame(&mut server).unwrap();
        assert_eq!(op, OP_HANDSHAKE);
        let hello: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(hello["client_id"], "123");
        ipc::write_frame(&mut server, OP_FRAME, b"{\"evt\":\"READY\"}").unwrap();

        let (op, body) = ipc::read_frame(&mut server).unwrap();
        assert_eq!(op, OP_FRAME);
        let set: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(set["cmd"], "SET_ACTIVITY");
        assert_eq!(set["args"]["pid"], 7);
        assert_eq!(set["args"]["activity"]["details"], "In the menu");
        ipc::write_frame(&mut server, OP_FRAME, b"{}").unwrap();

        tx.send(Msg::Set(None)).unwrap();
        let (_, body) = ipc::read_frame(&mut server).unwrap();
        let clear: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(clear["args"]["activity"].is_null());
        ipc::write_frame(&mut server, OP_FRAME, b"{}").unwrap();

        // Going away clears the status once more.
        tx.send(Msg::Stop).unwrap();
        let (_, body) = ipc::read_frame(&mut server).unwrap();
        let clear: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(clear["args"]["activity"].is_null());
        ipc::write_frame(&mut server, OP_FRAME, b"{}").unwrap();
        thread.join().unwrap();
    }

    #[test]
    fn a_close_frame_ends_the_connection() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        ipc::write_frame(&mut server, OP_PING, b"x").unwrap();
        ipc::write_frame(&mut server, OP_CLOSE, b"{}").unwrap();
        let e = read_response(&mut client).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionAborted);
        // The ping was answered first.
        let (op, payload) = ipc::read_frame(&mut server).unwrap();
        assert_eq!((op, payload.as_slice()), (OP_PONG, &b"x"[..]));
    }
}
