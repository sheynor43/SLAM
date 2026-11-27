//! The update thread's logic, apart from the thread: it takes a command and reports events.

use std::fs::{self, File};
use std::path::PathBuf;

use super::fetch::{Fetch, FetchError};
use super::release::{self, Found};
use super::verify::{self, Reject};
use super::{Available, Channel, Command, Config, Event, Failure, State, install};

/// The most of the releases list read.
const MAX_LIST: u64 = 8 << 20;
/// The most of a `.sig` file read.
const MAX_SIGNATURE: u64 = 4096;

pub struct Worker<F> {
    config: Config,
    fetch: F,
    found: Option<Found>,
    /// The verified download waiting for the install.
    ready: Option<PathBuf>,
}

impl<F: Fetch> Worker<F> {
    pub fn new(config: Config, fetch: F) -> Worker<F> {
        Worker {
            config,
            fetch,
            found: None,
            ready: None,
        }
    }

    /// What runs once when the thread starts: deletes what the last update left.
    pub fn start_up(&mut self) {
        if let Some(install::Target::Portable(exe)) = &self.config.target {
            install::clean_old(exe);
        }
        // Downloads of an earlier run: installed already, or never wanted.
        if let Ok(entries) = fs::read_dir(&self.config.dir) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    pub fn handle(&mut self, command: Command, emit: &mut dyn FnMut(Event)) {
        match command {
            Command::Check { channel, manual } => self.check(channel, manual, emit),
            Command::Download => self.download(emit),
            Command::Install => self.install(emit),
        }
    }

    fn state(emit: &mut dyn FnMut(Event), state: State) {
        emit(Event::State(state));
    }

    fn check(&mut self, channel: Channel, manual: bool, emit: &mut dyn FnMut(Event)) {
        if !verify::key_is_set(&self.config.key) {
            Self::state(emit, State::Unavailable);
            return;
        }
        Self::state(emit, State::Checking);
        let answer = self
            .fetch
            .get(&self.config.releases_url, MAX_LIST)
            .map_err(|e| e.to_string())
            .and_then(|json| {
                release::select(&json, channel, &self.config.current, release::asset_name)
                    .map_err(|e| e.0)
            });
        match answer {
            Ok(None) => {
                self.found = None;
                Self::state(emit, State::UpToDate);
            }
            Ok(Some(found)) => {
                tracing::info!("update: {} is available", found.tag);
                let installable = found.asset.is_some() && self.config.target.is_some();
                let available = Available {
                    version: found.version.to_string(),
                    page_url: found.page_url.clone(),
                    installable,
                };
                self.found = Some(found);
                Self::state(emit, State::Available(available));
            }
            Err(e) => {
                tracing::warn!("update: check failed: {e}");
                // A start-up check says nothing when it fails.
                Self::state(
                    emit,
                    if manual {
                        State::Failed(Failure::Network)
                    } else {
                        State::Idle
                    },
                );
            }
        }
    }

    fn download(&mut self, emit: &mut dyn FnMut(Event)) {
        let Some(found) = self.found.clone() else {
            return;
        };
        let Some(asset) = found.asset.clone() else {
            return;
        };
        if !verify::key_is_set(&self.config.key) || self.config.target.is_none() {
            Self::state(emit, State::Unavailable);
            return;
        }
        Self::state(emit, State::Downloading(0));
        match self.fetch_verified(&asset, emit) {
            Ok(path) => {
                tracing::info!("update: {} downloaded and verified", found.tag);
                self.ready = Some(path);
                Self::state(
                    emit,
                    State::Ready {
                        version: found.version.to_string(),
                    },
                );
            }
            Err(failure) => Self::state(emit, State::Failed(failure)),
        }
    }

    fn fetch_verified(
        &self,
        asset: &release::AssetRef,
        emit: &mut dyn FnMut(Event),
    ) -> Result<PathBuf, Failure> {
        let network = |e: FetchError| {
            tracing::warn!("update: download failed: {e}");
            Failure::Network
        };
        let io = |e: std::io::Error| {
            tracing::warn!("update: cannot write the download: {e}");
            Failure::Install
        };
        fs::create_dir_all(&self.config.dir).map_err(io)?;
        let signature = self
            .fetch
            .get(&asset.sig_url, MAX_SIGNATURE)
            .map_err(network)?;
        let signature = String::from_utf8_lossy(&signature).into_owned();
        let part = self.config.dir.join(format!("{}.part", asset.name));
        let result = (|| {
            let mut file = File::create(&part).map_err(io)?;
            let mut percent = 0u8;
            self.fetch
                .download(&asset.url, &mut file, &mut |got, total| {
                    let now = total
                        .filter(|t| *t > 0)
                        .map_or(0, |t| (got.saturating_mul(100) / t).min(100) as u8);
                    if now != percent {
                        percent = now;
                        Self::state(emit, State::Downloading(now));
                    }
                })
                .map_err(network)?;
            file.sync_all().map_err(io)?;
            drop(file);
            let digest = verify::digest_file(&part).map_err(io)?;
            verify::verify(&self.config.key, &digest, &signature).map_err(|reject| {
                tracing::warn!("update: {} rejected: {reject:?}", asset.name);
                match reject {
                    Reject::NoKey | Reject::BadFormat | Reject::Mismatch => Failure::Signature,
                }
            })
        })();
        match result {
            Ok(()) => {
                let path = self.config.dir.join(&asset.name);
                fs::rename(&part, &path).map_err(io)?;
                Ok(path)
            }
            Err(failure) => {
                let _ = fs::remove_file(&part);
                Err(failure)
            }
        }
    }

    fn install(&mut self, emit: &mut dyn FnMut(Event)) {
        let (Some(path), Some(target)) = (self.ready.clone(), self.config.target.clone()) else {
            return;
        };
        match install::install(&path, &target) {
            Ok(()) => {
                tracing::info!("update: installed, restarting");
                self.ready = None;
                let _ = fs::remove_file(&path);
                emit(Event::Installed);
            }
            Err(e) => {
                tracing::warn!("update: install failed: {e}");
                Self::state(emit, State::Failed(Failure::Install));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::Arc;

    use sha2::{Digest, Sha256};

    use super::super::verify::tests::{sign_hex, test_key};
    use super::*;

    #[derive(Clone, Default)]
    struct Mock(Arc<HashMap<String, Vec<u8>>>);

    impl Fetch for Mock {
        fn get(&self, url: &str, _max: u64) -> Result<Vec<u8>, FetchError> {
            self.0
                .get(url)
                .cloned()
                .ok_or_else(|| FetchError(format!("no {url}")))
        }

        fn download(
            &self,
            url: &str,
            dest: &mut dyn Write,
            progress: &mut dyn FnMut(u64, Option<u64>),
        ) -> Result<(), FetchError> {
            let body = self.get(url, u64::MAX)?;
            for (i, chunk) in body.chunks(4).enumerate() {
                dest.write_all(chunk)?;
                progress(
                    ((i + 1) * 4).min(body.len()) as u64,
                    Some(body.len() as u64),
                );
            }
            Ok(())
        }
    }

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir =
                std::env::temp_dir().join(format!("slam-update-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tag_json(version: &str) -> Vec<u8> {
        let name = release::asset_name(&format!("v{version}")).unwrap_or_default();
        format!(
            r#"[{{"tag_name":"v{version}","prerelease":true,"html_url":"page","assets":[
            {{"name":"{name}","browser_download_url":"https://x/file"}},
            {{"name":"{name}.sig","browser_download_url":"https://x/sig"}}]}}]"#
        )
        .into_bytes()
    }

    fn config(dir: &Temp, key: [u8; 32], target: Option<install::Target>) -> Config {
        Config {
            key,
            releases_url: "https://x/list".to_owned(),
            dir: dir.0.join("update"),
            current: semver::Version::parse("0.1.0-alpha").unwrap(),
            target,
        }
    }

    fn states(worker: &mut Worker<Mock>, command: Command) -> Vec<Event> {
        let mut events = Vec::new();
        worker.handle(command, &mut |e| events.push(e));
        events
    }

    fn check() -> Command {
        Command::Check {
            channel: Channel::Prerelease,
            manual: true,
        }
    }

    fn mock(version: &str, file: &[u8], signature: Option<String>) -> Mock {
        let (signing, _) = test_key();
        let digest: [u8; 32] = Sha256::digest(file).into();
        let mut files = HashMap::new();
        files.insert("https://x/list".to_owned(), tag_json(version));
        files.insert("https://x/file".to_owned(), file.to_vec());
        files.insert(
            "https://x/sig".to_owned(),
            signature
                .unwrap_or_else(|| sign_hex(&signing, &digest))
                .into_bytes(),
        );
        Mock(Arc::new(files))
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn check_download_verify_and_install_on_an_appimage() {
        let dir = Temp::new("full");
        let app = dir.0.join("SLAM.AppImage");
        fs::write(&app, b"old").unwrap();
        let (_, public) = test_key();
        let target = install::Target::AppImage(app.clone());
        let mut worker = Worker::new(
            config(&dir, public, Some(target)),
            mock("0.2.0-alpha", b"new build bytes", None),
        );
        let events = states(&mut worker, check());
        assert_eq!(events[0], Event::State(State::Checking));
        assert_eq!(
            events[1],
            Event::State(State::Available(Available {
                version: "0.2.0-alpha".into(),
                page_url: "page".into(),
                installable: true,
            }))
        );
        let events = states(&mut worker, Command::Download);
        assert_eq!(events[0], Event::State(State::Downloading(0)));
        assert!(events.contains(&Event::State(State::Downloading(100))));
        assert_eq!(
            events.last(),
            Some(&Event::State(State::Ready {
                version: "0.2.0-alpha".into()
            }))
        );
        assert_eq!(states(&mut worker, Command::Install), [Event::Installed]);
        assert_eq!(fs::read(&app).unwrap(), b"new build bytes");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_wrong_signature_deletes_the_download_and_fails() {
        let dir = Temp::new("badsig");
        let (_, public) = test_key();
        let target = install::Target::AppImage(dir.0.join("SLAM.AppImage"));
        let (signing, _) = test_key();
        let other: [u8; 32] = Sha256::digest(b"something else").into();
        let other = sign_hex(&signing, &other);
        let mut worker = Worker::new(
            config(&dir, public, Some(target)),
            mock("0.2.0-alpha", b"tampered", Some(other)),
        );
        states(&mut worker, check());
        let events = states(&mut worker, Command::Download);
        assert_eq!(
            events.last(),
            Some(&Event::State(State::Failed(Failure::Signature)))
        );
        let left: Vec<_> = fs::read_dir(dir.0.join("update")).unwrap().collect();
        assert!(left.is_empty());
        // Nothing to install after a failure.
        assert!(states(&mut worker, Command::Install).is_empty());
    }

    #[test]
    fn the_placeholder_key_makes_updates_unavailable_without_asking_the_server() {
        let dir = Temp::new("placeholder");
        let mut worker = Worker::new(
            config(&dir, verify::PLACEHOLDER_KEY, None),
            // An empty server: any request would fail the check instead.
            Mock::default(),
        );
        assert_eq!(
            states(&mut worker, check()),
            [Event::State(State::Unavailable)]
        );
        assert!(states(&mut worker, Command::Download).is_empty());
    }

    #[test]
    fn a_silent_check_forgets_a_network_failure_and_a_manual_one_tells() {
        let dir = Temp::new("offline");
        let (_, public) = test_key();
        let mut worker = Worker::new(config(&dir, public, None), Mock::default());
        let quiet = Command::Check {
            channel: Channel::Prerelease,
            manual: false,
        };
        assert_eq!(
            states(&mut worker, quiet).last(),
            Some(&Event::State(State::Idle))
        );
        assert_eq!(
            states(&mut worker, check()).last(),
            Some(&Event::State(State::Failed(Failure::Network)))
        );
    }

    #[test]
    fn the_same_version_is_up_to_date_and_without_a_target_it_is_not_installable() {
        let dir = Temp::new("uptodate");
        let (_, public) = test_key();
        let mut worker = Worker::new(config(&dir, public, None), mock("0.1.0-alpha", b"x", None));
        assert_eq!(
            states(&mut worker, check()).last(),
            Some(&Event::State(State::UpToDate))
        );
        let mut worker = Worker::new(config(&dir, public, None), mock("0.3.0-alpha", b"x", None));
        let events = states(&mut worker, check());
        let Some(Event::State(State::Available(a))) = events.last() else {
            panic!("{events:?}");
        };
        assert!(!a.installable);
        // No target: a download is refused.
        assert_eq!(
            states(&mut worker, Command::Download),
            [Event::State(State::Unavailable)]
        );
    }

    #[test]
    fn start_up_deletes_the_old_executable_and_stale_downloads() {
        let dir = Temp::new("startup");
        let exe = dir.0.join("slam.exe");
        fs::write(install::old_path(&exe), b"old").unwrap();
        let update = dir.0.join("update");
        fs::create_dir_all(&update).unwrap();
        fs::write(update.join("stale.part"), b"x").unwrap();
        let (_, public) = test_key();
        let mut worker = Worker::new(
            config(&dir, public, Some(install::Target::Portable(exe.clone()))),
            Mock::default(),
        );
        worker.start_up();
        assert!(!install::old_path(&exe).exists());
        assert!(!update.join("stale.part").exists());
    }

    #[test]
    fn the_shipped_key_is_set_and_decodes_to_a_point() {
        assert!(crate::update::available());
        assert!(ed25519_dalek::VerifyingKey::from_bytes(&crate::update::UPDATE_PUBLIC_KEY).is_ok());
    }
}
