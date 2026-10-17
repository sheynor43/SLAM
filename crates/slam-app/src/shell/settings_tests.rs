//! The settings screen in the shell: opening, editing, committing and saving.

use slam_engine::limiter::LimiterMode;
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
        points_per_pixel: 1.0,
        exited: Arc::new(AtomicBool::new(false)),
        draw_rate: DrawRateControl::new(),
        rate_switch: DrawRateSwitch::new(LimiterMode::Hz(1000.0)),
        update_rate: AppliedUpdateRate::new(UpdateRateControl::new(), 1000.0),
        saver: Some(link),
    }
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
