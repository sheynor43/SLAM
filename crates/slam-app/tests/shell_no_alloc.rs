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

#[test]
fn song_select_ticks_and_new_lists_do_not_allocate() {
    use std::path::PathBuf;
    use std::sync::Arc;

    use slam_app::songs::{link, publish};
    use slam_library::scan::{BeatmapSetEntry, DifficultyEntry};
    use slam_ui::song_select::SongSelectStatus;

    let sets: Vec<BeatmapSetEntry> = (0..50)
        .map(|i| BeatmapSetEntry {
            dir: PathBuf::from(format!("set {i}")),
            artist: format!("artist {i}"),
            title: format!("title {i}"),
            creator: "mapper".to_owned(),
            difficulties: (0..4)
                .map(|d| DifficultyEntry {
                    path: PathBuf::from(format!("set {i}/{d}.osu")),
                    version: format!("diff {d}"),
                    overall_difficulty: 5.0,
                    approach_rate: 5.0,
                    circle_size: 4.0,
                    hp: 5.0,
                })
                .collect(),
        })
        .collect();
    let (mut producer, songs) = link();
    let mut state = ShellState::new(Theme::default(), 1280.0, 720.0, 1.0).with_songs(songs);
    let mut snapshot = ShellSnapshot::default();
    let event = |kind| InputEvent { time_ns: 0, kind };
    let key = |scancode| {
        event(InputKind::Key {
            scancode,
            down: true,
        })
    };
    // Into the song select: navigation may allocate.
    state.tick(0, &[key(40)], &mut snapshot);
    state.tick(TRANSITION_NS, &[], &mut snapshot);

    // As the library thread does, the test keeps every list it publishes: the update
    // thread only drops its references.
    let first = publish(
        &sets[..10],
        SongSelectStatus::Scanning { sets: 10 },
        Arc::from(""),
    );
    let second = publish(&sets, SongSelectStatus::Ready, Arc::from(""));
    let kept = [&first, &second].map(|p| {
        (
            Arc::clone(&p.list),
            Arc::clone(&p.files),
            Arc::clone(&p.scanning_text),
        )
    });
    producer.push(first).unwrap();
    let input: Vec<InputEvent> = [81, 81, 79, 80, 78, 75, 77, 74, 82]
        .into_iter()
        .map(key)
        .chain([
            event(InputKind::MouseWheel { y: -2.0 }),
            event(InputKind::MouseMove { x: 900.0, y: 300.0 }),
            event(InputKind::MouseWheel { y: 1.0 }),
        ])
        .collect();
    assert_no_alloc(|| {
        let start = 2 * TRANSITION_NS;
        for now in start..start + 50 {
            state.tick(now, &input, &mut snapshot);
        }
    });
    producer.push(second).unwrap();
    assert_no_alloc(|| {
        let start = 3 * TRANSITION_NS;
        for now in start..start + 50 {
            state.tick(now, &input, &mut snapshot);
        }
    });
    drop(kept);
}
