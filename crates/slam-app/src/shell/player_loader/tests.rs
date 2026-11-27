//! The screen before play in the shell (ADR-0190).

use slam_ui::screen::TRANSITION_NS;

use super::super::tests::in_song_select;
use super::*;

const MS: u64 = 1_000_000;

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

fn delay() -> u64 {
    Theme::default().loader_push_delay_ms as u64 * MS
}

fn out() -> u64 {
    Theme::default().loader_out_ms as u64 * MS
}

/// The song select with the screen before play on, Enter pressed at `TRANSITION_NS`.
fn entered() -> (ShellState, ShellSnapshot, u64) {
    let (mut s, mut snap, producer) = in_song_select();
    // The published list must outlive the shell's first look at it.
    std::mem::forget(producer);
    s.settings.gameplay.player_loader = true;
    let now = TRANSITION_NS + 1;
    s.tick(now, &[key(KEY_ENTER)], &mut snap);
    (s, snap, now)
}

#[test]
fn the_screen_comes_in_with_the_card_and_the_play_is_asked_for() {
    let (mut s, mut snap, now) = entered();
    assert!(matches!(s.stack().top(), AppScreen::Loader(_)));
    assert!(s.is_loading());
    assert_eq!(s.take_play_request(), Some((0, 0)));
    s.tick(now + 1, &[], &mut snap);
    assert_eq!(
        &*snap.loader_card.title,
        &*s.song_select().list().sets[0].title
    );
    assert_eq!(&*snap.loader_card.version, "Easy");
    // The toolbar and the settings stay away.
    assert!(!s.settings_allowed());
    let top = snap.layers.iter().flatten().last().unwrap();
    assert!(matches!(top.view, ScreenView::Loader(_)));
}

#[test]
fn the_play_starts_after_the_delay_once_prepared_through_black() {
    let (mut s, mut snap, now) = entered();
    let depth = s.stack().depth();
    s.take_play_request();
    assert!(!s.play_may_start(now + 1));
    s.set_play_prepared();
    // Not before the delay.
    s.tick(now + delay() - MS, &[], &mut snap);
    assert!(!s.play_may_start(now + delay() - MS));
    // Then the screen goes out, and the play may start once it is gone.
    let leave = now + delay();
    s.tick(leave, &[], &mut snap);
    assert!(!s.play_may_start(leave + out() - 1));
    assert!(s.play_may_start(leave + out()));
    s.finish_loading(leave + out(), Some(1));
    assert_eq!(*s.stack().top(), AppScreen::Play(1));
    assert_eq!(s.stack().depth(), depth, "in the screen's place");
    let fx = s.stack().fx(leave + out() + 1).unwrap();
    assert_eq!(fx.style, slam_ui::screen::TransitionStyle::Fade);
    // It begins from black: the screen before play went there itself.
    assert!(fx.linear >= slam_ui::theme::transition::FADE_BLACK.1 - 1e-3);
}

#[test]
fn the_pointer_on_the_quick_settings_holds_the_play() {
    let (mut s, mut snap, now) = entered();
    s.take_play_request();
    s.set_play_prepared();
    let layout = s.loader_layout().unwrap();
    // The middle of the column is not enough while it is hidden.
    let [x, y] = layout.column().centre();
    s.tick(now + 1, &[at(x, y)], &mut snap);
    let AppScreen::Loader(loader) = s.stack().top() else {
        unreachable!()
    };
    assert!(!loader.holds());
    // The right edge reveals it; then the column holds.
    s.tick(now + 2, &[at(1279.0, y), at(x, y)], &mut snap);
    s.tick(now + 10 * delay(), &[], &mut snap);
    assert!(!s.play_may_start(now + 10 * delay() + out()));
    // Away from it, the screen goes.
    let later = now + 11 * delay();
    s.tick(later, &[at(10.0, 10.0)], &mut snap);
    assert!(s.play_may_start(later + out()));
}

#[test]
fn escape_goes_back_and_drops_the_play() {
    let (mut s, mut snap, now) = entered();
    s.tick(now + 1, &[key(KEY_ESCAPE)], &mut snap);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    assert!(!s.is_loading());
    assert_eq!(s.take_play_request(), None);
}

#[test]
fn a_failed_load_goes_back_to_the_song_select() {
    let (mut s, _snap, now) = entered();
    s.take_play_request();
    s.finish_loading(now + 1, None);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
}

#[test]
fn a_quick_setting_applies_and_the_play_is_prepared_again_with_it() {
    let (mut s, mut snap, now) = entered();
    assert_eq!(s.take_play_request(), Some((0, 0)));
    s.set_play_prepared();
    // The input toggle (mouse buttons) is the last row.
    let layout = s.loader_layout().unwrap();
    let AppScreen::Loader(loader) = s.stack().top() else {
        unreachable!()
    };
    let last = loader.rows().count() - 1;
    let toggle = layout.row(last).widget(0);
    let [x, y] = toggle.centre();
    let before = s.live_settings().gameplay.mouse_buttons;
    s.tick(
        now + 1,
        &[
            at(1279.0, y),
            at(x, y),
            InputEvent {
                time_ns: 0,
                kind: InputKind::MouseButton {
                    button: MouseButton::Left,
                    down: true,
                },
            },
            InputEvent {
                time_ns: 0,
                kind: InputKind::MouseButton {
                    button: MouseButton::Left,
                    down: false,
                },
            },
            at(10.0, 10.0),
        ],
        &mut snap,
    );
    assert_eq!(s.live_settings().gameplay.mouse_buttons, !before);
    // When the screen could go, the play is asked for again and the screen waits.
    s.tick(now + delay(), &[], &mut snap);
    assert_eq!(s.settings().gameplay.mouse_buttons, !before, "committed");
    assert_eq!(s.take_play_request(), Some((0, 0)));
    assert!(!s.play_may_start(now + delay() + out()));
    s.set_play_prepared();
    s.tick(now + delay() + 1, &[], &mut snap);
    assert!(s.play_may_start(now + delay() + 1 + out()));
}

#[test]
fn off_the_play_comes_in_over_the_song_select_at_once() {
    let (mut s, mut snap, _producer) = in_song_select();
    s.settings.gameplay.player_loader = false;
    s.tick(TRANSITION_NS + 1, &[key(KEY_ENTER)], &mut snap);
    assert_eq!(*s.stack().top(), AppScreen::SongSelect);
    assert!(s.play_may_start(TRANSITION_NS + 1));
}
