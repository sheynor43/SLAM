//! Self-update from GitHub releases (ADR-0185).
//!
//! A thread named `slam-update` does all the network and file work. The update tick sends it
//! [`Command`]s and polls [`Event`]s; it never waits. Nothing is downloaded without a
//! command from the player, and a download is installed only after its ed25519 signature
//! checks out against [`UPDATE_PUBLIC_KEY`].

pub mod fetch;
pub mod install;
pub mod release;
pub mod verify;
mod worker;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

pub use install::Target;
pub use release::Found;
pub use worker::Worker;

/// The release signing key's public half: the raw 32 bytes of the ed25519 key, base64
/// `BzxK1gdR3hZWx165wjQYJQKUBMYWeVTiPih0uz3+z5U=`. The private half is the repository
/// secret `SLAM_UPDATE_KEY`. While this is all zeros ([`verify::PLACEHOLDER_KEY`]), updates
/// are unavailable: nothing is checked or downloaded.
pub const UPDATE_PUBLIC_KEY: [u8; 32] = [
    0x07, 0x3c, 0x4a, 0xd6, 0x07, 0x51, 0xde, 0x16, 0x56, 0xc7, 0x5e, 0xb9, 0xc2, 0x34, 0x18, 0x25,
    0x02, 0x94, 0x04, 0xc6, 0x16, 0x79, 0x54, 0xe2, 0x3e, 0x28, 0x74, 0xbb, 0x3d, 0xfe, 0xcf, 0x95,
];

/// The releases of the project, newest first.
pub const RELEASES_URL: &str = "https://api.github.com/repos/sheynor43/SLAM/releases";
/// Sent with every request; GitHub requires one and it carries nothing personal.
pub const USER_AGENT: &str = "SLAM-updater";

/// Which releases are offered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Every release, pre-releases too: the default while all releases are alpha.
    #[default]
    Prerelease,
    /// Releases that are not pre-releases.
    Stable,
}

impl Channel {
    pub const ALL: [Channel; 2] = [Channel::Prerelease, Channel::Stable];

    /// The name in `settings.toml`.
    pub fn name(self) -> &'static str {
        match self {
            Channel::Prerelease => "prerelease",
            Channel::Stable => "stable",
        }
    }

    pub fn from_name(name: &str) -> Option<Channel> {
        Channel::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// Why an update did not go through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The server could not be reached or answered something unusable.
    Network,
    /// The download is not signed by the project's key; it was deleted.
    Signature,
    /// The files could not be put in place.
    Install,
}

/// A newer version, as the interface shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Available {
    pub version: String,
    /// The release's page.
    pub page_url: String,
    /// Whether this install can download and install it; otherwise only the page is offered.
    pub installable: bool,
}

/// Where the updater stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    /// No key is set, so there are no updates.
    Unavailable,
    Checking,
    UpToDate,
    Available(Available),
    /// Percent of the file received.
    Downloading(u8),
    /// Downloaded and verified; installed on the restart.
    Ready {
        version: String,
    },
    Failed(Failure),
}

/// What the update tick asks of the thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Asks the server for the newest release. `manual`: a failure is shown, not only logged.
    Check { channel: Channel, manual: bool },
    /// Downloads and verifies the release found.
    Download,
    /// Puts the downloaded release in place.
    Install,
}

/// What the thread tells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    State(State),
    /// The new files are in place: restart.
    Installed,
}

/// What the worker needs to know.
#[derive(Clone, Debug)]
pub struct Config {
    pub key: [u8; 32],
    pub releases_url: String,
    /// Where downloads go.
    pub dir: PathBuf,
    pub current: semver::Version,
    /// What can be replaced; `None` when this build runs from elsewhere.
    pub target: Option<Target>,
}

impl Config {
    /// The running build's configuration with the project's key and server.
    pub fn detect(data_dir: PathBuf) -> Config {
        Config {
            key: UPDATE_PUBLIC_KEY,
            releases_url: RELEASES_URL.to_owned(),
            dir: data_dir.join("update"),
            current: semver::Version::parse(env!("CARGO_PKG_VERSION"))
                .unwrap_or_else(|_| semver::Version::new(0, 0, 0)),
            target: Target::detect(),
        }
    }
}

/// Whether the project's key is set: whether this build can update at all.
pub fn available() -> bool {
    verify::key_is_set(&UPDATE_PUBLIC_KEY)
}

static RESTART: AtomicBool = AtomicBool::new(false);

/// Asks for the executable to be started again once the window has closed.
pub fn request_restart() {
    RESTART.store(true, Ordering::Relaxed);
}

/// Starts the installed build again when an install asked for it. Call after the window and
/// every thread are gone.
pub fn relaunch_if_requested(target: Option<&Target>) {
    if !RESTART.load(Ordering::Relaxed) {
        return;
    }
    let Some(target) = target else { return };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match std::process::Command::new(target.executable())
        .args(args)
        .spawn()
    {
        Ok(_) => tracing::info!("update: restarted {}", target.executable().display()),
        Err(e) => tracing::warn!("update: cannot restart: {e}"),
    }
}

/// The handle the update tick holds. The thread ends when this is dropped.
pub struct Updater {
    commands: Sender<Command>,
    events: Receiver<Event>,
    target: Option<Target>,
}

impl Updater {
    /// Starts the thread, named `slam-update`, with the real network.
    pub fn spawn(config: Config) -> std::io::Result<Updater> {
        Updater::spawn_with(config, fetch::Http::new())
    }

    /// Starts the thread with another way to reach the server.
    pub fn spawn_with(config: Config, fetch: impl fetch::Fetch) -> std::io::Result<Updater> {
        let (commands, command_rx) = channel::<Command>();
        let (event_tx, events) = channel();
        let target = config.target.clone();
        std::thread::Builder::new()
            .name("slam-update".to_owned())
            .spawn(move || {
                let mut worker = Worker::new(config, fetch);
                worker.start_up();
                while let Ok(command) = command_rx.recv() {
                    worker.handle(command, &mut |event| {
                        let _ = event_tx.send(event);
                    });
                }
            })?;
        Ok(Updater {
            commands,
            events,
            target,
        })
    }

    /// Queues a command; false when the thread is gone.
    pub fn send(&self, command: Command) -> bool {
        self.commands.send(command).is_ok()
    }

    /// The next event, if any. Does not allocate when there is none.
    pub fn poll(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// What an install replaces; `None` when only the release page can be offered.
    pub fn target(&self) -> Option<&Target> {
        self.target.as_ref()
    }
}
