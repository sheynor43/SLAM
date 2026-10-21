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
    // The mod select: F1, toggles that drop conflicts, Backspace, the Control hotkeys, F1
    // again. The selection has
    // room reserved, so toggling allocates nothing.
    let [tile_x, tile_y] =
        slam_ui::song_select::SongSelectLayout::new(1280.0, 720.0, &Theme::default())
            .mods
            .tile_rect(1, 3)
            .centre();
    let click = [
        event(InputKind::MouseMove {
            x: tile_x,
            y: tile_y,
        }),
        event(InputKind::MouseButton {
            button: slam_input::MouseButton::Left,
            down: true,
        }),
    ];
    // Control with Up and Down changes the speed (back to 1: Double Time goes), F4 and F3 the
    // approach rate (Difficulty Adjust comes; Easy then drops it).
    let ctrl_up = event(InputKind::Key {
        scancode: 224,
        down: false,
    });
    let mods: Vec<InputEvent> = [
        58, 20, 4, 9, 20, 10, 26, 42, 224, 82, 82, 81, 81, 82, 61, 61, 60,
    ]
    .into_iter()
    .map(key)
    .chain([ctrl_up])
    .chain([20, 29].map(key))
    .chain(click)
    .chain([key(58)])
    .collect();
    state.tick(3 * TRANSITION_NS + 99, &mods, &mut snapshot);
    assert_no_alloc(|| {
        let start = 3 * TRANSITION_NS + 100;
        for now in start..start + 50 {
            state.tick(now, &mods, &mut snapshot);
        }
    });
    // The panel is closed again (a click on a tile toggles; F1 opens next round).
    state.tick(3 * TRANSITION_NS + 200, &[key(42)], &mut snapshot);
    // A load comes and the play screen goes on top: navigation may allocate, its ticks not.
    state.tick(4 * TRANSITION_NS, &[key(40)], &mut snapshot);
    assert_eq!(state.take_play_request(), Some((0, 0)));
    state.finish_loading(4 * TRANSITION_NS, Some(1));
    assert_no_alloc(|| {
        let start = 4 * TRANSITION_NS + 1;
        for now in (start..start + 3 * TRANSITION_NS).step_by(1_000_000) {
            state.tick(now, &input, &mut snapshot);
        }
    });
    drop(kept);
}

fn results_data(id: u64) -> slam_ui::results::ResultsData {
    use slam_ui::results::{Rank, ResultsData, ResultsMods, ResultsStat, StatKind};
    ResultsData {
        id,
        title: "artist - title [diff]".to_owned(),
        score: 1_000_000,
        accuracy: 0.97,
        rank: Rank::S,
        stats: [ResultsStat::default(); StatKind::COUNT],
        combo: 10,
        max_combo: 12,
        mods: ResultsMods::default(),
    }
}

/// A shell on the results of a play of the first difficulty of a one-set list, at rest;
/// returns the time it is at.
fn on_results() -> (
    ShellState,
    ShellSnapshot,
    rtrb::Producer<slam_app::songs::Published>,
    u64,
) {
    use std::path::PathBuf;
    use std::sync::Arc;

    use slam_app::songs::{link, publish};
    use slam_library::scan::{BeatmapSetEntry, DifficultyEntry};
    use slam_ui::song_select::SongSelectStatus;

    let sets = [BeatmapSetEntry {
        dir: PathBuf::from("set"),
        artist: "artist".to_owned(),
        title: "title".to_owned(),
        creator: "mapper".to_owned(),
        difficulties: vec![DifficultyEntry {
            path: PathBuf::from("set/0.osu"),
            version: "diff".to_owned(),
            overall_difficulty: 5.0,
            approach_rate: 5.0,
            circle_size: 4.0,
            hp: 5.0,
        }],
    }];
    let (mut producer, songs) = link();
    let mut state = ShellState::new(Theme::default(), 1280.0, 720.0, 1.0).with_songs(songs);
    let mut snapshot = ShellSnapshot::default();
    let key = |scancode| InputEvent {
        time_ns: 0,
        kind: InputKind::Key {
            scancode,
            down: true,
        },
    };
    producer
        .push(publish(&sets, SongSelectStatus::Ready, Arc::from("")))
        .unwrap();
    state.tick(0, &[key(40)], &mut snapshot);
    state.tick(TRANSITION_NS, &[key(40)], &mut snapshot);
    assert_eq!(state.take_play_request(), Some((0, 0)));
    let t = 2 * TRANSITION_NS;
    state.finish_loading(t, Some(1));
    state.leave_play(t + 1, Some(results_data(1)));
    // The transition and the buffers settle.
    for now in (t + 2..t + 2 + 2 * TRANSITION_NS).step_by(1_000_000) {
        state.tick(now, &[], &mut snapshot);
    }
    (state, snapshot, producer, t + 3 * TRANSITION_NS)
}

#[test]
fn results_ticks_do_not_allocate() {
    let (mut state, mut snapshot, _producer, t) = on_results();
    let moves: Vec<InputEvent> = (0..72)
        .map(|i| InputEvent {
            time_ns: 0,
            kind: InputKind::MouseMove {
                x: 640.0,
                y: i as f32 * 10.0,
            },
        })
        .collect();
    assert_no_alloc(|| {
        for now in t..t + 100 {
            state.tick(now, &moves, &mut snapshot);
        }
    });
}

/// After leaving the results by `key`, the ticks that follow the transition free nothing.
fn leaving_results_frees_nothing_on_a_tick(scancode: u16) {
    let (mut state, mut snapshot, _producer, t) = on_results();
    let press = InputEvent {
        time_ns: 0,
        kind: InputKind::Key {
            scancode,
            down: true,
        },
    };
    // Navigation may allocate and free.
    state.tick(t, &[press], &mut snapshot);
    if scancode == 21 {
        assert!(state.take_retry_request());
        state.finish_loading(t, Some(2));
    }
    assert_no_alloc(|| {
        for now in (t + 1..t + 2 * TRANSITION_NS).step_by(1_000_000) {
            state.tick(now, &[], &mut snapshot);
        }
        for now in t + 2 * TRANSITION_NS..t + 2 * TRANSITION_NS + 5 {
            state.tick(now, &[], &mut snapshot);
        }
    });
}

#[test]
fn back_from_the_results_frees_nothing_on_a_tick() {
    leaving_results_frees_nothing_on_a_tick(41);
}

#[test]
fn retry_from_the_results_frees_nothing_on_a_tick() {
    leaving_results_frees_nothing_on_a_tick(21);
}
