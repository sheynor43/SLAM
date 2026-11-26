//! StreamJsonRpc header-delimited framing: `Content-Length: N\r\n\r\n` + UTF-8 body.

use std::io::{self, BufRead, Read, Write};

/// Upper bound for one message body; daemon settings are a few KiB.
pub const MAX_BODY_LEN: usize = 16 * 1024 * 1024;
/// Upper bound for one header line.
const MAX_HEADER_LINE: u64 = 8 * 1024;

/// Writes one framed message.
pub fn write_message<W: Write>(w: &mut W, body: &[u8]) -> io::Result<()> {
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(body);
    w.write_all(&frame)?;
    w.flush()
}

/// Reads one framed message body. Returns `UnexpectedEof` when the stream ends.
pub fn read_message<R: BufRead>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if r.by_ref().take(MAX_HEADER_LINE).read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "pipe closed while reading headers",
            ));
        }
        if !line.ends_with('\n') && line.len() as u64 >= MAX_HEADER_LINE {
            return Err(invalid("header line too long".into()));
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            if len.is_some() {
                break;
            }
            // Tolerate stray blank lines before the first header.
            continue;
        }
        let Some((name, value)) = header.split_once(':') else {
            return Err(invalid(format!("malformed header line: {header:?}")));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let n: usize = value
                .trim()
                .parse()
                .map_err(|_| invalid(format!("bad Content-Length: {value:?}")))?;
            if n > MAX_BODY_LEN {
                return Err(invalid(format!("message too large: {n} bytes")));
            }
            len = Some(n);
        }
        // Other headers (Content-Type) are ignored; the body is always UTF-8 JSON.
    }
    let mut body = vec![0; len.unwrap_or(0)];
    r.read_exact(&mut body)?;
    Ok(body)
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip_several_messages() {
        let mut buf = Vec::new();
        write_message(&mut buf, br#"{"a":1}"#).unwrap();
        write_message(&mut buf, "{\"b\":\"ё\"}".as_bytes()).unwrap();
        assert!(buf.starts_with(b"Content-Length: 7\r\n\r\n{\"a\":1}"));
        let mut r = Cursor::new(buf);
        assert_eq!(read_message(&mut r).unwrap(), br#"{"a":1}"#);
        assert_eq!(read_message(&mut r).unwrap(), "{\"b\":\"ё\"}".as_bytes());
        let err = read_message(&mut r).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn accepts_content_type_and_any_case() {
        let raw = b"content-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}";
        assert_eq!(read_message(&mut Cursor::new(&raw[..])).unwrap(), b"{}");
    }

    #[test]
    fn rejects_bad_headers() {
        for raw in [
            &b"Content-Length: x\r\n\r\n"[..],
            b"garbage\r\n\r\n",
            b"Content-Length: 999999999999\r\n\r\n",
        ] {
            let err = read_message(&mut Cursor::new(raw)).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{raw:?}");
        }
    }

    #[test]
    fn an_endless_header_line_is_rejected() {
        let raw = vec![b'x'; 20_000];
        let err = read_message(&mut Cursor::new(raw)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn truncated_body_is_eof() {
        let err = read_message(&mut Cursor::new(&b"Content-Length: 10\r\n\r\n{}"[..])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
