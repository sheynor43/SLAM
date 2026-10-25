//! The settings screen in the shell: opening, editing, committing and saving.

use slam_engine::limiter::LimiterMode;
use slam_osu::replay::Actions;
use slam_play::Bindings;
use slam_play::input::Input;
use slam_ui::screen::TRANSITION_NS;
use slam_ui::settings_screen::{KeyValue, MouseKey, RowWidget};

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

/// Opens the settings from the menu and lets the transition end.
fn opened(s: &mut ShellState, snap: &mut ShellSnapshot) {
    s.tick(0, &[key(KEY_DOWN), key(KEY_ENTER)], snap);
    assert!(matches!(s.stack().top(), AppScreen::Settings(_)));
    s.tick(TRANSITION_NS, &[], snap);
}

const LATER: u64 = 10 * TRANSITION_NS;

#[test]
fn the_menu_opens_the_settings_and_escape_closes_them() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[key(KEY_DOWN), key(KEY_ENTER)], &mut snap);
    assert!(matches!(
        snap.layers[1].as_ref().unwrap().view,
        ScreenView::Settings(_)
    ));
    s.tick(TRANSITION_NS, &[], &mut snap);
    assert_eq!(snap.layers.iter().flatten().count(), 1);
    s.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
}

#[test]
fn closing_without_a_change_saves_nothing() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    opened(&mut s, &mut snap);
    s.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert!(s.take_save_request().is_none());
    assert_eq!(*s.settings(), Settings::default());
}

#[test]
fn edits_are_committed_and_saved_only_when_the_screen_closes() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    opened(&mut s, &mut snap);
    // The first row is the master volume, 100 %.
    s.tick(LATER, &[key(KEY_LEFT), key(KEY_LEFT)], &mut snap);
    assert_eq!(s.settings().audio.master_volume, 1.0);
    assert!(s.take_save_request().is_none());
    s.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert_eq!(s.settings().audio.master_volume, 0.98);
    let saved = s.take_save_request().expect("a save is asked for");
    assert_eq!(saved.audio.master_volume, 0.98);
    assert_eq!(*saved, *s.settings());
    assert!(s.take_save_request().is_none());
}

#[test]
fn the_frame_section_changes_the_draw_rates() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    opened(&mut s, &mut snap);
    // Audio, offset, keys, frame rates.
    let tabs = [key(KEY_TAB); 3];
    s.tick(LATER, &tabs, &mut snap);
    // Update rate, gameplay rate: the next option after 1000 Hz is 2000 Hz.
    s.tick(LATER, &[key(KEY_DOWN), key(KEY_RIGHT)], &mut snap);
    s.tick(LATER, &[key(KEY_DOWN), key(KEY_LEFT)], &mut snap);
    assert_eq!(s.settings().frame.draw, Settings::default().frame.draw);
    s.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    let frame = &s.settings().frame;
    assert_eq!(frame.draw, DrawRate::Hz(2000.0));
    assert_eq!(frame.menu_draw, DrawRate::Hz(360.0));
    assert_eq!(frame.update_hz, 1000.0);
}

#[test]
fn escape_closes_an_open_list_before_the_screen() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    opened(&mut s, &mut snap);
    s.tick(
        LATER,
        &[key(KEY_TAB), key(KEY_TAB), key(KEY_TAB)],
        &mut snap,
    );
    s.tick(LATER, &[key(KEY_ENTER)], &mut snap);
    s.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Settings(_)));
    s.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
}

#[test]
fn a_released_primary_button_ends_a_slider_drag() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    opened(&mut s, &mut snap);
    let layout = s.settings_layout;
    let widget = slam_ui::widgets::SliderLayout::new(layout.widget(0), &s.theme, layout.scale);
    let (y, x) = (
        widget.hit.y + 2.0,
        widget.track.x + widget.track.width * 0.5,
    );
    let button = |down| InputEvent {
        time_ns: 0,
        kind: InputKind::MouseButton {
            button: MouseButton::Left,
            down,
        },
    };
    let at = |x: f32| InputEvent {
        time_ns: 0,
        kind: InputKind::MouseMove { x, y },
    };
    // Press at the middle, drag to the left end, release, move again.
    s.tick(LATER, &[at(x), button(true), at(widget.track.x)], &mut snap);
    s.tick(LATER, &[button(false), at(x)], &mut snap);
    s.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert_eq!(s.settings().audio.master_volume, 0.0);
}

#[test]
fn gameplay_is_visible_while_a_play_is_on_a_layer() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    assert!(!s.gameplay_visible(0));
    s.tick(0, &[key(KEY_ENTER)], &mut snap);
    s.finish_loading(TRANSITION_NS, Some(7));
    assert!(s.playing());
    assert!(s.gameplay_visible(TRANSITION_NS));
    s.tick(2 * TRANSITION_NS, &[], &mut snap);
    assert!(s.gameplay_visible(2 * TRANSITION_NS));
    s.leave_play(3 * TRANSITION_NS, None);
    // Still leaving.
    assert!(s.gameplay_visible(3 * TRANSITION_NS));
    s.tick(5 * TRANSITION_NS, &[], &mut snap);
    assert!(!s.gameplay_visible(5 * TRANSITION_NS));
}

#[test]
fn a_shell_started_with_settings_offers_their_custom_rates() {
    let mut settings = Settings::default();
    settings.frame.menu_draw = DrawRate::Hz(75.0);
    let s = state().with_settings(settings);
    let specs = s.rows().specs(s.settings());
    let RowKind::Dropdown { options, selected } = specs[Row::MenuDrawHz as usize].kind else {
        panic!("a dropdown");
    };
    // Unlimited, the nine presets and 75 Hz in its place.
    assert_eq!(options, 11);
    assert_eq!(selected, 2);
}

use crate::settings::DrawRate;
use slam_ui::settings_screen::RowKind;

fn shell_with(link: crate::settings::saver::SaveLink) -> Shell {
    Shell {
        state: state(),
        host: None,
        surface: Arc::new(AtomicU64::new(0)),
        layout_size: 0,
        scale: slam_input::WindowScale::default(),
        pixels_per_point: 1.0,
        exited: Arc::new(AtomicBool::new(false)),
        draw_rate: DrawRateControl::new(),
        rate_switch: DrawRateSwitch::new(LimiterMode::Hz(1000.0)),
        update_rate: AppliedUpdateRate::new(UpdateRateControl::new(), 1000.0),
        saver: Some(link),
        text_input: slam_input::TextInput::default(),
        mouse: slam_input::MouseSettings::default(),
        window: slam_input::WindowSettings::default(),
        window_sent: Settings::default().window.state(),
    }
}

/// What the window does on Alt+Enter or F11: moves its own wish to the next mode.
fn switch_window(shell: &Shell) -> WindowState {
    let mut state = shell.window.get();
    state.mode = state.mode.next();
    shell.window.set(state);
    state
}

#[test]
fn a_window_mode_switch_is_saved_and_not_sent_back() {
    use slam_input::WindowMode;
    let (link, mut taken) = crate::settings::saver::detached(1);
    let mut shell = shell_with(link);
    shell.apply_settings(0);
    // Nothing changed: the window keeps what it was opened with, nothing is saved.
    assert_eq!(shell.window.get(), WindowState::default());
    assert!(taken.pop().is_err());
    // Its report may be lost to a full ring: the wish word alone tells the shell.
    let switched = switch_window(&shell);
    shell.apply_settings(LATER);
    assert_eq!(shell.state.settings().window.mode, switched.mode);
    assert_eq!(shell.window.get(), switched);
    assert_eq!(taken.pop().unwrap().window.mode, switched.mode);
    // A change in the settings reaches the window.
    let mut settings = shell.state.settings().clone();
    settings.window.mode = WindowMode::Windowed;
    settings.window.windowed_size = [800, 600];
    shell.state = state().with_settings(settings);
    shell.apply_settings(LATER);
    assert_eq!(shell.window.get().mode, WindowMode::Windowed);
    assert_eq!(shell.window.get().windowed_size, [800, 600]);
}

#[test]
fn a_settings_change_racing_a_switch_loses_to_it() {
    use slam_input::WindowMode;
    let (link, mut taken) = crate::settings::saver::detached(1);
    let mut shell = shell_with(link);
    shell.apply_settings(0);
    // The window switches while the settings ask for another mode in the same tick.
    let switched = switch_window(&shell);
    let mut settings = shell.state.settings().clone();
    settings.window.mode = WindowMode::Borderless;
    assert_ne!(switched.mode, WindowMode::Borderless);
    shell.state = state().with_settings(settings);
    shell.apply_settings(LATER);
    // The window and the settings agree on the player's switch.
    assert_eq!(shell.window.get(), switched);
    assert_eq!(shell.state.settings().window.mode, switched.mode);
    assert_eq!(taken.pop().unwrap().window.mode, switched.mode);
    shell.apply_settings(LATER);
    assert_eq!(shell.window.get(), switched);
}

#[test]
fn window_mode_events_do_not_reach_the_screens() {
    let mut s = state();
    let mode = s.settings().window.mode.next();
    let mut snap = ShellSnapshot::default();
    s.tick(
        LATER,
        &[InputEvent {
            time_ns: 0,
            kind: InputKind::WindowMode(mode),
        }],
        &mut snap,
    );
    // The shell takes the mode from the window's wish, not from the event.
    assert_ne!(s.settings().window.mode, mode);
    assert!(s.take_save_request().is_none());
}

#[test]
fn a_new_window_scale_remaps_the_pointer() {
    let (link, _taken) = crate::settings::saver::detached(1);
    let mut shell = shell_with(link);
    shell
        .surface
        .store(crate::play::pack_size(2560, 1440), Ordering::Relaxed);
    shell.follow_surface();
    assert_eq!(shell.state.pixels_per_point, 1.0);
    // Same surface, the window moved to a 2× display.
    shell.scale = slam_input::WindowScale::fixed(2.0);
    shell.follow_surface();
    assert_eq!(shell.state.pixels_per_point, 2.0);
    shell.state.follow_pointer(&[InputEvent {
        time_ns: 0,
        kind: InputKind::MouseMove { x: 100.0, y: 50.0 },
    }]);
    assert_eq!(shell.state.cursor, [200.0, 100.0]);
}

#[test]
fn a_committed_update_rate_reaches_the_engine() {
    let (link, _taken) = crate::settings::saver::detached(1);
    let mut shell = shell_with(link);
    let control = UpdateRateControl::new();
    shell.update_rate = AppliedUpdateRate::new(control.clone(), 1000.0);
    let mut snap = ShellSnapshot::default();
    opened(&mut shell.state, &mut snap);
    // Audio, offset, keys, frame rates; the update rate is the first row.
    let tabs = [key(KEY_TAB); 3];
    shell.state.tick(LATER, &tabs, &mut snap);
    shell.state.tick(LATER, &[key(KEY_RIGHT)], &mut snap);
    // Not before the screen closes.
    shell.apply_settings(LATER);
    assert_eq!(control.hz(), 1000.0);
    shell.state.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    shell.apply_settings(LATER);
    assert_eq!(control.hz(), 2000.0);
}

#[test]
fn a_save_the_full_ring_refused_is_retried_on_the_next_tick() {
    let (link, mut taken) = crate::settings::saver::detached(1);
    let mut shell = shell_with(link);
    let mut first = Settings::default();
    first.audio.master_volume = 0.5;
    // Fill the ring, then ask for another save.
    shell
        .saver
        .as_mut()
        .unwrap()
        .request(Box::new(first))
        .unwrap();
    let mut second = Settings::default();
    second.audio.master_volume = 0.25;
    shell.state.requeue_save_request(Box::new(second));
    shell.apply_settings(0);
    // Refused and kept.
    assert_eq!(taken.slots(), 1);
    assert!(shell.state.save_request.is_some());
    assert_eq!(taken.pop().unwrap().audio.master_volume, 0.5);
    shell.apply_settings(1);
    assert!(shell.state.save_request.is_none());
    assert_eq!(taken.pop().unwrap().audio.master_volume, 0.25);
}

fn mouse(button: MouseButton, down: bool) -> InputEvent {
    InputEvent {
        time_ns: 0,
        kind: InputKind::MouseButton { button, down },
    }
}

fn move_to(x: f32, y: f32) -> InputEvent {
    InputEvent {
        time_ns: 0,
        kind: InputKind::MouseMove { x, y },
    }
}

/// Opens the settings on the keys section, focusing the first key row (left, slot 1), past
/// the mouse toggles and the sensitivity.
fn on_keys(s: &mut ShellState, snap: &mut ShellSnapshot) {
    opened(s, snap);
    s.tick(
        LATER,
        &[
            key(KEY_TAB),
            key(KEY_TAB),
            key(KEY_DOWN),
            key(KEY_DOWN),
            key(KEY_DOWN),
        ],
        snap,
    );
    assert_eq!(settings_screen(s).focus(), Row::LeftKey1 as usize);
}

fn settings_screen(s: &ShellState) -> &SettingsScreen {
    match s.stack().top() {
        AppScreen::Settings(screen) => screen,
        other => panic!("not the settings: {other:?}"),
    }
}

fn edited(s: &ShellState) -> &Settings {
    s.editing.as_ref().expect("the screen is open")
}

const KEY_Q: u16 = 20;
const KEY_X: u16 = 27;
const KEY_Z: u16 = 29;

#[test]
fn a_key_the_menus_do_not_know_is_captured_and_saved_on_close() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    on_keys(&mut s, &mut snap);
    s.tick(LATER, &[key(KEY_ENTER)], &mut snap);
    assert!(settings_screen(&s).is_listening());
    s.tick(LATER, &[key(KEY_Q)], &mut snap);
    assert!(!settings_screen(&s).is_listening());
    assert_eq!(
        edited(&s).input.slot(Actions::LEFT, 0),
        Some(Input::Key(KEY_Q))
    );
    // Z was this slot's key and is gone; the settings in force wait for the close.
    assert_eq!(
        s.settings().input.slot(Actions::LEFT, 0),
        Some(Input::Key(KEY_Z))
    );
    s.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
    assert_eq!(
        s.settings().input.slot(Actions::LEFT, 0),
        Some(Input::Key(KEY_Q))
    );
    let saved = s.take_save_request().expect("a save is asked for");
    assert_eq!(saved.input.slot(Actions::LEFT, 0), Some(Input::Key(KEY_Q)));
}

#[test]
fn the_screen_shows_the_new_binding() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    on_keys(&mut s, &mut snap);
    s.tick(LATER, &[key(KEY_ENTER), key(KEY_Q)], &mut snap);
    let Some(RowWidget::Key(row)) = settings_screen(&s).widget(Row::LeftKey1 as usize) else {
        panic!("a key row");
    };
    assert_eq!(row.value(), KeyValue::Key(KEY_Q));
}

#[test]
fn escape_while_listening_cancels_and_keeps_the_screen_open() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    on_keys(&mut s, &mut snap);
    s.tick(LATER, &[key(KEY_ENTER), key(KEY_ESCAPE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Settings(_)));
    assert!(!settings_screen(&s).is_listening());
    assert_eq!(*edited(&s), Settings::default());
    // Now it closes.
    s.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
    assert!(s.take_save_request().is_none());
}

#[test]
fn delete_while_listening_clears_the_slot() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    on_keys(&mut s, &mut snap);
    s.tick(LATER, &[key(KEY_ENTER), key(KEY_DELETE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Settings(_)));
    // The mouse button moved up into the first slot.
    assert_eq!(
        edited(&s).input.slot(Actions::LEFT, 0),
        Some(Input::Mouse(MouseButton::Left))
    );
    assert_eq!(edited(&s).input.slot(Actions::LEFT, 1), None);
    let Some(RowWidget::Key(row)) = settings_screen(&s).widget(Row::LeftKey2 as usize) else {
        panic!("a key row");
    };
    assert_eq!(row.value(), KeyValue::Empty);
}

#[test]
fn a_click_starts_listening_and_the_next_press_of_any_button_is_captured() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    on_keys(&mut s, &mut snap);
    let layout = s.settings_layout;
    let widget = layout.widget(layout_slot(Row::LeftKey2));
    let (x, y) = (widget.x + 4.0, widget.y + 4.0);
    s.tick(
        LATER,
        &[
            move_to(x, y),
            mouse(MouseButton::Left, true),
            mouse(MouseButton::Left, false),
        ],
        &mut snap,
    );
    // The click itself was not taken as the binding.
    assert!(settings_screen(&s).is_listening());
    assert_eq!(*edited(&s), Settings::default());
    s.tick(LATER, &[mouse(MouseButton::X1, true)], &mut snap);
    assert!(!settings_screen(&s).is_listening());
    assert!(matches!(s.stack().top(), AppScreen::Settings(_)));
    assert_eq!(
        edited(&s).input.slot(Actions::LEFT, 1),
        Some(Input::Mouse(MouseButton::X1))
    );
    // The primary button counts too.
    s.tick(LATER, &[key(KEY_DOWN), key(KEY_ENTER)], &mut snap);
    s.tick(LATER, &[mouse(MouseButton::Middle, true)], &mut snap);
    assert_eq!(
        edited(&s).input.slot(Actions::RIGHT, 0),
        Some(Input::Mouse(MouseButton::Middle))
    );
}

/// The position of `row` in its section.
fn layout_slot(row: Row) -> usize {
    // The keys section starts with the mouse toggle.
    row as usize - Row::MouseButtons as usize
}

#[test]
fn a_taken_key_asks_and_keeping_changes_nothing() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    on_keys(&mut s, &mut snap);
    s.tick(LATER, &[key(KEY_ENTER), key(KEY_X)], &mut snap);
    let conflict = settings_screen(&s).conflict().copied().expect("asked");
    assert_eq!(conflict.row, Row::LeftKey1 as usize);
    assert_eq!(conflict.key, KeyValue::Key(KEY_X));
    assert_eq!(conflict.other, Some(Row::RightKey1 as usize));
    assert_eq!(*edited(&s), Settings::default());
    // Escape keeps and does not close.
    s.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert!(matches!(s.stack().top(), AppScreen::Settings(_)));
    assert!(settings_screen(&s).conflict().is_none());
    assert_eq!(*edited(&s), Settings::default());
    // Activate keeps by default.
    s.tick(
        LATER,
        &[key(KEY_ENTER), key(KEY_X), key(KEY_ENTER)],
        &mut snap,
    );
    assert!(settings_screen(&s).conflict().is_none());
    assert_eq!(*edited(&s), Settings::default());
}

#[test]
fn applying_a_taken_key_moves_it() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    on_keys(&mut s, &mut snap);
    s.tick(
        LATER,
        &[key(KEY_ENTER), key(KEY_X), key(KEY_RIGHT), key(KEY_ENTER)],
        &mut snap,
    );
    assert!(settings_screen(&s).conflict().is_none());
    let input = &edited(&s).input;
    assert_eq!(input.slot(Actions::LEFT, 0), Some(Input::Key(KEY_X)));
    // X left the right button; its mouse button moved up.
    assert_eq!(
        input.slot(Actions::RIGHT, 0),
        Some(Input::Mouse(MouseButton::Right))
    );
    assert_eq!(input.slot(Actions::RIGHT, 1), None);
    let Some(RowWidget::Key(row)) = settings_screen(&s).widget(Row::RightKey2 as usize) else {
        panic!("a key row");
    };
    assert_eq!(row.value(), KeyValue::Empty);
    let Some(RowWidget::Key(row)) = settings_screen(&s).widget(Row::RightKey1 as usize) else {
        panic!("a key row");
    };
    assert_eq!(row.value(), KeyValue::Mouse(MouseKey::Right));
}

#[test]
fn the_reset_button_restores_the_default_bindings() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    on_keys(&mut s, &mut snap);
    s.tick(LATER, &[key(KEY_ENTER), key(KEY_Q)], &mut snap);
    assert_ne!(edited(&s).input.bindings, Bindings::default());
    let downs = [key(KEY_DOWN); Row::ResetKeys as usize - Row::LeftKey1 as usize];
    s.tick(LATER, &downs, &mut snap);
    s.tick(LATER, &[key(KEY_ENTER)], &mut snap);
    assert_eq!(edited(&s).input.bindings, Bindings::default());
    let Some(RowWidget::Key(row)) = settings_screen(&s).widget(Row::LeftKey1 as usize) else {
        panic!("a key row");
    };
    assert_eq!(row.value(), KeyValue::Key(KEY_Z));
    // Back to the starting bindings: nothing to save.
    s.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert!(s.take_save_request().is_none());
}

const KEY_F12: u16 = 69;

fn with_fps_hotkey() -> ShellState {
    let mut s = state();
    s.settings.hotkeys.toggle_fps = Some(Input::Key(KEY_F12));
    s
}

#[test]
fn the_fps_hotkey_toggles_the_counter_on_the_menu_and_saves() {
    let mut s = with_fps_hotkey();
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[], &mut snap);
    assert!(!snap.fps.show);
    s.tick(LATER, &[key(KEY_F12)], &mut snap);
    assert!(snap.fps.show);
    assert!(matches!(s.stack().top(), AppScreen::Menu(_)));
    let saved = s.take_save_request().expect("a save is asked for");
    assert!(saved.frame.show_fps);
    s.tick(LATER, &[key(KEY_F12)], &mut snap);
    assert!(!snap.fps.show);
    assert!(!s.settings().frame.show_fps);
}

#[test]
fn without_a_hotkey_nothing_toggles() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    s.tick(0, &[key(KEY_F12)], &mut snap);
    assert!(!snap.fps.show);
    assert!(s.take_save_request().is_none());
}

#[test]
fn the_fps_hotkey_is_watched_while_the_play_has_the_input() {
    let mut s = with_fps_hotkey();
    s.watch_hotkeys(&[key(KEY_Q), key(KEY_F12)]);
    assert!(s.settings().frame.show_fps);
    assert!(s.take_save_request().is_some());
}

#[test]
fn the_snapshot_follows_the_edited_fps_settings() {
    let mut s = with_fps_hotkey();
    let mut snap = ShellSnapshot::default();
    opened(&mut s, &mut snap);
    s.editing.as_mut().unwrap().frame.fps_corner = slam_play::FpsCorner::TopLeft;
    s.editing.as_mut().unwrap().frame.fps_screens = crate::settings::FpsScreens::Menus;
    s.tick(LATER, &[], &mut snap);
    assert_eq!(snap.fps.corner, slam_play::FpsCorner::TopLeft);
    assert_eq!(snap.fps.screens, crate::settings::FpsScreens::Menus);
    // The hotkey changes the setting in force, the edited one and the row.
    s.tick(LATER, &[key(KEY_F12)], &mut snap);
    assert!(snap.fps.show);
    assert!(s.settings().frame.show_fps);
    assert!(edited(&s).frame.show_fps);
    let Some(RowWidget::Toggle(row)) = settings_screen(&s).widget(Row::ShowFps as usize) else {
        panic!("a toggle row");
    };
    assert!(row.value());
}

#[test]
fn the_fps_hotkey_row_captures_a_key_and_a_gameplay_key_asks() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    on_keys(&mut s, &mut snap);
    let downs = [key(KEY_DOWN); Row::ToggleFpsKey as usize - Row::LeftKey1 as usize];
    s.tick(LATER, &downs, &mut snap);
    assert_eq!(settings_screen(&s).focus(), Row::ToggleFpsKey as usize);
    s.tick(LATER, &[key(KEY_ENTER), key(KEY_F12)], &mut snap);
    assert_eq!(edited(&s).hotkeys.toggle_fps, Some(Input::Key(KEY_F12)));
    // Captured, not toggled: the hotkey in force is still unbound.
    assert!(!edited(&s).frame.show_fps);
    // Space skips in gameplay and a left click works the menus: refused, F11 stays.
    s.tick(LATER, &[key(KEY_ENTER), key(KEY_SPACE)], &mut snap);
    assert_eq!(edited(&s).hotkeys.toggle_fps, Some(Input::Key(KEY_F12)));
    let Some(RowWidget::Key(row)) = settings_screen(&s).widget(Row::ToggleFpsKey as usize) else {
        panic!("a key row");
    };
    assert_eq!(row.value(), KeyValue::Key(KEY_F12));
    s.tick(
        LATER,
        &[key(KEY_ENTER), mouse(MouseButton::Left, true)],
        &mut snap,
    );
    assert_eq!(edited(&s).hotkeys.toggle_fps, Some(Input::Key(KEY_F12)));
    assert!(settings_screen(&s).conflict().is_none());
    // Z is the left key: the screen asks before taking it.
    s.tick(LATER, &[key(KEY_ENTER), key(KEY_Z)], &mut snap);
    assert_eq!(
        settings_screen(&s).conflict().map(|c| c.other),
        Some(Some(Row::LeftKey1 as usize))
    );
    assert_eq!(edited(&s).hotkeys.toggle_fps, Some(Input::Key(KEY_F12)));
}

fn with_folder_commands() -> (
    ShellState,
    rtrb::Producer<crate::songs::Published>,
    rtrb::Consumer<crate::songs::Command>,
) {
    let (producer, link, commands) = crate::songs::link_with_commands();
    (state().with_songs(link), producer, commands)
}

fn published_folder(folder: &str) -> crate::songs::Published {
    let mut published = crate::songs::publish(&[], SongSelectStatus::Ready, Arc::from(""));
    published.folder = Arc::from(folder);
    published
}

#[test]
fn the_song_select_button_asks_for_the_folder_picker() {
    use crate::songs::Command;
    let (mut s, mut producer, mut commands) = with_folder_commands();
    let mut snap = ShellSnapshot::default();
    producer
        .push(published_folder(""))
        .unwrap_or_else(|_| panic!("ring"));
    s.tick(0, &[key(KEY_ENTER)], &mut snap);
    s.tick(TRANSITION_NS, &[], &mut snap);
    assert!(commands.pop().is_err());
    let b = s.song_layout.choose_folder;
    let click = [
        move_to(b.x + b.width / 2.0, b.y + b.height / 2.0),
        mouse(MouseButton::Left, true),
    ];
    s.tick(LATER, &click, &mut snap);
    assert_eq!(commands.pop(), Ok(Command::PickFolder));
    // Enter does the same on an empty list.
    s.tick(LATER, &[key(KEY_ENTER)], &mut snap);
    assert_eq!(commands.pop(), Ok(Command::PickFolder));
    assert!(commands.pop().is_err());
}

#[test]
fn the_songs_folder_row_shows_the_folder_and_opens_the_picker() {
    use crate::songs::Command;
    let (mut s, mut producer, mut commands) = with_folder_commands();
    let mut snap = ShellSnapshot::default();
    producer
        .push(published_folder("/games/osu/Songs"))
        .unwrap_or_else(|_| panic!("ring"));
    opened(&mut s, &mut snap);
    // The folder arrived while the screen was open.
    s.tick(LATER, &[], &mut snap);
    assert_eq!(settings_screen(&s).value_text(), "/games/osu/Songs");
    let ScreenView::Settings(view) = &snap.layers[0].as_ref().unwrap().view else {
        panic!("settings");
    };
    assert_eq!(&*view.value_text, "/games/osu/Songs");
    // The library section is the last; its only row is the focused one.
    let tabs = [key(KEY_TAB); SECTIONS - 1];
    s.tick(LATER, &tabs, &mut snap);
    assert_eq!(settings_screen(&s).focus(), Row::SongsFolder as usize);
    s.tick(LATER, &[key(KEY_ENTER)], &mut snap);
    assert_eq!(commands.pop(), Ok(Command::PickFolder));
    // A new folder replaces the text; an unchanged one keeps the same `Arc`.
    producer
        .push(published_folder("/other"))
        .unwrap_or_else(|_| panic!("ring"));
    s.tick(LATER, &[], &mut snap);
    assert_eq!(settings_screen(&s).value_text(), "/other");
    let kept = Arc::clone(&s.songs_folder);
    producer
        .push(published_folder("/other"))
        .unwrap_or_else(|_| panic!("ring"));
    s.tick(LATER, &[], &mut snap);
    assert!(Arc::ptr_eq(&kept, &s.songs_folder));
    // Opened again later, the screen starts with the folder.
    s.tick(2 * LATER, &[key(KEY_ESCAPE)], &mut snap);
    s.tick(3 * LATER, &[key(KEY_ENTER)], &mut snap);
    assert_eq!(settings_screen(&s).value_text(), "/other");
}

#[test]
fn a_new_folder_brings_its_skins_and_survives_closing_the_screen() {
    use crate::songs::{Command, FolderChange};
    let (mut s, mut producer, mut commands) = with_folder_commands();
    let mut snap = ShellSnapshot::default();
    opened(&mut s, &mut snap);
    let mut published = published_folder("/picked/Songs");
    published.change = Some(Box::new(FolderChange {
        songs: Some("/picked/Songs".into()),
        skins: vec!["Alpha".to_owned(), "Beta".to_owned()],
    }));
    producer.push(published).unwrap_or_else(|_| panic!("ring"));
    s.tick(LATER, &[], &mut snap);
    assert_eq!(
        s.settings().paths.songs.as_deref(),
        Some(std::path::Path::new("/picked/Songs"))
    );
    // The open screen's skin row lists the new skins (and the default one).
    let Some(RowWidget::Dropdown(skin)) = settings_screen(&s).widget(super::rows::skin_row())
    else {
        panic!("the skin row is a dropdown");
    };
    assert_eq!(skin.count(), 3);
    // The old values go back to the library thread.
    let Ok(Command::Retire(old)) = commands.pop() else {
        panic!("the change is handed back");
    };
    assert_eq!(old.songs, None);
    assert!(old.skins.is_empty());
    // Closing the screen keeps the library's folder.
    s.tick(2 * LATER, &[key(KEY_ESCAPE)], &mut snap);
    assert_eq!(
        s.settings().paths.songs.as_deref(),
        Some(std::path::Path::new("/picked/Songs"))
    );
}

/// Opens the settings on the keys section, focused on the high precision toggle.
fn on_mouse_rows(s: &mut ShellState, snap: &mut ShellSnapshot) -> usize {
    opened(s, snap);
    s.tick(LATER, &[key(KEY_TAB), key(KEY_TAB), key(KEY_DOWN)], snap);
    assert_eq!(settings_screen(s).focus(), Row::HighPrecisionMouse as usize);
    Row::HighPrecisionMouse as usize
}

#[test]
fn the_sensitivity_slider_is_locked_with_a_hint_until_high_precision_is_on() {
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    let toggle = on_mouse_rows(&mut s, &mut snap);
    let slider = Row::Sensitivity as usize;
    let off = !edited(&s).input.high_precision_mouse;
    // Whatever the platform's default is, bring the toggle to off first.
    if !off {
        s.tick(LATER, &[key(KEY_ENTER)], &mut snap);
    }
    assert!(!edited(&s).input.high_precision_mouse);
    assert!(settings_screen(&s).is_disabled(slider));
    assert!(settings_screen(&s).is_hinted(slider));
    assert!(!settings_screen(&s).is_hinted(toggle));
    // The slider does not move while it is locked.
    s.tick(LATER, &[key(KEY_DOWN), key(KEY_RIGHT)], &mut snap);
    assert_eq!(edited(&s).input.sensitivity, 1.0);
    // Turning high precision on unlocks it; the platform warning shows where it applies.
    s.tick(LATER, &[key(KEY_UP), key(KEY_ENTER)], &mut snap);
    assert!(edited(&s).input.high_precision_mouse);
    assert!(!settings_screen(&s).is_disabled(slider));
    assert!(!settings_screen(&s).is_hinted(slider));
    assert_eq!(
        settings_screen(&s).is_hinted(toggle),
        cfg!(any(target_os = "linux", target_os = "macos"))
    );
    s.tick(LATER, &[key(KEY_DOWN), key(KEY_RIGHT)], &mut snap);
    assert_eq!(edited(&s).input.sensitivity, 1.01);
}

#[test]
fn the_screen_opens_with_the_states_of_the_saved_settings() {
    let mut settings = Settings::default();
    settings.input.high_precision_mouse = true;
    let mut s = state().with_settings(settings);
    let mut snap = ShellSnapshot::default();
    opened(&mut s, &mut snap);
    let slider = Row::Sensitivity as usize;
    assert!(!settings_screen(&s).is_disabled(slider));
    assert!(!settings_screen(&s).is_hinted(slider));
}

#[test]
fn mouse_edits_reach_the_input_thread_at_once() {
    let (link, _taken) = crate::settings::saver::detached(1);
    let mut shell = shell_with(link);
    let mut snap = ShellSnapshot::default();
    let mouse = shell.mouse.clone();
    shell.apply_settings(LATER);
    assert_eq!(mouse.is_relative(), cfg!(target_os = "windows"));
    assert_eq!(mouse.sensitivity(), 1.0);
    on_mouse_rows(&mut shell.state, &mut snap);
    if !edited(&shell.state).input.high_precision_mouse {
        shell.state.tick(LATER, &[key(KEY_ENTER)], &mut snap);
    }
    shell.state.tick(
        LATER,
        &[key(KEY_DOWN), key(KEY_RIGHT), key(KEY_RIGHT)],
        &mut snap,
    );
    // The screen is still open, nothing is committed.
    assert!(shell.state.editing.is_some());
    shell.apply_settings(LATER);
    assert!(mouse.is_relative());
    assert_eq!(mouse.sensitivity(), 1.02);
    // Closing commits the edit; the thread keeps it.
    shell.state.tick(LATER, &[key(KEY_ESCAPE)], &mut snap);
    shell.apply_settings(LATER);
    assert!(mouse.is_relative());
    assert_eq!(mouse.sensitivity(), 1.02);
}

#[test]
fn a_window_mode_switch_reaches_an_open_settings_screen() {
    use slam_input::WindowMode;
    let mut s = state();
    let mut snap = ShellSnapshot::default();
    opened(&mut s, &mut snap);
    // The edited copy differs from the settings in force (as a mode row will allow).
    s.editing.as_mut().unwrap().window.mode = WindowMode::Borderless;
    let applied = s.settings().window.mode;
    s.window_mode_switched(applied);
    assert_eq!(edited(&s).window.mode, applied);
    // Same mode in force: nothing to save.
    assert!(s.take_save_request().is_none());
}
