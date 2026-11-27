//! The toolbar in the shell: pinning from the settings panel, the popup and its settings,
//! the buttons.

use slam_ui::screen::TRANSITION_NS;
use slam_ui::toast::ToastFrame;

use super::super::*;
use super::MusicCommands;

const LATER: u64 = 10 * TRANSITION_NS;

fn settings(pins: &[&str]) -> Settings {
    let mut settings = Settings::default();
    settings.general.transitions = slam_ui::screen::TransitionStyle::Simple;
    settings.general.pinned_settings = pins.iter().map(|p| (*p).to_owned()).collect();
    settings
}

fn state(pins: &[&str]) -> ShellState {
    ShellState::new(Theme::default(), 1280.0, 720.0, 1.0).with_settings(settings(pins))
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

fn at(x: f32, y: f32) -> InputEvent {
    InputEvent {
        time_ns: 0,
        kind: InputKind::MouseMove { x, y },
    }
}

fn button(button: MouseButton, down: bool) -> InputEvent {
    InputEvent {
        time_ns: 0,
        kind: InputKind::MouseButton { button, down },
    }
}

/// Moves to `point` and clicks.
fn click(s: &mut ShellState, snap: &mut ShellSnapshot, now: u64, point: [f32; 2]) {
    let [x, y] = point;
    s.tick(
        now,
        &[
            at(x, y),
            button(MouseButton::Left, true),
            button(MouseButton::Left, false),
        ],
        snap,
    );
}

fn pin_centre(s: &ShellState, pin: usize) -> [f32; 2] {
    s.toolbar_layout().pins()[pin].centre()
}

fn toast_variant(toast: ToastFrame) -> Option<u8> {
    toast.visible.then_some(toast.variant)
}

fn click_pin(s: &mut ShellState, snap: &mut ShellSnapshot, now: u64, pin: usize) {
    let point = pin_centre(s, pin);
    click(s, snap, now, point);
}

const MUSIC: &str = "audio-menu-music";
const VOLUME: &str = "audio-master-volume";

#[test]
fn the_toolbar_shows_on_the_menus_and_not_in_a_play() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[], &mut snap);
    assert!(snap.toolbar.visible);
    assert!(snap.toolbar.clock < 24 * 60, "the clock is read");
    s.tick(1, &[key(KEY_ENTER)], &mut snap);
    s.tick(TRANSITION_NS, &[], &mut snap);
    assert!(snap.toolbar.visible, "on the song select too");
    s.stack.navigate(Nav::Push(AppScreen::Play(1)), 0);
    s.tick(LATER, &[], &mut snap);
    // It slides out first, then nothing of it shows.
    s.tick(2 * LATER, &[], &mut snap);
    assert!(!snap.toolbar.visible);
}

#[test]
fn the_pins_come_from_the_settings_and_unknown_names_are_skipped() {
    let s = state(&[MUSIC, "no-such-setting", VOLUME]);
    let rows: Vec<usize> = s.toolbar.pins().iter().map(|r| usize::from(*r)).collect();
    assert_eq!(
        rows,
        [Row::MenuMusic, Row::MasterVolume]
            .map(|row| Row::ALL.iter().position(|r| *r == row).unwrap())
    );
}

#[test]
fn a_right_click_on_a_row_pins_it_and_again_unpins_it() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[key(KEY_DOWN), key(KEY_ENTER)], &mut snap);
    assert!(s.settings_open());
    s.tick(TRANSITION_NS, &[], &mut snap);
    let panel = s.panel.as_ref().unwrap();
    // The first row of the first section: the master volume.
    let row = s.settings_layout.row(panel.screen.slot(0)).centre();
    let right = |down| button(MouseButton::Right, down);
    s.tick(
        LATER,
        &[at(row[0], row[1]), right(true), right(false)],
        &mut snap,
    );
    assert_eq!(s.toolbar.pins().len(), 1);
    assert_eq!(s.live_settings().general.pinned_settings, [VOLUME]);
    assert_eq!(toast_variant(snap.toast), Some(crate::import::TOAST_PINNED));
    s.tick(LATER + 1, &[right(true)], &mut snap);
    assert!(s.toolbar.pins().is_empty());
    assert_eq!(
        toast_variant(snap.toast),
        Some(crate::import::TOAST_UNPINNED)
    );
    s.tick(LATER + 2, &[right(true)], &mut snap);
    // Closing commits and saves the pin.
    s.tick(LATER + 3, &[key(KEY_ESCAPE)], &mut snap);
    assert_eq!(s.settings().general.pinned_settings, [VOLUME]);
    let saved = s.take_save_request().expect("the pin is saved");
    assert_eq!(saved.general.pinned_settings, [VOLUME]);
}

#[test]
fn a_right_click_off_the_rows_or_on_a_button_row_pins_nothing() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[key(KEY_DOWN), key(KEY_ENTER)], &mut snap);
    s.tick(TRANSITION_NS, &[], &mut snap);
    let right = |down| button(MouseButton::Right, down);
    s.tick(
        LATER,
        &[at(1270.0, 700.0), right(true), right(false)],
        &mut snap,
    );
    assert!(s.live_settings().general.pinned_settings.is_empty());
    // A row that cannot be pinned: the language dropdown.
    let language = Row::ALL.iter().position(|r| *r == Row::Language).unwrap();
    s.toggle_pin(language, LATER);
    assert!(s.live_settings().general.pinned_settings.is_empty());
}

#[test]
fn at_most_eight_settings_can_be_pinned() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[key(KEY_DOWN), key(KEY_ENTER)], &mut snap);
    s.tick(TRANSITION_NS, &[], &mut snap);
    let pinnable: Vec<usize> = Row::ALL
        .iter()
        .enumerate()
        .filter(|(_, row)| s.rows.pin_widget(**row, &s.settings).is_some())
        .map(|(i, _)| i)
        .collect();
    assert!(pinnable.len() > 9);
    for row in &pinnable[..8] {
        s.toggle_pin(*row, LATER);
    }
    assert_eq!(s.toolbar.pins().len(), 8);
    assert_eq!(
        toast_variant(s.toast.frame()),
        Some(crate::import::TOAST_PINNED)
    );
    s.toggle_pin(pinnable[8], LATER);
    assert_eq!(s.toolbar.pins().len(), 8);
    assert_eq!(s.live_settings().general.pinned_settings.len(), 8);
    assert_eq!(
        toast_variant(s.toast.frame()),
        Some(crate::import::TOAST_PIN_FULL)
    );
    // Taking one off makes room.
    s.toggle_pin(pinnable[0], LATER);
    s.toggle_pin(pinnable[8], LATER);
    assert_eq!(s.toolbar.pins().len(), 8);
}

#[test]
fn clicking_a_pin_opens_its_popup_and_a_click_elsewhere_closes_it() {
    let mut s = state(&[MUSIC]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[], &mut snap);
    click_pin(&mut s, &mut snap, 1, 0);
    assert!(snap.toolbar.popup.is_some());
    assert!(s.popup_editing && s.editing.is_some());
    // Only one popup at a time; a click outside takes it away, and the click is its own.
    click(&mut s, &mut snap, 2, [20.0, 600.0]);
    assert!(snap.toolbar.popup.is_none());
    assert!(!s.popup_editing && s.editing.is_none());
    assert_eq!(*s.stack().top(), AppScreen::Menu(MainMenu::new()));
}

#[test]
fn escape_closes_the_popup_before_it_leaves_the_screen() {
    let mut s = state(&[MUSIC]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[], &mut snap);
    click_pin(&mut s, &mut snap, 1, 0);
    assert!(snap.toolbar.popup.is_some());
    // On the menu Escape would exit: it closes the popup instead.
    let nav = s.tick(2, &[key(KEY_ESCAPE)], &mut snap);
    assert_eq!(nav, Navigated::Stay);
    assert!(snap.toolbar.popup.is_none());
    let nav = s.tick(3, &[key(KEY_ESCAPE)], &mut snap);
    assert_eq!(nav, Navigated::Exit);
}

#[test]
fn a_value_from_the_popup_applies_at_once_and_is_saved_when_it_closes() {
    let mut s = state(&[MUSIC]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[], &mut snap);
    assert!(s.settings().audio.menu_music);
    click_pin(&mut s, &mut snap, 1, 0);
    let popup = snap.toolbar.popup.expect("open");
    assert_eq!(usize::from(popup.row), s.toolbar.pins()[0] as usize);
    let row = s.toolbar_layout().row(0, 0.0).row(0).centre();
    click(&mut s, &mut snap, 2, row);
    // Live, not yet committed.
    assert!(!s.live_settings().audio.menu_music);
    assert!(s.settings().audio.menu_music);
    assert!(s.take_save_request().is_none());
    click(&mut s, &mut snap, 3, [20.0, 600.0]);
    assert!(!s.settings().audio.menu_music);
    let saved = s.take_save_request().expect("saved once it closed");
    assert!(!saved.audio.menu_music);
    assert!(s.take_settings_changed());
}

#[test]
fn a_slider_popup_drags() {
    let mut s = state(&[VOLUME]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[], &mut snap);
    click_pin(&mut s, &mut snap, 1, 0);
    let widget = s.toolbar_layout().row(0, 0.0).widget(0);
    let y = widget.y + widget.height / 2.0;
    let x = widget.x + widget.width * 0.1;
    s.tick(
        2,
        &[at(x, y), button(MouseButton::Left, true), at(x + 5.0, y)],
        &mut snap,
    );
    let low = s.live_settings().audio.master_volume;
    assert!(low < 0.5, "{low}");
    s.tick(
        3,
        &[at(x + 100.0, y), button(MouseButton::Left, false)],
        &mut snap,
    );
    let high = s.live_settings().audio.master_volume;
    assert!(high > low, "{low} -> {high}");
    s.tick(4, &[at(x + 300.0, y)], &mut snap);
    assert_eq!(s.live_settings().audio.master_volume, high, "released");
}

#[test]
fn another_pin_switches_the_popup() {
    let mut s = state(&[MUSIC, VOLUME]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[], &mut snap);
    click_pin(&mut s, &mut snap, 1, 0);
    let first = snap.toolbar.popup.unwrap().row;
    click_pin(&mut s, &mut snap, 2, 1);
    let second = snap.toolbar.popup.unwrap();
    assert_ne!(first, second.row);
    assert_eq!(second.pin, 1);
    click_pin(&mut s, &mut snap, 3, 1);
    assert!(snap.toolbar.popup.is_none());
}

#[test]
fn the_settings_button_opens_the_panel_and_commits_the_popup_first() {
    let mut s = state(&[MUSIC]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[], &mut snap);
    click_pin(&mut s, &mut snap, 1, 0);
    let row = s.toolbar_layout().row(0, 0.0).row(0).centre();
    click(&mut s, &mut snap, 2, row);
    let button = s.toolbar_layout().settings.centre();
    click(&mut s, &mut snap, 3, button);
    assert!(s.settings_open());
    assert!(snap.toolbar.popup.is_none());
    assert!(
        !s.settings().audio.menu_music,
        "the popup's change was kept"
    );
    // The panel starts from it.
    assert!(!s.live_settings().audio.menu_music);
}

#[test]
fn the_home_button_goes_back_to_the_main_menu() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[key(KEY_ENTER)], &mut snap);
    s.tick(TRANSITION_NS, &[], &mut snap);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    let home = s.toolbar_layout().home.centre();
    click(&mut s, &mut snap, 2 * TRANSITION_NS, home);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
    assert_eq!(s.stack().depth(), 1);
}

#[test]
fn the_toolbar_takes_clicks_on_it_and_the_menu_below_still_works() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    // The bar's empty middle is not clicked through to anything.
    let bar = s.toolbar_layout().bar;
    click(&mut s, &mut snap, 0, [bar.width / 2.0, bar.height / 2.0]);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
    let [x, y] = s.layout.buttons[0].centre();
    click(&mut s, &mut snap, 1, [x, y]);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
}

/// Opens the player's popup by clicking its button, the bar being in.
fn open_player(s: &mut ShellState, snap: &mut ShellSnapshot, now: u64) {
    let point = s.toolbar_layout().player.centre();
    click(s, snap, now, point);
    assert!(snap.toolbar.player.is_some());
}

/// Clicks the player's button `button` (0 previous, 1 play/pause, 2 next).
fn click_player_button(s: &mut ShellState, snap: &mut ShellSnapshot, now: u64, button: usize) {
    let point = s.toolbar_layout().player_rects(0.0).buttons[button].centre();
    click(s, snap, now, point);
}

#[test]
fn the_player_button_opens_its_popup_and_a_click_outside_or_escape_closes_it() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    open_player(&mut s, &mut snap, LATER);
    click(&mut s, &mut snap, 2 * LATER, [5.0, 700.0]);
    assert!(snap.toolbar.player.is_none());
    open_player(&mut s, &mut snap, 3 * LATER);
    s.tick(4 * LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert!(snap.toolbar.player.is_none());
    // Escape closed the popup only: the menu is still there.
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
    // The button again closes it.
    open_player(&mut s, &mut snap, 5 * LATER);
    let point = s.toolbar_layout().player.centre();
    click(&mut s, &mut snap, 6 * LATER, point);
    assert!(snap.toolbar.player.is_none());
}

#[test]
fn the_player_and_a_setting_popup_do_not_show_together() {
    let mut s = state(&[MUSIC]);
    let mut snap = ShellSnapshot::default();
    click_pin(&mut s, &mut snap, LATER, 0);
    assert!(snap.toolbar.popup.is_some());
    open_player(&mut s, &mut snap, 2 * LATER);
    assert!(snap.toolbar.popup.is_none());
    click_pin(&mut s, &mut snap, 3 * LATER, 0);
    assert!(snap.toolbar.popup.is_some() && snap.toolbar.player.is_none());
}

#[test]
fn the_players_buttons_and_seek_reach_the_menu_music_once() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    open_player(&mut s, &mut snap, LATER);
    assert_eq!(s.take_music_commands(), MusicCommands::default());
    click_player_button(&mut s, &mut snap, 2 * LATER, 2);
    assert_eq!(
        s.take_music_commands(),
        MusicCommands {
            next: true,
            ..MusicCommands::default()
        }
    );
    assert_eq!(s.take_music_commands(), MusicCommands::default());
    click_player_button(&mut s, &mut snap, 3 * LATER, 0);
    assert!(s.take_music_commands().previous);
    click_player_button(&mut s, &mut snap, 4 * LATER, 1);
    assert!(s.take_music_commands().play_pause);
    assert!(snap.toolbar.player.is_some(), "the buttons keep it open");
    // A drag along the bar keeps its latest position.
    let rects = s.toolbar_layout().player_rects(0.0);
    let y = rects.bar.y + rects.bar.height / 2.0;
    let x_at = |share: f32| rects.bar.x + rects.bar.width * share;
    s.tick(
        5 * LATER,
        &[
            at(x_at(0.2), y),
            button(MouseButton::Left, true),
            at(x_at(0.5), y),
            at(x_at(0.75), y),
            button(MouseButton::Left, false),
            at(x_at(0.9), y),
        ],
        &mut snap,
    );
    let seek = s.take_music_commands().seek.expect("a seek");
    assert!((seek - 0.75).abs() < 1e-3, "{seek}");
}

#[test]
fn on_the_song_select_previous_and_next_move_the_selection_by_set() {
    let (mut s, mut snap, _producer) = super::super::tests::in_song_select();
    let t = 4 * LATER;
    s.tick(t, &[], &mut snap);
    assert_eq!(s.song_select().selected(), Some((0, 0)));
    open_player(&mut s, &mut snap, t + LATER);
    click_player_button(&mut s, &mut snap, t + 2 * LATER, 2);
    assert_eq!(s.song_select().selected().map(|(set, _)| set), Some(1));
    click_player_button(&mut s, &mut snap, t + 3 * LATER, 0);
    assert_eq!(s.song_select().selected().map(|(set, _)| set), Some(0));
    // The music is not asked to change track itself: the selection does that.
    let commands = s.take_music_commands();
    assert!(!commands.next && !commands.previous);
    // Play/pause and seek still go to the music.
    click_player_button(&mut s, &mut snap, t + 4 * LATER, 1);
    assert!(s.take_music_commands().play_pause);
}

#[test]
fn the_player_shows_the_title_and_artist_of_the_playing_set() {
    let (mut s, mut snap, _producer) = super::super::tests::in_song_select();
    s.set_menu_track(Some((1, 0)));
    s.tick(5 * LATER, &[], &mut snap);
    assert_eq!(&*snap.now_playing.title, "b");
    assert_eq!(&*snap.now_playing.artist, "b");
    let key = snap.now_playing.key;
    s.set_menu_track(Some((0, 1)));
    s.tick(6 * LATER, &[], &mut snap);
    assert_eq!(&*snap.now_playing.title, "a");
    assert_ne!(snap.now_playing.key, key);
    // The same set again keeps the same strings, without a new allocation.
    let title = Arc::clone(&snap.now_playing.title);
    s.tick(7 * LATER, &[], &mut snap);
    assert!(Arc::ptr_eq(&title, &snap.now_playing.title));
    s.set_menu_track(None);
    s.tick(8 * LATER, &[], &mut snap);
    assert!(snap.now_playing.title.is_empty());
}

#[test]
fn the_playing_state_reaches_the_frame() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    s.set_now_playing(true, true, Some(12_000.0), Some(60_000.0));
    s.tick(LATER, &[], &mut snap);
    let now = snap.toolbar.now_playing;
    assert!(now.available && now.paused);
    assert_eq!((now.position_ms, now.length_ms), (12_000.0, 60_000.0));
    assert_eq!(now.progress(), 0.2);
    s.set_now_playing(false, false, None, None);
    s.tick(2 * LATER, &[], &mut snap);
    assert_eq!(snap.toolbar.now_playing, Default::default());
}

#[test]
fn leaving_for_a_play_closes_the_players_popup() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    open_player(&mut s, &mut snap, LATER);
    s.stack.navigate(Nav::Push(AppScreen::Play(1)), 0);
    s.tick(2 * LATER, &[], &mut snap);
    assert!(snap.toolbar.player.is_none());
}

#[test]
fn the_song_select_makes_room_for_the_bar() {
    let s = state(&[]);
    let bar = s.toolbar_layout().height();
    assert!(s.song_layout.header.y >= bar);
}

#[test]
fn the_toolbar_leaves_with_the_screen_into_a_play() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[], &mut snap);
    s.tick(1, &[key(KEY_ENTER)], &mut snap);
    s.tick(LATER, &[], &mut snap);
    assert_eq!(snap.toolbar.slide, 1.0);
    assert!(!snap.toolbar.follows);
    let start = 2 * LATER;
    s.stack.navigate(Nav::Push(AppScreen::Play(1)), start);
    // Half-way the leaving screen is half gone, and so is the bar: it follows the pose.
    s.tick(start + TRANSITION_NS / 2, &[], &mut snap);
    assert!(snap.toolbar.follows);
    assert!(snap.toolbar.visible);
    let mid = snap.toolbar.slide;
    assert!(mid > 0.0 && mid < 1.0, "{mid}");
    let early = {
        s.tick(start + TRANSITION_NS / 8, &[], &mut snap);
        snap.toolbar.slide
    };
    assert!(
        early > mid,
        "{early} then {mid}: it goes out with the screen"
    );
    // At the end it is hidden and the own timer takes over.
    s.tick(start + TRANSITION_NS, &[], &mut snap);
    s.tick(start + 2 * TRANSITION_NS, &[], &mut snap);
    assert!(!snap.toolbar.follows);
    assert!(!snap.toolbar.visible);
}

#[test]
fn the_toolbar_stays_between_the_menus_during_their_transition() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[], &mut snap);
    s.tick(LATER, &[], &mut snap);
    s.stack
        .navigate(Nav::Push(AppScreen::SongSelect), 2 * LATER);
    s.tick(2 * LATER + TRANSITION_NS / 3, &[], &mut snap);
    assert!(snap.toolbar.follows);
    assert!(
        (snap.toolbar.slide - 1.0).abs() < 1e-3,
        "{}",
        snap.toolbar.slide
    );
}

#[test]
fn the_toolbar_comes_in_with_the_screen_leaving_a_play() {
    let mut s = state(&[]);
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[], &mut snap);
    s.stack.navigate(Nav::Push(AppScreen::Play(1)), 0);
    s.tick(LATER, &[], &mut snap);
    s.tick(2 * LATER, &[], &mut snap);
    assert!(!snap.toolbar.visible);
    let start = 3 * LATER;
    s.stack.navigate(Nav::Pop, start);
    s.tick(start + TRANSITION_NS / 4, &[], &mut snap);
    let early = snap.toolbar.slide;
    s.tick(start + 3 * TRANSITION_NS / 4, &[], &mut snap);
    assert!(snap.toolbar.follows);
    assert!(snap.toolbar.slide > early, "it arrives with the screen");
}
