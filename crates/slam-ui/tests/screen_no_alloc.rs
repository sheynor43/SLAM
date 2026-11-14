//! The screen stack's steady state does not touch the allocator: ticks and layers without
//! navigation, through and after a transition.

use slam_testkit::assert_no_alloc;
use slam_ui::screen::{Nav, ScreenStack, TRANSITION_NS};

slam_testkit::install_counting_allocator!();

#[test]
fn ticks_and_layers_do_not_allocate() {
    let mut stack = ScreenStack::new(String::from("menu"));
    stack.navigate(Nav::Push(String::from("select")), 0);
    let mut visible = 0;
    assert_no_alloc(|| {
        // Before the transition ends: nothing to drop yet.
        for now in (0..TRANSITION_NS).step_by(1_000_000) {
            stack.tick(now);
            visible += stack.layers(now).into_iter().flatten().count();
            std::hint::black_box(stack.top_mut());
        }
    });
    assert_eq!(visible, 2 * 200);
    // A push leaves nothing to drop at its end either.
    assert_no_alloc(|| {
        for now in (TRANSITION_NS..2 * TRANSITION_NS).step_by(1_000_000) {
            stack.tick(now);
            visible += stack.layers(now).into_iter().flatten().count();
        }
    });
}

#[test]
fn every_transition_overlay_is_emitted_without_allocating() {
    use slam_render::Extent;
    use slam_render::atlas::{AtlasRegion, UvRect};
    use slam_render::hal::Origin;
    use slam_render::textures::TextureId;
    use slam_ui::screen::{TransitionFx, TransitionStyle};
    use slam_ui::theme::Theme;

    let white = AtlasRegion {
        texture: TextureId(0),
        origin: Origin { x: 0, y: 0 },
        size: Extent::new(8, 8),
        uv: UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
    };
    let theme = Theme::default();
    let mut count = 0usize;
    assert_no_alloc(|| {
        for style in [
            TransitionStyle::Band,
            TransitionStyle::Slices,
            TransitionStyle::Iris,
            TransitionStyle::Shards,
            TransitionStyle::Shutter,
        ] {
            for step in 0..=20 {
                let fx = TransitionFx {
                    style,
                    forward: step % 2 == 0,
                    linear: step as f32 / 20.0,
                    origin: None,
                };
                fx.emit(&theme, white, 1280.0, 720.0, |_| count += 1);
            }
        }
    });
    assert!(count > 0);
}
