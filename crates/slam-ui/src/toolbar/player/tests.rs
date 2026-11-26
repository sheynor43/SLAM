use super::*;
use crate::settings_screen::{RowKind, widget_of};
use crate::toolbar::ToolbarFrame;

const W: f32 = 1280.0;
const H: f32 = 720.0;

fn layout(pins: usize) -> ToolbarLayout {
    ToolbarLayout::new(W, H, &Theme::default(), pins)
}

fn open_bar(pins: usize) -> (Toolbar, ToolbarLayout) {
    let mut bar = Toolbar::new();
    bar.set_pins(&[1, 2, 3][..pins]);
    bar.show_player(0);
    (bar, layout(pins))
}

#[test]
fn the_button_sits_left_of_the_pins_and_is_found_by_point() {
    let layout = layout(2);
    let [x, y] = layout.player.centre();
    assert_eq!(layout.target_at(x, y), Some(Target::Player));
    let pins = layout.pins();
    assert!(layout.player.x > pins[1].x);
    assert!(layout.player.x + layout.player.width <= layout.clock.x + 0.01);
    // No pin overlaps it.
    assert!(pins[1].x + pins[1].width <= layout.player.x + 0.01);
}

#[test]
fn the_popup_is_under_the_bar_within_the_window_and_holds_its_parts() {
    for width in [1280.0, 420.0] {
        let layout = ToolbarLayout::new(width, H, &Theme::default(), 3);
        let r = layout.player_rects(0.0);
        assert!(r.popup.y >= layout.height());
        assert!(r.popup.x >= 0.0 && r.popup.x + r.popup.width <= width + 0.01);
        let inside = |rect: &Rect| {
            rect.x >= r.popup.x - 0.01
                && rect.x + rect.width <= r.popup.x + r.popup.width + 0.01
                && rect.y >= r.popup.y - 0.01
                && rect.y + rect.height <= r.popup.y + r.popup.height + 0.01
        };
        for rect in [
            &r.title,
            &r.artist,
            &r.bar,
            &r.buttons[0],
            &r.buttons[1],
            &r.buttons[2],
        ] {
            assert!(inside(rect), "{rect:?} in {:?}", r.popup);
        }
        // From top to bottom: texts, bar, buttons; buttons left to right.
        assert!(r.title.y < r.artist.y && r.artist.y + r.artist.height <= r.bar.y);
        assert!(r.bar.y + r.bar.height <= r.buttons[0].y);
        assert!(r.buttons[0].x < r.buttons[1].x && r.buttons[1].x < r.buttons[2].x);
    }
}

#[test]
fn the_popup_is_four_hundred_wide_at_unit_scale() {
    let layout = ToolbarLayout::new(1366.0, 768.0, &Theme::default(), 0);
    assert!((layout.player_rects(0.0).popup.width - 400.0).abs() < 0.01);
}

#[test]
fn a_position_on_the_bar_maps_to_a_share_of_the_track() {
    let layout = layout(0);
    let bar = layout.player_rects(0.0).bar;
    assert_eq!(layout.seek_fraction(bar.x), 0.0);
    assert!((layout.seek_fraction(bar.x + bar.width / 2.0) - 0.5).abs() < 1e-6);
    assert_eq!(layout.seek_fraction(bar.x + bar.width), 1.0);
    assert_eq!(layout.seek_fraction(bar.x - 50.0), 0.0);
    assert_eq!(layout.seek_fraction(bar.x + bar.width + 50.0), 1.0);
}

#[test]
fn progress_follows_position_and_length() {
    let frame = |position_ms, length_ms| NowPlayingFrame {
        available: true,
        paused: false,
        position_ms,
        length_ms,
    };
    assert_eq!(frame(1_000.0, 4_000.0).progress(), 0.25);
    assert_eq!(frame(9_000.0, 4_000.0).progress(), 1.0);
    assert_eq!(frame(1_000.0, 0.0).progress(), 0.0);
}

#[test]
fn clicking_the_button_opens_the_popup_and_again_closes_it() {
    let layout = layout(1);
    let mut bar = Toolbar::new();
    bar.set_pins(&[1]);
    let [x, y] = layout.player.centre();
    assert_eq!(bar.press(x, y, &layout), Some(ToolbarEvent::OpenPlayer));
    bar.show_player(0);
    assert!(bar.player_open());
    assert_eq!(bar.press(x, y, &layout), Some(ToolbarEvent::ClosePlayer));
    assert!(!bar.player_open());
}

#[test]
fn a_press_outside_closes_it_and_the_other_buttons_still_work() {
    let (mut bar, layout) = open_bar(2);
    assert_eq!(
        bar.press(5.0, 700.0, &layout),
        Some(ToolbarEvent::ClosePlayer)
    );
    assert!(!bar.player_open());
    bar.show_player(0);
    let [x, y] = layout.pins()[1].centre();
    assert_eq!(
        bar.press(x, y, &layout),
        Some(ToolbarEvent::OpenPopup { pin: 1 })
    );
    assert!(!bar.player_open());
    bar.show_player(0);
    let [x, y] = layout.settings.centre();
    assert_eq!(bar.press(x, y, &layout), Some(ToolbarEvent::OpenSettings));
}

#[test]
fn escape_closes_the_popup() {
    let (mut bar, layout) = open_bar(0);
    assert_eq!(bar.back(&layout), Some(ToolbarEvent::ClosePlayer));
    assert!(!bar.player_open());
    assert_eq!(bar.back(&layout), None);
}

#[test]
fn the_buttons_ask_for_their_actions() {
    let (mut bar, layout) = open_bar(0);
    let r = layout.player_rects(0.0);
    let want = [
        PlayerAction::Previous,
        PlayerAction::PlayPause,
        PlayerAction::Next,
    ];
    for (rect, action) in r.buttons.iter().zip(want) {
        let [x, y] = rect.centre();
        assert_eq!(bar.press(x, y, &layout), Some(ToolbarEvent::Player(action)));
        assert!(bar.player_open(), "a button keeps the popup open");
    }
    // The popup's own empty place takes the press without a change.
    let p = r.popup;
    assert_eq!(
        bar.press(p.x + 2.0, p.y + p.height - 2.0, &layout),
        Some(ToolbarEvent::Absorbed)
    );
}

#[test]
fn pressing_the_bar_seeks_and_dragging_follows_until_release() {
    let (mut bar, layout) = open_bar(0);
    let r = layout.player_rects(0.0);
    let y = r.bar.y + r.bar.height / 2.0;
    let seek = |event: Option<ToolbarEvent>| match event {
        Some(ToolbarEvent::Player(PlayerAction::Seek(f))) => f,
        other => panic!("not a seek: {other:?}"),
    };
    let at = |share: f32| r.bar.x + r.bar.width * share;
    assert!((seek(bar.press(at(0.25), y, &layout)) - 0.25).abs() < 1e-4);
    // A drag may leave the bar vertically and goes on.
    assert!((seek(bar.pointer(at(0.6), y + 300.0, &layout)) - 0.6).abs() < 1e-4);
    assert_eq!(seek(bar.pointer(at(1.5), y, &layout)), 1.0);
    bar.release(&layout);
    assert_eq!(bar.pointer(at(0.3), y, &layout), None);
}

#[test]
fn the_hover_of_a_button_is_kept_for_the_frame() {
    let (mut bar, layout) = open_bar(0);
    let [x, y] = layout.player_rects(0.0).buttons[2].centre();
    bar.pointer(x, y, &layout);
    assert_eq!(bar.frame(true, 0).player.unwrap().hover, Some(2));
    bar.pointer(1.0, 700.0, &layout);
    assert_eq!(bar.frame(true, 0).player.unwrap().hover, None);
}

#[test]
fn the_player_and_a_setting_popup_exclude_each_other() {
    let mut bar = Toolbar::new();
    bar.set_pins(&[1]);
    bar.show_popup(0, widget_of(RowKind::Toggle { value: false }), 0);
    assert!(bar.popup_open());
    bar.show_player(5);
    assert!(bar.player_open() && !bar.popup_open());
    // With a setting popup open, the player's button asks to switch to it.
    let layout = layout(1);
    bar.close_player();
    bar.show_popup(0, widget_of(RowKind::Toggle { value: false }), 0);
    let [x, y] = layout.player.centre();
    assert_eq!(bar.press(x, y, &layout), Some(ToolbarEvent::OpenPlayer));
    assert!(!bar.popup_open());
}

#[test]
fn the_frame_slides_fully_in_by_default() {
    assert_eq!(ToolbarFrame::default().slide, 1.0);
    assert_eq!(Toolbar::new().frame(true, 0).slide, 1.0);
}
