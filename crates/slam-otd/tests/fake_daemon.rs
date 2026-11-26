//! The worker against a scripted in-process daemon over a Unix socket pair.
#![cfg(unix)]

use std::io::{self, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use slam_otd::framing::{read_message, write_message};
use slam_otd::worker::execute;
use slam_otd::{Command, Event, OtdService, Settings};

fn result(sample: &str) -> Value {
    serde_json::from_str::<Value>(sample).unwrap()["result"].clone()
}

struct Daemon {
    version: String,
    settings_file: PathBuf,
    settings: Value,
    tablets: Value,
    /// Methods received, in order.
    log: Vec<String>,
}

/// Serves one connection: a log notification first, then answers requests.
fn serve(mut stream: UnixStream, daemon: Arc<Mutex<Daemon>>) {
    let note =
        json!({"jsonrpc":"2.0","method":"Message","params":[{"Message":"Connected to a client."}]});
    write_message(&mut stream, note.to_string().as_bytes()).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    while let Ok(raw) = read_message(&mut reader) {
        let req: Value = serde_json::from_slice(&raw).unwrap();
        let method = req["method"].as_str().unwrap().to_owned();
        let mut d = daemon.lock().unwrap();
        d.log.push(method.clone());
        let result = match method.as_str() {
            "GetApplicationInfo" => json!({
                "Version": d.version,
                "SettingsFile": d.settings_file,
            }),
            "GetTablets" => d.tablets.clone(),
            "GetSettings" => d.settings.clone(),
            "SetSettings" => {
                d.settings = req["params"][0].clone();
                Value::Null
            }
            _ => {
                let err = json!({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32601,"message":"not found"}});
                write_message(&mut stream, err.to_string().as_bytes()).unwrap();
                continue;
            }
        };
        let resp = json!({"jsonrpc":"2.0","id":req["id"],"result":result});
        write_message(&mut stream, resp.to_string().as_bytes()).unwrap();
    }
}

fn daemon(version: &str, settings_file: PathBuf) -> Arc<Mutex<Daemon>> {
    Arc::new(Mutex::new(Daemon {
        version: version.into(),
        settings_file,
        settings: result(include_str!("data/get_settings.json")),
        tablets: result(include_str!("data/get_tablets_ctl470.json")),
        log: Vec::new(),
    }))
}

fn connector(d: Arc<Mutex<Daemon>>) -> impl FnMut() -> io::Result<UnixStream> + Send {
    move || {
        let (client, server) = UnixStream::pair()?;
        let d = d.clone();
        thread::spawn(move || serve(server, d));
        Ok(client)
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("slam-otd-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn refresh_reads_everything() {
    let d = daemon("0.6.8.1", PathBuf::from("/nonexistent/settings.json"));
    let mut connect = connector(d.clone());
    let Event::Snapshot(snap) = execute(&mut connect, Command::Refresh) else {
        panic!("expected a snapshot");
    };
    assert_eq!(snap.app.version, "0.6.8.1");
    assert_eq!(snap.tablets[0].name, "Wacom CTL-470");
    assert_eq!(snap.settings.profile_count(), 2);
    assert_eq!(
        d.lock().unwrap().log,
        ["GetApplicationInfo", "GetTablets", "GetSettings"]
    );
}

#[test]
fn apply_saves_file_then_sets_settings() {
    let dir = temp_dir("apply");
    let file = dir.join("OpenTabletDriver").join("settings.json");
    let d = daemon("0.6.8.1", file.clone());
    let mut connect = connector(d.clone());
    let mut s = Settings::from_value(result(include_str!("data/get_settings.json"))).unwrap();
    s.set_filter_enabled(0, 0, true).unwrap();

    let Event::Applied(snap) = execute(&mut connect, Command::Apply(Box::new(s.clone()))) else {
        panic!("expected applied");
    };
    assert_eq!(snap.settings, s);
    let d = d.lock().unwrap();
    assert_eq!(
        d.log,
        [
            "GetApplicationInfo",
            "SetSettings",
            "GetTablets",
            "GetSettings"
        ]
    );
    assert_eq!(&d.settings, s.as_value());
    let saved = std::fs::read_to_string(&file).unwrap();
    assert_eq!(saved, s.to_file_json());
    assert!(!dir.join("OpenTabletDriver/settings.json.slam-tmp").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsupported_version_changes_nothing() {
    let dir = temp_dir("unsupported");
    let file = dir.join("settings.json");
    let d = daemon("0.7.0.0", file.clone());
    let mut connect = connector(d.clone());
    let s = Settings::from_value(result(include_str!("data/get_settings.json"))).unwrap();
    assert_eq!(
        execute(&mut connect, Command::Apply(Box::new(s))),
        Event::Unsupported {
            version: "0.7.0.0".into()
        }
    );
    assert_eq!(d.lock().unwrap().log, ["GetApplicationInfo"]);
    assert!(!file.exists());
}

#[test]
fn no_daemon_is_not_running() {
    let mut connect = || -> io::Result<UnixStream> {
        UnixStream::connect(temp_dir("absent").join("CoreFxPipe_OpenTabletDriver.Daemon"))
    };
    assert_eq!(execute(&mut connect, Command::Refresh), Event::NotRunning);
    let mut refused =
        || -> io::Result<UnixStream> { Err(io::Error::from(io::ErrorKind::ConnectionRefused)) };
    assert_eq!(execute(&mut refused, Command::Refresh), Event::NotRunning);
    let mut denied =
        || -> io::Result<UnixStream> { Err(io::Error::from(io::ErrorKind::PermissionDenied)) };
    assert!(matches!(
        execute(&mut denied, Command::Refresh),
        Event::Failed(_)
    ));
}

#[test]
fn daemon_hanging_up_is_a_failure() {
    let mut connect = || -> io::Result<UnixStream> {
        let (client, server) = UnixStream::pair()?;
        drop(server);
        Ok(client)
    };
    assert!(matches!(
        execute(&mut connect, Command::Refresh),
        Event::Failed(_)
    ));
}

#[test]
fn service_runs_commands_in_background() {
    let d = daemon("0.6.8.1", PathBuf::from("/nonexistent/settings.json"));
    let mut svc = OtdService::spawn_with(connector(d)).unwrap();
    assert!(!svc.is_busy());
    assert!(svc.try_event().is_none());
    svc.send(Command::Refresh);
    assert!(svc.is_busy());
    let deadline = Instant::now() + Duration::from_secs(5);
    let ev = loop {
        if let Some(ev) = svc.try_event() {
            break ev;
        }
        assert!(Instant::now() < deadline, "no event from the worker");
        thread::yield_now();
    };
    assert!(matches!(ev, Event::Snapshot(_)));
    assert!(!svc.is_busy());
}

#[test]
fn a_settings_file_not_named_settings_json_is_not_written() {
    let dir = temp_dir("foreign");
    let file = dir.join("authorized_keys");
    let d = daemon("0.6.8.1", file.clone());
    let mut connect = connector(d.clone());
    let s = Settings::from_value(result(include_str!("data/get_settings.json"))).unwrap();
    assert!(matches!(
        execute(&mut connect, Command::Apply(Box::new(s))),
        Event::Failed(_)
    ));
    assert!(!file.exists());
    assert_eq!(d.lock().unwrap().log, ["GetApplicationInfo"]);
}

#[test]
fn saving_keeps_permissions_and_follows_a_link() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("perms");
    std::fs::create_dir_all(dir.join("real")).unwrap();
    let real = dir.join("real/settings.json");
    std::fs::write(&real, "{}").unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.join("settings.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let s = Settings::from_value(result(include_str!("data/get_settings.json"))).unwrap();
    slam_otd::worker::save_settings_file(&link, &s).unwrap();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read_to_string(&real).unwrap(), s.to_file_json());
    let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let _ = std::fs::remove_dir_all(&dir);
}
