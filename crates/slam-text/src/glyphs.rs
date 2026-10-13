//! Glyph images on demand: a generator thread makes MSDF bitmaps, the texture loader
//! puts them in atlas pages, and the draw thread asks a [`GlyphCache`] what is ready.
//!
//! The draw thread never makes a system call here: it does not wake the generator.
//! The generator polls its request ring with `park_timeout`, 5 ms at first, doubling
//! to 100 ms while idle and back to 5 ms after any work. The first glyph after a
//! long idle therefore shows after at most about 100 ms plus its generation and
//! upload (the glyph is [`Glyph::Pending`] until then).

use std::io;
use std::sync::Arc;
use std::thread::Thread;
use std::time::Duration;

use slam_render::Device;
use slam_render::Limits;
use slam_render::atlas::AtlasRegion;
use slam_render::image::Image;
use slam_render::textures::{TextureId, Textures};
use slam_render::upload::{
    RejectReason, Rejected, TextureHandle, TextureKind, TextureLoaderConfig, TextureRequests,
    TextureState, TextureUploads, UploadStats, texture_loader,
};

use crate::{FontId, FontRegistry, GlyphKey, generate_glyph};

/// Poll interval of an idle generator at first, and while it waits for the loader.
const POLL_MIN: Duration = Duration::from_millis(5);
/// Longest poll interval of an idle generator.
const POLL_MAX: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug)]
pub struct GlyphAtlasConfig {
    /// Loader of the glyph atlas pages. `glyph_atlas` raises `max_textures` to at
    /// least `max_glyphs`.
    pub loader: TextureLoaderConfig,
    /// Distinct glyphs the cache can hold; atlas space is never freed (ADR-0020), so
    /// glyphs past this count are not drawn.
    pub max_glyphs: u32,
    /// Glyphs requested and not yet answered (also the size of both rings).
    pub request_capacity: usize,
}

impl GlyphAtlasConfig {
    pub fn new(limits: Limits) -> Self {
        let max_glyphs = 4096;
        Self {
            loader: TextureLoaderConfig {
                max_textures: max_glyphs,
                ..TextureLoaderConfig::new(limits)
            },
            max_glyphs,
            request_capacity: 256,
        }
    }
}

/// What the draw thread sees of a glyph this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glyph {
    /// Drawable now. `offset` is the bitmap's top-left corner relative to the pen
    /// on the baseline, in atlas pixels (see [`crate::GlyphBitmap::offset`]). Do not
    /// keep the region across frames (ADR-0034).
    Ready {
        region: AtlasRegion,
        offset: [f32; 2],
    },
    /// Nothing to draw: a blank glyph, or one that failed or does not fit the cache.
    Empty,
    /// Not generated or uploaded yet; ask again next frame.
    Pending,
}

/// What [`GlyphCache::process`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphStats {
    /// Results taken from the generator.
    pub results: u32,
    /// Glyphs requested and not answered yet.
    pub outstanding: u32,
    /// Lookups since the last call that found the table full.
    pub table_full: u32,
    pub upload: UploadStats,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum GlyphResult {
    Ready {
        key: GlyphKey,
        handle: TextureHandle,
        offset: [f32; 2],
    },
    Empty(GlyphKey),
    Failed(GlyphKey),
}

/// Creates the draw-side cache and starts the generator thread `slam-glyph-gen` (and
/// the loader thread of the glyph pages). Both end once the cache is dropped.
pub fn glyph_atlas(
    registry: Arc<FontRegistry>,
    config: &GlyphAtlasConfig,
) -> io::Result<GlyphCache> {
    let (mut cache, generator) = parts(registry, config)?;
    let thread = std::thread::Builder::new()
        .name("slam-glyph-gen".into())
        .spawn(move || generator.run())?;
    cache.generator = Some(thread.thread().clone());
    Ok(cache)
}

/// The cache and the generator; the generator is not running yet.
pub(crate) fn parts(
    registry: Arc<FontRegistry>,
    config: &GlyphAtlasConfig,
) -> io::Result<(GlyphCache, Generator<TextureRequests>)> {
    let loader_config = TextureLoaderConfig {
        max_textures: config.loader.max_textures.max(config.max_glyphs),
        ..config.loader
    };
    let (sink, uploads) = texture_loader(&loader_config)?;
    let capacity = config.request_capacity.max(1);
    let (request_tx, request_rx) = rtrb::RingBuffer::new(capacity);
    // Holds one result per outstanding request, and the cache keeps outstanding at
    // or below `capacity`, so the generator's pushes cannot fail.
    let (result_tx, result_rx) = rtrb::RingBuffer::new(capacity);
    let slots = (config.max_glyphs as usize * 2).next_power_of_two().max(2);
    let cache = GlyphCache {
        requests: request_tx,
        results: result_rx,
        uploads,
        table: vec![Slot::VACANT; slots],
        shift: 64 - slots.trailing_zeros(),
        len: 0,
        max_glyphs: config.max_glyphs as usize,
        outstanding: 0,
        capacity,
        table_full: 0,
        generator: None,
    };
    Ok((cache, Generator::new(registry, sink, request_rx, result_tx)))
}

/// Where the generator puts finished bitmaps.
pub(crate) trait PixelSink {
    fn load(&mut self, image: Image) -> Result<TextureHandle, Rejected<Image>>;
}

impl PixelSink for TextureRequests {
    fn load(&mut self, image: Image) -> Result<TextureHandle, Rejected<Image>> {
        self.load_pixels(image, TextureKind::Atlas)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Did something; call again at once.
    Busy,
    /// Nothing requested.
    Idle,
    /// A bitmap waits for room in the loader's queue.
    Blocked,
    /// The cache is gone.
    Done,
}

struct Held {
    key: GlyphKey,
    offset: [f32; 2],
    image: Image,
}

/// Turns requested keys into bitmaps and hands them to the loader; runs on its own
/// thread. Answers every request exactly once.
pub(crate) struct Generator<S> {
    registry: Arc<FontRegistry>,
    sink: S,
    requests: rtrb::Consumer<GlyphKey>,
    results: rtrb::Producer<GlyphResult>,
    /// A bitmap the loader's queue had no room for; retried as is.
    held: Option<Held>,
}

impl<S: PixelSink> Generator<S> {
    pub(crate) fn new(
        registry: Arc<FontRegistry>,
        sink: S,
        requests: rtrb::Consumer<GlyphKey>,
        results: rtrb::Producer<GlyphResult>,
    ) -> Self {
        Self {
            registry,
            sink,
            requests,
            results,
            held: None,
        }
    }

    pub(crate) fn run(mut self) {
        let mut idle = POLL_MIN;
        loop {
            match self.step() {
                Step::Busy => idle = POLL_MIN,
                Step::Idle => {
                    std::thread::park_timeout(idle);
                    idle = (idle * 2).min(POLL_MAX);
                }
                Step::Blocked => std::thread::park_timeout(POLL_MIN),
                Step::Done => return,
            }
        }
    }

    /// Handles one request, or retries the held bitmap.
    pub(crate) fn step(&mut self) -> Step {
        // The cache owns the loader's draw side too: once it is gone nothing would
        // ever make room for a held bitmap.
        if self.requests.is_abandoned() {
            return Step::Done;
        }
        if let Some(held) = self.held.take() {
            return self.submit(held);
        }
        let Ok(key) = self.requests.pop() else {
            return Step::Idle;
        };
        match generate_glyph(&self.registry, key) {
            Ok(Some(bitmap)) => self.submit(Held {
                key,
                offset: bitmap.offset,
                image: bitmap.image,
            }),
            Ok(None) => {
                self.finish(GlyphResult::Empty(key));
                Step::Busy
            }
            Err(error) => {
                tracing::warn!(?key, %error, "glyph generation failed");
                self.finish(GlyphResult::Failed(key));
                Step::Busy
            }
        }
    }

    fn submit(&mut self, held: Held) -> Step {
        let Held { key, offset, image } = held;
        match self.sink.load(image) {
            Ok(handle) => self.finish(GlyphResult::Ready {
                key,
                handle,
                offset,
            }),
            Err(Rejected {
                reason: RejectReason::QueueFull,
                input,
            }) => {
                self.held = Some(Held {
                    key,
                    offset,
                    image: input,
                });
                return Step::Blocked;
            }
            Err(Rejected {
                reason: RejectReason::NoFreeHandles,
                ..
            }) => self.finish(GlyphResult::Failed(key)),
        }
        Step::Busy
    }

    fn finish(&mut self, result: GlyphResult) {
        // The cache never has more requests outstanding than the ring holds.
        self.results
            .push(result)
            .expect("the results ring has room for every outstanding request");
    }
}

#[derive(Clone, Copy, Debug)]
enum Entry {
    Vacant,
    Requested,
    Ready {
        handle: TextureHandle,
        offset: [f32; 2],
    },
    Empty,
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    key: GlyphKey,
    entry: Entry,
}

impl Slot {
    const VACANT: Self = Self {
        key: GlyphKey {
            font: FontId(0),
            weight: 0,
            glyph_id: 0,
        },
        entry: Entry::Vacant,
    };
}

/// Draw-thread side of the glyph atlas; see the module docs.
pub struct GlyphCache {
    requests: rtrb::Producer<GlyphKey>,
    results: rtrb::Consumer<GlyphResult>,
    uploads: TextureUploads,
    /// Open addressing with linear probing; never more than half full and never
    /// shrunk (atlas space is not freed).
    table: Vec<Slot>,
    shift: u32,
    len: usize,
    max_glyphs: usize,
    /// Requests pushed whose result is not drained yet.
    outstanding: usize,
    capacity: usize,
    table_full: u32,
    generator: Option<Thread>,
}

impl Drop for GlyphCache {
    fn drop(&mut self) {
        // Not a hot path: lets the generator notice at once instead of at its poll.
        if let Some(thread) = &self.generator {
            thread.unpark();
        }
    }
}

impl GlyphCache {
    /// Takes the generator's results, then uploads queued bitmaps. Call once per
    /// frame before recording text. Does not allocate if `textures` has spare
    /// capacity (see [`TextureUploads::process`]).
    pub fn process<D: Device>(
        &mut self,
        device: &mut D,
        textures: &mut Textures<D::Texture>,
    ) -> GlyphStats {
        let mut results = 0;
        while let Ok(result) = self.results.pop() {
            results += 1;
            self.outstanding -= 1;
            let (key, entry) = match result {
                GlyphResult::Ready {
                    key,
                    handle,
                    offset,
                } => (key, Entry::Ready { handle, offset }),
                GlyphResult::Empty(key) | GlyphResult::Failed(key) => (key, Entry::Empty),
            };
            let index = self.find(key);
            self.table[index].entry = entry;
        }
        let upload = self.uploads.process(device, textures);
        GlyphStats {
            results,
            outstanding: self.outstanding as u32,
            table_full: std::mem::take(&mut self.table_full),
            upload,
        }
    }

    /// The glyph's state this frame. A key seen for the first time is requested
    /// (if the request ring has room; otherwise it is asked for again by a later
    /// call).
    pub fn glyph(&mut self, key: GlyphKey) -> Glyph {
        let index = self.find(key);
        match self.table[index].entry {
            Entry::Requested => Glyph::Pending,
            Entry::Empty => Glyph::Empty,
            Entry::Ready { handle, offset } => match self.uploads.get(handle) {
                TextureState::Ready(region) => Glyph::Ready { region, offset },
                TextureState::Pending => Glyph::Pending,
                TextureState::Failed(_) => Glyph::Empty,
            },
            Entry::Vacant => {
                if self.len >= self.max_glyphs {
                    self.table_full = self.table_full.saturating_add(1);
                    return Glyph::Empty;
                }
                if self.outstanding >= self.capacity || self.requests.push(key).is_err() {
                    return Glyph::Pending;
                }
                self.table[index] = Slot {
                    key,
                    entry: Entry::Requested,
                };
                self.len += 1;
                self.outstanding += 1;
                Glyph::Pending
            }
        }
    }

    /// Ids of the glyph atlas pages; the caller destroys them with the rest of its
    /// textures.
    pub fn pages(&self) -> impl Iterator<Item = TextureId> + '_ {
        self.uploads.pages()
    }

    /// Slot of `key`, or the vacant slot where it would go.
    fn find(&self, key: GlyphKey) -> usize {
        let packed =
            (u64::from(key.font.0) << 32) | (u64::from(key.weight) << 16) | u64::from(key.glyph_id);
        let mask = self.table.len() - 1;
        let mut i = (packed.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> self.shift) as usize;
        loop {
            let slot = &self.table[i];
            if matches!(slot.entry, Entry::Vacant) || slot.key == key {
                return i;
            }
            i = (i + 1) & mask;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{LIMITS, Rig, key_of};
    use slam_testkit::count_allocs;

    #[test]
    fn same_key_is_requested_once() {
        let mut rig = Rig::new(16, 8);
        let a = key_of(&rig.registry, 400, 'A');
        assert_eq!(rig.cache.glyph(a), Glyph::Pending);
        assert_eq!(rig.cache.glyph(a), Glyph::Pending);
        rig.pump();
        let stats = rig.cache.process(&mut rig.device, &mut rig.textures);
        assert_eq!((stats.results, stats.outstanding), (1, 0));
        assert!(matches!(rig.settle(a), Glyph::Ready { .. }));
    }

    #[test]
    fn outstanding_requests_are_limited_and_retried_later() {
        let mut rig = Rig::new(16, 2);
        let [a, b, c] = ['A', 'B', 'C'].map(|ch| key_of(&rig.registry, 400, ch));
        assert_eq!(rig.cache.glyph(a), Glyph::Pending);
        assert_eq!(rig.cache.glyph(b), Glyph::Pending);
        assert_eq!(rig.cache.glyph(c), Glyph::Pending);
        let stats = rig.cache.process(&mut rig.device, &mut rig.textures);
        assert_eq!(stats.outstanding, 2);
        // The generator saw two requests, not three.
        rig.pump();
        let stats = rig.cache.process(&mut rig.device, &mut rig.textures);
        assert_eq!((stats.results, stats.outstanding), (2, 0));
        rig.pump();
        assert_eq!(
            rig.cache
                .process(&mut rig.device, &mut rig.textures)
                .results,
            0
        );
        // `c` is asked for now.
        assert!(matches!(rig.settle(c), Glyph::Ready { .. }));
    }

    #[test]
    fn blank_glyph_and_unknown_font_are_empty() {
        let mut rig = Rig::new(16, 8);
        let space = key_of(&rig.registry, 400, ' ');
        assert_eq!(rig.settle(space), Glyph::Empty);
        let missing = GlyphKey {
            font: FontId(9999),
            weight: 400,
            glyph_id: 1,
        };
        assert_eq!(rig.settle(missing), Glyph::Empty);
    }

    #[test]
    fn ready_glyph_has_region_and_offset() {
        let mut rig = Rig::new(16, 8);
        let key = key_of(&rig.registry, 400, 'O');
        let expected = generate_glyph(&rig.registry, key).unwrap().unwrap();
        let Glyph::Ready { region, offset } = rig.settle(key) else {
            panic!("not ready");
        };
        assert_eq!(offset, expected.offset);
        assert_eq!(region.size, expected.image.size);
        assert!(rig.cache.pages().any(|p| p == region.texture));
    }

    #[test]
    fn full_table_gives_empty_and_counts() {
        let mut rig = Rig::new(2, 8);
        let [a, b, c] = ['A', 'B', 'C'].map(|ch| key_of(&rig.registry, 400, ch));
        assert!(matches!(rig.settle(a), Glyph::Ready { .. }));
        assert!(matches!(rig.settle(b), Glyph::Ready { .. }));
        assert_eq!(rig.cache.glyph(c), Glyph::Empty);
        assert_eq!(rig.cache.glyph(c), Glyph::Empty);
        let stats = rig.cache.process(&mut rig.device, &mut rig.textures);
        assert_eq!(stats.table_full, 2);
        assert_eq!(
            rig.cache
                .process(&mut rig.device, &mut rig.textures)
                .table_full,
            0
        );
        // Known glyphs still work.
        assert!(matches!(rig.cache.glyph(a), Glyph::Ready { .. }));
    }

    /// Rejects the first `rejects` loads with `QueueFull`, then forwards.
    struct Flaky {
        real: TextureRequests,
        rejects: u32,
        seen: Vec<*const u8>,
    }

    impl PixelSink for Flaky {
        fn load(&mut self, image: Image) -> Result<TextureHandle, Rejected<Image>> {
            self.seen.push(image.pixels.as_ptr());
            if self.rejects > 0 {
                self.rejects -= 1;
                return Err(Rejected {
                    reason: RejectReason::QueueFull,
                    input: image,
                });
            }
            PixelSink::load(&mut self.real, image)
        }
    }

    struct Exhausted;

    impl PixelSink for Exhausted {
        fn load(&mut self, image: Image) -> Result<TextureHandle, Rejected<Image>> {
            Err(Rejected {
                reason: RejectReason::NoFreeHandles,
                input: image,
            })
        }
    }

    fn generator<S: PixelSink>(
        sink: S,
    ) -> (
        Generator<S>,
        rtrb::Producer<GlyphKey>,
        rtrb::Consumer<GlyphResult>,
        GlyphKey,
    ) {
        let registry = Arc::new(FontRegistry::with_builtin());
        let key = key_of(&registry, 400, 'A');
        let (request_tx, request_rx) = rtrb::RingBuffer::new(4);
        let (result_tx, result_rx) = rtrb::RingBuffer::new(4);
        (
            Generator::new(registry, sink, request_rx, result_tx),
            request_tx,
            result_rx,
            key,
        )
    }

    #[test]
    fn rejected_bitmap_is_kept_and_retried() {
        let (real, _uploads) = texture_loader(&TextureLoaderConfig::new(LIMITS)).unwrap();
        let sink = Flaky {
            real,
            rejects: 2,
            seen: Vec::new(),
        };
        let (mut generator, mut requests, mut results, key) = generator(sink);
        requests.push(key).unwrap();
        assert_eq!(generator.step(), Step::Blocked);
        assert_eq!(generator.step(), Step::Blocked);
        assert!(results.pop().is_err());
        assert_eq!(generator.step(), Step::Busy);
        // The same pixel buffer each time: nothing was regenerated.
        let seen = &generator.sink.seen;
        assert_eq!(seen.len(), 3);
        assert!(seen.iter().all(|p| *p == seen[0]));
        assert!(matches!(
            results.pop(),
            Ok(GlyphResult::Ready { key: k, .. }) if k == key
        ));
        assert_eq!(generator.step(), Step::Idle);
        assert!(results.pop().is_err());
    }

    #[test]
    fn no_free_handles_fails_the_glyph() {
        let (mut generator, mut requests, mut results, key) = generator(Exhausted);
        requests.push(key).unwrap();
        assert_eq!(generator.step(), Step::Busy);
        assert!(matches!(results.pop(), Ok(GlyphResult::Failed(k)) if k == key));
    }

    #[test]
    fn generator_ends_when_the_cache_is_gone() {
        let (mut generator, requests, _results, _) = generator(Exhausted);
        assert_eq!(generator.step(), Step::Idle);
        drop(requests);
        assert_eq!(generator.step(), Step::Done);
    }

    #[test]
    fn lookups_and_frames_do_not_allocate() {
        let mut rig = Rig::new(16, 8);
        let [a, b, c] = ['A', 'B', 'C'].map(|ch| key_of(&rig.registry, 400, ch));
        rig.settle(a);
        rig.settle(b);
        let (stats, allocs) = count_allocs(|| {
            let mut ready = 0;
            for key in [a, b, c, a, b] {
                ready += u32::from(matches!(rig.cache.glyph(key), Glyph::Ready { .. }));
            }
            let stats = rig.cache.process(&mut rig.device, &mut rig.textures);
            (ready, stats)
        });
        assert_eq!(stats.0, 4);
        assert_eq!(allocs.total(), 0, "{allocs}");
        assert_eq!(stats.1.outstanding, 1);
    }

    #[test]
    fn threads_make_a_glyph_ready_and_end_with_the_cache() {
        let registry = Arc::new(FontRegistry::with_builtin());
        let key = key_of(&registry, 400, 'g');
        let mut cache = glyph_atlas(Arc::clone(&registry), &config_for_test()).unwrap();
        let mut device = crate::test_support::TestDevice::default();
        let mut textures = Textures::with_capacity(64);
        let mut ready = false;
        for _ in 0..1000 {
            cache.process(&mut device, &mut textures);
            if matches!(cache.glyph(key), Glyph::Ready { .. }) {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ready, "glyph not ready in 5 s");
        drop(cache);
        // The generator drops its registry handle when it ends.
        for _ in 0..1000 {
            if Arc::strong_count(&registry) == 1 {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("generator thread did not end");
    }

    fn config_for_test() -> GlyphAtlasConfig {
        crate::test_support::config(64, 16)
    }
}
