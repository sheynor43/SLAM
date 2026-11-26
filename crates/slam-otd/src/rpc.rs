//! Minimal JSON-RPC 2.0 client over one framed stream (requests are serial).

use std::io::{BufReader, Read, Write};

use serde_json::{Value, json};

use crate::Error;
use crate::framing::{read_message, write_message};

/// Notifications skipped while waiting for one response before giving up.
const MAX_SKIPPED_NOTIFICATIONS: usize = 10_000;

/// A JSON-RPC client on a connected daemon stream.
pub struct RpcClient<S: Read + Write> {
    reader: BufReader<S>,
    next_id: u64,
}

impl<S: Read + Write> RpcClient<S> {
    pub fn new(stream: S) -> Self {
        Self {
            reader: BufReader::new(stream),
            next_id: 1,
        }
    }

    /// Calls `method` with positional `params` and returns its `result`.
    /// Notifications (daemon events) received before the response are skipped.
    pub fn call(&mut self, method: &str, params: Vec<Value>) -> Result<Value, Error> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let body = serde_json::to_vec(&request).map_err(|e| Error::Protocol(e.to_string()))?;
        write_message(self.reader.get_mut(), &body)?;

        for _ in 0..MAX_SKIPPED_NOTIFICATIONS {
            let raw = read_message(&mut self.reader)?;
            let msg: Value =
                serde_json::from_slice(&raw).map_err(|e| Error::Protocol(e.to_string()))?;
            match response_for(&msg, id) {
                Some(result) => return result,
                None => {
                    if let Some(m) = msg.get("method").and_then(Value::as_str) {
                        tracing::trace!(target: "slam_otd", "skipping daemon notification {m}");
                    }
                }
            }
        }
        Err(Error::Protocol(format!("no response to {method}")))
    }

    /// Access to the buffered reader (tests).
    #[cfg(test)]
    pub(crate) fn reader(&self) -> &BufReader<S> {
        &self.reader
    }
}

/// `Some` when `msg` is the response to request `id`.
pub(crate) fn response_for(msg: &Value, id: u64) -> Option<Result<Value, Error>> {
    if msg.get("method").is_some() || msg.get("id").and_then(Value::as_u64) != Some(id) {
        return None;
    }
    if let Some(err) = msg.get("error") {
        let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        return Some(Err(Error::Remote { code, message }));
    }
    Some(Ok(msg.get("result").cloned().unwrap_or(Value::Null)))
}

/// Reads framed messages until EOF (tests).
#[cfg(test)]
pub(crate) fn read_all<R: std::io::BufRead>(r: &mut R) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(raw) = read_message(r) {
        out.push(serde_json::from_slice(&raw).unwrap());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    /// A duplex stream: reads come from `input`, writes go to `output`.
    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn framed(msgs: &[&str]) -> Vec<u8> {
        let mut buf = Vec::new();
        for m in msgs {
            write_message(&mut buf, m.as_bytes()).unwrap();
        }
        buf
    }

    #[test]
    fn skips_notifications_and_returns_result() {
        let input = framed(&[
            r#"{"jsonrpc":"2.0","method":"Message","params":[{"Message":"Connected to a client."}]}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":[]}"#,
        ]);
        let mut c = RpcClient::new(Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        });
        assert_eq!(c.call("GetTablets", vec![]).unwrap(), json!([]));
        let sent = read_all(&mut Cursor::new(c.reader().get_ref().output.clone()));
        assert_eq!(
            sent,
            vec![json!({"jsonrpc":"2.0","id":1,"method":"GetTablets","params":[]})]
        );
    }

    #[test]
    fn remote_error_from_recorded_sample() {
        let sample = include_str!("../tests/data/error_method_not_found.json");
        let msg: Value = serde_json::from_str(sample).unwrap();
        match response_for(&msg, 4) {
            Some(Err(Error::Remote { code, .. })) => assert_eq!(code, -32601),
            other => panic!("unexpected {other:?}"),
        }
        assert!(response_for(&msg, 5).is_none());
    }

    #[test]
    fn closed_pipe_is_io_error() {
        let mut c = RpcClient::new(Duplex {
            input: Cursor::new(Vec::new()),
            output: Vec::new(),
        });
        assert!(matches!(c.call("GetSettings", vec![]), Err(Error::Io(_))));
    }
}
