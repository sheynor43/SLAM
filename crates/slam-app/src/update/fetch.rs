//! The updater's HTTP side: a trait the worker uses and the `ureq` client behind it.

use std::io::{Read, Write};
use std::time::Duration;

/// The most a download may be: far above any release.
pub const MAX_DOWNLOAD: u64 = 2 << 30;

/// A failed request, as text for the log.
#[derive(Debug)]
pub struct FetchError(pub String);

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<std::io::Error> for FetchError {
    fn from(e: std::io::Error) -> Self {
        FetchError(e.to_string())
    }
}

/// How the worker reaches the server.
pub trait Fetch: Send + 'static {
    /// The body at `url`, at most `max` bytes.
    fn get(&self, url: &str, max: u64) -> Result<Vec<u8>, FetchError>;

    /// Writes the body at `url` to `dest`, telling the bytes received and the total, if the
    /// server said it, as they come.
    fn download(
        &self,
        url: &str,
        dest: &mut dyn Write,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), FetchError>;
}

/// HTTPS with rustls.
pub struct Http {
    agent: ureq::Agent,
}

impl Http {
    pub fn new() -> Http {
        let agent = ureq::Agent::config_builder()
            .user_agent(super::USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_global(Some(Duration::from_secs(30 * 60)))
            .build()
            .into();
        Http { agent }
    }
}

impl Default for Http {
    fn default() -> Self {
        Http::new()
    }
}

impl Fetch for Http {
    fn get(&self, url: &str, max: u64) -> Result<Vec<u8>, FetchError> {
        let mut response = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| FetchError(e.to_string()))?;
        response
            .body_mut()
            .with_config()
            .limit(max)
            .read_to_vec()
            .map_err(|e| FetchError(e.to_string()))
    }

    fn download(
        &self,
        url: &str,
        dest: &mut dyn Write,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), FetchError> {
        let response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| FetchError(e.to_string()))?;
        let total = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let mut body = response.into_body();
        let mut reader = body.with_config().limit(MAX_DOWNLOAD).reader();
        let mut block = vec![0u8; 64 * 1024];
        let mut received = 0u64;
        loop {
            let n = reader.read(&mut block)?;
            if n == 0 {
                break;
            }
            dest.write_all(&block[..n])?;
            received += n as u64;
            progress(received, total);
        }
        // A cut connection is a short body: a size the server told must be met.
        match total {
            Some(total) if total != received => Err(FetchError(format!(
                "short download: {received} of {total} bytes"
            ))),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    use super::*;

    /// Serves `body` to `count` requests on a local port; returns the base URL.
    fn serve(body: &'static [u8], count: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..count {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok() && line.trim() != "" {
                    line.clear();
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{port}/")
    }

    #[test]
    fn get_returns_the_body_and_honours_the_limit() {
        let url = serve(b"hello world", 2);
        let http = Http::new();
        assert_eq!(http.get(&url, 100).unwrap(), b"hello world");
        assert!(http.get(&url, 5).is_err());
    }

    #[test]
    fn download_streams_with_progress() {
        let url = serve(b"0123456789", 1);
        let mut out = Vec::new();
        let mut last = (0, None);
        Http::new()
            .download(&url, &mut out, &mut |got, total| last = (got, total))
            .unwrap();
        assert_eq!(out, b"0123456789");
        assert_eq!(last, (10, Some(10)));
    }
}
