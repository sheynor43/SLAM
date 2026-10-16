use std::path::Path;

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
        _ => panic!("song select is not on top"),
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

/// A shell in the song select at rest over two published sets.
fn in_song_select() -> (
    ShellState,
    ShellSnapshot,
    rtrb::Producer<crate::songs::Published>,
) {
    let (mut producer, link) = crate::songs::link();
    let mut s = state().with_songs(link);
    let mut snap = ShellSnapshot::default();
    let sets = songs(&[("a", &["Easy", "Hard"]), ("b", &["Insane"])]);
    producer
        .push(crate::songs::publish(
            &sets,
            SongSelectStatus::Scanning { sets: 2 },
            Arc::from(""),
        ))
        .unwrap();
    s.tick(0, &[key(KEY_ENTER)], &mut snap);
    s.tick(TRANSITION_NS, &[], &mut snap);
    (s, snap, producer)
}

#[test]
fn enter_asks_for_the_selected_difficulty_once() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(TRANSITION_NS + 1, &[key(KEY_DOWN)], &mut snap);
    assert_eq!(s.take_play_request(), None);
    s.tick(TRANSITION_NS + 2, &[key(KEY_ENTER)], &mut snap);
    assert!(s.is_loading());
    assert_eq!(s.song_select().status(), SongSelectStatus::Loading);
    assert_eq!(s.take_play_request(), Some((0, 1)));
    assert_eq!(s.take_play_request(), None);
    assert_eq!(s.file(0, 1), Some(&PathBuf::from("a").join("Hard.osu")));
}

#[test]
fn a_load_shows_over_new_lists_and_back_cancels_it_in_place() {
    let (mut s, mut snap, mut producer) = in_song_select();
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    assert!(s.is_loading());
    let sets = songs(&[("a", &["Easy", "Hard"]), ("b", &["Insane"])]);
    producer
        .push(crate::songs::publish(
            &sets,
            SongSelectStatus::Ready,
            Arc::from(""),
        ))
        .unwrap();
    s.tick(TRANSITION_NS + 2, &[], &mut snap);
    assert_eq!(s.song_select().status(), SongSelectStatus::Loading);

    s.tick(TRANSITION_NS + 3, &[key(KEY_ESCAPE)], &mut snap);
    assert!(!s.is_loading());
    assert_eq!(s.take_play_request(), None);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    // The library's status is back.
    assert_eq!(s.song_select().status(), SongSelectStatus::Ready);
}

#[test]
fn a_failed_load_returns_to_the_list() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    s.finish_loading(TRANSITION_NS + 2, None);
    assert!(!s.is_loading());
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    assert_eq!(
        s.song_select().status(),
        SongSelectStatus::Scanning { sets: 2 }
    );
}

#[test]
fn the_play_screen_comes_in_ignores_screen_input_and_leaves() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    let start = 2 * TRANSITION_NS;
    s.finish_loading(start, Some(1));
    assert_eq!(*s.stack().top(), AppScreen::Play(1));
    s.tick(start + TRANSITION_NS, &[], &mut snap);
    assert!(matches!(
        snap.layers[0].as_ref().unwrap().view,
        ScreenView::Play(1)
    ));
    // Escape belongs to the play (it pauses there), not to the stack.
    s.tick(start + TRANSITION_NS + 1, &[key(KEY_ESCAPE)], &mut snap);
    assert_eq!(*s.stack().top(), AppScreen::Play(1));

    s.leave_play(start + 2 * TRANSITION_NS);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    // The play going out leaves the layers at once: its snapshot is stale.
    s.tick(start + 2 * TRANSITION_NS + 1, &[], &mut snap);
    let views: Vec<_> = snap.layers.iter().flatten().map(|l| &l.view).collect();
    assert_eq!(views.len(), 1);
    assert!(matches!(views[0], ScreenView::SongSelect(_)));
    // Leaving twice does not leave the song select.
    s.leave_play(start + 2 * TRANSITION_NS);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    assert!(!s.is_loading());
}

#[test]
fn each_play_comes_in_with_its_own_id() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    s.finish_loading(TRANSITION_NS + 2, Some(1));
    s.leave_play(TRANSITION_NS + 3);
    // The next play comes in while the first would still be sliding out.
    s.tick(TRANSITION_NS + 4, &[key(KEY_ENTER)], &mut snap);
    assert_eq!(s.take_play_request(), Some((0, 0)));
    s.finish_loading(TRANSITION_NS + 5, Some(2));
    s.tick(TRANSITION_NS + 6, &[], &mut snap);
    let plays: Vec<u64> = snap
        .layers
        .iter()
        .flatten()
        .filter_map(|l| match l.view {
            ScreenView::Play(id) => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(plays, [2]);
}

#[test]
fn the_pointer_is_followed_under_the_play() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    s.finish_loading(TRANSITION_NS + 2, Some(1));
    s.follow_pointer(&[mouse(InputKind::MouseMove { x: 12.0, y: 34.0 })]);
    assert_eq!(s.cursor, [12.0, 34.0]);
}

/// A play host without a window or output: the mixer and decoder run unheard.
struct HostFixture {
    host: PlayHost,
    scenes: rtrb::Consumer<(u64, PlayScene)>,
    _scene_trash: crate::loader::SceneTrash,
    _mixer: slam_audio::Mixer,
    _loader: crate::loader::LoaderThread,
    dir: PathBuf,
}

impl Drop for HostFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A set whose map plays `count.wav` of the default skin when `with_track`.
fn host(name: &str, with_track: bool) -> HostFixture {
    let dir = std::env::temp_dir().join(format!("slam-shell-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let data = Path::new(env!("CARGO_MANIFEST_DIR"));
    let map =
        std::fs::read_to_string(data.join("../slam-osu/tests/data/slider-samples.osu")).unwrap();
    let map = if with_track {
        std::fs::copy(
            data.join("../../assets/default-skin/count.wav"),
            dir.join("audio.wav"),
        )
        .unwrap();
        map.replacen("[General]\n", "[General]\nAudioFilename: audio.wav\n", 1)
    } else {
        map
    };
    std::fs::write(dir.join("map.osu"), map).unwrap();
    let (loader, scene_trash, thread) = crate::loader::spawn(LoaderConfig {
        settings: PlaySettings::default(),
        limits: slam_render::Limits {
            max_texture_size: 4096,
            max_uniform_block_size: 16384,
            max_uniform_buffer_slots: 12,
            max_texture_slots: 16,
        },
        registry: Arc::new(FontRegistry::with_builtin()),
        skip_label: "Skip".to_owned(),
    })
    .unwrap();
    let (mut mixer, mixer_handle) = slam_audio::mixer(&MixerConfig {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        ..MixerConfig::default()
    })
    .unwrap();
    let (music, player) = slam_audio::music(&MusicConfig {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        ..MusicConfig::default()
    })
    .unwrap();
    mixer.set_music(music).unwrap();
    let (scene_tx, scenes) = rtrb::RingBuffer::new(2);
    let host = PlayHost {
        audio: Audio {
            output: Arc::new(AudioPosition::new()),
            music: Arc::clone(player.position()),
            player,
            mixer: mixer_handle,
        },
        loader,
        scenes: scene_tx,
        os_cursor: CursorVisibility::default(),
        pending: Pending::Idle,
        game: None,
        retiring: None,
        next_request: 0,
        points: [1280.0, 720.0],
    };
    HostFixture {
        host,
        scenes,
        _scene_trash: scene_trash,
        _mixer: mixer,
        _loader: thread,
        dir,
    }
}

/// A shell in the song select listing the fixture's one map.
fn listing(fixture: &HostFixture) -> (ShellState, ShellSnapshot) {
    let (mut producer, link) = crate::songs::link();
    let mut s = state().with_songs(link);
    let mut snap = ShellSnapshot::default();
    let mut sets = songs(&[("set", &["Normal"])]);
    sets[0].difficulties[0].path = fixture.dir.join("map.osu");
    producer
        .push(crate::songs::publish(
            &sets,
            SongSelectStatus::Ready,
            Arc::from(""),
        ))
        .unwrap();
    s.tick(0, &[key(KEY_ENTER)], &mut snap);
    s.tick(TRANSITION_NS, &[], &mut snap);
    (s, snap)
}

/// Advances `host` until `done` or 20 s pass.
fn advance_until(
    fixture: &mut HostFixture,
    s: &mut ShellState,
    mut now: u64,
    done: impl Fn(&PlayHost, &ShellState) -> bool,
) -> u64 {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !done(&fixture.host, s) {
        assert!(
            std::time::Instant::now() < deadline,
            "the host did not get there"
        );
        now += 1_000_000;
        fixture.host.advance(now, s);
        std::thread::sleep(Duration::from_millis(1));
    }
    now
}

#[test]
fn a_play_starts_with_its_scene_and_ends_back_in_the_list() {
    let mut fixture = host("start", true);
    let (mut s, mut snap) = listing(&fixture);
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    assert!(s.is_loading());
    let now = advance_until(&mut fixture, &mut s, TRANSITION_NS + 1, |_, s| s.playing());
    assert_eq!(*s.stack().top(), AppScreen::Play(1));
    assert!(!s.is_loading());
    let (id, scene) = fixture.scenes.pop().unwrap();
    assert_eq!(id, 1);
    assert!(!scene.started());
    assert!(fixture.host.game.is_some());

    // The player quits: sounds stop and the screen goes at once.
    fixture.host.game.as_mut().unwrap().ending = Ending::Sounds;
    fixture.host.advance(now + 1, &mut s);
    assert!(fixture.host.game.is_none());
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    // The session waits for the mixer to be idle long enough, then goes to the loader.
    assert!(fixture.host.retiring.is_some());
    fixture.host.advance(now + 2, &mut s);
    assert!(fixture.host.retiring.is_some());
    fixture.host.advance(now + 2 + VOICES_IDLE_NS, &mut s);
    assert!(fixture.host.retiring.is_none());
    assert!(matches!(fixture.host.pending, Pending::Idle));
}

#[test]
fn a_track_that_does_not_open_returns_to_the_list() {
    let mut fixture = host("no-track", false);
    let (mut s, mut snap) = listing(&fixture);
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    advance_until(&mut fixture, &mut s, TRANSITION_NS + 1, |_, s| {
        !s.is_loading()
    });
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    assert!(fixture.host.game.is_none());
    assert!(matches!(fixture.host.pending, Pending::Idle));
    assert!(fixture.scenes.pop().is_err());
}

#[test]
fn back_while_preparing_drops_the_play() {
    let mut fixture = host("cancel", true);
    let (mut s, mut snap) = listing(&fixture);
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    fixture.host.advance(TRANSITION_NS + 2, &mut s);
    assert!(matches!(fixture.host.pending, Pending::Preparing(1)));
    s.tick(TRANSITION_NS + 3, &[key(KEY_ESCAPE)], &mut snap);
    assert!(!s.is_loading());
    // The preparation comes back and is freed; nothing starts.
    advance_until(&mut fixture, &mut s, TRANSITION_NS + 3, |h, _| {
        matches!(h.pending, Pending::Idle)
    });
    for t in 0..50 {
        fixture
            .host
            .advance(TRANSITION_NS + 100_000_000 + t, &mut s);
    }
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    assert!(fixture.host.game.is_none());
    assert!(fixture.scenes.pop().is_err());
}
