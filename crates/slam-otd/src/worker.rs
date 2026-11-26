//! Background worker: one daemon connection per command (ADR-0166).

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use crate::model::{AppInfo, Settings, Tablet, parse_tablets};
use crate::rpc::RpcClient;
use crate::{Error, transport};

/// What the UI asks the worker to do.
#[derive(Debug)]
pub enum Command {
    /// Read application info, tablets and settings.
    Refresh,
    /// Save `settings.json`, send `SetSettings`, then read back.
    Apply(Box<Settings>),
}

/// Daemon state as last read.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub app: AppInfo,
    pub tablets: Vec<Tablet>,
    pub settings: Settings,
}

/// What the worker reports back.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The daemon is not running (no pipe or connection refused).
    NotRunning,
    /// A daemon is running but speaks an unsupported protocol version.
    Unsupported { version: String },
    /// Fresh state after `Refresh`.
    Snapshot(Box<Snapshot>),
    /// Fresh state after a successful `Apply`.
    Applied(Box<Snapshot>),
    /// The command failed; the message is for the log and a status line.
    Failed(String),
}

/// Handle to the worker thread. Dropping it stops the worker after its
/// current command; the thread is detached, never joined from the UI.
pub struct OtdService {
    commands: Sender<Command>,
    events: Receiver<Event>,
    busy: usize,
}

impl OtdService {
    /// Starts the worker talking to the real daemon pipe.
    pub fn spawn() -> io::Result<Self> {
        Self::spawn_with(transport::connect)
    }

    /// Starts the worker with a custom connector (tests).
    pub fn spawn_with<S, C>(connect: C) -> io::Result<Self>
    where
        S: Read + Write,
        C: FnMut() -> io::Result<S> + Send + 'static,
    {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        thread::Builder::new()
            .name("slam-otd".into())
            .spawn(move || run(connect, &cmd_rx, &ev_tx))?;
        Ok(Self {
            commands: cmd_tx,
            events: ev_rx,
            busy: 0,
        })
    }

    /// Queues `command`; `false` when the worker has stopped (the service is then dead).
    pub fn send(&mut self, command: Command) -> bool {
        let sent = self.commands.send(command).is_ok();
        if sent {
            self.busy += 1;
        }
        sent
    }

    /// Next event without blocking.
    pub fn try_event(&mut self) -> Option<Event> {
        match self.events.try_recv() {
            Ok(ev) => {
                self.busy = self.busy.saturating_sub(1);
                Some(ev)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if self.busy > 0 {
                    self.busy = 0;
                    return Some(Event::Failed("worker thread stopped".into()));
                }
                None
            }
        }
    }

    /// Whether commands are still in flight.
    pub fn is_busy(&self) -> bool {
        self.busy > 0
    }
}

fn run<S, C>(mut connect: C, commands: &Receiver<Command>, events: &Sender<Event>)
where
    S: Read + Write,
    C: FnMut() -> io::Result<S>,
{
    while let Ok(cmd) = commands.recv() {
        let ev = execute(&mut connect, cmd);
        if let Event::Failed(msg) = &ev {
            tracing::warn!(target: "slam_otd", "OpenTabletDriver: {msg}");
        }
        if events.send(ev).is_err() {
            break;
        }
    }
}

/// Runs one command on a fresh connection.
pub fn execute<S, C>(connect: &mut C, cmd: Command) -> Event
where
    S: Read + Write,
    C: FnMut() -> io::Result<S>,
{
    let stream = match connect() {
        Ok(s) => s,
        Err(e) if transport::is_not_running(&e) => return Event::NotRunning,
        Err(e) => return Event::Failed(format!("connect: {e}")),
    };
    let mut client = RpcClient::new(stream);
    let result = (|| {
        let app = AppInfo::from_value(&client.call("GetApplicationInfo", vec![])?)?;
        if !app.is_supported() {
            return Ok(Event::Unsupported {
                version: app.version,
            });
        }
        let applied = match cmd {
            Command::Refresh => false,
            Command::Apply(settings) => {
                check_settings_file(&app.settings_file)?;
                save_settings_file(&app.settings_file, &settings)?;
                client.call("SetSettings", vec![settings.as_value().clone()])?;
                true
            }
        };
        let snapshot = Box::new(read_snapshot(&mut client, app)?);
        Ok(if applied {
            Event::Applied(snapshot)
        } else {
            Event::Snapshot(snapshot)
        })
    })();
    result.unwrap_or_else(|e: Error| Event::Failed(e.to_string()))
}

fn read_snapshot<S: Read + Write>(
    client: &mut RpcClient<S>,
    app: AppInfo,
) -> Result<Snapshot, Error> {
    let tablets = parse_tablets(&client.call("GetTablets", vec![])?)?;
    let settings = Settings::from_value(client.call("GetSettings", vec![])?)?;
    Ok(Snapshot {
        app,
        tablets,
        settings,
    })
}

/// The daemon names the file SLAM writes: only a `settings.json` is accepted, so that a
/// foreign pipe cannot point the write at any other file of the player.
pub fn check_settings_file(path: &Path) -> Result<(), Error> {
    if path.is_absolute() && path.file_name() == Some(std::ffi::OsStr::new("settings.json")) {
        Ok(())
    } else {
        Err(Error::Protocol(format!(
            "unexpected settings file {}",
            path.display()
        )))
    }
}

/// Writes `settings.json` through a temporary file and a rename, as OTD's UX
/// persists settings before `SetSettings` (the daemon never saves them itself). A
/// symbolic link is followed (its target is replaced, the link stays) and the old
/// file's permissions are kept.
pub fn save_settings_file(path: &Path, settings: &Settings) -> Result<(), Error> {
    let resolved = std::fs::canonicalize(path).ok();
    let path = resolved.as_deref().unwrap_or(path);
    let permissions = std::fs::metadata(path).ok().map(|m| m.permissions());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".slam-tmp");
    let tmp = std::path::PathBuf::from(tmp);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(settings.to_file_json().as_bytes())?;
        if let Some(permissions) = permissions {
            f.set_permissions(permissions)?;
        }
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}
