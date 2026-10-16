use slam_ui::screen::TRANSITION_NS;

use super::*;

fn state() -> ShellState {
    ShellState::new(Theme::default(), 1280.0, 720.0, 1.0)
}

fn key(scancode: u16) -> InputEvent {
    InputEvent {
        time_ns: 0,
        kind: InputKind::Key {
            scancode,
            down: true,
        },
    }
}

fn mouse(kind: InputKind) -> InputEvent {
    InputEvent { time_ns: 0, kind }
}

fn layers(snapshot: &ShellSnapshot) -> usize {
    snapshot.layers.iter().flatten().count()
}

#[test]
fn enter_on_play_pushes_song_select_and_escape_returns() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    assert_eq!(s.tick(0, &[], &mut snap), Navigated::Stay);
    assert_eq!(layers(&snap), 1);

    assert_eq!(s.tick(10, &[key(KEY_ENTER)], &mut snap), Navigated::Stay);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    // Mid-transition: both screens, the new one on top.
    assert_eq!(layers(&snap), 2);
    assert!(matches!(
        snap.layers[0].as_ref().unwrap().view,
        ScreenView::Menu(_)
    ));
    assert!(matches!(
        snap.layers[1].as_ref().unwrap().view,
        ScreenView::SongSelect(_)
    ));
    // At rest: only the song select.
    s.tick(10 + TRANSITION_NS, &[], &mut snap);
    assert_eq!(layers(&snap), 1);
    assert_eq!(snap.layers[0].as_ref().unwrap().pose, Pose::SHOWN);

    let now = 2 * TRANSITION_NS;
    assert_eq!(s.tick(now, &[key(KEY_ESCAPE)], &mut snap), Navigated::Stay);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
    assert_eq!(layers(&snap), 2);
}

#[test]
fn escape_at_the_root_exits() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    assert_eq!(s.tick(0, &[key(KEY_ESCAPE)], &mut snap), Navigated::Exit);
}

#[test]
fn down_enter_exits_and_other_keys_and_releases_do_nothing() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    let release = mouse(InputKind::Key {
        scancode: KEY_ESCAPE,
        down: false,
    });
    assert_eq!(s.tick(0, &[key(4), release], &mut snap), Navigated::Stay);
    assert_eq!(
        s.tick(0, &[key(KEY_DOWN), key(KEY_KEYPAD_ENTER)], &mut snap),
        Navigated::Exit
    );
}

#[test]
fn mouse_clicks_buttons_and_x1_goes_back() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    let [x, y] = s.layout.buttons[0].centre();
    let events = [
        mouse(InputKind::MouseMove { x, y }),
        mouse(InputKind::MouseButton {
            button: MouseButton::Left,
            down: true,
        }),
    ];
    s.tick(0, &events, &mut snap);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    let x1 = mouse(InputKind::MouseButton {
        button: MouseButton::X1,
        down: true,
    });
    s.tick(TRANSITION_NS, &[x1], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
}

#[test]
fn pointer_points_are_scaled_to_pixels() {
    // A 2x display: the layout is in pixels, events in points.
    let mut s = ShellState::new(Theme::default(), 2560.0, 1440.0, 2.0);
    let mut snap = ShellSnapshot::default();
    let [x, y] = s.layout.buttons[0].centre();
    let events = [
        mouse(InputKind::MouseMove {
            x: x / 2.0,
            y: y / 2.0,
        }),
        mouse(InputKind::MouseButton {
            button: MouseButton::Left,
            down: true,
        }),
    ];
    s.tick(0, &events, &mut snap);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
}

fn songs(sets: &[(&str, &[&str])]) -> Vec<slam_library::scan::BeatmapSetEntry> {
    sets.iter()
        .map(|(dir, versions)| slam_library::scan::BeatmapSetEntry {
            dir: PathBuf::from(dir),
            artist: (*dir).to_owned(),
            title: (*dir).to_owned(),
            creator: "mapper".to_owned(),
            difficulties: versions
                .iter()
                .map(|v| slam_library::scan::DifficultyEntry {
                    path: PathBuf::from(dir).join(format!("{v}.osu")),
                    version: (*v).to_owned(),
                    overall_difficulty: 5.0,
                    approach_rate: 5.0,
                    circle_size: 4.0,
                    hp: 5.0,
                })
                .collect(),
        })
        .collect()
}

fn song_view(snap: &ShellSnapshot) -> &SongSelectSnapshot {
    match &snap.layers[0].as_ref().unwrap().view {
        ScreenView::SongSelect(view) => view,
        ScreenView::Menu(_) => panic!("song select is not on top"),
    }
}

#[test]
fn without_a_folder_the_song_select_says_so() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[key(KEY_ENTER)], &mut snap);
    s.tick(TRANSITION_NS, &[], &mut snap);
    assert_eq!(song_view(&snap).status, SongSelectStatus::NoFolder);
    assert!(song_view(&snap).list.sets.is_empty());
}

#[test]
fn published_lists_reach_the_song_select_and_keys_move_through_them() {
    let (mut producer, link) = crate::songs::link();
    let mut s = state().with_songs(link);
    let mut snap = ShellSnapshot::default();
    assert_eq!(
        s.song_select().status(),
        SongSelectStatus::Scanning { sets: 0 }
    );
    let sets = songs(&[("a", &["Easy", "Hard"]), ("b", &["Insane"])]);
    producer
        .push(crate::songs::publish(
            &sets[..1],
            SongSelectStatus::Scanning { sets: 1 },
            Arc::from(""),
        ))
        .unwrap();
    // Only the newest of several waiting lists is taken.
    producer
        .push(crate::songs::publish(
            &sets,
            SongSelectStatus::Ready,
            Arc::from(""),
        ))
        .unwrap();
    s.tick(0, &[key(KEY_ENTER)], &mut snap);
    s.tick(TRANSITION_NS, &[], &mut snap);
    let view = song_view(&snap);
    assert_eq!(view.status, SongSelectStatus::Ready);
    assert_eq!(view.list.sets.len(), 2);

    let keys = [key(KEY_DOWN), key(KEY_DOWN)];
    s.tick(TRANSITION_NS + 1, &keys, &mut snap);
    assert_eq!(s.song_select().selected(), Some((1, 0)));
    assert_eq!(s.file(1, 0), Some(&PathBuf::from("b").join("Insane.osu")));
    s.tick(TRANSITION_NS + 2, &[key(KEY_LEFT)], &mut snap);
    assert_eq!(s.song_select().selected(), Some((0, 0)));
    s.tick(TRANSITION_NS + 3, &[key(KEY_END)], &mut snap);
    assert_eq!(s.song_select().selected(), Some((1, 0)));
    // The wheel scrolls without moving the selection.
    let wheel = mouse(InputKind::MouseWheel { y: -3.0 });
    s.tick(TRANSITION_NS + 4, &[wheel], &mut snap);
    assert_eq!(s.song_select().selected(), Some((1, 0)));

    // Back to the menu and in again: the list and selection stay.
    s.tick(TRANSITION_NS + 5, &[key(KEY_ESCAPE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
    s.tick(3 * TRANSITION_NS, &[key(KEY_ENTER)], &mut snap);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    assert_eq!(s.song_select().selected(), Some((1, 0)));
}

#[test]
fn menu_ignores_song_select_keys() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    let keys = [key(KEY_LEFT), key(KEY_PAGE_DOWN), key(KEY_END)];
    assert_eq!(s.tick(0, &keys, &mut snap), Navigated::Stay);
    assert!(matches!(s.stack().top(), AppScreen::Menu(menu) if *menu == MainMenu::new()));
}

#[test]
fn songs_flag() {
    let parse = |args: &[&str]| parse_songs(args.iter().map(|s| s.to_string()));
    assert_eq!(parse(&[]), Ok(None));
    assert_eq!(parse(&["--songs", "S"]), Ok(Some(PathBuf::from("S"))));
    assert!(parse(&["--songs"]).is_err());
    assert!(parse(&["--what"]).is_err());
}
