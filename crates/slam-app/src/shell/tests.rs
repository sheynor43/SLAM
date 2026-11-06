use std::path::Path;

use slam_osu::mods::GameplayMod as _;
use slam_osu::replay::Actions;
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
        s.tick(
            0,
            &[key(KEY_DOWN), key(KEY_DOWN), key(KEY_KEYPAD_ENTER)],
            &mut snap
        ),
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
            artist_unicode: String::new(),
            title_unicode: String::new(),
            source: String::new(),
            tags: String::new(),
            difficulties: versions
                .iter()
                .map(|v| slam_library::scan::DifficultyEntry {
                    path: PathBuf::from(dir).join(format!("{v}.osu")),
                    version: (*v).to_owned(),
                    overall_difficulty: 5.0,
                    approach_rate: 5.0,
                    circle_size: 4.0,
                    hp: 5.0,
                    background: None,
                    audio: String::new(),
                    preview_time: -1,
                    metadata: Default::default(),
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
pub(super) fn in_song_select() -> (
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

    s.leave_play(start + 2 * TRANSITION_NS, None);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    // The play going out leaves the layers at once: its snapshot is stale.
    s.tick(start + 2 * TRANSITION_NS + 1, &[], &mut snap);
    let views: Vec<_> = snap.layers.iter().flatten().map(|l| &l.view).collect();
    assert_eq!(views.len(), 1);
    assert!(matches!(views[0], ScreenView::SongSelect(_)));
    // Leaving twice does not leave the song select.
    s.leave_play(start + 2 * TRANSITION_NS, None);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    assert!(!s.is_loading());
}

#[test]
fn each_play_comes_in_with_its_own_id() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    s.finish_loading(TRANSITION_NS + 2, Some(1));
    s.leave_play(TRANSITION_NS + 3, None);
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

fn results_data(id: u64) -> ResultsData {
    ResultsData {
        id,
        title: "artist - title [Hard]".to_owned(),
        score: 1_234_567,
        accuracy: 0.97,
        rank: slam_ui::results::Rank::S,
        stats: [slam_ui::results::ResultsStat::default(); StatKind::COUNT],
        combo: 10,
        max_combo: 12,
        mods: ResultsMods::default(),
        average_hit_error: None,
        average_hit_error_text: None,
        unstable_rate: None,
    }
}

/// A shell on the results of play 1, over the song select, with the song select's
/// difficulty `Hard` as the last play.
fn in_results() -> (
    ShellState,
    ShellSnapshot,
    rtrb::Producer<crate::songs::Published>,
    u64,
) {
    let (mut s, mut snap, producer) = in_song_select();
    s.tick(TRANSITION_NS + 1, &[key(KEY_DOWN)], &mut snap);
    s.tick(TRANSITION_NS + 2, &[key(KEY_ENTER)], &mut snap);
    assert_eq!(s.take_play_request(), Some((0, 1)));
    let start = 2 * TRANSITION_NS;
    s.finish_loading(start, Some(1));
    let end = start + 2 * TRANSITION_NS;
    s.leave_play(end, Some(results_data(1)));
    s.tick(end + TRANSITION_NS, &[], &mut snap);
    (s, snap, producer, end + TRANSITION_NS)
}

#[test]
fn a_passed_play_goes_to_the_results_and_a_failed_one_pops() {
    assert!(matches!(
        after_play(Some(results_data(1))),
        Nav::Replace(AppScreen::Results(_))
    ));
    assert!(matches!(after_play(None), Nav::Pop));
}

#[test]
fn results_replace_the_play_and_back_returns_to_the_song_select() {
    let (mut s, mut snap, _producer, now) = in_results();
    assert!(matches!(s.stack().top(), AppScreen::Results(_)));
    // Menu, song select, results: the play was replaced, not kept below.
    assert_eq!(s.stack().depth(), 3);
    let views: Vec<_> = snap.layers.iter().flatten().map(|l| &l.view).collect();
    assert_eq!(views.len(), 1);
    assert!(matches!(views[0], ScreenView::Results(r) if r.data.id == 1));
    assert_eq!(s.last_title(), "a - a [Hard]");

    // The track plays on under the results and stops on the way back.
    assert!(!s.take_track_stop());
    s.tick(now + 1, &[key(KEY_ESCAPE)], &mut snap);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    assert!(!s.is_loading());
    assert!(!s.take_retry_request());
    assert!(s.take_track_stop());
    assert!(!s.take_track_stop());
}

#[test]
fn retry_plays_the_same_beatmap_in_place_of_the_results() {
    let (mut s, mut snap, _producer, now) = in_results();
    s.tick(now + 1, &[key(KEY_R)], &mut snap);
    assert!(s.is_loading());
    assert!(s.take_retry_request());
    assert!(!s.take_retry_request());
    assert_eq!(s.last_file(), Some(&PathBuf::from("a").join("Hard.osu")));
    // Keys while loading do not ask again.
    s.tick(now + 2, &[key(KEY_R)], &mut snap);
    assert!(!s.take_retry_request());

    s.finish_loading(now + 3, Some(2));
    assert_eq!(*s.stack().top(), AppScreen::Play(2));
    assert_eq!(s.stack().depth(), 3);
    // Quitting that play lands on the song select, not on the old results.
    s.leave_play(now + 4, None);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    assert_eq!(s.stack().depth(), 2);
}

#[test]
fn back_while_the_retry_loads_cancels_it_and_stays_on_the_results() {
    let (mut s, mut snap, _producer, now) = in_results();
    s.tick(now + 1, &[key(KEY_R)], &mut snap);
    s.tick(now + 2, &[key(KEY_ESCAPE)], &mut snap);
    assert!(!s.is_loading());
    assert!(!s.take_retry_request());
    assert!(matches!(s.stack().top(), AppScreen::Results(_)));
    // Still on the results: the track plays on.
    assert!(!s.take_track_stop());
    // A failed load returns to the results as well.
    s.tick(now + 3, &[key(KEY_R)], &mut snap);
    s.finish_loading(now + 4, None);
    assert!(!s.is_loading());
    assert!(matches!(s.stack().top(), AppScreen::Results(_)));
}

#[test]
fn the_results_buttons_are_clickable() {
    let (mut s, mut snap, _producer, now) = in_results();
    let click = mouse(InputKind::MouseButton {
        button: MouseButton::Left,
        down: true,
    });
    let [x, y] = s.results_layout.buttons[0].centre();
    s.tick(now + 1, &[mouse(InputKind::MouseMove { x, y })], &mut snap);
    let hovered = snap.layers.iter().flatten().find_map(|l| match &l.view {
        ScreenView::Results(r) => r.hovered,
        _ => None,
    });
    assert_eq!(hovered, Some(slam_ui::results::ResultsButton::Retry));
    s.tick(now + 2, &[click], &mut snap);
    assert!(s.is_loading());
    assert!(s.take_retry_request());
    s.tick(now + 3, &[key(KEY_ESCAPE)], &mut snap);

    let [x, y] = s.results_layout.buttons[1].centre();
    s.tick(
        now + 4,
        &[mouse(InputKind::MouseMove { x, y }), click],
        &mut snap,
    );
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
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

/// An output's update-side handles and its mixer, without a device: tests run the
/// callback themselves.
fn detached_audio() -> (Audio, slam_audio::Mixer) {
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
    let previews = [0, 1].map(|_| {
        let (music, preview) = slam_audio::music(&MusicConfig {
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            ..MusicConfig::default()
        })
        .unwrap();
        mixer.add_music(music).unwrap();
        preview
    });
    let audio = Audio {
        output: Arc::new(AudioPosition::new()),
        music: Arc::clone(player.position()),
        player,
        previews,
        mixer: mixer_handle,
        volumes: AppliedVolumes::new(Volumes::of(&Settings::default().audio)),
    };
    (audio, mixer)
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
        // Any line ending: the data may be checked out with CRLF.
        let map = map.replacen("[General]", "[General]\nAudioFilename: audio.wav", 1);
        assert!(map.contains("AudioFilename: audio.wav"));
        map
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
        localizer: Arc::new(Localizer::builtin().unwrap()),
        replays: Some(dir.join("replays")),
    })
    .unwrap();
    let (audio, mixer) = detached_audio();
    let (scene_tx, scenes) = rtrb::RingBuffer::new(2);
    let host = PlayHost {
        audio,
        output: None,
        requested: Settings::default().audio,
        switching: false,
        no_output: false,
        retired: None,
        track: None,
        menu_music: MenuMusic::default(),
        selection: (None, 0),
        rng: 1,
        loader,
        scenes: scene_tx,
        os_cursor: CursorVisibility::default(),
        win_key: WinKeyBlock::default(),
        menu_shown: false,
        pending: Pending::Idle,
        game: None,
        retiring: None,
        track_reset: false,
        next_request: 0,
        play_requested: None,
        reskin: Reskin::Idle,
        points: [1280.0, 720.0],
        input: PlayInput::new(&Settings::default()),
        beatmap_offsets: HashMap::new(),
        localizer: Arc::new(Localizer::builtin().unwrap()),
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
    let (id, _scene) = fixture.scenes.pop().unwrap();
    assert_eq!(id, 1);
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
fn a_play_that_ends_resets_the_track_once_its_queue_takes_it() {
    let mut fixture = host("track-reset", true);
    let (mut s, mut snap) = listing(&fixture);
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    let now = advance_until(&mut fixture, &mut s, TRANSITION_NS + 1, |_, s| s.playing());
    // While a play owns the track, its session gives it its sound.
    fixture.host.track_reset = true;
    fixture.host.advance(now + 1, &mut s);
    assert!(fixture.host.track_reset);
    fixture.host.track_reset = false;

    // No callback runs: fill the music queue, then quit.
    while fixture.host.audio.player.pause().is_ok() {}
    fixture.host.game.as_mut().unwrap().ending = Ending::Sounds;
    fixture.host.advance(now + 2, &mut s);
    assert!(fixture.host.game.is_none());
    assert!(
        fixture.host.track_reset,
        "the queue is full: tried again later"
    );
    fixture.host.advance(now + 3, &mut s);
    assert!(fixture.host.track_reset);
    // The callback takes the queued commands; the next tick resets the track.
    let mut out = vec![0.0f32; usize::from(CHANNELS) * 64];
    let info = slam_audio::CallbackInfo {
        frame: 0,
        timestamp_ns: 0,
        latency_ns: 0,
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
    };
    slam_audio::AudioCallback::process(&mut fixture._mixer, &mut out, &info);
    fixture.host.advance(now + 4, &mut s);
    assert!(!fixture.host.track_reset);
}

#[test]
fn a_play_takes_its_beatmap_offset_and_keeps_it_when_it_ends() {
    let mut fixture = host("beatmap-offset", true);
    let map = fixture.dir.join("map.osu");
    fixture.host.beatmap_offsets.insert(map.clone(), -7.0);
    let (mut s, mut snap) = listing(&fixture);
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    let now = advance_until(&mut fixture, &mut s, TRANSITION_NS + 1, |_, s| s.playing());
    let game = fixture.host.game.as_mut().unwrap();
    assert_eq!(game.session.beatmap_offset(), -7.0);

    // The play's offset goes back when it ends.
    fixture.host.beatmap_offsets.clear();
    fixture.host.game.as_mut().unwrap().ending = Ending::Sounds;
    fixture.host.advance(now + 1, &mut s);
    assert!(fixture.host.game.is_none());
    assert_eq!(fixture.host.beatmap_offsets.get(&map), Some(&-7.0));
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

#[test]
fn the_play_goes_when_the_results_show_or_the_player_quits() {
    for state in [PlayState::Results, PlayState::Exited] {
        assert!(play_over(state), "{state:?}");
    }
    for state in [
        PlayState::Waiting,
        PlayState::Playing,
        PlayState::Paused,
        PlayState::Resuming,
        PlayState::Failing,
        PlayState::Failed,
        PlayState::Passed,
    ] {
        assert!(!play_over(state), "{state:?}");
    }
}

#[test]
fn the_windows_key_is_blocked_only_while_playing() {
    // As lazer's `LocalUserPlayingState.Playing`: not paused, resuming, in a break, failed
    // or passed.
    for state in [PlayState::Waiting, PlayState::Playing] {
        assert!(blocks_win_key(state, false), "{state:?}");
        assert!(!blocks_win_key(state, true), "{state:?} in a break");
    }
    for state in [
        PlayState::Paused,
        PlayState::Resuming,
        PlayState::Failing,
        PlayState::Failed,
        PlayState::Passed,
        PlayState::Results,
        PlayState::Exited,
    ] {
        assert!(!blocks_win_key(state, false), "{state:?}");
    }
}

#[test]
fn the_host_blocks_the_windows_key_while_playing() {
    let mut fixture = host("win-key", false);
    let block = WinKeyBlock::default();
    fixture.host.win_key = block.clone();
    let mut snapshot = PlaySnapshot {
        state: PlayState::Playing,
        is_break_time: false,
        ..PlaySnapshot::default()
    };
    fixture.host.follow_play(&snapshot);
    assert!(block.is_blocked());
    snapshot.state = PlayState::Paused;
    fixture.host.follow_play(&snapshot);
    assert!(!block.is_blocked());
    // Switched off in the pause: playing on does not block.
    let mut settings = Settings::default();
    settings.gameplay.disable_win_key = false;
    fixture.host.apply_settings(&settings);
    snapshot.state = PlayState::Playing;
    fixture.host.follow_play(&snapshot);
    assert!(!block.is_blocked());
    settings.gameplay.disable_win_key = true;
    fixture.host.apply_settings(&settings);
    fixture.host.follow_play(&snapshot);
    assert!(block.is_blocked());
}

#[test]
fn the_windows_key_setting_applies_at_once() {
    let mut settings = Settings::default();
    assert!(PlayInput::new(&settings).disable_win_key);
    settings.gameplay.disable_win_key = false;
    assert!(!PlayInput::new(&settings).disable_win_key);
}

#[test]
fn changed_settings_reskin_the_running_play_in_its_menu() {
    let mut fixture = host("reskin", true);
    let (mut s, mut snap) = listing(&fixture);
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    let now = advance_until(&mut fixture, &mut s, TRANSITION_NS + 1, |_, s| s.playing());
    let (id, _) = fixture.scenes.pop().expect("the play's scene");
    let loaded_with = fixture.host.game.as_ref().unwrap().options.clone().unwrap();
    // Closed with no change: nothing is asked.
    s.settings_changed = true;
    fixture.host.advance(now + 1, &mut s);
    assert!(matches!(fixture.host.reskin, Reskin::Idle));
    // A look changed: the play is prepared again, and waits for its menu.
    s.settings.gameplay.hit_lighting = !s.settings.gameplay.hit_lighting;
    s.settings_changed = true;
    let now = advance_until(&mut fixture, &mut s, now + 2, |h, _| {
        matches!(h.reskin, Reskin::Ready(..))
    });
    for t in 1..20 {
        fixture.host.advance(now + t, &mut s);
    }
    assert!(matches!(fixture.host.reskin, Reskin::Ready(..)));
    assert!(fixture.scenes.pop().is_err());
    // In the pause menu it takes the new scene, under the same id, and the new options.
    fixture.host.menu_shown = true;
    fixture.host.advance(now + 30, &mut s);
    assert!(matches!(fixture.host.reskin, Reskin::Idle));
    let (again, _) = fixture.scenes.pop().expect("the new scene");
    assert_eq!(again, id);
    let game = fixture.host.game.as_ref().unwrap();
    let options = game.options.as_ref().unwrap();
    assert_ne!(options.hit_lighting, loaded_with.hit_lighting);
    assert_eq!(options.mods, loaded_with.mods);
    // The attempt goes on: the same session.
    assert_eq!(game.id, id);
}

/// A play on its way to a reskin that is ready: returns the time and the play's id.
fn ready_reskin(fixture: &mut HostFixture, s: &mut ShellState, snap: &mut ShellSnapshot) -> u64 {
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], snap);
    let now = advance_until(fixture, s, TRANSITION_NS + 1, |_, s| s.playing());
    let _ = fixture.scenes.pop();
    s.settings.gameplay.hit_lighting = !s.settings.gameplay.hit_lighting;
    s.settings_changed = true;
    advance_until(fixture, s, now + 1, |h, _| {
        matches!(h.reskin, Reskin::Ready(..))
    })
}

#[test]
fn a_reskin_waits_for_room_for_its_scene() {
    let mut fixture = host("reskin-full", true);
    let (mut s, mut snap) = listing(&fixture);
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    let now = advance_until(&mut fixture, &mut s, TRANSITION_NS + 1, |_, s| s.playing());
    // The draw thread takes no scene in this test: the play's own and a first reskin's
    // fill the ring of two.
    fixture.host.menu_shown = true;
    s.settings.gameplay.hit_lighting = !s.settings.gameplay.hit_lighting;
    s.settings_changed = true;
    let now = advance_until(&mut fixture, &mut s, now + 1, |h, _| {
        matches!(h.reskin, Reskin::Idle) && h.scenes.slots() == 0
    });
    let options = fixture.host.game.as_ref().unwrap().options.clone();
    s.settings.gameplay.cursor_trail = !s.settings.gameplay.cursor_trail;
    s.settings_changed = true;
    let now = advance_until(&mut fixture, &mut s, now + 1, |h, _| {
        matches!(h.reskin, Reskin::Ready(..))
    });
    // No room: neither the samples nor the scene change.
    for t in 1..10 {
        fixture.host.advance(now + t, &mut s);
    }
    assert!(matches!(fixture.host.reskin, Reskin::Ready(..)));
    assert_eq!(fixture.host.game.as_ref().unwrap().options, options);
    // Taken: it goes through.
    while fixture.scenes.pop().is_ok() {}
    fixture.host.advance(now + 20, &mut s);
    assert!(matches!(fixture.host.reskin, Reskin::Idle));
    assert!(fixture.scenes.pop().is_ok());
    assert_ne!(fixture.host.game.as_ref().unwrap().options, options);
}

#[test]
fn settings_back_to_the_plays_own_cancel_the_reskin() {
    let mut fixture = host("reskin-back", true);
    let (mut s, mut snap) = listing(&fixture);
    let now = ready_reskin(&mut fixture, &mut s, &mut snap);
    s.settings.gameplay.hit_lighting = !s.settings.gameplay.hit_lighting;
    s.settings_changed = true;
    fixture.host.advance(now + 1, &mut s);
    assert!(matches!(fixture.host.reskin, Reskin::Idle));
    fixture.host.menu_shown = true;
    fixture.host.advance(now + 2, &mut s);
    assert!(fixture.scenes.pop().is_err());
}

#[test]
fn a_reskin_of_another_beatmap_file_is_dropped() {
    let mut fixture = host("reskin-hash", true);
    let (mut s, mut snap) = listing(&fixture);
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    let now = advance_until(&mut fixture, &mut s, TRANSITION_NS + 1, |_, s| s.playing());
    let _ = fixture.scenes.pop();
    // As if the file changed on disk after the play started.
    let game = fixture.host.game.as_mut().unwrap();
    let mut beatmap = (*game.beatmap).clone();
    beatmap.hash = "changed".into();
    game.beatmap = Arc::new(beatmap);
    s.settings.gameplay.hit_lighting = !s.settings.gameplay.hit_lighting;
    s.settings_changed = true;
    fixture.host.advance(now + 1, &mut s);
    assert!(matches!(fixture.host.reskin, Reskin::Preparing(..)));
    let now = advance_until(&mut fixture, &mut s, now + 2, |h, _| {
        matches!(h.reskin, Reskin::Idle)
    });
    fixture.host.menu_shown = true;
    for t in 1..50 {
        fixture.host.advance(now + t, &mut s);
    }
    assert!(matches!(fixture.host.reskin, Reskin::Idle));
    assert!(fixture.scenes.pop().is_err());
}

#[test]
fn a_reskin_for_an_ended_play_is_dropped() {
    let mut fixture = host("reskin-ended", true);
    let (mut s, mut snap) = listing(&fixture);
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    let now = advance_until(&mut fixture, &mut s, TRANSITION_NS + 1, |_, s| s.playing());
    let _ = fixture.scenes.pop();
    s.settings.gameplay.hit_lighting = !s.settings.gameplay.hit_lighting;
    s.settings_changed = true;
    let now = advance_until(&mut fixture, &mut s, now + 1, |h, _| {
        matches!(h.reskin, Reskin::Ready(..))
    });
    // The play goes before its menu shows: the preparation is freed, never applied.
    fixture.host.game = None;
    fixture.host.advance(now + 1, &mut s);
    assert!(matches!(fixture.host.reskin, Reskin::Idle));
    assert!(fixture.scenes.pop().is_err());
}

#[test]
fn a_play_takes_its_input_from_the_settings() {
    let mut fixture = host("settings-input", true);
    let mut settings = Settings::default();
    settings.gameplay.mouse_buttons = false;
    fixture.host.input = PlayInput::new(&settings);
    let (mut s, mut snap) = listing(&fixture);
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    advance_until(&mut fixture, &mut s, TRANSITION_NS + 1, |_, s| s.playing());
    let game = fixture.host.game.as_ref().unwrap();
    assert!(game.session.input().mouse_buttons_disabled());
}

#[test]
fn play_input_follows_the_settings() {
    let defaults = PlayInput::new(&Settings::default());
    assert_eq!(defaults.bindings, Bindings::default());
    assert!(defaults.mouse_buttons);
    assert_eq!(defaults.offsets, Offsets::default());

    let mut settings = Settings::default();
    settings.timing.global_offset_ms = -25.0;
    settings.input.bindings = Bindings::new(&Bindings::default().as_slice()[..1]);
    settings.gameplay.mouse_buttons = false;
    let input = PlayInput::new(&settings);
    assert_eq!(input.offsets.global_ms, -25.0);
    assert_eq!(input.offsets.beatmap_ms, 0.0);
    assert_eq!(
        input.bindings.as_slice(),
        &Bindings::default().as_slice()[..1]
    );
    assert!(!input.mouse_buttons);
}

#[test]
fn the_songs_flag_overrides_the_file() {
    let mut settings = Settings::default();
    settings.paths.songs = Some("/from/file".into());
    let kept = with_songs(settings.clone(), None);
    assert_eq!(kept.paths.songs.as_deref(), Some(Path::new("/from/file")));
    let flag = with_songs(settings, Some("/from/flag".into()));
    assert_eq!(flag.paths.songs.as_deref(), Some(Path::new("/from/flag")));
}

#[test]
fn changed_settings_reach_the_audio_and_the_next_play() {
    let mut fixture = host("live", false);
    let mut settings = Settings::default();
    settings.audio.master_volume = 0.5;
    settings.audio.hitsound_volume = 0.25;
    settings.audio.hitsound_delay = crate::settings::HitsoundDelay::Stable;
    settings.timing.global_offset_ms = -12.0;
    settings.gameplay.mouse_buttons = !settings.gameplay.mouse_buttons;
    settings.input.clear(Actions::SMOKE, 0);
    fixture.host.apply_settings(&settings);
    let host = &fixture.host;
    assert_eq!(
        host.audio.volumes,
        AppliedVolumes::new(Volumes::of(&settings.audio))
    );
    assert_eq!(host.input.offsets.global_ms, -12.0);
    assert_eq!(host.input.mouse_buttons, settings.gameplay.mouse_buttons);
    assert_eq!(host.input.bindings, settings.input.bindings);
    assert_ne!(host.input.bindings, Bindings::default());
}

#[test]
fn a_changed_output_is_asked_of_the_output_thread_once() {
    let mut fixture = host("output-ask", false);
    let (link, mut ends) = output::test_link();
    fixture.host.output = Some(link);
    let mut settings = Settings::default();
    // Volumes alone keep the output.
    settings.audio.master_volume = 0.5;
    fixture.host.apply_settings(&settings);
    assert!(ends.requests.pop().is_err());
    settings.audio.device = Some("hdmi".into());
    fixture.host.apply_settings(&settings);
    assert!(fixture.host.switching);
    assert!(matches!(
        ends.requests.pop(),
        Ok(Request::Open(a)) if a.device.as_deref() == Some("hdmi")
    ));
    // Asked once, however long it takes.
    fixture.host.apply_settings(&settings);
    assert!(ends.requests.pop().is_err());
    // An output that did not open is not asked for again until the settings change.
    assert!(ends.opened.push(output::Opened::Lost).is_ok());
    let mut s = state();
    fixture.host.advance(1, &mut s);
    assert!(!fixture.host.switching);
    fixture.host.apply_settings(&settings);
    assert!(ends.requests.pop().is_err());
    settings.audio.buffer_frames *= 2;
    fixture.host.apply_settings(&settings);
    assert!(matches!(ends.requests.pop(), Ok(Request::Open(_))));
}

/// Runs `mixer`'s callback for `buffers` buffers of 64 frames from stream frame `frame`;
/// the frame after them.
fn run_callback(mixer: &mut slam_audio::Mixer, mut frame: u64, buffers: usize) -> u64 {
    let mut out = vec![0.0f32; usize::from(CHANNELS) * 64];
    for _ in 0..buffers {
        let info = slam_audio::CallbackInfo {
            frame,
            timestamp_ns: 0,
            latency_ns: 0,
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
        };
        slam_audio::AudioCallback::process(mixer, &mut out, &info);
        frame += 64;
        std::thread::sleep(Duration::from_millis(1));
    }
    frame
}

#[test]
fn a_new_output_plays_the_track_on_from_where_the_old_one_left_it() {
    let mut fixture = host("output-switch", true);
    let track = fixture.dir.join("audio.wav");
    let player = &mut fixture.host.audio.player;
    player.set_rate(1.5).unwrap();
    let generation = player.load(TrackSource::File(track.clone())).unwrap();
    fixture.host.track = Some(track);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut frame = 0;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the track did not load"
        );
        frame = run_callback(&mut fixture._mixer, frame, 1);
        match fixture.host.audio.player.poll_event() {
            Some(MusicEvent::Loaded { generation: g, .. }) if g == generation => break,
            Some(MusicEvent::Failed { .. }) => panic!("the track did not load"),
            _ => {}
        }
    }
    fixture.host.audio.player.play().unwrap();
    // The decoder thread fills the track's buffer at its own pace: on a slow machine the
    // first buffers after `play` may still find it empty (an underrun stops the anchor).
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let left = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the track did not start"
        );
        frame = run_callback(&mut fixture._mixer, frame, 1);
        fixture
            .host
            .audio
            .output
            .publish(slam_audio::PositionSnapshot {
                frame,
                timestamp_ns: 1,
                latency_ns: 0,
                sample_rate: SAMPLE_RATE,
            });
        if let Some((left, true)) = output::resume_point(
            fixture.host.audio.output.read(),
            fixture.host.audio.music.read(),
        ) && left > 0
        {
            break left;
        }
    };

    let (link, mut ends) = output::test_link();
    fixture.host.output = Some(link);
    fixture.host.switching = true;
    let (new, mut new_mixer) = detached_audio();
    assert!(
        ends.opened
            .push(output::Opened::Asked(Box::new(new)))
            .is_ok()
    );
    let mut s = state();
    fixture.host.advance(1, &mut s);
    assert!(!fixture.host.switching);
    assert!(matches!(ends.requests.pop(), Ok(Request::Retire(_))));
    let player = &fixture.host.audio.player;
    assert_eq!(player.rate(), 1.5);

    // The new output plays the same track on from there.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut frame = 0;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the track did not go on"
        );
        frame = run_callback(&mut new_mixer, frame, 1);
        let _ = fixture.host.audio.player.poll_event();
        if let Some(music) = fixture.host.audio.music.read()
            && music.generation == fixture.host.audio.player.generation()
            && music.anchor.moves()
        {
            assert!(music.anchor.track_frame >= left as f64, "{music:?}");
            assert!(
                music.anchor.track_frame < left as f64 + 48_000.0,
                "{music:?}"
            );
            break;
        }
    }
}

#[test]
fn no_play_starts_while_the_output_switches() {
    let mut fixture = host("output-wait", true);
    let (link, _ends) = output::test_link();
    fixture.host.output = Some(link);
    fixture.host.switching = true;
    let (mut s, mut snap) = listing(&fixture);
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    fixture.host.advance(TRANSITION_NS + 2, &mut s);
    assert!(!s.is_loading());
    assert!(matches!(fixture.host.pending, Pending::Idle));
}

#[test]
fn no_play_starts_without_an_output_until_one_opens() {
    let mut fixture = host("output-lost", true);
    let (link, mut ends) = output::test_link();
    fixture.host.output = Some(link);
    fixture.host.switching = true;
    assert!(ends.opened.push(output::Opened::Lost).is_ok());
    let (mut s, mut snap) = listing(&fixture);
    fixture.host.advance(TRANSITION_NS + 1, &mut s);
    assert!(fixture.host.no_output && !fixture.host.switching);
    s.tick(TRANSITION_NS + 2, &[key(KEY_ENTER)], &mut snap);
    fixture.host.advance(TRANSITION_NS + 3, &mut s);
    assert!(!s.is_loading());
    // The previous output, opened again, brings the plays back.
    fixture.host.switching = true;
    let (audio, _mixer) = detached_audio();
    assert!(
        ends.opened
            .push(output::Opened::Previous(Box::new(audio)))
            .is_ok()
    );
    fixture.host.advance(TRANSITION_NS + 4, &mut s);
    assert!(!fixture.host.no_output && !fixture.host.switching);
    assert!(matches!(ends.requests.pop(), Ok(Request::Retire(_))));
}

#[test]
fn old_handles_wait_for_room_in_the_ring() {
    let mut fixture = host("output-later", false);
    let (link, mut ends) = output::test_link();
    fixture.host.output = Some(link);
    fixture.host.switching = true;
    let (audio, _mixer) = detached_audio();
    assert!(
        ends.opened
            .push(output::Opened::Asked(Box::new(audio)))
            .is_ok()
    );
    // A full ring keeps the old handles on the host until it has room.
    let link = fixture.host.output.as_mut().unwrap();
    while link
        .request(Request::List(Box::new(Settings::default().audio)))
        .is_ok()
    {}
    fixture.host.switch_output();
    assert!(!fixture.host.switching);
    assert!(fixture.host.retired.is_some());
    while ends.requests.pop().is_ok() {}
    fixture.host.switch_output();
    assert!(fixture.host.retired.is_none());
    assert!(matches!(ends.requests.pop(), Ok(Request::Retire(_))));
}

#[test]
fn a_full_ring_asks_for_the_output_again() {
    let mut fixture = host("output-again", false);
    let (link, mut ends) = output::test_link();
    fixture.host.output = Some(link);
    let link = fixture.host.output.as_mut().unwrap();
    while link
        .request(Request::List(Box::new(Settings::default().audio)))
        .is_ok()
    {}
    let mut settings = Settings::default();
    settings.audio.device = Some("hdmi".into());
    fixture.host.apply_settings(&settings);
    assert!(!fixture.host.switching);
    while ends.requests.pop().is_ok() {}
    fixture.host.apply_settings(&settings);
    assert!(fixture.host.switching);
    assert!(matches!(ends.requests.pop(), Ok(Request::Open(_))));
}

#[test]
fn opening_the_settings_lists_the_devices_again() {
    let mut fixture = host("output-list", false);
    let (link, mut ends) = output::test_link();
    fixture.host.output = Some(link);
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    // The menu's settings entry.
    s.tick(0, &[key(KEY_DOWN), key(KEY_ENTER)], &mut snap);
    assert!(s.settings_open());
    fixture.host.advance(1, &mut s);
    assert!(matches!(ends.requests.pop(), Ok(Request::List(_))));
    fixture.host.advance(2, &mut s);
    assert!(ends.requests.pop().is_err(), "once per opening");
    let devices = vec![
        slam_audio::DeviceInfo {
            id: "usb".into(),
            name: "USB".into(),
        },
        slam_audio::DeviceInfo {
            id: "hdmi".into(),
            name: "HDMI".into(),
        },
    ];
    assert!(
        ends.lists
            .push(Box::new(output::DeviceList {
                devices,
                saved: None
            }))
            .is_ok()
    );
    fixture.host.advance(3, &mut s);
    let snapshot = s.settings_screen().expect("the settings panel").snapshot();
    let Some(RowWidget::Dropdown(device)) = snapshot.widgets.get(rows::device_row()) else {
        panic!("the device row");
    };
    assert_eq!((device.count(), device.selected()), (3, 0));
}

#[test]
fn the_song_select_previews_the_settled_selection_unless_turned_off() {
    let mut fixture = host("preview", true);
    let (mut producer, link) = crate::songs::link();
    let mut s = state().with_songs(link);
    let mut snap = ShellSnapshot::default();
    let mut sets = songs(&[("set", &["Normal"])]);
    sets[0].difficulties[0].path = fixture.dir.join("map.osu");
    sets[0].difficulties[0].audio = "audio.wav".into();
    producer
        .push(crate::songs::publish(
            &sets,
            SongSelectStatus::Ready,
            Arc::from(""),
        ))
        .unwrap();
    s.tick(0, &[key(KEY_ENTER)], &mut snap);
    s.tick(TRANSITION_NS, &[], &mut snap);
    assert!(s.selected().is_some());
    // Not before the selection settles.
    fixture.host.advance(TRANSITION_NS, &mut s);
    fixture.host.advance(TRANSITION_NS + 1, &mut s);
    let loads = |h: &PlayHost| {
        h.audio
            .previews
            .iter()
            .map(MusicPlayer::generation)
            .sum::<u32>()
    };
    assert_eq!(loads(&fixture.host), 0);
    fixture
        .host
        .advance(TRANSITION_NS + SELECTION_DEBOUNCE_NS + 1, &mut s);
    assert_eq!(loads(&fixture.host), 1, "the preview loads");
    // Turned off in the settings, nothing new loads.
    let mut off = s.settings().clone();
    off.audio.menu_music = false;
    let mut quiet = state().with_settings(off);
    fixture.host.menu_music = MenuMusic::default();
    for audio in &mut fixture.host.audio.previews {
        while audio.poll_event().is_some() {}
    }
    let before = loads(&fixture.host);
    fixture
        .host
        .advance(TRANSITION_NS + 2 * SELECTION_DEBOUNCE_NS, &mut quiet);
    assert_eq!(loads(&fixture.host), before);
}

#[test]
fn the_menu_picks_only_difficulties_with_an_audio_file() {
    let file = |audio: Option<&str>| crate::songs::SongFile {
        path: PathBuf::from("set/map.osu"),
        approach_rate: 5.0,
        circle_size: 4.0,
        drain_rate: 5.0,
        overall_difficulty: 5.0,
        background: None,
        audio: audio.map(PathBuf::from),
        preview_ms: 1_000,
    };
    let mut rng = 1;
    assert!(random_track(&mut rng, &[]).is_none());
    let files: Vec<Box<[crate::songs::SongFile]>> = vec![
        vec![file(None)].into(),
        vec![file(None), file(Some("set/b.mp3"))].into(),
    ];
    for _ in 0..20 {
        if let Some(picked) = random_track(&mut rng, &files) {
            assert_eq!(picked.path, Path::new("set/b.mp3"));
            assert_eq!(picked.preview_ms, 1_000);
        }
    }
    let none: Vec<Box<[crate::songs::SongFile]>> = vec![vec![file(None)].into()];
    assert!(random_track(&mut rng, &none).is_none());
}

#[test]
fn an_unknown_backend_falls_back_to_the_first_available() {
    let available = Backend::available();
    let first = available.first().copied();
    assert_eq!(pick_backend(None, available), first);
    assert_eq!(pick_backend(Some("no-such-backend"), available), first);
    for &backend in available {
        assert_eq!(pick_backend(Some(backend.name()), available), Some(backend));
    }
    assert_eq!(pick_backend(Some("pipewire"), &[]), None);
}

#[test]
fn a_vanished_saved_device_falls_back_to_the_default() {
    let device = |id: &str| slam_audio::DeviceInfo {
        id: id.to_owned(),
        name: id.to_owned(),
    };
    let listed = [device("{a}"), device("{b}")];
    // Still there: kept.
    assert_eq!(
        pick_device(Some("{b}"), Some(&listed)),
        Some("{b}".to_owned())
    );
    // Recreated with a new id or unplugged: the default.
    assert_eq!(pick_device(Some("{old}"), Some(&listed)), None);
    assert_eq!(pick_device(Some("{old}"), Some(&[])), None);
    // The backend cannot list: try the saved one anyway.
    assert_eq!(pick_device(Some("{old}"), None), Some("{old}".to_owned()));
    // Nothing saved: the default.
    assert_eq!(pick_device(None, Some(&listed)), None);
}

// Mod hotkeys: the top letter row is the first column, the home row the second.
const KEY_Q: u16 = 20; // EZ
const KEY_W: u16 = 26; // NF
const KEY_E: u16 = 8; // HT
const KEY_R_HOTKEY: u16 = 21; // DC
const KEY_A: u16 = 4; // HR
const KEY_F: u16 = 9; // DT
const KEY_G: u16 = 10; // NC
const KEY_L: u16 = 15; // past the column
const KEY_Z: u16 = 29;

/// The selected acronyms, sorted.
fn acronyms(s: &ShellState) -> Vec<&str> {
    let mut list: Vec<_> = s
        .selected_mods()
        .mods()
        .iter()
        .map(|m| m.acronym())
        .collect();
    list.sort_unstable();
    list
}

/// A tick `n` after the song select came in.
pub(super) fn at(n: u64) -> u64 {
    TRANSITION_NS + n
}

#[test]
fn f1_opens_the_mod_panel_and_escape_closes_it_without_leaving() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(at(1), &[key(KEY_F1)], &mut snap);
    assert!(song_view(&snap).mods.open);
    // The panel takes the arrows: the carousel stays.
    s.tick(at(2), &[key(KEY_DOWN), key(KEY_END)], &mut snap);
    assert_eq!(s.song_select().selected(), Some((0, 0)));
    s.tick(at(3), &[key(KEY_ESCAPE)], &mut snap);
    assert!(!song_view(&snap).mods.open);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    // Enter closes it instead of playing; F1 closes it too.
    s.tick(at(4), &[key(KEY_F1), key(KEY_ENTER)], &mut snap);
    assert!(!song_view(&snap).mods.open);
    assert_eq!(s.take_play_request(), None);
    s.tick(at(5), &[key(KEY_F1), key(KEY_F1)], &mut snap);
    assert!(!song_view(&snap).mods.open);
    // Closed, Escape leaves.
    s.tick(at(6), &[key(KEY_ESCAPE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
}

#[test]
fn hotkeys_toggle_mods_by_key_row_and_sync_the_tiles() {
    let (mut s, mut snap, _producer) = in_song_select();
    // Closed, letters do nothing.
    s.tick(at(1), &[key(KEY_Q)], &mut snap);
    assert!(acronyms(&s).is_empty());
    s.tick(at(2), &[key(KEY_F1)], &mut snap);
    // Easy, then Hard Rock, which it conflicts with: the added one wins.
    s.tick(at(3), &[key(KEY_Q), key(KEY_A)], &mut snap);
    assert_eq!(acronyms(&s), ["HR"]);
    assert_eq!(song_view(&snap).mods.active, [0, 0b1, 0]);
    s.tick(at(4), &[key(KEY_W), key(KEY_G)], &mut snap);
    assert_eq!(acronyms(&s), ["HR", "NC", "NF"]);
    assert_eq!(song_view(&snap).mods.active, [0b10, 0b10001, 0]);
    // A selected mod's key deselects it.
    s.tick(at(5), &[key(KEY_W)], &mut snap);
    assert_eq!(acronyms(&s), ["HR", "NC"]);
    // Keys past the column's last mod and other keys do nothing.
    s.tick(at(6), &[key(KEY_L), key(KEY_Z)], &mut snap);
    assert_eq!(acronyms(&s), ["HR", "NC"]);
    // R is a hotkey here, not Retry; Daycore and Nightcore conflict.
    s.tick(at(7), &[key(KEY_R_HOTKEY)], &mut snap);
    assert_eq!(acronyms(&s), ["DC", "HR"]);
    assert_eq!(song_view(&snap).mods.active, [0b1000, 0b1, 0]);
    assert!(!s.is_loading());
}

#[test]
fn a_conflicting_mod_replaces_the_selected_one() {
    let (mut s, mut snap, _producer) = in_song_select();
    // DT, then HT: only HT is left.
    s.tick(at(1), &[key(KEY_F1), key(KEY_F)], &mut snap);
    assert_eq!(acronyms(&s), ["DT"]);
    s.tick(at(2), &[key(KEY_E)], &mut snap);
    assert_eq!(acronyms(&s), ["HT"]);
    assert_eq!(song_view(&snap).mods.active, [0b100, 0, 0]);
}

#[test]
fn backspace_deselects_all_with_the_panel_open_or_closed() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(at(1), &[key(KEY_F1), key(KEY_Q), key(KEY_F)], &mut snap);
    assert_eq!(acronyms(&s), ["DT", "EZ"]);
    s.tick(at(2), &[key(KEY_BACKSPACE)], &mut snap);
    assert!(acronyms(&s).is_empty());
    assert_eq!(song_view(&snap).mods.active, [0, 0, 0]);
    assert!(song_view(&snap).mods.open);
    s.tick(at(3), &[key(KEY_Q), key(KEY_ESCAPE)], &mut snap);
    assert_eq!(acronyms(&s), ["EZ"]);
    s.tick(at(4), &[key(KEY_BACKSPACE)], &mut snap);
    assert!(acronyms(&s).is_empty());
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
}

#[test]
fn a_click_on_a_tile_toggles_it() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(at(1), &[key(KEY_F1)], &mut snap);
    // The conversion column has no hotkeys but takes the mouse.
    let [x, y] = s.song_layout.mods.tile_rect(2, 0).centre();
    let click = [
        mouse(InputKind::MouseMove { x, y }),
        mouse(InputKind::MouseButton {
            button: MouseButton::Left,
            down: true,
        }),
    ];
    s.tick(at(2), &click, &mut snap);
    assert_eq!(acronyms(&s), ["DA"]);
    assert_eq!(song_view(&snap).mods.hover, Some((2, 0)));
}

#[test]
fn the_mods_outlast_a_visit_and_the_menu_ignores_the_panel_keys() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(
        at(1),
        &[key(KEY_F1), key(KEY_Q), key(KEY_ESCAPE)],
        &mut snap,
    );
    s.tick(at(2), &[key(KEY_ESCAPE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
    assert_eq!(acronyms(&s), ["EZ"]);
    s.tick(
        at(3),
        &[key(KEY_F1), key(KEY_BACKSPACE), key(KEY_A)],
        &mut snap,
    );
    assert_eq!(acronyms(&s), ["EZ"]);
}

fn key_up(scancode: u16) -> InputEvent {
    InputEvent {
        time_ns: 0,
        kind: InputKind::Key {
            scancode,
            down: false,
        },
    }
}

/// The mods line's detail of the tile `index` of `column`.
fn detail(snap: &ShellSnapshot, column: usize, index: usize) -> TileDetail {
    song_view(snap).mods.details[column][index]
}

#[test]
fn ctrl_up_and_down_change_the_mod_speed() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(at(1), &[key(KEY_LEFT_CTRL), key(KEY_UP)], &mut snap);
    assert_eq!(acronyms(&s), ["DT"]);
    // Double Time is the fourth tile of the second column.
    assert_eq!(song_view(&snap).mods.active, [0, 0b1000, 0]);
    assert_eq!(detail(&snap, 1, 3), TileDetail::Speed(105));
    // The carousel stays while Control is held.
    assert_eq!(s.song_select().selected(), Some((0, 0)));
    s.tick(at(2), &[key(KEY_DOWN), key(KEY_DOWN)], &mut snap);
    assert_eq!(acronyms(&s), ["HT"]);
    assert_eq!(detail(&snap, 0, 2), TileDetail::Speed(95));
    // Released, Down moves through the list again.
    s.tick(at(3), &[key_up(KEY_LEFT_CTRL), key(KEY_DOWN)], &mut snap);
    assert_eq!(s.song_select().selected(), Some((0, 1)));
    assert_eq!(acronyms(&s), ["HT"]);
    // The right Control works too, with the panel open; Nightcore stays Nightcore.
    s.tick(
        at(4),
        &[key(KEY_BACKSPACE), key(KEY_F1), key(KEY_G)],
        &mut snap,
    );
    assert_eq!(acronyms(&s), ["NC"]);
    assert_eq!(detail(&snap, 1, 4), TileDetail::None);
    s.tick(at(5), &[key(KEY_RIGHT_CTRL), key(KEY_UP)], &mut snap);
    assert_eq!(acronyms(&s), ["NC"]);
    assert_eq!(detail(&snap, 1, 4), TileDetail::Speed(155));
}

#[test]
fn held_keys_repeat_the_walks_and_the_mod_steps_only() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(
        at(1),
        &[key(KEY_DOWN), repeat(KEY_UP), repeat(KEY_DOWN)],
        &mut snap,
    );
    assert_eq!(s.song_select().selected(), Some((0, 1)));
    s.tick(
        at(2),
        &[
            key(KEY_LEFT_CTRL),
            key(KEY_UP),
            repeat(KEY_UP),
            repeat(KEY_F4),
        ],
        &mut snap,
    );
    assert_eq!(detail(&snap, 1, 3), TileDetail::Speed(110));
    assert_eq!(detail(&snap, 2, 0), TileDetail::ApproachRate(51));
    // The carousel stays while Control is held.
    assert_eq!(s.song_select().selected(), Some((0, 1)));
    // A held letter or F1 acts once.
    s.tick(
        at(3),
        &[
            key_up(KEY_LEFT_CTRL),
            key(KEY_F1),
            repeat(KEY_F1),
            repeat(KEY_Q),
        ],
        &mut snap,
    );
    assert!(song_view(&snap).mods.open);
    assert_eq!(acronyms(&s), ["DA", "DT"]);
}

#[test]
fn a_held_backspace_erases_with_control_too() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(at(1), &[text("abc")], &mut snap);
    s.tick(
        at(2),
        &[
            key(KEY_LEFT_CTRL),
            repeat(KEY_BACKSPACE),
            repeat(KEY_BACKSPACE),
        ],
        &mut snap,
    );
    assert_eq!(s.song_select().query().as_str(), "a");
}

#[test]
fn ctrl_f3_and_f4_change_the_approach_rate() {
    let (mut s, mut snap, _producer) = in_song_select();
    // The test difficulties have an approach rate of 5.
    s.tick(
        at(1),
        &[key(KEY_LEFT_CTRL), key(KEY_F4), key(KEY_F4)],
        &mut snap,
    );
    assert_eq!(acronyms(&s), ["DA"]);
    assert_eq!(song_view(&snap).mods.active, [0, 0, 0b1]);
    assert_eq!(detail(&snap, 2, 0), TileDetail::ApproachRate(52));
    s.tick(at(2), &[key(KEY_F3), key(KEY_F3)], &mut snap);
    assert!(acronyms(&s).is_empty());
    s.tick(at(3), &[key(KEY_F3)], &mut snap);
    assert_eq!(detail(&snap, 2, 0), TileDetail::ApproachRate(49));
    // Hard Rock and Difficulty Adjust do not go together: the hotkey does nothing.
    s.tick(
        at(4),
        &[key(KEY_BACKSPACE), key(KEY_F1), key(KEY_A), key(KEY_F4)],
        &mut snap,
    );
    assert_eq!(acronyms(&s), ["HR"]);
    // Without Control, F3 and F4 do nothing.
    s.tick(
        at(5),
        &[key_up(KEY_LEFT_CTRL), key(KEY_ESCAPE), key(KEY_F4)],
        &mut snap,
    );
    assert_eq!(acronyms(&s), ["HR"]);
}

#[test]
fn the_control_hotkeys_wait_while_a_play_loads() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(at(1), &[key(KEY_ENTER)], &mut snap);
    assert!(s.is_loading());
    s.tick(
        at(2),
        &[key(KEY_LEFT_CTRL), key(KEY_UP), key(KEY_F4)],
        &mut snap,
    );
    assert!(acronyms(&s).is_empty());
}

#[test]
fn control_released_during_a_play_is_released_after_it() {
    let (mut s, _snap, _producer) = in_song_select();
    s.follow_pointer(&[key(KEY_LEFT_CTRL), key_up(KEY_LEFT_CTRL)]);
    assert_eq!(s.modifiers, 0);
    s.follow_pointer(&[key(KEY_RIGHT_CTRL)]);
    assert_eq!(s.modifiers, 2);
}

#[test]
fn a_play_takes_the_selected_mods() {
    let mut fixture = host("mods", true);
    let (mut s, mut snap) = listing(&fixture);
    s.tick(
        at(1),
        &[key(KEY_F1), key(KEY_F), key(KEY_ESCAPE)],
        &mut snap,
    );
    s.tick(at(2), &[key(KEY_ENTER)], &mut snap);
    let now = advance_until(&mut fixture, &mut s, at(2), |_, s| s.playing());
    let game = fixture.host.game.as_mut().unwrap();
    assert_eq!(
        game.mods.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
        ["DT"]
    );
    game.ending = Ending::Sounds;
    fixture.host.advance(now + 1, &mut s);
    // They stay for the next play.
    assert_eq!(acronyms(&s), ["DT"]);
}

#[test]
fn the_panel_shows_every_difficulty_increase_mod() {
    let columns = mod_columns();
    assert_eq!(columns[1], mods_for(ModType::DifficultyIncrease));
    assert!(columns[1].contains(&"TC"));
}

fn customise(snap: &ShellSnapshot) -> slam_ui::song_select::CustomisationSnapshot {
    song_view(snap).mods.customise
}

fn state_of(s: &ShellState, acronym: &str, key: &str) -> Option<slam_osu::mods::SettingState> {
    s.selected_mods().get(acronym)?.setting_state(key)
}

#[test]
fn the_settings_panel_lists_the_selected_mods_and_edits_them() {
    use slam_osu::mods::SettingState;
    use slam_ui::song_select::{CustomisationState, SettingRowKind};
    let (mut s, mut snap, _producer) = in_song_select();
    // Nothing selected: nothing to customise, Tab does nothing.
    s.tick(at(1), &[key(KEY_F1), key(KEY_TAB)], &mut snap);
    assert!(!customise(&snap).is_enabled());
    assert_eq!(customise(&snap).state, CustomisationState::Collapsed);
    // Double Time: a section header, the speed and the pitch.
    s.tick(at(2), &[key(KEY_F)], &mut snap);
    let c = customise(&snap);
    assert_eq!(c.len, 3);
    assert_eq!(c.rows[0].kind, SettingRowKind::Header);
    assert_eq!(c.rows[1].label, "song-select-mods-setting-speed-increase");
    assert_eq!(c.rows[1].suffix, "x");
    assert_eq!(c.state, CustomisationState::Collapsed);
    // Tab expands; the arrows now act on the panel, the carousel stays.
    s.tick(
        at(3),
        &[key(KEY_TAB), key(KEY_RIGHT), key(KEY_RIGHT)],
        &mut snap,
    );
    assert_eq!(customise(&snap).state, CustomisationState::Expanded);
    assert_eq!(
        state_of(&s, "DT", "speed_change"),
        Some(SettingState::Number(Some(1.52)))
    );
    assert_eq!(detail(&snap, 1, 3), TileDetail::Speed(152));
    assert_eq!(s.song_select().selected(), Some((0, 0)));
    // The panel took the keys: a hotkey and Backspace do nothing.
    s.tick(at(4), &[key(KEY_A), key(KEY_BACKSPACE)], &mut snap);
    assert_eq!(acronyms(&s), ["DT"]);
    // Space flips the pitch toggle, Delete resets it.
    s.tick(at(5), &[key(KEY_DOWN), key(KEY_SPACE)], &mut snap);
    assert_eq!(
        state_of(&s, "DT", "adjust_pitch"),
        Some(SettingState::Toggle(true))
    );
    s.tick(at(6), &[key(KEY_DELETE)], &mut snap);
    assert_eq!(
        state_of(&s, "DT", "adjust_pitch"),
        Some(SettingState::Toggle(false))
    );
    // Edits from the panel keep it open; Esc collapses it, a second Esc closes the select.
    assert!(customise(&snap).state != CustomisationState::Collapsed);
    s.tick(at(7), &[key(KEY_ESCAPE)], &mut snap);
    assert_eq!(customise(&snap).state, CustomisationState::Collapsed);
    assert!(song_view(&snap).mods.open);
    s.tick(at(8), &[key(KEY_ESCAPE)], &mut snap);
    assert!(!song_view(&snap).mods.open);
}

#[test]
fn changing_the_selection_collapses_the_panel() {
    use slam_ui::song_select::CustomisationState;
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(at(1), &[key(KEY_F1), key(KEY_F), key(KEY_TAB)], &mut snap);
    assert_eq!(customise(&snap).state, CustomisationState::Expanded);
    // The speed hotkey changes the speed only: the panel stays.
    s.tick(at(2), &[key(KEY_LEFT_CTRL), key(KEY_UP)], &mut snap);
    assert_eq!(customise(&snap).state, CustomisationState::Expanded);
    s.tick(
        at(3),
        &[key_up(KEY_LEFT_CTRL), key(KEY_TAB), key(KEY_G)],
        &mut snap,
    );
    assert_eq!(acronyms(&s), ["NC"]);
    assert_eq!(customise(&snap).state, CustomisationState::Collapsed);
}

#[test]
fn difficulty_adjust_expands_the_panel_and_shows_the_beatmap_values() {
    use slam_osu::mods::SettingState;
    use slam_ui::song_select::{CustomisationState, SettingRowKind};
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(
        at(1),
        &[key(KEY_F1), key(KEY_LEFT_CTRL), key(KEY_F4)],
        &mut snap,
    );
    assert_eq!(acronyms(&s), ["DA"]);
    let c = customise(&snap);
    assert_eq!(c.state, CustomisationState::ExpandedByMod);
    // Header, CS, HP, OD, AR and Extended Limits.
    assert_eq!(c.len, 6);
    // The test difficulties have CS 4, HP 5, OD 5; the hotkey set the approach rate only.
    let number = |row: usize| match c.rows[row].kind {
        SettingRowKind::Number { value, unset, .. } => (value, unset),
        other => panic!("{other:?}"),
    };
    assert_eq!(number(1), (4.0, true));
    assert_eq!(number(2), (5.0, true));
    assert_eq!(number(3), (5.0, true));
    assert!(!number(4).1);
    // Stepping an unset value sets it from the beatmap's; Delete unsets it again.
    s.tick(at(2), &[key_up(KEY_LEFT_CTRL), key(KEY_RIGHT)], &mut snap);
    // Stored as an f32, as lazer does.
    assert!(matches!(
        state_of(&s, "DA", "circle_size"),
        Some(SettingState::Number(Some(v))) if (v - 4.1).abs() < 1e-6
    ));
    s.tick(at(3), &[key(KEY_DELETE)], &mut snap);
    assert_eq!(
        state_of(&s, "DA", "circle_size"),
        Some(SettingState::Number(None))
    );
    // Another difficulty brings its own values.
    s.tick(
        at(4),
        &[key(KEY_ESCAPE), key(KEY_ESCAPE), key(KEY_DOWN)],
        &mut snap,
    );
    assert_eq!(s.song_select().selected(), Some((0, 1)));
}

fn text(text: &str) -> InputEvent {
    InputEvent {
        time_ns: 0,
        kind: InputKind::Text(slam_input::TextChunk::new(text).unwrap()),
    }
}

fn repeat(scancode: u16) -> InputEvent {
    InputEvent {
        time_ns: 0,
        kind: InputKind::KeyRepeat { scancode },
    }
}

#[test]
fn typing_on_the_song_select_searches_and_backspace_and_escape_edit_first() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(at(1), &[text("b"), text("ins")], &mut snap);
    assert_eq!(s.song_select().query().as_str(), "bins");
    s.tick(
        at(2),
        &[key(KEY_BACKSPACE), repeat(KEY_BACKSPACE)],
        &mut snap,
    );
    assert_eq!(s.song_select().query().as_str(), "bi");
    s.tick(at(3), &[text(" "), text("Ins")], &mut snap);
    assert_eq!(song_view(&snap).query.as_str(), "bi Ins");
    // Only set "b" has "Ins" (its difficulty Insane); "bi" is in no field.
    assert_eq!(song_view(&snap).row_count, 0);
    s.tick(at(4), &[key(KEY_BACKSPACE); 4], &mut snap);
    assert_eq!(s.song_select().query().as_str(), "bi");
    s.tick(
        at(5),
        &[key(KEY_BACKSPACE), key(KEY_BACKSPACE), text("Ins")],
        &mut snap,
    );
    assert_eq!(s.song_select().query().as_str(), "Ins");
    assert_eq!(s.song_select().selected(), Some((1, 0)));

    // Escape clears the query and stays; the next leaves.
    s.tick(at(6), &[key(KEY_ESCAPE)], &mut snap);
    assert_eq!(s.song_select().query().as_str(), "");
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    s.tick(at(7), &[key(KEY_ESCAPE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
    // The menu takes no text.
    s.tick(at(8), &[text("a")], &mut snap);
    assert_eq!(s.song_select().query().as_str(), "");
}

#[test]
fn backspace_repeats_only_erase_and_the_open_panel_takes_no_text() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.tick(at(1), &[key(KEY_F1), key(KEY_Q)], &mut snap);
    assert_eq!(acronyms(&s), ["EZ"]);
    // With the panel open the letters are hotkeys: their text is not typed.
    s.tick(at(2), &[text("q")], &mut snap);
    assert_eq!(s.song_select().query().as_str(), "");
    s.tick(at(3), &[key(KEY_ESCAPE), text("a")], &mut snap);
    assert_eq!(s.song_select().query().as_str(), "a");
    // A held Backspace erases the query and then stops; it never drops the mods.
    s.tick(
        at(4),
        &[key(KEY_BACKSPACE), repeat(KEY_BACKSPACE)],
        &mut snap,
    );
    assert_eq!(s.song_select().query().as_str(), "");
    assert_eq!(acronyms(&s), ["EZ"]);
    // A fresh press on the empty query deselects them, as before.
    s.tick(at(5), &[key(KEY_BACKSPACE)], &mut snap);
    assert!(acronyms(&s).is_empty());
}

#[test]
fn a_passed_play_saves_its_replay_before_the_results() {
    let mut fixture = host("replay", true);
    // The slider early on: the play passes soon after the track starts.
    let path = fixture.dir.join("map.osu");
    let map = std::fs::read_to_string(&path).unwrap();
    let map = map.replacen("168,256,30579,", "168,256,500,", 1);
    assert!(map.contains("168,256,500,"));
    std::fs::write(&path, map).unwrap();
    let (mut s, mut snap) = listing(&fixture);
    // No Fail: the play passes without input.
    s.tick(
        at(1),
        &[key(KEY_F1), key(KEY_W), key(KEY_ESCAPE)],
        &mut snap,
    );
    s.tick(at(2), &[key(KEY_ENTER)], &mut snap);
    let mut now = advance_until(&mut fixture, &mut s, at(2), |_, s| s.playing());
    // The output plays in steps of 16 ms, as its callback would report them.
    const STEP_FRAMES: u64 = SAMPLE_RATE as u64 * 16 / 1000;
    let mut out = vec![0.0f32; usize::from(CHANNELS) * STEP_FRAMES as usize];
    let mut frame = 0;
    let mut play = PlaySnapshot::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !fixture.host.game.as_ref().unwrap().session.passed() {
        assert!(
            std::time::Instant::now() < deadline,
            "the play did not pass"
        );
        let info = slam_audio::CallbackInfo {
            frame,
            timestamp_ns: now,
            latency_ns: 0,
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
        };
        slam_audio::AudioCallback::process(&mut fixture._mixer, &mut out, &info);
        fixture
            .host
            .audio
            .output
            .publish(slam_audio::PositionSnapshot {
                frame,
                timestamp_ns: now,
                latency_ns: 0,
                sample_rate: SAMPLE_RATE,
            });
        frame += STEP_FRAMES;
        now += 16_000_000;
        fixture.host.tick_game(now, &[], &mut play);
        fixture.host.advance(now, &mut s);
        // The decoder keeps ahead of the callback.
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(*s.stack().top(), AppScreen::Play(1));
    // The loader saves it while the results wait to show.
    let replays = fixture.dir.join("replays");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let file = loop {
        let file = std::fs::read_dir(&replays)
            .ok()
            .and_then(|mut entries| entries.next())
            .map(|entry| entry.unwrap().path());
        if let Some(file) = file {
            break file;
        }
        assert!(std::time::Instant::now() < deadline, "no replay saved");
        std::thread::sleep(Duration::from_millis(1));
    };
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("map (") && name.ends_with(").osr"),
        "{name}"
    );
    let replay = slam_formats::osr::decode(&std::fs::read(&file).unwrap())
        .unwrap()
        .replay;
    assert_eq!(
        replay.player_name.as_deref(),
        Some(crate::replays::PLAYER_NAME)
    );
    let map = std::fs::read(fixture.dir.join("map.osu")).unwrap();
    assert_eq!(
        replay.beatmap_hash.as_deref(),
        Some(crate::replays::md5_hex(&map).as_str())
    );
}

#[test]
fn the_interface_starts_in_the_settings_language() {
    let mut settings = Settings::default();
    let menu = |settings: &Settings| {
        interface_localizer(settings)
            .unwrap()
            .format("settings-layout-language", None)
            .into_owned()
    };
    assert_eq!(menu(&settings), "Language");
    settings.general.language = Some("ru".to_owned());
    assert_eq!(menu(&settings), "Язык");
    // Not from the file reader: an unknown tag stays English.
    settings.general.language = Some("de".to_owned());
    assert_eq!(menu(&settings), "Language");
}
