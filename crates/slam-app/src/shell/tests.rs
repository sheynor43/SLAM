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
    assert!(matches!(snap.layers[0].unwrap().view, ScreenView::Menu(_)));
    assert_eq!(snap.layers[1].unwrap().view, ScreenView::SongSelect);
    // At rest: only the placeholder.
    s.tick(10 + TRANSITION_NS, &[], &mut snap);
    assert_eq!(layers(&snap), 1);
    assert_eq!(snap.layers[0].unwrap().pose, Pose::SHOWN);

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
