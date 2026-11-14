//! The settings screen's Feedback section: links to the project page and to pre-filled
//! issue forms. The shell only records which link was pressed; the URL is built and handed
//! to the system's opener on a short-lived thread, so the update tick never reads files or
//! spawns processes.

use std::fmt::Write as _;
use std::sync::{Mutex, PoisonError};

/// The project page.
pub const REPO_URL: &str = "https://github.com/sheynor43/SLAM";

/// The longest URL opened. Browsers and GitHub take far more, but `xdg-open` and the
/// Windows shell are safer well below it.
pub const MAX_URL_LEN: usize = 8000;

/// What a Feedback row opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The project page.
    Github,
    /// The bug report form.
    Bug,
    /// The feature request form.
    Feature,
}

/// What the bug report form is filled with. Every field is plain text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Environment {
    pub version: String,
    pub os: String,
    /// Render backend and the adapter's description.
    pub gpu: String,
    pub audio: String,
}

static DRIVER: Mutex<String> = Mutex::new(String::new());
static AUDIO: Mutex<String> = Mutex::new(String::new());

/// Remembers the render backend and adapter text of the draw thread's device.
pub fn set_driver(text: String) {
    *DRIVER.lock().unwrap_or_else(PoisonError::into_inner) = text;
}

/// Remembers the audio backend the output opened with.
pub fn set_audio(text: String) {
    *AUDIO.lock().unwrap_or_else(PoisonError::into_inner) = text;
}

impl Environment {
    /// Reads the client's version, the system and what the draw and audio threads
    /// reported. Reads a file: not for the update tick.
    pub fn detect() -> Self {
        let known = |m: &Mutex<String>| {
            let text = m.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if text.is_empty() {
                "unknown".to_owned()
            } else {
                text
            }
        };
        Self {
            version: concat!("slam-", env!("CARGO_PKG_VERSION")).to_owned(),
            os: system_name(),
            gpu: known(&DRIVER),
            audio: known(&AUDIO),
        }
    }
}

/// The OS name and version when the system tells, with the architecture.
fn system_name() -> String {
    let name = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| pretty_name(&text))
        .unwrap_or_else(|| std::env::consts::OS.to_owned());
    format!("{name} ({})", std::env::consts::ARCH)
}

/// `PRETTY_NAME` of an `os-release` file.
fn pretty_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim().trim_matches('"').to_owned())
        .filter(|v| !v.is_empty())
}

/// Percent-encodes `text` for a URL query value: unreserved characters stay, everything
/// else becomes `%XX` per UTF-8 byte.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// The URL `kind` opens, the forms pre-filled from `env`. A field that would push the URL
/// past [`MAX_URL_LEN`] is left out.
pub fn url(kind: Kind, env: &Environment) -> String {
    let (template, fields) = match kind {
        Kind::Github => return REPO_URL.to_owned(),
        Kind::Bug => (
            "bug_report.yml",
            vec![
                ("version", &env.version),
                ("os", &env.os),
                ("gpu", &env.gpu),
                ("audio", &env.audio),
            ],
        ),
        Kind::Feature => ("feature_request.yml", Vec::new()),
    };
    let mut url = format!("{REPO_URL}/issues/new?template={template}");
    for (id, value) in fields {
        let piece = format!("&{id}={}", encode(value));
        if url.len() + piece.len() <= MAX_URL_LEN {
            url.push_str(&piece);
        }
    }
    url
}

/// Opens `kind`'s page in the default browser from a short-lived thread. Failures are
/// logged.
pub fn open(kind: Kind) {
    let spawned = std::thread::Builder::new()
        .name("slam-feedback".to_owned())
        .spawn(move || {
            let url = url(kind, &Environment::detect());
            if let Err(e) = open_url(&url) {
                tracing::warn!("feedback: cannot open the browser: {e}");
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("feedback: cannot start the thread: {e}");
    }
}

fn open_url(url: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let mut command = if cfg!(windows) {
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Reaped here, so the opener never lingers as a zombie.
    child.wait().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Environment {
        Environment {
            version: "slam-0.1.0".into(),
            os: "Arch Linux (x86_64)".into(),
            gpu: "OpenGL 3.3 & Mesa/ü".into(),
            audio: "PipeWire".into(),
        }
    }

    #[test]
    fn encoding_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(encode("aZ09-_.~"), "aZ09-_.~");
        assert_eq!(encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        assert_eq!(encode("\n#"), "%0A%23");
    }

    #[test]
    fn the_github_link_is_the_repository() {
        assert_eq!(url(Kind::Github, &env()), REPO_URL);
    }

    #[test]
    fn the_bug_url_carries_every_field() {
        let url = url(Kind::Bug, &env());
        assert!(
            url.starts_with(
                "https://github.com/sheynor43/SLAM/issues/new?template=bug_report.yml&"
            )
        );
        assert!(url.contains("&version=slam-0.1.0"));
        assert!(url.contains("&os=Arch%20Linux%20%28x86_64%29"));
        assert!(url.contains("&gpu=OpenGL%203.3%20%26%20Mesa%2F%C3%BC"));
        assert!(url.contains("&audio=PipeWire"));
        assert!(url.is_ascii());
    }

    #[test]
    fn the_feature_url_names_its_form_only() {
        assert_eq!(
            url(Kind::Feature, &env()),
            "https://github.com/sheynor43/SLAM/issues/new?template=feature_request.yml"
        );
    }

    #[test]
    fn a_huge_field_is_dropped_to_keep_the_url_short() {
        let mut e = env();
        e.gpu = "x".repeat(20_000);
        let url = url(Kind::Bug, &e);
        assert!(url.len() <= MAX_URL_LEN);
        assert!(!url.contains("&gpu="));
        assert!(url.contains("&audio=PipeWire"));
    }

    #[test]
    fn os_release_pretty_name_is_read() {
        let text = "NAME=\"Arch\"\nPRETTY_NAME=\"Arch Linux\"\n";
        assert_eq!(pretty_name(text).as_deref(), Some("Arch Linux"));
        assert_eq!(pretty_name("NAME=x\n"), None);
    }
}
