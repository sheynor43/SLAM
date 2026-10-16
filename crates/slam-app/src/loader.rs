//! The shell's play loader thread (ADR-0045): prepares plays off the hot paths and frees
//! what the update and draw threads are done with, so neither frees a play's memory.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use slam_audio::{MusicEvent, PlayError};
use slam_render::Limits;
use slam_text::FontRegistry;

use crate::play::scene::{PlayScene, PlaySettings, Prepared, prepare};

/// Requests and results in flight: one play is prepared at a time.
const RING: usize = 4;
/// Things to free; when full, the sender frees them itself.
const TRASH: usize = 16;
/// How long the loader sleeps when there is nothing to do.
const POLL: Duration = Duration::from_millis(5);

/// A play to prepare, named by the update thread.
pub struct LoadRequest {
    pub id: u64,
    pub map: PathBuf,
}

/// A prepared play, or why it could not be.
pub struct LoadResult {
    pub id: u64,
    pub result: Result<Box<Prepared>, String>,
}

/// Something the update thread is done with; messages are logged here.
pub enum Trash {
    Prepared(Box<Prepared>),
    /// Any other value: a finished play's session and beatmap.
    Other(Box<dyn Send>),
    /// A failed preparation's message, logged when it failed.
    Message(String),
    /// A track that failed to open.
    Music(MusicEvent),
    /// A track that could not be asked for.
    TrackCommand(PlayError),
}

fn log(trash: Trash) {
    match trash {
        Trash::Music(MusicEvent::Failed { error, .. }) => tracing::warn!("track: {error}"),
        Trash::TrackCommand(error) => tracing::warn!("track: {error}"),
        _ => {}
    }
}

/// What the loader needs to prepare a play.
pub struct LoaderConfig {
    pub settings: PlaySettings,
    pub limits: Limits,
    pub registry: Arc<FontRegistry>,
    /// The skip button's text.
    pub skip_label: String,
}

/// The update thread's end.
pub struct LoaderLink {
    requests: rtrb::Producer<LoadRequest>,
    results: rtrb::Consumer<LoadResult>,
    trash: rtrb::Producer<Trash>,
}

impl LoaderLink {
    /// Asks for a play of `map`; `false` if too many requests wait.
    pub fn request(&mut self, id: u64, map: PathBuf) -> bool {
        self.requests.push(LoadRequest { id, map }).is_ok()
    }

    /// A finished preparation, if any.
    pub fn poll(&mut self) -> Option<LoadResult> {
        self.results.pop().ok()
    }

    /// Hands `trash` to the loader to free; frees it here if the loader is behind.
    pub fn free(&mut self, trash: Trash) {
        let _ = self.trash.push(trash);
    }
}

/// The draw thread's end: scenes it no longer shows.
pub struct SceneTrash {
    trash: rtrb::Producer<PlayScene>,
}

impl SceneTrash {
    /// Hands `scene` (stopped) to the loader to free; frees it here if the loader is behind.
    pub fn free(&mut self, scene: PlayScene) {
        let _ = self.trash.push(scene);
    }
}

/// The loader thread; stops and joins on drop, freeing what is left.
pub struct LoaderThread {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for LoaderThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts the loader on a thread named `slam-play-loader`.
pub fn spawn(config: LoaderConfig) -> std::io::Result<(LoaderLink, SceneTrash, LoaderThread)> {
    let (requests, mut request_rx) = rtrb::RingBuffer::new(RING);
    let (mut result_tx, results) = rtrb::RingBuffer::new(RING);
    let (trash, mut trash_rx) = rtrb::RingBuffer::<Trash>::new(TRASH);
    let (scene_trash, mut scene_rx) = rtrb::RingBuffer::<PlayScene>::new(TRASH);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("slam-play-loader".to_owned())
        .spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                let mut idle = true;
                while let Ok(trash) = trash_rx.pop() {
                    log(trash);
                    idle = false;
                }
                while scene_rx.pop().is_ok() {
                    idle = false;
                }
                if result_tx.slots() > 0
                    && let Ok(LoadRequest { id, map }) = request_rx.pop()
                {
                    idle = false;
                    let result = prepare(
                        &map,
                        &config.settings,
                        config.limits,
                        Arc::clone(&config.registry),
                        &config.skip_label,
                    )
                    .map(Box::new)
                    .inspect_err(|e| tracing::warn!("{e}"));
                    // A slot was free and this thread is the only producer.
                    let _ = result_tx.push(LoadResult { id, result });
                }
                if idle {
                    std::thread::sleep(POLL);
                }
            }
        })?;
    Ok((
        LoaderLink {
            requests,
            results,
            trash,
        },
        SceneTrash { trash: scene_trash },
        LoaderThread {
            stop,
            thread: Some(thread),
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Instant;

    use super::*;

    fn config() -> LoaderConfig {
        LoaderConfig {
            settings: PlaySettings::default(),
            limits: Limits {
                max_texture_size: 4096,
                max_uniform_block_size: 16384,
                max_uniform_buffer_slots: 12,
                max_texture_slots: 16,
            },
            registry: Arc::new(FontRegistry::with_builtin()),
            skip_label: "Skip".to_owned(),
        }
    }

    fn wait(link: &mut LoaderLink) -> LoadResult {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(result) = link.poll() {
                return result;
            }
            assert!(Instant::now() < deadline, "the loader did not answer");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn plays_are_prepared_and_failures_reported() {
        let (mut link, _scenes, thread) = spawn(config()).unwrap();
        let map =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../slam-osu/tests/data/slider-samples.osu");
        assert!(link.request(7, map));
        let result = wait(&mut link);
        assert_eq!(result.id, 7);
        let prepared = result.result.unwrap();
        assert!(!prepared.beatmap.hit_objects.is_empty());
        assert!(!prepared.scene.started());
        link.free(Trash::Prepared(prepared));

        assert!(link.request(8, PathBuf::from("/nonexistent/map.osu")));
        let result = wait(&mut link);
        assert_eq!(result.id, 8);
        assert!(result.result.is_err());
        drop(thread);
    }
}
