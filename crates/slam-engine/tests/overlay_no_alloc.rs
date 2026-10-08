//! Recording frames into the overlay, its periodic refresh (percentiles, rates, text)
//! and pushing it into a sprite batch must not allocate.

use std::sync::Arc;

use slam_engine::limiter::FrameWait;
use slam_engine::overlay::{FrameOverlay, OverlayConfig, font_sheet};
use slam_engine::{EngineCounters, FrameInfo};
use slam_input::InputCounters;
use slam_render::Origin;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_testkit::count_allocs;

slam_testkit::install_counting_allocator!();

#[test]
fn record_refresh_and_push_do_not_allocate() {
    let config = OverlayConfig::default();
    let (size, _) = font_sheet(config.scale);
    let font = AtlasRegion {
        texture: TextureId(0),
        origin: Origin::new(0, 0),
        size,
        uv: UvRect::FULL,
    };
    let engine = Arc::new(EngineCounters::default());
    let input = Arc::new(InputCounters::default());
    let mut overlay =
        FrameOverlay::new(config, font, Arc::clone(&engine), Some(Arc::clone(&input)));
    let mut batch = SpriteBatch::with_capacity(2048);

    let frame = |overlay: &mut FrameOverlay, batch: &mut SpriteBatch, i: u64| {
        // ~1 ms frames with jitter and an occasional spike.
        let t =
            i * 1_000_000 + (i * 7919 % 50_000) + if i.is_multiple_of(997) { 9_000_000 } else { 0 };
        input.record_poll();
        overlay.record(&FrameInfo {
            wait: FrameWait {
                deadline_ns: t,
                woke_ns: t,
                resynced: false,
            },
            fresh: true,
        });
        batch.clear();
        overlay.push(batch, [8.0, 8.0]);
    };
    // Warm up: fill the frame-time ring and the batch's batch list.
    for i in 0..5_000 {
        frame(&mut overlay, &mut batch, i);
    }

    let ((), stats) = count_allocs(|| {
        // Spans many refreshes (every 250 frames).
        for i in 5_000..10_000 {
            frame(&mut overlay, &mut batch, i);
        }
    });
    assert_eq!(stats.total(), 0, "overlay allocated: {stats}");
    assert!(overlay.summary().is_some());
    assert!(!batch.is_empty());
}
