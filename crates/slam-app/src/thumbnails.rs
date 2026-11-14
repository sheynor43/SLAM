//! Thumbnails of the beatmaps in the song list (ADR-0127).
//!
//! Four ends. The update thread's [`Thumbnails`] looks at the rows in view, asks for the
//! missing thumbnails and keeps the loaded ones in a small LRU of fixed size. The
//! `slam-thumbnails` thread ([`worker`]) makes the pixels; the library thread's
//! [`ThumbBridge`] queues them on the menu's texture loader (whose requests it owns, like
//! the backgrounds, ADR-0066) and answers with the handle. The draw thread shows the
//! handles of the snapshot and tells the update thread which snapshot it drew last, after
//! which a thumbnail that left the cache is released.
//!
//! Neither the update thread's end nor the draw thread's reads a file, blocks or allocates.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use slam_render::image::{Image, ImageLimits};
use slam_render::upload::{ReleaseError, TextureHandle, TextureKind, TextureRequests};
use slam_ui::song_select::MAX_VISIBLE_ROWS;

use crate::songs::SongFiles;

pub mod make;
pub mod worker;

pub use worker::{Made, ThumbRequest, ThumbWorker, WorkerOut};

/// Textures kept in memory.
pub const CAPACITY: usize = 64;
/// Rows asked for above the snapshot's first row.
pub const MARGIN: usize = 2;
/// Rows the update thread wants thumbnails of: the margin and the snapshot's rows.
pub const WANTED: usize = MARGIN + MAX_VISIBLE_ROWS;
/// Requests waiting for the thumbnail thread or for its answer at once.
pub const MAX_IN_FLIGHT: usize = 12;
/// Evicted textures waiting for the draw thread to let go of them.
const RELEASING: usize = 192;
/// Texture handles the thumbnails may hold alive at once: the cache, the ones waiting to
/// be released after a change of height and the answers in flight.
pub const MAX_HANDLES: u32 = (CAPACITY + RELEASING + MAX_IN_FLIGHT) as u32;

const REQUEST_RING: usize = 16;
const OUT_RING: usize = 8;
const REPLY_RING: usize = 32;
const RELEASE_RING: usize = 256;

/// What the library thread says of a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ready(TextureHandle),
    /// No thumbnail: no background, unreadable, or the loader refused it.
    Missing,
    /// To be asked again: the thumbnail thread's queue overflowed.
    Dropped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThumbReply {
    pub id: u32,
    pub outcome: Outcome,
}

/// The thumbnails of the snapshot's rows, as the draw thread gets them.
#[derive(Clone, Copy, Debug)]
pub struct ThumbSnapshot {
    /// Grows with every update tick; the draw thread tells the update thread the last one
    /// it drew.
    pub seq: u32,
    /// The rows have room for thumbnails.
    pub enabled: bool,
    /// By snapshot row.
    pub handles: [Option<TextureHandle>; MAX_VISIBLE_ROWS],
}

impl Default for ThumbSnapshot {
    fn default() -> Self {
        Self {
            seq: 0,
            enabled: false,
            handles: [None; MAX_VISIBLE_ROWS],
        }
    }
}

/// The draw thread's end: the sequence number of the snapshot drawn last.
#[derive(Clone, Debug, Default)]
pub struct ThumbShown(Arc<AtomicU32>);

impl ThumbShown {
    /// Records that the snapshot `seq` is being drawn.
    pub fn set(&self, seq: u32) {
        self.0.store(seq, Ordering::Release);
    }
}

/// Hashes the background of `files[set][0]` (its folder and name): what makes two rows'
/// thumbnails the same. `None` for a set without a background.
fn background_key(files: &SongFiles, set: usize) -> Option<u64> {
    let file = files.get(set)?.first()?;
    let name = file.background.as_deref()?;
    let dir = file.path.parent()?;
    let mut hash = make::fnv_start();
    hash = make::fnv(hash, dir.as_os_str().as_encoded_bytes());
    hash = make::fnv(hash, &[0]);
    Some(make::fnv(hash, name.as_bytes()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Empty,
    Loading(u32),
    Ready(TextureHandle),
    Missing,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    key: u64,
    state: State,
    /// The tick that last wanted it.
    used: u64,
}

const EMPTY: Entry = Entry {
    key: 0,
    state: State::Empty,
    used: 0,
};

/// Whether `shown` is at or past `seq`, with the counter allowed to wrap.
fn reached(shown: u32, seq: u32) -> bool {
    shown.wrapping_sub(seq) < u32::MAX / 2
}

/// The update thread's end.
pub struct Thumbnails {
    requests: rtrb::Producer<ThumbRequest>,
    replies: rtrb::Consumer<ThumbReply>,
    releases: rtrb::Producer<TextureHandle>,
    shown: Arc<AtomicU32>,
    /// The height asked for; `None`: off.
    height: Option<u32>,
    entries: [Entry; CAPACITY],
    in_flight: usize,
    next_id: u32,
    ticks: u64,
    /// Textures that left the cache, with the snapshot they were last in; released once
    /// the draw thread has drawn a later one.
    releasing: [Option<(TextureHandle, u32)>; RELEASING],
}

impl Thumbnails {
    fn new(
        requests: rtrb::Producer<ThumbRequest>,
        replies: rtrb::Consumer<ThumbReply>,
        releases: rtrb::Producer<TextureHandle>,
        shown: Arc<AtomicU32>,
    ) -> Thumbnails {
        Thumbnails {
            requests,
            replies,
            releases,
            shown,
            height: None,
            entries: [EMPTY; CAPACITY],
            in_flight: 0,
            next_id: 0,
            ticks: 0,
            releasing: [None; RELEASING],
        }
    }

    /// Follows the rows in view at snapshot `seq`: `wanted` has the set of each row
    /// (the `MARGIN` rows above the snapshot's first row, then the snapshot's rows;
    /// `None` for difficulty rows and rows past the end). Returns the loaded thumbnails
    /// in the same order. Allocates nothing; `files` is cloned into requests as an `Arc`.
    pub fn tick(
        &mut self,
        seq: u32,
        height: Option<u32>,
        files: &Arc<SongFiles>,
        wanted: &[Option<usize>; WANTED],
    ) -> [Option<TextureHandle>; WANTED] {
        self.ticks += 1;
        self.receive(seq);
        self.flush_releases();
        if height != self.height {
            self.reset(seq);
            self.height = height;
        }
        let mut out = [None; WANTED];
        let Some(height) = self.height else {
            return out;
        };
        let mut keys = [None; WANTED];
        for (i, set) in wanted.iter().enumerate() {
            let Some(key) = set.and_then(|set| background_key(files, set)) else {
                continue;
            };
            keys[i] = Some(key);
            if let Some(at) = self.find(key) {
                self.entries[at].used = self.ticks;
                if let State::Ready(handle) = self.entries[at].state {
                    out[i] = Some(handle);
                }
            }
        }
        // The first visible row last: the thumbnail thread takes the newest first.
        let order = (0..MARGIN).chain((MARGIN..WANTED).rev());
        for i in order {
            let (Some(key), Some(set)) = (keys[i], wanted[i]) else {
                continue;
            };
            if self.find(key).is_some()
                || self.in_flight >= MAX_IN_FLIGHT
                || self.requests.slots() == 0
            {
                continue;
            }
            let Some(at) = self.free_entry(seq) else {
                continue;
            };
            let id = self.next_id;
            let request = ThumbRequest {
                id,
                height,
                files: Arc::clone(files),
                set,
                difficulty: 0,
            };
            if self.requests.push(request).is_err() {
                continue;
            }
            self.next_id = id.wrapping_add(1);
            self.in_flight += 1;
            self.entries[at] = Entry {
                key,
                state: State::Loading(id),
                used: self.ticks,
            };
        }
        out
    }

    fn find(&self, key: u64) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.state != State::Empty && e.key == key)
    }

    /// A free entry, evicting the least recently wanted one if need be; `None` when every
    /// entry is wanted by this tick.
    fn free_entry(&mut self, seq: u32) -> Option<usize> {
        if let Some(at) = self.entries.iter().position(|e| e.state == State::Empty) {
            return Some(at);
        }
        let at = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.used < self.ticks)
            .min_by_key(|(_, e)| e.used)
            .map(|(at, _)| at)?;
        if let State::Ready(handle) = self.entries[at].state {
            self.retire(handle, seq);
        }
        self.entries[at] = EMPTY;
        Some(at)
    }

    /// Takes the library thread's answers.
    fn receive(&mut self, seq: u32) {
        while let Ok(reply) = self.replies.pop() {
            self.in_flight = self.in_flight.saturating_sub(1);
            let at = self
                .entries
                .iter()
                .position(|e| e.state == State::Loading(reply.id));
            match (at, reply.outcome) {
                (Some(at), Outcome::Ready(handle)) => self.entries[at].state = State::Ready(handle),
                (Some(at), Outcome::Missing) => self.entries[at].state = State::Missing,
                (Some(at), Outcome::Dropped) => self.entries[at] = EMPTY,
                // Evicted or cleared while loading: never shown, so released at once.
                (None, Outcome::Ready(handle)) => self.retire(handle, seq.wrapping_sub(1)),
                (None, _) => {}
            }
        }
    }

    /// Forgets every thumbnail (the height changed or thumbnails went off).
    fn reset(&mut self, seq: u32) {
        for at in 0..CAPACITY {
            if let State::Ready(handle) = self.entries[at].state {
                self.retire(handle, seq);
            }
            self.entries[at] = EMPTY;
        }
    }

    /// Releases `handle` once the draw thread has drawn a snapshot from `seq` on; with
    /// every slot taken, at once (a thumbnail may blink).
    fn retire(&mut self, handle: TextureHandle, seq: u32) {
        if let Some(slot) = self.releasing.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some((handle, seq));
        } else {
            let _ = self.releases.push(handle);
        }
    }

    fn flush_releases(&mut self) {
        let shown = self.shown.load(Ordering::Acquire);
        for slot in &mut self.releasing {
            if let Some((handle, seq)) = *slot
                && reached(shown, seq)
                && self.releases.push(handle).is_ok()
            {
                *slot = None;
            }
        }
    }

    /// Thumbnails in memory, loading or failed ones too.
    pub fn cached(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state != State::Empty)
            .count()
    }

    /// Textures waiting for the draw thread to let go of them.
    pub fn releasing(&self) -> usize {
        self.releasing.iter().flatten().count()
    }

    /// Requests sent and not answered.
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }
}

/// The library thread's end: queues the thumbnail thread's pixels on the texture loader
/// and releases what the update thread gave up.
pub struct ThumbBridge {
    made: rtrb::Consumer<WorkerOut>,
    releases: rtrb::Consumer<TextureHandle>,
    replies: rtrb::Producer<ThumbReply>,
    /// An answer the full ring did not take yet.
    unsent: Option<ThumbReply>,
    /// Releases the full request queue did not take yet.
    stuck: Vec<TextureHandle>,
}

impl ThumbBridge {
    /// Does what is waiting; `requests` is the menu's texture loader.
    pub fn poll(&mut self, requests: &mut TextureRequests) {
        self.stuck
            .retain(|&h| requests.release(h) == Err(ReleaseError::QueueFull));
        while let Ok(handle) = self.releases.pop() {
            if requests.release(handle) == Err(ReleaseError::QueueFull) {
                self.stuck.push(handle);
            }
        }
        if let Some(reply) = self.unsent.take()
            && let Err(rtrb::PushError::Full(reply)) = self.replies.push(reply)
        {
            self.unsent = Some(reply);
            return;
        }
        while self.replies.slots() > 0 {
            let Ok(out) = self.made.pop() else { break };
            let outcome = match out.made {
                Made::Image(image) => queue(requests, image),
                Made::Missing => Outcome::Missing,
                Made::Dropped => Outcome::Dropped,
            };
            let reply = ThumbReply {
                id: out.id,
                outcome,
            };
            if let Err(rtrb::PushError::Full(reply)) = self.replies.push(reply) {
                self.unsent = Some(reply);
                break;
            }
        }
    }
}

fn queue(requests: &mut TextureRequests, image: Image) -> Outcome {
    use slam_render::upload::RejectReason;
    match requests.load_pixels(image, TextureKind::Standalone) {
        Ok(handle) => Outcome::Ready(handle),
        Err(rejected) => match rejected.reason {
            RejectReason::QueueFull => Outcome::Dropped,
            RejectReason::NoFreeHandles => Outcome::Missing,
        },
    }
}

/// Connects the ends and starts the thumbnail thread. `cache` is the `cache` folder (none:
/// nothing is cached), `aspect` the thumbnails' width over height and `limits` those of
/// the texture loader's decoder.
pub fn link(
    cache: Option<std::path::PathBuf>,
    aspect: f32,
    limits: ImageLimits,
) -> std::io::Result<(Thumbnails, ThumbBridge, ThumbWorker, ThumbShown)> {
    let (requests, requests_rx) = rtrb::RingBuffer::new(REQUEST_RING);
    let (out_tx, made) = rtrb::RingBuffer::new(OUT_RING);
    let (replies_tx, replies) = rtrb::RingBuffer::new(REPLY_RING);
    let (releases, releases_rx) = rtrb::RingBuffer::new(RELEASE_RING);
    let shown = Arc::new(AtomicU32::new(0));
    let worker = worker::spawn(make::Maker::new(cache, aspect, limits), requests_rx, out_tx)?;
    Ok((
        Thumbnails::new(requests, replies, releases, Arc::clone(&shown)),
        ThumbBridge {
            made,
            releases: releases_rx,
            replies: replies_tx,
            unsent: None,
            stuck: Vec::new(),
        },
        worker,
        ThumbShown(shown),
    ))
}

#[cfg(test)]
mod tests;
