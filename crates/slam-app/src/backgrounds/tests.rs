use std::path::PathBuf;

use slam_render::atlas::UvRect;
use slam_render::hal::{Extent, Limits, Origin};
use slam_render::textures::TextureId;
use slam_render::upload::{TextureLoaderConfig, texture_loader};

use super::*;
use crate::songs::SongFile;

const MS: u64 = 1_000_000;

const LIMITS: Limits = Limits {
    max_texture_size: 4096,
    max_uniform_block_size: 16384,
    max_uniform_buffer_slots: 12,
    max_texture_slots: 16,
};

fn requests() -> TextureRequests {
    texture_loader(&TextureLoaderConfig {
        max_textures: 32,
        ..TextureLoaderConfig::new(LIMITS)
    })
    .unwrap()
    .0
}

/// Distinct handles: the loader hands them out at once, whatever the bytes.
fn handles(requests: &mut TextureRequests, n: usize) -> Vec<TextureHandle> {
    (0..n)
        .map(|_| requests.load(Vec::new(), TextureKind::Standalone).unwrap())
        .collect()
}

fn file(dir: &str, background: Option<&str>) -> SongFile {
    SongFile {
        path: PathBuf::from(dir).join("map.osu"),
        approach_rate: 9.0,
        circle_size: 4.0,
        drain_rate: 5.0,
        overall_difficulty: 8.0,
        background: background.map(str::to_owned),
    }
}

/// Set 0: two difficulties sharing `a.jpg` and one with `b.jpg`. Set 1: no background.
fn files() -> Arc<SongFiles> {
    Arc::from(vec![
        vec![
            file("/s0", Some("a.jpg")),
            file("/s0", Some("a.jpg")),
            file("/s0", Some("b.jpg")),
        ]
        .into_boxed_slice(),
        vec![file("/s1", None)].into_boxed_slice(),
    ])
}

struct Rig {
    picker: BackgroundPicker,
    commands: rtrb::Consumer<BackgroundCommand>,
    replies: rtrb::Producer<BackgroundReply>,
    shown: Arc<AtomicU32>,
    files: Arc<SongFiles>,
}

impl Rig {
    fn new() -> Self {
        let (commands_tx, commands) = rtrb::RingBuffer::new(RING);
        let (replies, replies_rx) = rtrb::RingBuffer::new(RING);
        let shown = Arc::new(AtomicU32::new(0));
        Self {
            picker: BackgroundPicker::new(commands_tx, replies_rx, Arc::clone(&shown)),
            commands,
            replies,
            shown,
            files: files(),
        }
    }

    fn tick(&mut self, now_ns: u64, selected: Option<(usize, usize)>) -> BackgroundTarget {
        self.picker.tick(now_ns, &self.files, selected)
    }

    /// The loads asked for, as `(generation, set, difficulty)`, and the releases.
    fn sent(&mut self) -> (Vec<(u32, usize, usize)>, Vec<TextureHandle>) {
        let (mut loads, mut releases) = (Vec::new(), Vec::new());
        while let Ok(command) = self.commands.pop() {
            match command {
                BackgroundCommand::Load {
                    generation,
                    set,
                    difficulty,
                    ..
                } => loads.push((generation, set, difficulty)),
                BackgroundCommand::Release(handle) => releases.push(handle),
            }
        }
        (loads, releases)
    }

    fn reply(&mut self, generation: u32, handle: Option<TextureHandle>) {
        self.replies
            .push(BackgroundReply { generation, handle })
            .unwrap();
    }
}

#[test]
fn asks_once_the_selection_settles() {
    let mut rig = Rig::new();
    rig.tick(0, Some((0, 2)));
    rig.tick(100 * MS, Some((0, 0)));
    // The selection moved at 100 ms: it settles at 250 ms.
    rig.tick(249 * MS, Some((0, 0)));
    assert_eq!(rig.sent().0, []);
    rig.tick(250 * MS, Some((0, 0)));
    assert_eq!(rig.sent().0, [(1, 0, 0)]);
    rig.tick(500 * MS, Some((0, 0)));
    assert_eq!(rig.sent().0, [], "asked once");
}

#[test]
fn difficulties_sharing_a_background_do_not_reload_it() {
    let mut rig = Rig::new();
    rig.tick(0, Some((0, 0)));
    rig.tick(200 * MS, Some((0, 0)));
    assert_eq!(rig.sent().0, [(1, 0, 0)]);
    rig.tick(300 * MS, Some((0, 1)));
    rig.tick(600 * MS, Some((0, 1)));
    assert_eq!(rig.sent().0, []);
    rig.tick(700 * MS, Some((0, 2)));
    rig.tick(900 * MS, Some((0, 2)));
    assert_eq!(rig.sent().0, [(2, 0, 2)]);
}

#[test]
fn nothing_is_asked_without_a_background_at_first() {
    let mut rig = Rig::new();
    rig.tick(0, None);
    assert_eq!(rig.tick(500 * MS, None), BackgroundTarget::default());
    rig.tick(600 * MS, Some((1, 0)));
    assert_eq!(
        rig.tick(900 * MS, Some((1, 0))),
        BackgroundTarget::default()
    );
    assert_eq!(rig.sent(), (vec![], vec![]));
}

#[test]
fn the_old_background_is_released_after_the_draw_thread_shows_the_new_one() {
    let mut requests = requests();
    let h = handles(&mut requests, 2);
    let mut rig = Rig::new();
    rig.tick(0, Some((0, 0)));
    rig.tick(200 * MS, Some((0, 0)));
    assert_eq!(rig.sent().0, [(1, 0, 0)]);
    rig.reply(1, Some(h[0]));
    let target = rig.tick(210 * MS, Some((0, 2)));
    assert_eq!(
        target,
        BackgroundTarget {
            generation: 1,
            handle: Some(h[0])
        }
    );
    rig.shown.store(1, Ordering::Relaxed);
    rig.tick(400 * MS, Some((0, 2)));
    assert_eq!(rig.sent().0, [(2, 0, 2)]);
    rig.reply(2, Some(h[1]));
    let target = rig.tick(410 * MS, Some((0, 2)));
    assert_eq!(
        target,
        BackgroundTarget {
            generation: 2,
            handle: Some(h[1])
        }
    );
    // The draw thread still fades from the first one.
    assert_eq!(rig.sent().1, []);
    rig.shown.store(2, Ordering::Relaxed);
    rig.tick(420 * MS, Some((0, 2)));
    assert_eq!(rig.sent().1, [h[0]]);
}

#[test]
fn an_answer_to_an_older_request_is_released_unseen() {
    let mut requests = requests();
    let h = handles(&mut requests, 2);
    let mut rig = Rig::new();
    rig.tick(0, Some((0, 0)));
    rig.tick(200 * MS, Some((0, 0)));
    rig.tick(210 * MS, Some((0, 2)));
    rig.tick(400 * MS, Some((0, 2)));
    assert_eq!(rig.sent().0, [(1, 0, 0), (2, 0, 2)]);
    rig.reply(1, Some(h[0]));
    let target = rig.tick(410 * MS, Some((0, 2)));
    assert_eq!(target, BackgroundTarget::default());
    assert_eq!(rig.sent().1, [h[0]]);
    rig.reply(2, Some(h[1]));
    assert_eq!(
        rig.tick(420 * MS, Some((0, 2))),
        BackgroundTarget {
            generation: 2,
            handle: Some(h[1])
        }
    );
}

#[test]
fn a_selection_without_a_background_goes_to_the_theme_at_once() {
    let mut requests = requests();
    let h = handles(&mut requests, 1);
    let mut rig = Rig::new();
    rig.tick(0, Some((0, 0)));
    rig.tick(200 * MS, Some((0, 0)));
    rig.reply(1, Some(h[0]));
    rig.tick(210 * MS, Some((1, 0)));
    let target = rig.tick(400 * MS, Some((1, 0)));
    assert_eq!(
        target,
        BackgroundTarget {
            generation: 2,
            handle: None
        }
    );
    assert_eq!(rig.sent(), (vec![(1, 0, 0)], vec![]));
    rig.shown.store(2, Ordering::Relaxed);
    rig.tick(410 * MS, Some((1, 0)));
    assert_eq!(rig.sent().1, [h[0]]);
}

#[test]
fn no_new_request_while_the_older_backgrounds_are_held() {
    let mut requests = requests();
    let h = handles(&mut requests, OLDER + 1);
    let mut rig = Rig::new();
    let mut now = 0;
    // The draw thread never confirms: each answer pushes the target to the older ones.
    for (i, &handle) in h.iter().enumerate() {
        let selected = Some((0, if i % 2 == 0 { 0 } else { 2 }));
        rig.tick(now, selected);
        rig.tick(now + 200 * MS, selected);
        assert_eq!(rig.sent().0.len(), 1);
        rig.reply(rig.picker.generation, Some(handle));
        now += 210 * MS;
    }
    // The last answer was for `a.jpg`: ask for `b.jpg` again.
    rig.tick(now, Some((0, 2)));
    assert!(rig.picker.older.iter().all(Option::is_some));
    rig.tick(now + 200 * MS, Some((0, 2)));
    assert_eq!(rig.sent().0, [], "waits for the draw thread");
    rig.shown
        .store(rig.picker.target.generation, Ordering::Relaxed);
    rig.tick(now + 210 * MS, Some((0, 2)));
    let (loads, releases) = rig.sent();
    assert_eq!(releases.len(), OLDER);
    assert_eq!(loads.len(), 1);
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("slam-backgrounds-{name}-{}", std::process::id()));
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

fn loader() -> (
    BackgroundLoader,
    rtrb::Producer<BackgroundCommand>,
    rtrb::Consumer<BackgroundReply>,
) {
    let (commands, commands_rx) = rtrb::RingBuffer::new(RING);
    let (replies_tx, replies) = rtrb::RingBuffer::new(RING);
    let loader = BackgroundLoader {
        requests: requests(),
        commands: commands_rx,
        replies: replies_tx,
        unsent: None,
        releases: Vec::new(),
    };
    (loader, commands, replies)
}

#[test]
fn the_loader_reads_only_the_newest_request_and_finds_the_file_without_case() {
    let t = TempDir::new("loader");
    std::fs::write(t.0.join("BG.JPG"), b"not decoded here").unwrap();
    let dir = t.0.to_str().unwrap();
    let files: Arc<SongFiles> = Arc::from(vec![
        vec![file(dir, Some("bg.jpg")), file(dir, Some("missing.png"))].into_boxed_slice(),
    ]);
    let (mut loader, mut commands, mut replies) = loader();
    for (generation, difficulty) in [(1, 1), (2, 0)] {
        let load = BackgroundCommand::Load {
            generation,
            files: Arc::clone(&files),
            set: 0,
            difficulty,
        };
        assert!(commands.push(load).is_ok());
    }
    loader.poll();
    let reply = replies.pop().unwrap();
    assert_eq!(reply.generation, 2);
    assert!(reply.handle.is_some());
    assert!(replies.pop().is_err(), "the older load is skipped");

    let load = BackgroundCommand::Load {
        generation: 3,
        files: Arc::clone(&files),
        set: 0,
        difficulty: 1,
    };
    assert!(commands.push(load).is_ok());
    loader.poll();
    assert_eq!(
        replies.pop().unwrap(),
        BackgroundReply {
            generation: 3,
            handle: None
        }
    );
    // Its `Arc` was the loader's to drop.
    assert_eq!(Arc::strong_count(&files), 1);
}

#[test]
fn the_loader_releases_handles() {
    let (mut loader, mut commands, _replies) = loader();
    let handle = loader
        .requests
        .load(Vec::new(), TextureKind::Standalone)
        .unwrap();
    assert!(commands.push(BackgroundCommand::Release(handle)).is_ok());
    loader.poll();
    assert!(loader.requests.release(handle).is_err(), "already released");
}

fn ready() -> TextureState {
    TextureState::Ready(AtlasRegion {
        texture: TextureId(0),
        origin: Origin { x: 0, y: 0 },
        size: Extent::new(16, 9),
        uv: UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
    })
}

#[test]
fn the_fade_waits_for_the_upload_then_fades_in_and_confirms() {
    let mut requests = requests();
    let h = handles(&mut requests, 1);
    let shown = Arc::new(AtomicU32::new(0));
    let mut fade = BackgroundFade::new(Arc::clone(&shown), None);
    let target = BackgroundTarget {
        generation: 1,
        handle: Some(h[0]),
    };
    fade.advance(target, 0, |_| TextureState::Pending);
    fade.advance(target, 100 * MS, |_| TextureState::Pending);
    assert_eq!(fade.layers(100 * MS), [None, None]);
    fade.advance(target, 200 * MS, |_| ready());
    let [under, over] = fade.layers(200 * MS);
    assert_eq!((under, over), (Some((h[0], 0.0)), None));
    fade.advance(target, 450 * MS, |_| ready());
    let alpha = fade.layers(450 * MS)[0].unwrap().1;
    assert!(alpha > 0.9 && alpha < 1.0, "OutQuint: {alpha}");
    assert_eq!(shown.load(Ordering::Relaxed), 0);
    fade.advance(target, 700 * MS, |_| ready());
    assert_eq!(shown.load(Ordering::Relaxed), 1);
    assert_eq!(fade.layers(700 * MS), [Some((h[0], 1.0)), None]);
}

#[test]
fn the_old_background_fades_out_over_the_new_one() {
    let mut requests = requests();
    let h = handles(&mut requests, 2);
    let shown = Arc::new(AtomicU32::new(0));
    let mut fade = BackgroundFade::new(Arc::clone(&shown), None);
    let first = BackgroundTarget {
        generation: 1,
        handle: Some(h[0]),
    };
    fade.advance(first, 0, |_| ready());
    fade.advance(first, 500 * MS, |_| ready());
    let second = BackgroundTarget {
        generation: 2,
        handle: Some(h[1]),
    };
    fade.advance(second, 600 * MS, |_| ready());
    let [under, over] = fade.layers(725 * MS);
    assert_eq!(under.unwrap().0, h[1]);
    assert_eq!(over, Some((h[0], 0.5)));
    fade.advance(second, 1100 * MS, |_| ready());
    assert_eq!(shown.load(Ordering::Relaxed), 2);
    assert_eq!(fade.layers(1100 * MS), [Some((h[1], 1.0)), None]);
}

#[test]
fn without_an_image_the_old_background_fades_to_the_theme() {
    let mut requests = requests();
    let h = handles(&mut requests, 2);
    let shown = Arc::new(AtomicU32::new(0));
    let mut fade = BackgroundFade::new(Arc::clone(&shown), None);
    let first = BackgroundTarget {
        generation: 1,
        handle: Some(h[0]),
    };
    fade.advance(first, 0, |_| ready());
    fade.advance(first, 500 * MS, |_| ready());
    // The new image failed to decode.
    let failed = BackgroundTarget {
        generation: 2,
        handle: Some(h[1]),
    };
    let state = |handle| {
        if handle == h[1] {
            TextureState::Failed(slam_render::upload::LoadError::Decode)
        } else {
            ready()
        }
    };
    fade.advance(failed, 600 * MS, state);
    assert_eq!(fade.layers(700 * MS)[1], Some((h[0], 0.6)));
    fade.advance(failed, 850 * MS, state);
    assert_eq!(shown.load(Ordering::Relaxed), 2);
    assert_eq!(fade.layers(850 * MS), [None, None]);
    // No image at all.
    fade.advance(first, 900 * MS, |_| ready());
    fade.advance(first, 1400 * MS, |_| ready());
    let none = BackgroundTarget {
        generation: 3,
        handle: None,
    };
    fade.advance(none, 1500 * MS, |_| ready());
    fade.advance(none, 1750 * MS, |_| ready());
    assert_eq!(shown.load(Ordering::Relaxed), 3);
    assert_eq!(fade.layers(1750 * MS), [None, None]);
}

#[test]
fn without_an_image_the_default_fades_in_under_the_old_background() {
    let mut requests = requests();
    let h = handles(&mut requests, 3);
    let default = h[2];
    let shown = Arc::new(AtomicU32::new(0));
    let mut fade = BackgroundFade::new(Arc::clone(&shown), Some(default));
    // Before any target: the default, whole.
    assert_eq!(fade.layers(0), [Some((default, 1.0)), None]);
    let first = BackgroundTarget {
        generation: 1,
        handle: Some(h[0]),
    };
    fade.advance(first, 0, |_| ready());
    fade.advance(first, 500 * MS, |_| ready());
    // The new image failed to decode: the default comes in as an image would.
    let failed = BackgroundTarget {
        generation: 2,
        handle: Some(h[1]),
    };
    let state = |handle| {
        if handle == h[1] {
            TextureState::Failed(slam_render::upload::LoadError::Decode)
        } else {
            ready()
        }
    };
    fade.advance(failed, 600 * MS, state);
    let [under, over] = fade.layers(725 * MS);
    assert_eq!(under.unwrap().0, default);
    assert_eq!(over, Some((h[0], 0.5)));
    fade.advance(failed, 1100 * MS, state);
    assert_eq!(shown.load(Ordering::Relaxed), 2);
    assert_eq!(fade.layers(1100 * MS), [Some((default, 1.0)), None]);
    // Another beatmap without an image: the default stays, without a fade over itself.
    let none = BackgroundTarget {
        generation: 3,
        handle: None,
    };
    fade.advance(none, 1200 * MS, |_| ready());
    assert_eq!(shown.load(Ordering::Relaxed), 3);
    assert_eq!(fade.layers(1200 * MS), [Some((default, 1.0)), None]);
}

#[test]
fn releases_the_full_ring_refused_go_out_on_a_later_tick() {
    let mut requests = requests();
    let h = handles(&mut requests, RING + 1);
    let mut rig = Rig::new();
    for &handle in &h {
        rig.picker.release(handle);
    }
    assert_eq!(rig.sent().1, h[..RING]);
    rig.tick(0, None);
    assert_eq!(rig.sent().1, [h[RING]]);
}
