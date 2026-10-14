//! Texture loading (ADR-0034): images are decoded and laid out in atlas pages on the
//! loader thread `slam-texture-load`; the draw thread uploads them in strips of rows,
//! at most a byte budget per frame, without allocating. Pixels made elsewhere (MSDF
//! glyphs, ADR-0038) take the same path without the decoding step.
//!
//! [`texture_loader`] returns [`TextureRequests`] for the thread that asks for
//! textures and [`TextureUploads`] for the draw thread. Every request travels the same
//! ordered path (requests → loader → draw), so a release always reaches the draw
//! thread before a later load that reuses its handle.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::Thread;
use std::time::Duration;

use crate::atlas::{Atlas, AtlasRegion, ShelfPacker, UvRect, extrude};
use crate::hal::{Device, Extent, Limits, Origin, TextureDesc, TextureFormat, TextureUsage};
use crate::image::{Image, ImageError, ImageLimits, decode_image};
use crate::textures::{TextureId, Textures};

/// How long the loader sleeps when it has nothing to do or the draw thread is behind.
/// The draw thread never wakes it (a system call); requests do.
const LOADER_POLL: Duration = Duration::from_millis(5);

const BPP: u32 = TextureFormat::Rgba8.bytes_per_pixel();

#[derive(Clone, Copy, Debug)]
pub struct TextureLoaderConfig {
    /// Limits of the device the textures are for.
    pub limits: Limits,
    /// Bytes uploaded per [`TextureUploads::process`] call, but at least one row.
    pub budget: usize,
    /// Handles alive at once.
    pub max_textures: u32,
    /// Atlas pages; an atlas image that finds no room fails with
    /// [`LoadError::AtlasFull`].
    pub max_pages: u32,
    /// Atlas page size, clamped to the device limit.
    pub page_size: Extent,
    /// Decoded bytes of one image.
    pub max_image_bytes: usize,
    /// Requests waiting for the loader.
    pub request_capacity: usize,
    /// Decoded images waiting for the draw thread; bounds the memory they hold.
    pub job_capacity: usize,
}

impl TextureLoaderConfig {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            budget: 2 << 20,
            max_textures: 4096,
            max_pages: 8,
            page_size: Atlas::DEFAULT_PAGE_SIZE,
            max_image_bytes: 64 << 20,
            request_capacity: 256,
            job_capacity: 16,
        }
    }
}

/// A requested texture: index and generation, so a reused index is not mistaken
/// for the texture it held before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureHandle {
    index: u32,
    generation: u32,
}

/// Where an image goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureKind {
    /// Into an atlas page (skin elements); stays until the pages are destroyed. An
    /// image too large for a page gets its own texture.
    Atlas,
    /// Its own texture, destroyed on release (backgrounds).
    Standalone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    #[error("the image could not be decoded")]
    Decode,
    #[error("the image exceeds the size limits")]
    TooLarge,
    #[error("no atlas page has room for the image")]
    AtlasFull,
    /// Also every later image of an atlas page whose creation failed: the page keeps
    /// its place among `max_pages` until it is full.
    #[error("the device failed to create or write the texture")]
    Device,
}

/// What the draw thread knows about a handle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextureState {
    /// Not uploaded yet, or released.
    Pending,
    Ready(AtlasRegion),
    Failed(LoadError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RejectReason {
    #[error("the request queue is full")]
    QueueFull,
    #[error("all texture handles are in use")]
    NoFreeHandles,
}

/// A load that was not queued; the caller gets its input back (the PNG bytes or the
/// [`Image`]).
#[derive(Debug, thiserror::Error)]
#[error("{reason}")]
pub struct Rejected<T = Vec<u8>> {
    pub reason: RejectReason,
    pub input: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReleaseError {
    #[error("the request queue is full")]
    QueueFull,
    #[error("the handle is not alive")]
    Stale,
}

/// Counters for the debug overlay; written by the loader thread.
#[derive(Debug, Default)]
pub struct LoaderStats {
    /// Images accepted for upload: decoded PNGs and valid ready pixels.
    pub decoded: AtomicU64,
    pub failed: AtomicU64,
}

enum Command {
    Load {
        handle: TextureHandle,
        source: Source,
        kind: TextureKind,
    },
    Release(TextureHandle),
}

enum Source {
    Encoded(Vec<u8>),
    Pixels(Image),
}

/// Pixels for one texture region, tightly packed `Rgba8` rows.
struct Upload {
    handle: TextureHandle,
    /// Atlas page index, or `None` for a texture of its own of `size`.
    page: Option<u32>,
    /// Where `pixels` (`size`, with the atlas border) go.
    at: Origin,
    size: Extent,
    /// The image inside the texture, without the border, and the texture's size.
    region: (Origin, Extent),
    texture_size: Extent,
    pixels: Vec<u8>,
}

enum Job {
    NewPage { page: u32, size: Extent },
    Upload(Upload),
    Failed(TextureHandle, LoadError),
    Release(TextureHandle),
}

/// Creates the request and upload sides and starts the loader thread
/// `slam-texture-load`. The thread ends once the upload side is dropped, or once the
/// request side is dropped and its queued work is handed over and every pixel buffer
/// has come back (so the draw thread never frees one).
pub fn texture_loader(
    config: &TextureLoaderConfig,
) -> std::io::Result<(TextureRequests, TextureUploads)> {
    let (mut requests, mut uploads, loader) = parts(config);
    let thread = std::thread::Builder::new()
        .name("slam-texture-load".into())
        .spawn(move || loader.run())?;
    requests.loader = Some(thread.thread().clone());
    uploads.loader = Some(thread.thread().clone());
    Ok((requests, uploads))
}

/// The three sides; the loader is not running yet.
fn parts(config: &TextureLoaderConfig) -> (TextureRequests, TextureUploads, Loader) {
    let max_side = config.limits.max_texture_size;
    let job_capacity = config.job_capacity.max(1);
    let (command_tx, command_rx) = rtrb::RingBuffer::new(config.request_capacity.max(1));
    let (job_tx, job_rx) = rtrb::RingBuffer::new(job_capacity);
    // The loader keeps at most this many pixel buffers away from itself (in the job
    // ring, being uploaded or in this ring; see `Loader::in_flight`), so this ring
    // never fills.
    let spent_capacity = job_capacity + 1;
    let (spent_tx, spent_rx) = rtrb::RingBuffer::new(spent_capacity);
    let stats = Arc::new(LoaderStats::default());
    let n = config.max_textures as usize;
    let requests = TextureRequests {
        commands: command_tx,
        free: (0..config.max_textures).rev().collect(),
        generations: vec![0; n],
        alive: vec![false; n],
        loader: None,
        stats: Arc::clone(&stats),
    };
    let page_size = Extent::new(
        config.page_size.width.min(max_side),
        config.page_size.height.min(max_side),
    );
    let uploads = TextureUploads {
        jobs: job_rx,
        spent: spent_tx,
        slots: vec![Slot::default(); n],
        pages: vec![Page::Missing; config.max_pages as usize],
        current: None,
        zeros: vec![0; config.budget.max(max_side as usize * BPP as usize)].into_boxed_slice(),
        budget: config.budget,
        loader: None,
    };
    let loader = Loader {
        commands: command_rx,
        jobs: job_tx,
        spent: spent_rx,
        outbox: VecDeque::new(),
        in_flight: 0,
        spent_capacity,
        image_limits: ImageLimits {
            max_side,
            max_bytes: config.max_image_bytes,
        },
        page_size,
        max_pages: config.max_pages as usize,
        pages: Vec::new(),
        stats,
    };
    (requests, uploads, loader)
}

/// Asks for textures; owned by one thread (the queue has a single writer).
pub struct TextureRequests {
    commands: rtrb::Producer<Command>,
    free: Vec<u32>,
    generations: Vec<u32>,
    alive: Vec<bool>,
    loader: Option<Thread>,
    stats: Arc<LoaderStats>,
}

impl TextureRequests {
    /// Queues an encoded image (PNG or JPEG) to load and returns its handle at once; the
    /// draw thread sees it as [`TextureState::Pending`] until it is uploaded.
    pub fn load(&mut self, bytes: Vec<u8>, kind: TextureKind) -> Result<TextureHandle, Rejected> {
        self.enqueue(Source::Encoded(bytes), kind)
            .map_err(|(reason, source)| match source {
                Source::Encoded(input) => Rejected { reason, input },
                Source::Pixels(_) => unreachable!("the source is handed back unchanged"),
            })
    }

    /// Queues pixels made elsewhere (`Rgba8`, tight rows, top row first) like a
    /// decoded PNG: the same size limits, atlas layout and uploads apply. A buffer
    /// that does not match `image.size` fails the handle with [`LoadError::Decode`].
    pub fn load_pixels(
        &mut self,
        image: Image,
        kind: TextureKind,
    ) -> Result<TextureHandle, Rejected<Image>> {
        self.enqueue(Source::Pixels(image), kind)
            .map_err(|(reason, source)| match source {
                Source::Pixels(input) => Rejected { reason, input },
                Source::Encoded(_) => unreachable!("the source is handed back unchanged"),
            })
    }

    fn enqueue(
        &mut self,
        source: Source,
        kind: TextureKind,
    ) -> Result<TextureHandle, (RejectReason, Source)> {
        if self.commands.is_full() {
            return Err((RejectReason::QueueFull, source));
        }
        let Some(index) = self.free.pop() else {
            return Err((RejectReason::NoFreeHandles, source));
        };
        let i = index as usize;
        self.generations[i] = self.generations[i].wrapping_add(1);
        self.alive[i] = true;
        let handle = TextureHandle {
            index,
            generation: self.generations[i],
        };
        if self
            .commands
            .push(Command::Load {
                handle,
                source,
                kind,
            })
            .is_err()
        {
            unreachable!("room was checked and this is the only writer");
        }
        self.wake();
        Ok(handle)
    }

    /// Frees the handle: a texture of its own is destroyed, atlas space is kept
    /// (ADR-0020). The handle's index may be handed out again at once.
    pub fn release(&mut self, handle: TextureHandle) -> Result<(), ReleaseError> {
        let i = handle.index as usize;
        if !self.alive.get(i).copied().unwrap_or(false) || self.generations[i] != handle.generation
        {
            return Err(ReleaseError::Stale);
        }
        if self.commands.push(Command::Release(handle)).is_err() {
            return Err(ReleaseError::QueueFull);
        }
        self.alive[i] = false;
        self.free.push(handle.index);
        self.wake();
        Ok(())
    }

    pub fn stats(&self) -> &LoaderStats {
        &self.stats
    }

    fn wake(&self) {
        if let Some(thread) = &self.loader {
            thread.unpark();
        }
    }
}

impl Drop for TextureRequests {
    fn drop(&mut self) {
        // Lets the loader notice the closed queue without waiting for its poll.
        self.wake();
    }
}

/// Decodes requests and lays out atlas pages; runs on its own thread.
struct Loader {
    commands: rtrb::Consumer<Command>,
    jobs: rtrb::Producer<Job>,
    spent: rtrb::Consumer<Vec<u8>>,
    /// Jobs of the current request not yet in the ring (a load can make two).
    outbox: VecDeque<Job>,
    /// Pixel buffers sent to the draw thread and not yet back.
    in_flight: usize,
    spent_capacity: usize,
    image_limits: ImageLimits,
    page_size: Extent,
    max_pages: usize,
    pages: Vec<ShelfPacker>,
    stats: Arc<LoaderStats>,
}

impl Loader {
    fn run(mut self) {
        loop {
            match self.step() {
                Step::Busy => {}
                // Waiting for nothing but a request (or for the draw thread to go):
                // both wake the thread.
                Step::Idle if self.outbox.is_empty() && self.in_flight == 0 => {
                    std::thread::park();
                }
                Step::Idle => std::thread::park_timeout(LOADER_POLL),
                Step::Done => break,
            }
        }
    }

    /// Does one piece of work.
    fn step(&mut self) -> Step {
        // Spent buffers are freed here, off the draw thread.
        while self.spent.pop().is_ok() {
            self.in_flight -= 1;
        }
        if self.jobs.is_abandoned() {
            return Step::Done;
        }
        if let Some(job) = self.outbox.pop_front() {
            let carries_pixels = matches!(job, Job::Upload(_));
            if carries_pixels && self.in_flight == self.spent_capacity {
                self.outbox.push_front(job);
                return Step::Idle;
            }
            return match self.jobs.push(job) {
                Ok(()) => {
                    self.in_flight += usize::from(carries_pixels);
                    Step::Busy
                }
                Err(rtrb::PushError::Full(job)) => {
                    self.outbox.push_front(job);
                    Step::Idle
                }
            };
        }
        match self.commands.pop() {
            Ok(Command::Load {
                handle,
                source,
                kind,
            }) => {
                self.load(handle, source, kind);
                Step::Busy
            }
            Ok(Command::Release(handle)) => {
                self.outbox.push_back(Job::Release(handle));
                Step::Busy
            }
            Err(_) if self.commands.is_abandoned() && self.in_flight == 0 => Step::Done,
            Err(_) => Step::Idle,
        }
    }

    fn load(&mut self, handle: TextureHandle, source: Source, kind: TextureKind) {
        let image = match source {
            Source::Encoded(bytes) => match decode_image(&bytes, self.image_limits) {
                Ok(image) => image,
                Err(e) => {
                    tracing::warn!("texture {handle:?}: {e}");
                    let error = match e {
                        ImageError::TooLarge { .. } => LoadError::TooLarge,
                        ImageError::Png(_)
                        | ImageError::Jpeg(_)
                        | ImageError::FrameMismatch
                        | ImageError::Unsupported => LoadError::Decode,
                    };
                    self.fail(handle, error);
                    return;
                }
            },
            Source::Pixels(image) => {
                if let Err(error) = self.check_pixels(&image) {
                    tracing::warn!("texture {handle:?}: {error} ({:?})", image.size);
                    self.fail(handle, error);
                    return;
                }
                image
            }
        };
        self.stats.decoded.fetch_add(1, Ordering::Relaxed);
        let size = image.size;
        let b = Atlas::DEFAULT_BORDER;
        let padded = Extent::new(size.width + 2 * b, size.height + 2 * b);
        let fits_page =
            padded.width <= self.page_size.width && padded.height <= self.page_size.height;
        if kind == TextureKind::Standalone || !fits_page {
            self.outbox.push_back(Job::Upload(Upload {
                handle,
                page: None,
                at: Origin::default(),
                size,
                region: (Origin::default(), size),
                texture_size: size,
                pixels: image.pixels,
            }));
            return;
        }
        let found = self
            .pages
            .iter_mut()
            .enumerate()
            .find_map(|(i, packer)| packer.allocate(padded).map(|o| (i, o)));
        let (page, corner) = match found {
            Some(found) => found,
            None if self.pages.len() < self.max_pages => {
                let mut packer = ShelfPacker::new(self.page_size);
                let corner = packer
                    .allocate(padded)
                    .expect("an image that fits a page fits an empty one");
                self.pages.push(packer);
                let page = self.pages.len() - 1;
                self.outbox.push_back(Job::NewPage {
                    page: page as u32,
                    size: self.page_size,
                });
                (page, corner)
            }
            None => {
                tracing::warn!("texture {handle:?}: the atlas is full");
                self.fail(handle, LoadError::AtlasFull);
                return;
            }
        };
        let mut pixels = Vec::new();
        extrude(&mut pixels, &image.pixels, size, b, BPP as usize);
        self.outbox.push_back(Job::Upload(Upload {
            handle,
            page: Some(page as u32),
            at: corner,
            size: padded,
            region: (Origin::new(corner.x + b, corner.y + b), size),
            texture_size: self.page_size,
            pixels,
        }));
    }

    /// The limits a decoded PNG is held to, and a buffer matching its size.
    fn check_pixels(&self, image: &Image) -> Result<(), LoadError> {
        let Extent { width, height } = image.size;
        let max_side = self.image_limits.max_side;
        if width > max_side || height > max_side {
            return Err(LoadError::TooLarge);
        }
        // In u64 and after the side check: two u32 sides times 4 overflow even u64,
        // and the product overflows usize on 32-bit targets.
        let bytes = u64::from(width) * u64::from(height) * u64::from(BPP);
        if bytes > self.image_limits.max_bytes as u64 {
            return Err(LoadError::TooLarge);
        }
        if width == 0 || height == 0 || image.pixels.len() as u64 != bytes {
            return Err(LoadError::Decode);
        }
        Ok(())
    }

    fn fail(&mut self, handle: TextureHandle, error: LoadError) {
        self.stats.failed.fetch_add(1, Ordering::Relaxed);
        self.outbox.push_back(Job::Failed(handle, error));
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Busy,
    Idle,
    Done,
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    generation: u32,
    state: TextureState,
    /// A texture of its own, destroyed on release.
    owned: Option<TextureId>,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            generation: 0,
            state: TextureState::Pending,
            owned: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Missing,
    Ready(TextureId),
    Failed,
}

/// The job being uploaded in strips.
struct Current {
    texture: TextureId,
    /// Rows already written.
    rows: u32,
    work: Work,
}

enum Work {
    /// Zeroing a new page.
    Clear {
        page: u32,
        size: Extent,
    },
    Upload(Upload),
}

impl Work {
    fn size(&self) -> Extent {
        match self {
            Work::Clear { size, .. } => *size,
            Work::Upload(u) => u.size,
        }
    }
}

/// What one [`TextureUploads::process`] call did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UploadStats {
    /// Bytes written to textures.
    pub bytes: usize,
    /// Handles that became ready or failed.
    pub finished: u32,
    /// Textures the device failed to create or write (not logged on the draw thread).
    pub device_errors: u32,
}

/// Uploads loaded images on the draw thread and answers which are ready.
pub struct TextureUploads {
    jobs: rtrb::Consumer<Job>,
    spent: rtrb::Producer<Vec<u8>>,
    slots: Vec<Slot>,
    pages: Vec<Page>,
    current: Option<Current>,
    /// Source of page clears: at least the budget and one row of the widest texture.
    zeros: Box<[u8]>,
    budget: usize,
    loader: Option<Thread>,
}

impl Drop for TextureUploads {
    fn drop(&mut self) {
        // Lets an idle loader notice that the draw side is gone.
        if let Some(thread) = &self.loader {
            thread.unpark();
        }
    }
}

impl TextureUploads {
    /// The state of a handle in this frame. Look it up every frame instead of keeping
    /// the region: after a release the texture is destroyed and its [`TextureId`] may
    /// name another texture.
    pub fn get(&self, handle: TextureHandle) -> TextureState {
        match self.slots.get(handle.index as usize) {
            Some(slot) if slot.generation == handle.generation => slot.state,
            _ => TextureState::Pending,
        }
    }

    /// Ids of the atlas pages made so far; the caller destroys them (and the
    /// textures of unreleased standalone handles) with the rest of `textures`.
    pub fn pages(&self) -> impl Iterator<Item = TextureId> + '_ {
        self.pages.iter().filter_map(|p| match p {
            Page::Ready(id) => Some(*id),
            _ => None,
        })
    }

    /// Uploads queued images, at most the budget of bytes but at least one row if
    /// anything is queued. Call once per frame, before recording sprites.
    ///
    /// Does not allocate if `textures` has spare capacity for the textures it adds;
    /// only an error value returned by the device may free the memory of its message.
    pub fn process<D: Device>(
        &mut self,
        device: &mut D,
        textures: &mut Textures<D::Texture>,
    ) -> UploadStats {
        let mut stats = UploadStats::default();
        loop {
            if self.current.is_none() {
                let Ok(job) = self.jobs.pop() else { break };
                self.start(job, device, textures, &mut stats);
                continue;
            }
            let Some(current) = &mut self.current else {
                unreachable!("checked above")
            };
            let size = current.work.size();
            let row = size.width as usize * BPP as usize;
            let left = (size.height - current.rows) as usize;
            let rows = match self.budget.saturating_sub(stats.bytes) / row {
                0 if stats.bytes > 0 => break,
                0 => 1,
                fit => fit.min(left) as u32,
            };
            let (at, data) = match &current.work {
                Work::Clear { .. } => (
                    Origin::new(0, current.rows),
                    &self.zeros[..rows as usize * row],
                ),
                Work::Upload(u) => (
                    Origin::new(u.at.x, u.at.y + current.rows),
                    &u.pixels[current.rows as usize * row..][..rows as usize * row],
                ),
            };
            let written = textures.get(current.texture).is_some_and(|t| {
                device
                    .write_texture(t, at, Extent::new(size.width, rows), data)
                    .is_ok()
            });
            stats.bytes += rows as usize * row;
            current.rows += rows;
            if !written {
                stats.device_errors += 1;
                self.finish(Err(LoadError::Device), &mut stats);
            } else if current.rows == size.height {
                self.finish(Ok(()), &mut stats);
            }
        }
        stats
    }

    fn start<D: Device>(
        &mut self,
        job: Job,
        device: &mut D,
        textures: &mut Textures<D::Texture>,
        stats: &mut UploadStats,
    ) {
        match job {
            Job::NewPage { page, size } => {
                let desc = TextureDesc {
                    size,
                    format: TextureFormat::Rgba8,
                    usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
                };
                match device.create_texture(&desc, None) {
                    Ok(texture) => {
                        self.current = Some(Current {
                            texture: textures.insert(texture),
                            rows: 0,
                            work: Work::Clear { page, size },
                        });
                    }
                    Err(_) => {
                        stats.device_errors += 1;
                        self.pages[page as usize] = Page::Failed;
                    }
                }
            }
            Job::Upload(upload) => {
                let slot = &mut self.slots[upload.handle.index as usize];
                // A release always comes before its index is loaded again.
                debug_assert!(slot.owned.is_none(), "{:?} was not released", upload.handle);
                *slot = Slot {
                    generation: upload.handle.generation,
                    ..Slot::default()
                };
                let texture = match upload.page {
                    Some(page) => match self.pages[page as usize] {
                        Page::Ready(id) => Some(id),
                        Page::Missing | Page::Failed => None,
                    },
                    None => {
                        let desc = TextureDesc {
                            size: upload.texture_size,
                            format: TextureFormat::Rgba8,
                            usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
                        };
                        match device.create_texture(&desc, None) {
                            Ok(texture) => {
                                let id = textures.insert(texture);
                                slot.owned = Some(id);
                                Some(id)
                            }
                            Err(_) => {
                                stats.device_errors += 1;
                                None
                            }
                        }
                    }
                };
                match texture {
                    Some(texture) => {
                        self.current = Some(Current {
                            texture,
                            rows: 0,
                            work: Work::Upload(upload),
                        });
                    }
                    None => {
                        slot.state = TextureState::Failed(LoadError::Device);
                        stats.finished += 1;
                        self.give_back(upload.pixels);
                    }
                }
            }
            Job::Failed(handle, error) => {
                self.slots[handle.index as usize] = Slot {
                    generation: handle.generation,
                    state: TextureState::Failed(error),
                    owned: None,
                };
                stats.finished += 1;
            }
            Job::Release(handle) => {
                let slot = &mut self.slots[handle.index as usize];
                if slot.generation == handle.generation {
                    if let Some(id) = slot.owned.take()
                        && let Some(texture) = textures.remove(id)
                    {
                        device.destroy_texture(texture);
                    }
                    slot.state = TextureState::Pending;
                }
            }
        }
    }

    /// Ends the current job.
    fn finish(&mut self, result: Result<(), LoadError>, stats: &mut UploadStats) {
        let Some(current) = self.current.take() else {
            return;
        };
        match current.work {
            Work::Clear { page, .. } => {
                self.pages[page as usize] = match result {
                    Ok(()) => Page::Ready(current.texture),
                    // The texture stays in the set, unused, until it is destroyed.
                    Err(_) => Page::Failed,
                };
            }
            Work::Upload(upload) => {
                let slot = &mut self.slots[upload.handle.index as usize];
                let (origin, size) = upload.region;
                slot.state = match result {
                    Ok(()) => TextureState::Ready(AtlasRegion {
                        texture: current.texture,
                        origin,
                        size,
                        uv: if upload.page.is_some() {
                            UvRect::of(origin, size, upload.texture_size)
                        } else {
                            UvRect::FULL
                        },
                    }),
                    Err(e) => TextureState::Failed(e),
                };
                stats.finished += 1;
                self.give_back(upload.pixels);
            }
        }
    }

    fn give_back(&mut self, pixels: Vec<u8>) {
        // Cannot fail: the loader keeps at most the ring's capacity in flight
        // (`Loader::in_flight`). It only stops early once this side is dropped.
        let _ = self.spent.push(pixels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_device::{Call, Fake, LIMITS};

    // `count_allocs` relies on the counting allocator that the `sprite` tests install
    // for this (single) unit-test binary.

    fn png(width: u32, height: u32, seed: u8) -> Vec<u8> {
        let data: Vec<u8> = (0..width * height * 4)
            .map(|i| (i as u8).wrapping_mul(seed))
            .collect();
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, width, height);
        e.set_color(png::ColorType::Rgba);
        let mut w = e.write_header().unwrap();
        w.write_image_data(&data).unwrap();
        w.finish().unwrap();
        out
    }

    fn config(budget: usize) -> TextureLoaderConfig {
        TextureLoaderConfig {
            budget,
            max_textures: 8,
            max_pages: 2,
            page_size: Extent::new(64, 64),
            job_capacity: 4,
            ..TextureLoaderConfig::new(LIMITS)
        }
    }

    /// Runs the loader until it waits.
    fn pump(loader: &mut Loader) {
        while loader.step() == Step::Busy {}
    }

    struct Rig {
        requests: TextureRequests,
        uploads: TextureUploads,
        loader: Loader,
        device: Fake,
        textures: Textures<u32>,
    }

    impl Rig {
        fn new(config: TextureLoaderConfig) -> Self {
            let (requests, uploads, loader) = parts(&config);
            let mut device = Fake::default();
            // Room for allocation-free frames.
            device.log.reserve(4096);
            device.live_textures.reserve(64);
            Self {
                requests,
                uploads,
                loader,
                device,
                textures: Textures::with_capacity(64),
            }
        }

        fn frame(&mut self) -> UploadStats {
            pump(&mut self.loader);
            self.device.log.clear();
            self.uploads.process(&mut self.device, &mut self.textures)
        }

        /// Frames until nothing is left to upload.
        fn settle(&mut self) -> Vec<UploadStats> {
            let mut frames = Vec::new();
            loop {
                let stats = self.frame();
                if stats == UploadStats::default() {
                    return frames;
                }
                frames.push(stats);
            }
        }

        fn ready(&self, handle: TextureHandle) -> AtlasRegion {
            match self.uploads.get(handle) {
                TextureState::Ready(region) => region,
                other => panic!("{handle:?} is {other:?}"),
            }
        }
    }

    #[test]
    fn small_images_go_to_one_atlas_page() {
        let mut rig = Rig::new(config(1 << 20));
        let a = rig
            .requests
            .load(png(10, 4, 3), TextureKind::Atlas)
            .unwrap();
        let b = rig.requests.load(png(5, 7, 5), TextureKind::Atlas).unwrap();
        assert_eq!(rig.uploads.get(a), TextureState::Pending);
        rig.settle();
        let (ra, rb) = (rig.ready(a), rig.ready(b));
        assert_eq!(ra.texture, rb.texture);
        assert_eq!(rig.uploads.pages().collect::<Vec<_>>(), [ra.texture]);
        assert_eq!((ra.size, rb.size), (Extent::new(10, 4), Extent::new(5, 7)));
        // Inside the 1-pixel border.
        assert_eq!(ra.origin, Origin::new(1, 1));
        assert_eq!(ra.uv, UvRect::of(ra.origin, ra.size, Extent::new(64, 64)));
        let live = &rig.device.live_textures;
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].1.size, Extent::new(64, 64));
    }

    #[test]
    fn standalone_and_oversized_images_get_their_own_texture() {
        let mut rig = Rig::new(config(1 << 20));
        let bg = rig
            .requests
            .load(png(8, 8, 3), TextureKind::Standalone)
            .unwrap();
        // 63 + 2 border pixels do not fit a 64-pixel page.
        let big = rig
            .requests
            .load(png(63, 2, 5), TextureKind::Atlas)
            .unwrap();
        rig.settle();
        for (h, size) in [(bg, Extent::new(8, 8)), (big, Extent::new(63, 2))] {
            let r = rig.ready(h);
            assert_eq!(
                (r.origin, r.size, r.uv),
                (Origin::default(), size, UvRect::FULL)
            );
        }
        assert_eq!(rig.uploads.pages().count(), 0);
        let sizes: Vec<_> = rig.device.live_textures.iter().map(|t| t.1.size).collect();
        assert_eq!(sizes, [Extent::new(8, 8), Extent::new(63, 2)]);
    }

    #[test]
    fn no_frame_writes_more_than_the_budget() {
        // 40 rows of a 64-pixel page per frame.
        let budget = 64 * 4 * 40;
        let mut rig = Rig::new(config(budget));
        let a = rig
            .requests
            .load(png(30, 30, 3), TextureKind::Atlas)
            .unwrap();
        let bg = rig
            .requests
            .load(png(50, 50, 7), TextureKind::Standalone)
            .unwrap();
        let mut total = 0;
        let mut frames = 0;
        loop {
            let stats = rig.frame();
            if stats == UploadStats::default() {
                break;
            }
            assert!(stats.bytes <= budget, "{stats:?}");
            let logged: usize = rig
                .device
                .log
                .iter()
                .map(|c| match c {
                    Call::WriteTexture { size, .. } => (size.width * size.height * 4) as usize,
                    _ => 0,
                })
                .sum();
            assert_eq!(logged, stats.bytes);
            total += stats.bytes;
            frames += 1;
        }
        // Page clear (64x64), padded image (32x32) and background (50x50).
        assert_eq!(total, (64 * 64 + 32 * 32 + 50 * 50) * 4);
        assert!(frames >= total.div_ceil(budget), "{frames}");
        rig.ready(a);
        rig.ready(bg);
    }

    #[test]
    fn strips_cover_the_image_exactly_once() {
        let mut rig = Rig::new(config(50 * 4 * 7));
        let bg = rig
            .requests
            .load(png(50, 20, 3), TextureKind::Standalone)
            .unwrap();
        pump(&mut rig.loader);
        let mut rows = Vec::new();
        loop {
            rig.device.log.clear();
            if rig.uploads.process(&mut rig.device, &mut rig.textures) == UploadStats::default() {
                break;
            }
            for c in &rig.device.log {
                if let Call::WriteTexture { origin, size, .. } = c {
                    assert_eq!((origin.x, size.width), (0, 50));
                    rows.push((origin.y, size.height));
                }
            }
        }
        assert_eq!(rows, [(0, 7), (7, 7), (14, 6)]);
        rig.ready(bg);
    }

    #[test]
    fn a_row_wider_than_the_budget_still_uploads_one_row_per_frame() {
        let mut rig = Rig::new(config(16));
        let bg = rig
            .requests
            .load(png(10, 3, 3), TextureKind::Standalone)
            .unwrap();
        let frames = rig.settle();
        assert_eq!(
            frames.iter().map(|s| s.bytes).collect::<Vec<_>>(),
            [40, 40, 40]
        );
        rig.ready(bg);
    }

    #[test]
    fn a_page_is_cleared_before_its_images_are_written() {
        let mut rig = Rig::new(config(1 << 20));
        rig.requests.load(png(4, 4, 3), TextureKind::Atlas).unwrap();
        rig.frame();
        let writes: Vec<_> = rig
            .device
            .log
            .iter()
            .filter_map(|c| match c {
                Call::WriteTexture { origin, size, .. } => Some((*origin, *size)),
                _ => None,
            })
            .collect();
        assert_eq!(
            writes,
            [
                (Origin::new(0, 0), Extent::new(64, 64)),
                (Origin::new(0, 0), Extent::new(6, 6)),
            ]
        );
    }

    #[test]
    fn decoded_pixels_reach_the_atlas_with_their_border() {
        let mut rig = Rig::new(config(1 << 20));
        let h = rig.requests.load(png(2, 1, 9), TextureKind::Atlas).unwrap();
        pump(&mut rig.loader);
        // The upload job in the ring holds the extruded image.
        let jobs: Vec<_> = std::iter::from_fn(|| rig.uploads.jobs.pop().ok()).collect();
        let Some(Job::Upload(u)) = jobs.last() else {
            panic!("no upload")
        };
        let src: Vec<u8> = (0..8u32).map(|i| (i as u8).wrapping_mul(9)).collect();
        let mut expected = Vec::new();
        extrude(&mut expected, &src, Extent::new(2, 1), 1, 4);
        assert_eq!(
            (u.handle, u.size, &u.pixels),
            (h, Extent::new(4, 3), &expected)
        );
    }

    #[test]
    fn ready_pixels_take_the_png_path_without_decoding() {
        let mut rig = Rig::new(config(1 << 20));
        let src: Vec<u8> = (0..8u8).collect();
        let image = Image {
            size: Extent::new(2, 1),
            pixels: src.clone(),
        };
        let h = rig.requests.load_pixels(image, TextureKind::Atlas).unwrap();
        pump(&mut rig.loader);
        let jobs: Vec<_> = std::iter::from_fn(|| rig.uploads.jobs.pop().ok()).collect();
        let Some(Job::Upload(u)) = jobs.last() else {
            panic!("no upload")
        };
        let mut expected = Vec::new();
        extrude(&mut expected, &src, Extent::new(2, 1), 1, 4);
        assert_eq!(
            (u.handle, u.page, u.size, &u.pixels),
            (h, Some(0), Extent::new(4, 3), &expected)
        );
        assert_eq!(rig.requests.stats().decoded.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn ready_pixels_are_held_to_the_size_limits() {
        let mut rig = Rig::new(config(1 << 20));
        let image = |width, height, len| Image {
            size: Extent::new(width, height),
            pixels: vec![0; len],
        };
        let side = LIMITS.max_texture_size + 1;
        let huge = rig
            .requests
            .load_pixels(image(side, 1, side as usize * 4), TextureKind::Standalone)
            .unwrap();
        // Sides that overflow any byte count, and too many bytes within the sides.
        let absurd = rig
            .requests
            .load_pixels(image(u32::MAX, u32::MAX, 0), TextureKind::Standalone)
            .unwrap();
        let short = rig
            .requests
            .load_pixels(image(2, 2, 15), TextureKind::Atlas)
            .unwrap();
        let empty = rig
            .requests
            .load_pixels(image(0, 3, 0), TextureKind::Atlas)
            .unwrap();
        let good = rig
            .requests
            .load_pixels(image(3, 3, 36), TextureKind::Atlas)
            .unwrap();
        rig.settle();
        assert_eq!(
            [huge, absurd, short, empty].map(|h| rig.uploads.get(h)),
            [
                TextureState::Failed(LoadError::TooLarge),
                TextureState::Failed(LoadError::TooLarge),
                TextureState::Failed(LoadError::Decode),
                TextureState::Failed(LoadError::Decode),
            ]
        );
        assert_eq!(rig.ready(good).size, Extent::new(3, 3));
        assert_eq!(rig.requests.stats().failed.load(Ordering::Relaxed), 4);

        let mut rig = Rig::new(TextureLoaderConfig {
            max_image_bytes: 15,
            ..config(1 << 20)
        });
        let over = rig
            .requests
            .load_pixels(image(2, 2, 16), TextureKind::Atlas)
            .unwrap();
        rig.settle();
        assert_eq!(
            rig.uploads.get(over),
            TextureState::Failed(LoadError::TooLarge)
        );
    }

    #[test]
    fn rejected_pixels_are_handed_back() {
        let mut rig = Rig::new(TextureLoaderConfig {
            request_capacity: 1,
            ..config(1 << 20)
        });
        let image = Image {
            size: Extent::new(1, 1),
            pixels: vec![1, 2, 3, 4],
        };
        rig.requests
            .load_pixels(image.clone(), TextureKind::Atlas)
            .unwrap();
        let e = rig
            .requests
            .load_pixels(image.clone(), TextureKind::Atlas)
            .unwrap_err();
        assert_eq!((e.reason, e.input), (RejectReason::QueueFull, image));
    }

    #[test]
    fn jpeg_files_load_like_png_files() {
        let mut rig = Rig::new(config(1 << 20));
        let h = rig
            .requests
            .load(
                include_bytes!("../tests/data/rgb-32x16.jpg").to_vec(),
                TextureKind::Atlas,
            )
            .unwrap();
        rig.settle();
        assert_eq!(rig.ready(h).size, Extent::new(32, 16));
    }

    #[test]
    fn broken_and_oversized_files_fail_their_handle_only() {
        let mut rig = Rig::new(config(1 << 20));
        let bad = rig
            .requests
            .load(b"garbage".to_vec(), TextureKind::Atlas)
            .unwrap();
        let huge = rig
            .requests
            .load(
                png(LIMITS.max_texture_size + 1, 1, 3),
                TextureKind::Standalone,
            )
            .unwrap();
        let good = rig.requests.load(png(3, 3, 3), TextureKind::Atlas).unwrap();
        rig.settle();
        assert_eq!(
            rig.uploads.get(bad),
            TextureState::Failed(LoadError::Decode)
        );
        assert_eq!(
            rig.uploads.get(huge),
            TextureState::Failed(LoadError::TooLarge)
        );
        rig.ready(good);
        assert_eq!(rig.requests.stats().failed.load(Ordering::Relaxed), 2);
        assert_eq!(rig.requests.stats().decoded.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_full_atlas_fails_further_images() {
        let mut rig = Rig::new(config(1 << 20));
        // Each fills a 64x64 page with its border.
        let a = rig
            .requests
            .load(png(62, 62, 1), TextureKind::Atlas)
            .unwrap();
        let b = rig
            .requests
            .load(png(62, 62, 2), TextureKind::Atlas)
            .unwrap();
        let c = rig
            .requests
            .load(png(62, 62, 3), TextureKind::Atlas)
            .unwrap();
        rig.settle();
        assert_ne!(rig.ready(a).texture, rig.ready(b).texture);
        assert_eq!(
            rig.uploads.get(c),
            TextureState::Failed(LoadError::AtlasFull)
        );
    }

    #[test]
    fn device_failures_fail_the_handle() {
        let mut rig = Rig::new(config(1 << 20));
        rig.device = Fake::failing_after(0);
        let a = rig.requests.load(png(3, 3, 3), TextureKind::Atlas).unwrap();
        let bg = rig
            .requests
            .load(png(3, 3, 3), TextureKind::Standalone)
            .unwrap();
        rig.settle();
        assert_eq!(rig.uploads.get(a), TextureState::Failed(LoadError::Device));
        assert_eq!(rig.uploads.get(bg), TextureState::Failed(LoadError::Device));
    }

    #[test]
    fn release_destroys_standalone_textures_and_handles_are_reused() {
        let mut rig = Rig::new(config(1 << 20));
        let bg = rig
            .requests
            .load(png(8, 8, 3), TextureKind::Standalone)
            .unwrap();
        rig.settle();
        assert_eq!(rig.device.live_textures.len(), 1);
        rig.requests.release(bg).unwrap();
        assert_eq!(rig.requests.release(bg), Err(ReleaseError::Stale));
        // Same index, new generation; queued behind the release.
        let next = rig
            .requests
            .load(png(4, 4, 5), TextureKind::Standalone)
            .unwrap();
        assert_eq!(next.index, bg.index);
        assert_ne!(next, bg);
        // Before the draw thread sees the release, the new handle is pending even
        // though its slot still holds the old texture.
        assert_eq!(rig.uploads.get(next), TextureState::Pending);
        rig.settle();
        assert_eq!(rig.uploads.get(bg), TextureState::Pending);
        assert_eq!(rig.ready(next).size, Extent::new(4, 4));
        let sizes: Vec<_> = rig.device.live_textures.iter().map(|t| t.1.size).collect();
        assert_eq!(sizes, [Extent::new(4, 4)]);
    }

    #[test]
    fn requests_are_rejected_when_handles_or_the_queue_run_out() {
        let mut rig = Rig::new(TextureLoaderConfig {
            max_textures: 2,
            request_capacity: 8,
            ..config(1 << 20)
        });
        rig.requests.load(vec![1], TextureKind::Atlas).unwrap();
        rig.requests.load(vec![2], TextureKind::Atlas).unwrap();
        let e = rig.requests.load(vec![3], TextureKind::Atlas).unwrap_err();
        assert_eq!((e.reason, e.input), (RejectReason::NoFreeHandles, vec![3]));

        let mut rig = Rig::new(TextureLoaderConfig {
            request_capacity: 1,
            ..config(1 << 20)
        });
        let h = rig.requests.load(vec![1], TextureKind::Atlas).unwrap();
        let e = rig.requests.load(vec![2], TextureKind::Atlas).unwrap_err();
        assert_eq!(e.reason, RejectReason::QueueFull);
        assert_eq!(rig.requests.release(h), Err(ReleaseError::QueueFull));
    }

    #[test]
    fn the_loader_waits_for_the_draw_thread_with_bounded_memory() {
        let mut rig = Rig::new(config(1 << 20));
        for i in 0..8 {
            rig.requests
                .load(png(4, 4, i), TextureKind::Standalone)
                .unwrap();
        }
        pump(&mut rig.loader);
        // The job ring (4) is full; the next decoded image waits in the outbox and no
        // more are decoded.
        assert_eq!(rig.uploads.jobs.slots(), 4);
        assert_eq!(rig.loader.outbox.len(), 1);
        assert_eq!(rig.loader.stats.decoded.load(Ordering::Relaxed), 5);
        rig.settle();
        assert_eq!(rig.loader.stats.decoded.load(Ordering::Relaxed), 8);
        // Every buffer came back to the loader and was freed there.
        pump(&mut rig.loader);
        assert!(rig.loader.spent.is_empty());
    }

    #[test]
    fn the_loader_stops_when_either_side_is_gone() {
        let mut rig = Rig::new(config(1 << 20));
        rig.requests.load(png(2, 2, 1), TextureKind::Atlas).unwrap();
        drop(rig.requests);
        // Queued work is handed over first, and the loader waits for its buffers so
        // that the draw thread never frees one.
        pump(&mut rig.loader);
        assert_eq!(rig.loader.step(), Step::Idle);
        let (_, allocs) =
            slam_testkit::count_allocs(|| rig.uploads.process(&mut rig.device, &mut rig.textures));
        assert_eq!(allocs.deallocs, 0, "{allocs}");
        assert_eq!(rig.loader.step(), Step::Done);

        let (_requests, uploads, mut loader) = parts(&config(1 << 20));
        drop(uploads);
        assert_eq!(loader.step(), Step::Done);
    }

    #[test]
    fn the_draw_thread_never_frees_pixels_whatever_the_interleaving() {
        let mut rig = Rig::new(TextureLoaderConfig {
            max_textures: 64,
            request_capacity: 64,
            ..config(1 << 20)
        });
        for i in 0..48 {
            let kind = if i % 3 == 0 {
                TextureKind::Atlas
            } else {
                TextureKind::Standalone
            };
            rig.requests.load(png(3, 3, i as u8), kind).unwrap();
        }
        let mut seed = 7u32;
        for _ in 0..2000 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            if (seed >> 16).is_multiple_of(3) {
                let (_, allocs) = slam_testkit::count_allocs(|| {
                    rig.uploads.process(&mut rig.device, &mut rig.textures)
                });
                assert_eq!(allocs.total(), 0, "{allocs}");
            } else {
                rig.loader.step();
            }
            assert!(rig.loader.in_flight <= rig.loader.spent_capacity);
        }
        rig.settle();
        assert_eq!(rig.loader.stats.decoded.load(Ordering::Relaxed), 48);
    }

    #[test]
    fn a_failed_page_clear_fails_the_images_on_that_page() {
        let mut rig = Rig::new(config(1 << 20));
        rig.device.fail_writes_after = Some(0);
        let a = rig.requests.load(png(3, 3, 1), TextureKind::Atlas).unwrap();
        let b = rig.requests.load(png(3, 3, 2), TextureKind::Atlas).unwrap();
        pump(&mut rig.loader);
        let (stats, allocs) =
            slam_testkit::count_allocs(|| rig.uploads.process(&mut rig.device, &mut rig.textures));
        assert_eq!(allocs.total(), 0, "{allocs}");
        assert_eq!((stats.device_errors, stats.finished), (1, 2));
        assert_eq!(rig.uploads.get(a), TextureState::Failed(LoadError::Device));
        assert_eq!(rig.uploads.get(b), TextureState::Failed(LoadError::Device));
        assert_eq!(rig.uploads.pages().count(), 0);
    }

    #[test]
    fn a_failed_strip_fails_the_texture_and_release_still_destroys_it() {
        // One row per frame; the second write fails.
        let mut rig = Rig::new(config(16));
        rig.device.fail_writes_after = Some(1);
        let bg = rig
            .requests
            .load(png(10, 3, 3), TextureKind::Standalone)
            .unwrap();
        pump(&mut rig.loader);
        let mut errors = 0;
        for _ in 0..3 {
            let (stats, allocs) = slam_testkit::count_allocs(|| {
                rig.uploads.process(&mut rig.device, &mut rig.textures)
            });
            assert_eq!(allocs.total(), 0, "{allocs}");
            errors += stats.device_errors;
        }
        assert_eq!(errors, 1);
        assert_eq!(rig.uploads.get(bg), TextureState::Failed(LoadError::Device));
        assert_eq!(rig.device.live_textures.len(), 1);
        rig.requests.release(bg).unwrap();
        rig.settle();
        assert!(rig.device.live_textures.is_empty());
    }

    #[test]
    fn the_thread_loads_end_to_end() {
        let (mut requests, mut uploads) = texture_loader(&config(1 << 20)).unwrap();
        let h = requests.load(png(3, 2, 1), TextureKind::Atlas).unwrap();
        let mut device = Fake::default();
        let mut textures = Textures::with_capacity(4);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while uploads.get(h) == TextureState::Pending {
            assert!(std::time::Instant::now() < deadline, "timed out");
            uploads.process(&mut device, &mut textures);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(matches!(uploads.get(h), TextureState::Ready(_)));
    }

    #[test]
    fn processing_does_not_allocate() {
        let mut rig = Rig::new(config(64 * 4 * 16));
        let mut handles = Vec::new();
        for (i, kind) in [
            TextureKind::Atlas,
            TextureKind::Standalone,
            TextureKind::Atlas,
        ]
        .into_iter()
        .enumerate()
        {
            handles.push(rig.requests.load(png(20, 20, i as u8), kind).unwrap());
        }
        pump(&mut rig.loader);
        let mut frames = 0;
        loop {
            let (stats, allocs) = slam_testkit::count_allocs(|| {
                rig.uploads.process(&mut rig.device, &mut rig.textures)
            });
            assert_eq!(allocs.total(), 0, "frame {frames}: {allocs}");
            if stats == UploadStats::default() {
                break;
            }
            frames += 1;
        }
        assert!(frames > 1);
        let bg = handles[1];
        rig.requests.release(bg).unwrap();
        pump(&mut rig.loader);
        let (_, allocs) =
            slam_testkit::count_allocs(|| rig.uploads.process(&mut rig.device, &mut rig.textures));
        assert_eq!(allocs.total(), 0, "release: {allocs}");
        for h in handles {
            assert_ne!(rig.uploads.get(h), TextureState::Failed(LoadError::Device));
        }
    }
}
