use std::ops::Range;
use std::path::{Path, PathBuf};

use slam_render::hal::{Extent, Limits};
use slam_render::upload::{TextureLoaderConfig, texture_loader};

use super::make::{Maker, cache_key, cover_crop, downscale, fnv, fnv_start, output_size};
use super::worker::{QUEUE, Queue};
use super::*;
use crate::songs::SongFile;

const LIMITS: Limits = Limits {
    max_texture_size: 4096,
    max_uniform_block_size: 16384,
    max_uniform_buffer_slots: 12,
    max_texture_slots: 16,
};

fn song_file(set: usize, background: Option<&str>) -> SongFile {
    SongFile {
        path: PathBuf::from(format!("/s{set}")).join("map.osu"),
        approach_rate: 9.0,
        circle_size: 4.0,
        drain_rate: 5.0,
        overall_difficulty: 8.0,
        background: background.map(str::to_owned),
        audio: None,
        preview_ms: -1,
    }
}

/// `n` sets, each with its own background; set 7 has none.
fn files(n: usize) -> Arc<SongFiles> {
    (0..n)
        .map(|i| {
            let name = format!("bg{i}.jpg");
            vec![song_file(i, (i != 7).then_some(name.as_str()))].into_boxed_slice()
        })
        .collect::<Vec<_>>()
        .into()
}

struct Rig {
    thumbnails: Thumbnails,
    requests: rtrb::Consumer<ThumbRequest>,
    replies: rtrb::Producer<ThumbReply>,
    releases: rtrb::Consumer<TextureHandle>,
    shown: Arc<AtomicU32>,
    pool: TextureRequests,
    files: Arc<SongFiles>,
    seq: u32,
}

impl Rig {
    fn new(sets: usize) -> Rig {
        let (requests_tx, requests) = rtrb::RingBuffer::new(REQUEST_RING);
        let (replies, replies_rx) = rtrb::RingBuffer::new(REPLY_RING);
        let (releases_tx, releases) = rtrb::RingBuffer::new(RELEASE_RING);
        let shown = Arc::new(AtomicU32::new(0));
        let pool = texture_loader(&TextureLoaderConfig {
            max_textures: 1024,
            ..TextureLoaderConfig::new(LIMITS)
        })
        .unwrap()
        .0;
        Rig {
            thumbnails: Thumbnails::new(requests_tx, replies_rx, releases_tx, Arc::clone(&shown)),
            requests,
            replies,
            releases,
            shown,
            pool,
            files: files(sets),
            seq: 0,
        }
    }

    /// One update tick wanting the sets of `sets` in the first slots after the margin.
    fn tick(&mut self, height: Option<u32>, sets: Range<usize>) -> [Option<TextureHandle>; WANTED] {
        self.seq += 1;
        let mut wanted = [None; WANTED];
        for (slot, set) in wanted[MARGIN..].iter_mut().zip(sets) {
            *slot = Some(set);
        }
        self.thumbnails.tick(self.seq, height, &self.files, &wanted)
    }

    /// The thumbnail thread's side: takes every request, answers each with a new handle.
    fn serve(&mut self) -> Vec<ThumbRequest> {
        let mut served = Vec::new();
        while let Ok(request) = self.requests.pop() {
            let handle = self.pool.load(Vec::new(), TextureKind::Standalone).unwrap();
            self.replies
                .push(ThumbReply {
                    id: request.id,
                    outcome: Outcome::Ready(handle),
                })
                .unwrap();
            served.push(request);
        }
        served
    }

    fn released(&mut self) -> Vec<TextureHandle> {
        let mut out = Vec::new();
        while let Ok(h) = self.releases.pop() {
            out.push(h);
        }
        out
    }
}

#[test]
fn cover_crop_cuts_the_longer_side_around_the_middle() {
    // 200x100 is wider than 1.6: the sides go.
    assert_eq!(cover_crop((200, 100), 1.6), (20, 0, 160, 100));
    // 100x200 is taller: the top and bottom go.
    let (x, y, w, h) = cover_crop((100, 200), 1.6);
    assert_eq!((x, w), (0, 100));
    assert_eq!(h, 63);
    assert_eq!(y, (200 - 63) / 2);
    // Already the aspect: nothing is cut.
    assert_eq!(cover_crop((160, 100), 1.6), (0, 0, 160, 100));
    // Degenerate input keeps the whole image.
    assert_eq!(cover_crop((0, 0), 1.6), (0, 0, 0, 0));
    assert_eq!(cover_crop((10, 10), 0.0), (0, 0, 10, 10));
    assert_eq!(cover_crop((10, 10), f32::NAN), (0, 0, 10, 10));
}

#[test]
fn the_output_is_the_height_and_never_larger_than_the_crop() {
    assert_eq!(output_size(1600, 1000, 128), (205, 128));
    assert_eq!(output_size(160, 100, 128), (160, 100));
    // Smaller than the height: kept as it is.
    assert_eq!(output_size(80, 50, 128), (80, 50));
    assert_eq!(output_size(0, 0, 128), (1, 1));
}

fn image(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Image {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            pixels.extend(pixel(x, y));
        }
    }
    Image {
        size: Extent::new(width, height),
        pixels,
    }
}

#[test]
fn downscaling_averages_the_covered_pixels() {
    let checker = image(4, 4, |x, y| {
        if (x + y) % 2 == 0 {
            [200, 100, 0, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    let one = downscale(&checker, (0, 0, 4, 4), 1, 1);
    assert_eq!(one.pixels, [100, 50, 0, 255]);
    // A flat image stays flat at any size, also with fractional edges.
    let flat = image(10, 7, |_, _| [9, 8, 7, 255]);
    let small = downscale(&flat, (0, 0, 10, 7), 3, 2);
    assert_eq!(small.size, Extent::new(3, 2));
    assert!(small.pixels.chunks(4).all(|p| p == [9, 8, 7, 255]));
    // Only the crop counts.
    let halves = image(4, 2, |x, _| {
        if x < 2 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    });
    let right = downscale(&halves, (2, 0, 2, 2), 1, 1);
    assert_eq!(right.pixels, [0, 0, 255, 255]);
}

#[test]
fn the_cache_key_is_stable_and_follows_path_size_and_time() {
    // FNV-1a's published test value.
    assert_eq!(fnv(fnv_start(), b"a"), 0xaf63_dc4c_8601_ec8c);
    let path = Path::new("/songs/set/bg.jpg");
    let key = cache_key(path, 1234, 99);
    assert_eq!(key, cache_key(path, 1234, 99));
    assert_ne!(key, cache_key(path, 1235, 99));
    assert_ne!(key, cache_key(path, 1234, 100));
    assert_ne!(key, cache_key(Path::new("/songs/set/bg2.jpg"), 1234, 99));
    // Pinned: a change of the key would orphan every cache on disk.
    assert_eq!(key, 15_456_268_764_923_145_111);
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("slam-thumbs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn png_bytes(image: &Image) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut e = png::Encoder::new(&mut bytes, image.size.width, image.size.height);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    let mut w = e.write_header().unwrap();
    w.write_image_data(&image.pixels).unwrap();
    w.finish().unwrap();
    bytes
}

#[test]
fn a_thumbnail_is_made_cropped_and_cached_then_read_back_from_the_cache() {
    let t = TempDir::new("make");
    let set = t.0.join("set");
    std::fs::create_dir_all(&set).unwrap();
    // 64x32 with a red left quarter: the crop to 1.6 cuts the sides evenly.
    let source = image(64, 32, |x, _| {
        if x < 16 {
            [255, 0, 0, 255]
        } else {
            [0, 255, 0, 255]
        }
    });
    std::fs::write(set.join("BG.PNG"), png_bytes(&source)).unwrap();
    let cache = t.0.join("cache");
    let maker = Maker::new(Some(cache.clone()), 1.6, ImageLimits::default());
    // The name is found without regard to case.
    let made = maker.make(&set, "bg.png", 16).unwrap();
    assert_eq!(made.size, Extent::new(26, 16));
    let files: Vec<_> = std::fs::read_dir(cache.join("thumbnails").join("16"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(
        files[0].ends_with(".png") && files[0].len() == 20,
        "{files:?}"
    );
    // Read from the cache: the same pixels, and the source is not needed any more.
    std::fs::write(set.join("BG.PNG"), png_bytes(&source)).unwrap();
    let again = maker.make(&set, "bg.png", 16).unwrap();
    assert_eq!(again, made);
    // Another height is another file.
    assert!(maker.make(&set, "bg.png", 8).is_some());
    assert!(cache.join("thumbnails").join("8").exists());
    // Missing and undecodable backgrounds give nothing.
    assert!(maker.make(&set, "missing.png", 16).is_none());
    std::fs::write(set.join("junk.jpg"), b"not an image").unwrap();
    assert!(maker.make(&set, "junk.jpg", 16).is_none());
}

#[test]
fn a_changed_background_gets_a_new_cache_file() {
    let t = TempDir::new("changed");
    let set = t.0.join("set");
    std::fs::create_dir_all(&set).unwrap();
    let a = image(32, 20, |_, _| [255, 0, 0, 255]);
    std::fs::write(set.join("bg.png"), png_bytes(&a)).unwrap();
    let maker = Maker::new(Some(t.0.join("cache")), 1.6, ImageLimits::default());
    let first = maker.make(&set, "bg.png", 10).unwrap();
    // Another size: another key, whatever the modification time.
    let b = image(48, 30, |_, _| [0, 0, 255, 255]);
    std::fs::write(set.join("bg.png"), png_bytes(&b)).unwrap();
    let second = maker.make(&set, "bg.png", 10).unwrap();
    assert_ne!(first, second);
    assert_eq!(second.pixels[..4], [0, 0, 255, 255]);
}

#[test]
fn the_queue_takes_the_newest_first_and_drops_the_oldest_on_overflow() {
    let request = |id| ThumbRequest {
        id,
        height: 64,
        files: files(1),
        set: 0,
        difficulty: 0,
    };
    let mut queue = Queue::new();
    for id in 0..QUEUE as u32 {
        assert_eq!(queue.push(request(id)), None);
    }
    assert_eq!(queue.len(), QUEUE);
    // One too many: request 0 goes.
    assert_eq!(queue.push(request(100)), Some(0));
    assert_eq!(queue.len(), QUEUE);
    assert_eq!(queue.pop().map(|r| r.id), Some(100));
    assert_eq!(queue.pop().map(|r| r.id), Some(QUEUE as u32 - 1));
}

#[test]
fn a_row_is_asked_for_once_and_rows_without_a_background_never() {
    let mut rig = Rig::new(20);
    rig.tick(Some(128), 5..10);
    rig.tick(Some(128), 5..10);
    let served = rig.serve();
    // Set 7 has no background; the other four are asked for once, each at 128.
    let mut sets: Vec<usize> = served.iter().map(|r| r.set).collect();
    sets.sort_unstable();
    assert_eq!(sets, [5, 6, 8, 9]);
    assert!(served.iter().all(|r| r.height == 128));
    // Still nothing new on later ticks, answered or not.
    rig.tick(Some(128), 5..10);
    assert!(rig.serve().is_empty());
    assert_eq!(rig.thumbnails.in_flight(), 0);
}

#[test]
fn the_visible_rows_are_the_newest_requests() {
    let mut rig = Rig::new(30);
    // A margin set (slot 0) and a visible window: the first visible row goes last.
    rig.seq += 1;
    let mut wanted = [None; WANTED];
    wanted[0] = Some(1);
    wanted[MARGIN] = Some(10);
    wanted[MARGIN + 1] = Some(11);
    rig.thumbnails.tick(rig.seq, Some(64), &rig.files, &wanted);
    let order: Vec<usize> = rig.serve().iter().map(|r| r.set).collect();
    assert_eq!(order, [1, 11, 10]);
}

#[test]
fn requests_in_flight_are_capped() {
    let mut rig = Rig::new(40);
    rig.tick(Some(128), 10..28);
    assert_eq!(rig.thumbnails.in_flight(), MAX_IN_FLIGHT);
    assert_eq!(rig.serve().len(), MAX_IN_FLIGHT);
    // Answered: the rest follow.
    rig.tick(Some(128), 10..28);
    assert_eq!(rig.serve().len(), 16 - MAX_IN_FLIGHT);
}

#[test]
fn loaded_thumbnails_come_back_for_their_rows() {
    let mut rig = Rig::new(20);
    rig.tick(Some(128), 0..3);
    rig.serve();
    let out = rig.tick(Some(128), 0..3);
    assert!(out[MARGIN..MARGIN + 3].iter().all(Option::is_some));
    assert!(out[..MARGIN].iter().all(Option::is_none));
    assert!(out[MARGIN + 3..].iter().all(Option::is_none));
    // Off: nothing comes back and nothing is asked.
    let out = rig.tick(None, 0..3);
    assert!(out.iter().all(Option::is_none));
}

#[test]
fn a_missing_answer_is_not_asked_again_but_a_dropped_one_is() {
    let mut rig = Rig::new(10);
    rig.tick(Some(64), 0..2);
    let mut served = Vec::new();
    while let Ok(request) = rig.requests.pop() {
        served.push(request.id);
    }
    assert_eq!(served.len(), 2);
    rig.replies
        .push(ThumbReply {
            id: served[0],
            outcome: Outcome::Missing,
        })
        .unwrap();
    rig.replies
        .push(ThumbReply {
            id: served[1],
            outcome: Outcome::Dropped,
        })
        .unwrap();
    rig.tick(Some(64), 0..2);
    let again: Vec<usize> = rig.serve().iter().map(|r| r.set).collect();
    // Which one was dropped: the one answered second.
    assert_eq!(again.len(), 1);
    rig.tick(Some(64), 0..2);
    assert!(rig.serve().is_empty());
}

#[test]
fn the_least_recently_wanted_thumbnail_is_evicted_and_released_after_the_draw() {
    let mut rig = Rig::new(200);
    // Fill the cache: sets 0 to 63, a window of 16 at a time.
    for start in (0..CAPACITY).step_by(16) {
        for _ in 0..3 {
            rig.tick(Some(128), start..start + 16);
            rig.serve();
        }
    }
    rig.tick(Some(128), 48..64);
    // Set 7 has no background and takes no place.
    assert_eq!(rig.thumbnails.cached(), CAPACITY - 1);
    assert_eq!(rig.thumbnails.releasing(), 0);
    // A new window evicts the least recently wanted ones, not the wanted ones.
    for _ in 0..3 {
        rig.tick(Some(128), 64..80);
        rig.serve();
    }
    let loaded = rig.tick(Some(128), 64..80);
    assert!(loaded[MARGIN..MARGIN + 16].iter().all(Option::is_some));
    assert_eq!(rig.thumbnails.cached(), CAPACITY);
    let evicted = rig.thumbnails.releasing();
    assert_eq!(evicted, 15);
    // Until the draw thread shows a snapshot after the eviction, nothing is released.
    assert!(rig.released().is_empty());
    rig.tick(Some(128), 64..80);
    assert!(rig.released().is_empty());
    rig.shown.store(rig.seq, Ordering::Release);
    rig.tick(Some(128), 64..80);
    assert_eq!(rig.released().len(), evicted);
    assert_eq!(rig.thumbnails.releasing(), 0);
    // The wanted ones are still loaded.
    let loaded = rig.tick(Some(128), 64..80);
    assert!(loaded[MARGIN..MARGIN + 16].iter().all(Option::is_some));
}

#[test]
fn a_texture_is_released_only_after_the_snapshot_that_left_it_out_was_drawn() {
    let mut rig = Rig::new(100);
    for _ in 0..3 {
        rig.tick(Some(128), 0..4);
        rig.serve();
    }
    let loaded = rig.tick(Some(128), 0..4);
    let handles: Vec<TextureHandle> = loaded.iter().flatten().copied().collect();
    assert_eq!(handles.len(), 4);
    // Eviction happens in the tick numbered `evicted_at`; its snapshot leaves them out.
    rig.tick(Some(256), 0..4);
    let evicted_at = rig.seq;
    assert_eq!(rig.thumbnails.releasing(), 4);
    // The draw thread is still on the snapshot before it.
    rig.shown.store(evicted_at - 1, Ordering::Release);
    rig.tick(Some(256), 0..4);
    assert!(rig.released().is_empty());
    rig.shown.store(evicted_at, Ordering::Release);
    rig.tick(Some(256), 0..4);
    let mut released = rig.released();
    released.sort_by_key(|h| format!("{h:?}"));
    let mut expected = handles;
    expected.sort_by_key(|h| format!("{h:?}"));
    assert_eq!(released, expected);
}

#[test]
fn another_height_clears_the_cache_and_asks_again() {
    let mut rig = Rig::new(20);
    for _ in 0..2 {
        rig.tick(Some(64), 0..5);
        rig.serve();
    }
    assert_eq!(rig.thumbnails.cached(), 5);
    let out = rig.tick(Some(128), 0..5);
    // The old textures are not shown at the new height.
    assert!(out.iter().all(Option::is_none));
    assert_eq!(rig.thumbnails.releasing(), 5);
    let served = rig.serve();
    assert!(served.iter().all(|r| r.height == 128), "asked again");
    assert_eq!(served.len(), 5);
}

#[test]
fn an_answer_to_a_cleared_request_is_released_unseen() {
    let mut rig = Rig::new(10);
    rig.tick(Some(64), 0..3);
    // The height changes while the answers are on their way.
    rig.tick(Some(128), 0..3);
    let before = rig.requests.slots();
    let served = rig.serve();
    assert!(!served.is_empty() && before > 0);
    rig.tick(Some(128), 0..3);
    // The three answers to the 64 px requests are released (their texture was never shown).
    rig.shown.store(rig.seq, Ordering::Release);
    rig.tick(Some(128), 0..3);
    assert!(rig.released().len() >= 2);
}

#[test]
fn the_ring_between_the_update_and_the_thumbnail_thread_never_overflows() {
    // More wanted rows than the ring holds, ticks without anyone serving: nothing is lost
    // or blocked, requests stop at the cap.
    let mut rig = Rig::new(100);
    for start in 0..30 {
        rig.tick(Some(128), start..start + 16);
    }
    assert_eq!(rig.thumbnails.in_flight(), MAX_IN_FLIGHT);
    assert_eq!(rig.serve().len(), MAX_IN_FLIGHT);
}

#[test]
fn the_bridge_queues_pixels_forwards_answers_and_releases() {
    let (requests_tx, _requests) = rtrb::RingBuffer::<ThumbRequest>::new(4);
    drop(requests_tx);
    let (mut made_tx, made) = rtrb::RingBuffer::new(OUT_RING);
    let (replies_tx, mut replies) = rtrb::RingBuffer::new(REPLY_RING);
    let (mut releases_tx, releases) = rtrb::RingBuffer::new(RELEASE_RING);
    let mut bridge = ThumbBridge {
        made,
        releases,
        replies: replies_tx,
        unsent: None,
        stuck: Vec::new(),
    };
    let mut loader = texture_loader(&TextureLoaderConfig::new(LIMITS)).unwrap().0;
    made_tx
        .push(WorkerOut {
            id: 1,
            made: Made::Image(image(2, 2, |_, _| [1, 2, 3, 255])),
        })
        .unwrap();
    made_tx
        .push(WorkerOut {
            id: 2,
            made: Made::Missing,
        })
        .unwrap();
    made_tx
        .push(WorkerOut {
            id: 3,
            made: Made::Dropped,
        })
        .unwrap();
    bridge.poll(&mut loader);
    let first = replies.pop().unwrap();
    let Outcome::Ready(handle) = first.outcome else {
        panic!("{first:?}")
    };
    assert_eq!(first.id, 1);
    assert_eq!(replies.pop().unwrap().outcome, Outcome::Missing);
    assert_eq!(replies.pop().unwrap().outcome, Outcome::Dropped);
    // A release goes to the loader: the handle is dead afterwards.
    releases_tx.push(handle).unwrap();
    bridge.poll(&mut loader);
    assert_eq!(loader.release(handle), Err(ReleaseError::Stale));
}
