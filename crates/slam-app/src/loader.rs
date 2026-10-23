//! The shell's play loader thread (ADR-0045): prepares plays off the hot paths and frees
//! what the update and draw threads are done with, so neither frees a play's memory.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use slam_audio::{MusicEvent, PlayError};
use slam_osu::mods::ModSet;
use slam_play::HudLabels;
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
    pub options: PlayOptions,
}

/// The settings a play takes when it is prepared, sent with each request so that a
/// change applies to the next play (ADR-0048).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayOptions {
    /// How dark the background is, 0 ..= 1.
    pub dim: f64,
    pub beatmap_skins: bool,
    pub beatmap_colours: bool,
    pub beatmap_hitsounds: bool,
    /// Size multiplier of the hit error meter (ADR-0050).
    pub hit_error_scale: f64,
    /// The chosen skin's folder (ADR-0051); `None` plays on the default skin.
    pub skin: Option<PathBuf>,
    /// The mods the player selected; the settings' fixed field is replaced by this.
    pub mods: ModSet,
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
    /// Each request's [`PlayOptions`] replace the fields they name.
    pub settings: PlaySettings,
    pub limits: Limits,
    pub registry: Arc<FontRegistry>,
    /// The HUD's texts.
    pub hud_labels: HudLabels,
}

/// The update thread's end.
pub struct LoaderLink {
    requests: rtrb::Producer<LoadRequest>,
    results: rtrb::Consumer<LoadResult>,
    trash: rtrb::Producer<Trash>,
}

impl LoaderLink {
    /// Asks for a play of `map`; `false` if too many requests wait.
    pub fn request(&mut self, id: u64, map: PathBuf, options: PlayOptions) -> bool {
        self.requests.push(LoadRequest { id, map, options }).is_ok()
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
pub fn spawn(mut config: LoaderConfig) -> std::io::Result<(LoaderLink, SceneTrash, LoaderThread)> {
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
                    && let Ok(LoadRequest { id, map, options }) = request_rx.pop()
                {
                    idle = false;
                    config.settings.dim = options.dim;
                    config.settings.beatmap_skins = options.beatmap_skins;
                    config.settings.beatmap_colours = options.beatmap_colours;
                    config.settings.beatmap_hitsounds = options.beatmap_hitsounds;
                    config.settings.hit_error_scale = options.hit_error_scale;
                    config.settings.skin = options.skin;
                    config.settings.mods = options.mods;
                    let result = prepare(
                        &map,
                        &config.settings,
                        config.limits,
                        Arc::clone(&config.registry),
                        &config.hud_labels,
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

    use slam_ui::i18n::Localizer;

    use super::*;

    fn options() -> PlayOptions {
        PlayOptions {
            dim: 0.7,
            beatmap_skins: true,
            beatmap_colours: true,
            beatmap_hitsounds: true,
            hit_error_scale: 1.0,
            skin: None,
            mods: ModSet::default(),
        }
    }

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
            hud_labels: crate::play::scene::hud_labels(&Localizer::builtin().unwrap()),
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
        assert!(link.request(7, map, options()));
        let result = wait(&mut link);
        assert_eq!(result.id, 7);
        let prepared = result.result.unwrap();
        assert!(!prepared.beatmap.hit_objects.is_empty());
        assert!(!prepared.scene.started());
        assert_eq!(prepared.scene.dim(), 0.7);
        link.free(Trash::Prepared(prepared));

        assert!(link.request(8, PathBuf::from("/nonexistent/map.osu"), options()));
        let result = wait(&mut link);
        assert_eq!(result.id, 8);
        assert!(result.result.is_err());
        drop(thread);
    }

    #[test]
    fn a_skin_that_does_not_load_falls_back_to_the_default() {
        let (mut link, _scenes, thread) = spawn(config()).unwrap();
        let map =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../slam-osu/tests/data/slider-samples.osu");
        let options = PlayOptions {
            skin: Some(PathBuf::from("/nonexistent/Skins/Gone")),
            ..options()
        };
        assert!(link.request(3, map, options));
        let result = wait(&mut link);
        assert_eq!(result.id, 3);
        link.free(Trash::Prepared(result.result.unwrap()));
        drop(thread);
    }

    #[test]
    fn each_play_takes_the_options_of_its_request() {
        let (mut link, _scenes, thread) = spawn(config()).unwrap();
        let map =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../slam-osu/tests/data/slider-samples.osu");
        for (id, dim) in [(1, 0.25), (2, 0.9)] {
            let options = PlayOptions { dim, ..options() };
            assert!(link.request(id, map.clone(), options));
            let result = wait(&mut link);
            assert_eq!(result.id, id);
            let prepared = result.result.unwrap();
            assert_eq!(prepared.scene.dim(), dim);
            link.free(Trash::Prepared(prepared));
        }
        drop(thread);
    }

    #[test]
    fn a_play_takes_the_mods_of_its_request_and_the_next_does_not_keep_them() {
        use slam_osu::mods::Mod;
        let (mut link, _scenes, thread) = spawn(config()).unwrap();
        let map =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../slam-osu/tests/data/slider-samples.osu");
        let dt = ModSet::new(vec![Mod::from_acronym("DT")]).unwrap();
        for (id, mods) in [(1, dt), (2, ModSet::default())] {
            let options = PlayOptions {
                mods: mods.clone(),
                ..options()
            };
            assert!(link.request(id, map.clone(), options));
            let prepared = wait(&mut link).result.unwrap();
            assert_eq!(prepared.mods, mods);
            link.free(Trash::Prepared(prepared));
        }
        drop(thread);
    }
}
