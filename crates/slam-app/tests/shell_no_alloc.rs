//! A shell tick without navigation does not allocate, through a transition and at rest.

use slam_app::shell::{ShellSnapshot, ShellState};
use slam_input::{InputEvent, InputKind};
use slam_testkit::assert_no_alloc;
use slam_ui::screen::TRANSITION_NS;
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

#[test]
fn ticks_without_navigation_do_not_allocate() {
    let mut state = ShellState::new(Theme::default(), 1280.0, 720.0, 1.0);
    let mut snapshot = ShellSnapshot::default();
    let key = |scancode| InputEvent {
        time_ns: 0,
        kind: InputKind::Key {
            scancode,
            down: true,
        },
    };
    // The menu at rest: hover sweeps over every button, arrows move the selection.
    let sweep: Vec<InputEvent> = (0..72)
        .map(|i| InputEvent {
            time_ns: 0,
            kind: InputKind::MouseMove {
                x: 640.0,
                y: i as f32 * 10.0,
            },
        })
        .chain([key(81), key(81), key(82)])
        .collect();
    assert_no_alloc(|| {
        for now in 0..100 {
            state.tick(now, &sweep, &mut snapshot);
        }
    });
    let enter = InputEvent {
        time_ns: 0,
        kind: InputKind::Key {
            scancode: 40,
            down: true,
        },
    };
    // Navigation may allocate.
    state.tick(0, &[enter], &mut snapshot);
    let moves = [InputEvent {
        time_ns: 0,
        kind: InputKind::MouseMove { x: 5.0, y: 5.0 },
    }];
    assert_no_alloc(|| {
        for now in (1..TRANSITION_NS).step_by(1_000_000) {
            state.tick(now, &moves, &mut snapshot);
        }
        // Past the end of a push nothing is dropped.
        for now in (TRANSITION_NS..2 * TRANSITION_NS).step_by(1_000_000) {
            state.tick(now, &moves, &mut snapshot);
        }
    });
}
