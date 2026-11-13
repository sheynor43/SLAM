use slam_render::atlas::AtlasRegion;
use slam_render::shape::ShapeBatch;

use super::*;

const MS: u64 = 1_000_000;

fn white() -> AtlasRegion {
    AtlasRegion {
        texture: slam_render::textures::TextureId(0),
        origin: slam_render::hal::Origin { x: 0, y: 0 },
        size: slam_render::Extent::new(8, 8),
        uv: slam_render::atlas::UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
    }
}

/// The slats' sprites at `now_ns` on a 1000 × 500 window.
fn sprites(curtain: &Curtain, now_ns: u64) -> usize {
    let mut shapes = ShapeBatch::with_capacity(64);
    curtain.push(
        &mut shapes,
        &Theme::default(),
        1000.0,
        500.0,
        now_ns,
        white(),
    );
    shapes.len()
}

#[test]
fn closes_holds_until_ready_and_opens() {
    let mut curtain = Curtain::new();
    assert!(!curtain.is_shown());
    curtain.close(0);
    assert!(curtain.is_shown());
    curtain.advance(PHASE_NS - 1, true);
    assert!(!curtain.is_closed(), "still closing");
    curtain.advance(PHASE_NS, false);
    assert!(curtain.is_closed());
    curtain.advance(PHASE_NS + 100 * MS, false);
    assert!(curtain.is_closed(), "waits while not ready");
    curtain.advance(PHASE_NS + 200 * MS, true);
    assert!(!curtain.is_closed());
    assert!(curtain.is_shown(), "opening");
    curtain.advance(2 * PHASE_NS + 200 * MS, true);
    assert!(!curtain.is_shown());
}

#[test]
fn opens_after_the_hold_limit_even_when_not_ready() {
    let mut curtain = Curtain::new();
    curtain.close(0);
    curtain.advance(PHASE_NS, false);
    curtain.advance(PHASE_NS + HOLD_LIMIT_NS - 1, false);
    assert!(curtain.is_closed());
    curtain.advance(PHASE_NS + HOLD_LIMIT_NS, false);
    assert!(!curtain.is_closed());
}

#[test]
fn a_close_while_opening_closes_again_once_open() {
    let mut curtain = Curtain::new();
    curtain.close(0);
    curtain.advance(PHASE_NS, true);
    curtain.advance(PHASE_NS + 1, true);
    curtain.close(PHASE_NS + 10 * MS);
    curtain.advance(2 * PHASE_NS + 1, true);
    assert!(curtain.is_shown(), "closing again");
    curtain.advance(3 * PHASE_NS + 1, false);
    assert!(curtain.is_closed());
}

#[test]
fn the_centre_slats_lead_and_all_end_with_the_phase() {
    let centre = SLATS / 2;
    assert!(slat(centre, 0, 50 * MS) > slat(0, 0, 50 * MS));
    assert!(
        slat(SLATS - 1, 0, 50 * MS) == slat(0, 0, 50 * MS),
        "symmetric"
    );
    for i in 0..SLATS {
        assert_eq!(slat(i, 0, PHASE_NS), 1.0, "slat {i}");
        assert_eq!(slat(i, 0, 0), 0.0, "slat {i}");
    }
}

#[test]
fn draws_nothing_open_and_a_full_cover_closed() {
    let mut curtain = Curtain::new();
    assert_eq!(sprites(&curtain, 0), 0);
    curtain.close(0);
    // Mid-way, each slat has a body and an edge.
    assert_eq!(sprites(&curtain, PHASE_NS / 2), 2 * SLATS);
    curtain.advance(PHASE_NS, false);
    assert_eq!(sprites(&curtain, PHASE_NS), SLATS);
}
