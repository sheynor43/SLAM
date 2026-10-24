//! The background of the selected beatmap behind the song select (ADR-0066).
//!
//! Three ends: the update thread's [`BackgroundPicker`] waits for the selection to settle
//! and asks for the background; the library thread's [`BackgroundLoader`] finds and reads
//! the file and queues it on the menu's texture loader; the draw thread's
//! [`BackgroundFade`] fades from the background it shows to the asked one and then
//! confirms it, after which the picker releases the older textures. Neither the picker nor
//! the fade allocates, blocks or touches the disk.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use slam_render::atlas::AtlasRegion;
use slam_render::hal::Color;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::upload::{TextureHandle, TextureKind, TextureRequests, TextureState};

use crate::play::find_beatmap_file;
use crate::songs::SongFiles;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/SongSelect.cs (SELECTION_DEBOUNCE)
/// How long the selection stays on one background before it is loaded.
pub const SELECTION_DEBOUNCE_NS: u64 = 150_000_000;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Backgrounds/BackgroundScreenBeatmap.cs (switchBackground)
/// How long a new background fades in (`OutQuint`).
pub const FADE_IN_NS: u64 = 500_000_000;
/// How long the old background fades out (linear).
pub const FADE_OUT_NS: u64 = 250_000_000;

/// How dark the background is behind the song select; a setting later (ADR-0066).
pub const DIM: f32 = 0.7;

/// Textures given back and not yet released: the picker asks for no new background while
/// all are taken.
const OLDER: usize = 4;

/// Commands and answers waiting between the update and the library thread.
const RING: usize = 8;

/// Handles of the menu's texture loader alive at once: the target, the older ones and a
/// few answers in flight.
pub const MAX_BACKGROUNDS: u32 = 8;

/// The background the draw thread is asked to show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackgroundTarget {
    /// Grows with each new target; the draw thread confirms it when shown.
    pub generation: u32,
    /// `None`: the theme's background.
    pub handle: Option<TextureHandle>,
}

/// What the update thread asks of the library thread.
pub enum BackgroundCommand {
    /// Load the background of `files[set][difficulty]`.
    Load {
        generation: u32,
        files: Arc<SongFiles>,
        set: usize,
        difficulty: usize,
    },
    Release(TextureHandle),
}

/// The library thread's answer to a [`BackgroundCommand::Load`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackgroundReply {
    pub generation: u32,
    /// `None` when the file is missing or unreadable or the loader refused it.
    pub handle: Option<TextureHandle>,
}

/// Connects the three ends; `requests` is the menu's texture loader, whose uploads the
/// draw thread processes.
pub fn link(requests: TextureRequests) -> (BackgroundPicker, BackgroundLoader, BackgroundFade) {
    let (commands, commands_rx) = rtrb::RingBuffer::new(RING);
    let (replies_tx, replies) = rtrb::RingBuffer::new(RING);
    let shown = Arc::new(AtomicU32::new(0));
    (
        BackgroundPicker::new(commands, replies, Arc::clone(&shown)),
        BackgroundLoader {
            requests,
            commands: commands_rx,
            replies: replies_tx,
            unsent: None,
            releases: Vec::with_capacity(MAX_BACKGROUNDS as usize),
        },
        BackgroundFade::new(shown),
    )
}

/// The file a selection's background comes from: its set folder and the name in the
/// `.osu` file. `None` without a selection or a background.
fn background_of(files: &SongFiles, set: usize, difficulty: usize) -> Option<(&Path, &str)> {
    let file = files.get(set)?.get(difficulty)?;
    let name = file.background.as_deref()?;
    Some((file.path.parent()?, name))
}

/// A selection the picker remembers: the list it indexes, kept alive by the `Arc`.
#[derive(Clone)]
struct Selected {
    files: Arc<SongFiles>,
    at: Option<(usize, usize)>,
}

impl Selected {
    fn background(&self) -> Option<(&Path, &str)> {
        let (set, difficulty) = self.at?;
        background_of(&self.files, set, difficulty)
    }
}

/// The update thread's end.
pub struct BackgroundPicker {
    commands: rtrb::Producer<BackgroundCommand>,
    replies: rtrb::Consumer<BackgroundReply>,
    /// The generation the draw thread showed last.
    shown: Arc<AtomicU32>,
    /// The newest generation handed out; answers to older ones are released unseen.
    generation: u32,
    target: BackgroundTarget,
    /// Former targets the draw thread may still show.
    older: [Option<TextureHandle>; OLDER],
    /// Releases the full command ring did not take, sent on the next ticks.
    unsent: [Option<TextureHandle>; OLDER],
    /// The selection seen last and since when.
    seen: Option<Selected>,
    seen_ns: u64,
    /// The selection the newest generation is for.
    asked: Option<Selected>,
}

impl BackgroundPicker {
    fn new(
        commands: rtrb::Producer<BackgroundCommand>,
        replies: rtrb::Consumer<BackgroundReply>,
        shown: Arc<AtomicU32>,
    ) -> Self {
        Self {
            commands,
            replies,
            shown,
            generation: 0,
            target: BackgroundTarget::default(),
            older: [None; OLDER],
            unsent: [None; OLDER],
            seen: None,
            seen_ns: 0,
            asked: None,
        }
    }

    /// Follows the selection `selected` of `files` at `now_ns` and returns the background
    /// to show. Neither blocks nor allocates; clones of `files` are freed by the library
    /// thread, which keeps its lists until only it holds them.
    pub fn tick(
        &mut self,
        now_ns: u64,
        files: &Arc<SongFiles>,
        selected: Option<(usize, usize)>,
    ) -> BackgroundTarget {
        while let Ok(reply) = self.replies.pop() {
            if reply.generation == self.generation {
                self.replace_target(BackgroundTarget {
                    generation: reply.generation,
                    handle: reply.handle,
                });
            } else if let Some(handle) = reply.handle {
                // Never shown: free at once.
                self.release(handle);
            }
        }
        for slot in &mut self.unsent {
            if let Some(handle) = *slot
                && self
                    .commands
                    .push(BackgroundCommand::Release(handle))
                    .is_ok()
            {
                *slot = None;
            }
        }
        if self.shown.load(Ordering::Acquire) == self.target.generation {
            for slot in &mut self.older {
                if let Some(handle) = *slot
                    && self
                        .commands
                        .push(BackgroundCommand::Release(handle))
                        .is_ok()
                {
                    *slot = None;
                }
            }
        }
        let now_at = selected.and_then(|(s, d)| background_of(files, s, d));
        let seen_at = self.seen.as_ref().and_then(Selected::background);
        if self.seen.is_none() || now_at != seen_at {
            self.seen = Some(Selected {
                files: Arc::clone(files),
                at: selected,
            });
            self.seen_ns = now_ns;
        } else if now_ns.saturating_sub(self.seen_ns) >= SELECTION_DEBOUNCE_NS {
            self.ask(now_at.is_some());
        }
        self.target
    }

    /// Asks for the settled selection's background unless it was asked for already.
    fn ask(&mut self, has_background: bool) {
        // Never asked counts as asked for none: the first target is the theme's.
        let asked_at = self.asked.as_ref().and_then(Selected::background);
        let seen_at = self.seen.as_ref().and_then(Selected::background);
        if asked_at == seen_at {
            return;
        }
        // Wait until the draw thread lets go of the older backgrounds.
        if self.older.iter().all(Option::is_some) {
            return;
        }
        let Some(seen) = &self.seen else { return };
        let generation = self.generation.wrapping_add(1);
        if has_background {
            let Some((set, difficulty)) = seen.at else {
                return;
            };
            let command = BackgroundCommand::Load {
                generation,
                files: Arc::clone(&seen.files),
                set,
                difficulty,
            };
            if self.commands.push(command).is_err() {
                return;
            }
            self.generation = generation;
        } else {
            self.generation = generation;
            self.replace_target(BackgroundTarget {
                generation,
                handle: None,
            });
        }
        self.asked = self.seen.clone();
    }

    fn replace_target(&mut self, target: BackgroundTarget) {
        if let Some(old) = self.target.handle {
            self.retire(old);
        }
        self.target = target;
    }

    /// Releases `handle` once the draw thread shows the target; with every slot taken,
    /// at once (the draw thread may lose a background for a moment).
    fn retire(&mut self, handle: TextureHandle) {
        if let Some(slot) = self.older.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(handle);
        } else {
            self.release(handle);
        }
    }

    /// Sends a release now, or on a later tick while the command ring is full. With that
    /// queue full too the handle is lost: the library thread stalled for many ticks.
    fn release(&mut self, handle: TextureHandle) {
        if self
            .commands
            .push(BackgroundCommand::Release(handle))
            .is_err()
            && let Some(slot) = self.unsent.iter_mut().find(|slot| slot.is_none())
        {
            *slot = Some(handle);
        }
    }
}

/// The library thread's end: owns the menu's texture requests.
pub struct BackgroundLoader {
    requests: TextureRequests,
    commands: rtrb::Consumer<BackgroundCommand>,
    replies: rtrb::Producer<BackgroundReply>,
    /// An answer the full ring did not take yet.
    unsent: Option<BackgroundReply>,
    /// Releases the full request queue did not take yet.
    releases: Vec<TextureHandle>,
}

impl BackgroundLoader {
    /// Handles the waiting commands: releases all, loads only the newest of the loads.
    /// Reads the file here, so the library thread waits for the disk.
    pub fn poll(&mut self) {
        self.releases
            .retain(|&handle| self.requests.release(handle).is_err_and(is_queue_full));
        if let Some(reply) = self.unsent.take()
            && let Err(rtrb::PushError::Full(reply)) = self.replies.push(reply)
        {
            self.unsent = Some(reply);
            return;
        }
        let mut load = None;
        while let Ok(command) = self.commands.pop() {
            match command {
                BackgroundCommand::Release(handle) => self.release(handle),
                BackgroundCommand::Load {
                    generation,
                    files,
                    set,
                    difficulty,
                } => load = Some((generation, files, set, difficulty)),
            }
        }
        let Some((generation, files, set, difficulty)) = load else {
            return;
        };
        let handle = background_of(&files, set, difficulty).and_then(|(dir, name)| {
            let bytes = find_beatmap_file(dir, name).and_then(|path| std::fs::read(path).ok());
            let Some(bytes) = bytes else {
                tracing::warn!("background {name}: not found or unreadable");
                return None;
            };
            self.requests
                .load(bytes, TextureKind::Standalone)
                .inspect_err(|e| tracing::warn!("background {name}: {}", e.reason))
                .ok()
        });
        let reply = BackgroundReply { generation, handle };
        if let Err(rtrb::PushError::Full(reply)) = self.replies.push(reply) {
            self.unsent = Some(reply);
        }
    }

    fn release(&mut self, handle: TextureHandle) {
        if self.requests.release(handle).is_err_and(is_queue_full) {
            self.releases.push(handle);
        }
    }
}

fn is_queue_full(e: slam_render::upload::ReleaseError) -> bool {
    e == slam_render::upload::ReleaseError::QueueFull
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/DefaultEasingFunction.cs (OutQuint)
fn out_quint(time: f64) -> f64 {
    let t = time - 1.0;
    t * t * t * t * t + 1.0
}

/// The draw thread's end: fades between backgrounds.
pub struct BackgroundFade {
    shown_generation: Arc<AtomicU32>,
    /// The background shown whole, under a fade.
    shown: Option<TextureHandle>,
    target: BackgroundTarget,
    /// When the target was ready (or found to have no image) and its fade began.
    fade_from_ns: Option<u64>,
}

/// One sprite of a background: its texture and opacity.
type Layer = (TextureHandle, f32);

impl BackgroundFade {
    fn new(shown_generation: Arc<AtomicU32>) -> Self {
        Self {
            shown_generation,
            shown: None,
            target: BackgroundTarget::default(),
            fade_from_ns: None,
        }
    }

    /// Advances to `target` at `now_ns`; `state` tells what the loader made of a handle.
    pub fn advance(
        &mut self,
        target: BackgroundTarget,
        now_ns: u64,
        state: impl Fn(TextureHandle) -> TextureState,
    ) {
        if target.generation != self.target.generation {
            // A new target restarts the fade from the background shown whole.
            self.target = target;
            self.fade_from_ns = None;
        }
        if self.shown_generation.load(Ordering::Relaxed) == self.target.generation {
            return;
        }
        // Failed: fade out to the theme as without an image.
        let image = match self.target.handle.map(&state) {
            Some(TextureState::Pending) => return,
            Some(TextureState::Ready(_)) => self.target.handle,
            Some(TextureState::Failed(_)) | None => None,
        };
        let from = *self.fade_from_ns.get_or_insert(now_ns);
        let length = if image.is_some() {
            FADE_IN_NS
        } else {
            FADE_OUT_NS
        };
        if now_ns.saturating_sub(from) >= length {
            self.shown = image;
            self.fade_from_ns = None;
            self.shown_generation
                .store(self.target.generation, Ordering::Release);
        }
    }

    /// What to draw at `now_ns`, bottom first: the target fading in under the shown
    /// background fading out (lazer puts the new one behind, `Depth + 1`).
    fn layers(&self, now_ns: u64) -> [Option<Layer>; 2] {
        let Some(from) = self.fade_from_ns else {
            return [self.shown.map(|h| (h, 1.0)), None];
        };
        let elapsed = now_ns.saturating_sub(from) as f64;
        let out = (1.0 - elapsed / FADE_OUT_NS as f64).max(0.0) as f32;
        let fade_in = out_quint((elapsed / FADE_IN_NS as f64).min(1.0)) as f32;
        [
            self.target.handle.map(|h| (h, fade_in)),
            self.shown.map(|h| (h, out)),
        ]
    }

    /// Pushes the background covering a `width` x `height` window at `now_ns`, faded by
    /// `opacity` (the layer's). Nothing for handles not ready: the theme shows.
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        width: f32,
        height: f32,
        now_ns: u64,
        opacity: f32,
        region: impl Fn(TextureHandle) -> Option<AtlasRegion>,
    ) {
        let grey = 1.0 - DIM;
        for (handle, alpha) in self.layers(now_ns).into_iter().flatten() {
            let Some(region) = region(handle) else {
                continue;
            };
            let (position, size) = slam_play::background::cover(
                [region.size.width as f32, region.size.height as f32],
                [width, height],
            );
            let mut sprite = Sprite::new(region.texture, region.uv, position, size);
            sprite.colour = Color {
                r: grey,
                g: grey,
                b: grey,
                a: alpha * opacity,
            };
            batch.push(&sprite);
        }
    }
}

#[cfg(test)]
mod tests;
