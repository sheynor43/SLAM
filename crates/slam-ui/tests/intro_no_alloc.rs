//! The intro does not touch the allocator once warm: its state on the update side and
//! `IntroView::push` on the draw side (ADR-0114, ADR-0186).

use slam_render::Extent;
use slam_render::Origin;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::shape::ShapeBatch;
use slam_render::textures::TextureId;
use slam_testkit::assert_no_alloc;
use slam_ui::intro::{Intro, IntroView, LENGTH_NS};
use slam_ui::menu::LogoArt;
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

const FRAME_NS: u64 = 16_000_000;

#[test]
fn intro_state_does_not_allocate() {
    let mut intro = Intro::new();
    assert_no_alloc(|| {
        intro.start(0);
        let mut now = 0;
        while now < LENGTH_NS + FRAME_NS {
            std::hint::black_box(intro.snapshot(now));
            std::hint::black_box(intro.music_due(now));
            now += FRAME_NS;
        }
        intro.skip(now);
    });
}

#[test]
fn view_pushes_without_allocating_once_warm() {
    let view = IntroView::new(Theme::default());
    let white = AtlasRegion {
        texture: TextureId(0),
        origin: Origin::new(0, 0),
        size: Extent::new(1, 1),
        uv: UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
    };
    let art = LogoArt::all(white);
    let mut shapes = ShapeBatch::with_capacity(16);
    let mut intro = Intro::new();
    intro.start(0);
    let mut most = 0;
    let mut play = || {
        let mut now = 0;
        while now < LENGTH_NS {
            shapes.clear();
            if let Some(snapshot) = intro.snapshot(now) {
                view.push(
                    &mut shapes,
                    1280.0,
                    720.0,
                    &snapshot,
                    white,
                    Some(&art),
                    None,
                );
            }
            most = most.max(shapes.len());
            now += FRAME_NS;
        }
    };
    // Warm: the first run grows the batch to the explosion's size.
    play();
    assert_no_alloc(play);
    // Black, glow, beams, the logo with its glitch copies, waves, burst and flash.
    assert!(most > 30, "{most}");
}
