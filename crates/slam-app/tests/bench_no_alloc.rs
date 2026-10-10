//! The benchmark's per-frame work outside the renderer must not allocate. The
//! renderer's own frame (`begin_frame`, `draw`, `present`) is covered by the GL
//! tests in `slam-engine` (`tests/gl_sprites.rs`).

use slam_app::bench::{FrameWork, Recorder, fill};
use slam_engine::limiter::FrameWait;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::{SpriteBatch, SpriteStats};
use slam_render::textures::TextureId;
use slam_render::{Extent, Origin};

slam_testkit::install_counting_allocator!();

#[test]
fn frame_fill_and_record_do_not_allocate() {
    let region = AtlasRegion {
        texture: TextureId(0),
        origin: Origin::default(),
        size: Extent::new(8, 8),
        uv: UvRect::FULL,
    };
    let mut batch = SpriteBatch::with_capacity(500);
    let mut recorder = Recorder::new(0, u64::MAX, 100);
    slam_testkit::assert_no_alloc(|| {
        for frame in 0..100u64 {
            fill(
                &mut batch,
                &region,
                500,
                frame as f32 * 1e-4,
                [1280.0, 720.0],
            );
            let wait = FrameWait {
                deadline_ns: frame * 125_000,
                woke_ns: frame * 125_000 + 3,
                resynced: false,
            };
            let work = FrameWork {
                drawn_ns: wait.woke_ns + 50_000,
                presented_ns: wait.woke_ns + 60_000,
            };
            recorder.record(&wait, work, Some(SpriteStats::default()));
        }
        // Past capacity: counted, still no allocation.
        recorder.record(
            &FrameWait {
                deadline_ns: u64::MAX - 1,
                woke_ns: u64::MAX - 1,
                resynced: false,
            },
            FrameWork::default(),
            None,
        );
    });
    assert_eq!(batch.len(), 500);
    assert_eq!(recorder.report().unrecorded, 1);
}
