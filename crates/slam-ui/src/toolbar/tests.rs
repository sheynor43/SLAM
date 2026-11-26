use super::*;
use crate::settings_screen::{RowKind, widget_of};

const W: f32 = 1280.0;
const H: f32 = 720.0;

fn theme() -> Theme {
    Theme::default()
}

fn layout(pins: usize) -> ToolbarLayout {
    ToolbarLayout::new(W, H, &theme(), pins)
}

fn toggle() -> RowWidget {
    widget_of(RowKind::Toggle { value: false })
}

fn centre(rect: &Rect) -> [f32; 2] {
    rect.centre()
}

fn with_pins(rows: &[u16]) -> Toolbar {
    let mut bar = Toolbar::new();
    bar.set_pins(rows);
    bar
}

#[test]
fn the_buttons_are_on_the_left_and_the_pins_before_the_clock() {
    let layout = layout(3);
    assert_eq!(layout.settings.x, 0.0);
    assert!(layout.home.x >= layout.settings.x + layout.settings.width);
    let pins = layout.pins();
    assert_eq!(pins.len(), 3);
    assert!(pins[0].x < pins[1].x && pins[1].x < pins[2].x);
    assert!(pins[2].x + pins[2].width <= layout.clock.x + 0.01);
    assert!(pins[0].x > layout.home.x + layout.home.width);
    assert_eq!(layout.bar.width, W);
    assert!(layout.height() > 0.0);
}

#[test]
fn the_height_follows_the_theme_scale() {
    let big = ToolbarLayout::new(W, H, &theme().with_ui_scale(1.25), 0);
    assert!(big.height() > layout(0).height() * 1.2);
}

#[test]
fn targets_are_found_by_point() {
    let layout = layout(2);
    let [x, y] = centre(&layout.settings);
    assert_eq!(layout.target_at(x, y), Some(Target::Settings));
    let [x, y] = centre(&layout.home);
    assert_eq!(layout.target_at(x, y), Some(Target::Home));
    let [x, y] = centre(&layout.pins()[1]);
    assert_eq!(layout.target_at(x, y), Some(Target::Pin(1)));
    assert_eq!(layout.target_at(W / 2.0, layout.height() / 2.0), None);
}

#[test]
fn the_popup_is_under_its_pin_and_within_the_window() {
    let layout = layout(8);
    for pin in 0..8 {
        let popup = layout.popup(pin);
        assert!(popup.y >= layout.height());
        assert!(popup.x >= 0.0 && popup.x + popup.width <= W + 0.01);
    }
    // Pins at the far left still keep the popup in the window.
    let narrow = ToolbarLayout::new(300.0, H, &theme(), 8);
    for pin in 0..8 {
        let popup = narrow.popup(pin);
        assert!(popup.x >= 0.0);
    }
}

#[test]
fn buttons_ask_for_their_action() {
    let layout = layout(1);
    let mut bar = with_pins(&[5]);
    let [x, y] = centre(&layout.settings);
    assert_eq!(bar.press(x, y, &layout), Some(ToolbarEvent::OpenSettings));
    let [x, y] = centre(&layout.home);
    assert_eq!(bar.press(x, y, &layout), Some(ToolbarEvent::Home));
    let [x, y] = centre(&layout.pins()[0]);
    assert_eq!(
        bar.press(x, y, &layout),
        Some(ToolbarEvent::OpenPopup { pin: 0 })
    );
}

#[test]
fn the_bar_takes_presses_on_it_but_not_below() {
    let layout = layout(0);
    let mut bar = Toolbar::new();
    assert_eq!(
        bar.press(W / 2.0, layout.height() / 2.0, &layout),
        Some(ToolbarEvent::Absorbed)
    );
    assert_eq!(bar.press(W / 2.0, layout.height() + 5.0, &layout), None);
}

#[test]
fn a_click_outside_closes_the_popup_and_takes_the_click() {
    let layout = layout(2);
    let mut bar = with_pins(&[1, 2]);
    bar.show_popup(0, toggle(), 0);
    assert!(bar.popup_open());
    assert_eq!(
        bar.press(5.0, 600.0, &layout),
        Some(ToolbarEvent::ClosePopup)
    );
    assert!(!bar.popup_open());
}

#[test]
fn clicking_the_pin_of_the_open_popup_closes_it_and_another_pin_switches() {
    let layout = layout(2);
    let mut bar = with_pins(&[1, 2]);
    bar.show_popup(0, toggle(), 0);
    let [x, y] = centre(&layout.pins()[0]);
    assert_eq!(bar.press(x, y, &layout), Some(ToolbarEvent::ClosePopup));
    bar.show_popup(0, toggle(), 0);
    let [x, y] = centre(&layout.pins()[1]);
    assert_eq!(
        bar.press(x, y, &layout),
        Some(ToolbarEvent::OpenPopup { pin: 1 })
    );
    assert!(!bar.popup_open(), "the application opens the next one");
}

#[test]
fn escape_closes_the_popup() {
    let layout = layout(1);
    let mut bar = with_pins(&[1]);
    assert_eq!(bar.back(&layout), None);
    bar.show_popup(0, toggle(), 0);
    assert_eq!(bar.back(&layout), Some(ToolbarEvent::ClosePopup));
    assert!(!bar.popup_open());
}

#[test]
fn a_press_on_the_popups_toggle_changes_the_value() {
    let layout = layout(1);
    let mut bar = with_pins(&[7]);
    bar.show_popup(0, toggle(), 0);
    let row = layout.row(0, 0.0).row(0);
    let [x, y] = centre(&row);
    assert_eq!(
        bar.press(x, y, &layout),
        Some(ToolbarEvent::Changed {
            row: 7,
            value: RowValue::Toggle(true)
        })
    );
    assert!(bar.popup_open(), "a change keeps the popup");
}

#[test]
fn a_slider_in_the_popup_follows_a_drag() {
    let layout = layout(1);
    let mut bar = with_pins(&[3]);
    let slider = widget_of(RowKind::Slider {
        min: 0.0,
        max: 100.0,
        step: 1.0,
        value: 0.0,
    });
    bar.show_popup(0, slider, 0);
    let widget = layout.row(0, 0.0).widget(0);
    let (x, y) = (
        widget.x + widget.width * 0.1,
        widget.y + widget.height / 2.0,
    );
    let Some(ToolbarEvent::Changed {
        row: 3,
        value: RowValue::Slider(first),
    }) = bar.press(x, y, &layout)
    else {
        panic!("the press moves the slider");
    };
    let Some(ToolbarEvent::Changed {
        value: RowValue::Slider(second),
        ..
    }) = bar.pointer(x + 60.0, y, &layout)
    else {
        panic!("the drag moves it on");
    };
    assert!(second > first);
    bar.release(&layout);
    assert_eq!(bar.pointer(x + 120.0, y, &layout), None, "released");
}

#[test]
fn a_dropdown_opens_picks_and_its_list_counts_as_the_popup() {
    let layout = layout(1);
    let mut bar = with_pins(&[2]);
    let dropdown = widget_of(RowKind::Dropdown {
        options: 4,
        selected: 0,
    });
    bar.show_popup(0, dropdown, 0);
    let row = layout.row(0, 0.0);
    let [x, y] = centre(&row.widget(0));
    assert_eq!(bar.press(x, y, &layout), Some(ToolbarEvent::Absorbed));
    let list = row.dropdown_at(0, 4);
    let option = list.option(2);
    let [ox, oy] = centre(&option);
    // The option may lie outside the popup's plate: still the popup's.
    assert_eq!(
        bar.press(ox, oy, &layout),
        Some(ToolbarEvent::Changed {
            row: 2,
            value: RowValue::Choice(2)
        })
    );
    assert!(bar.popup_open());
}

#[test]
fn escape_closes_an_open_list_before_the_popup() {
    let layout = layout(1);
    let mut bar = with_pins(&[2]);
    bar.show_popup(
        0,
        widget_of(RowKind::Dropdown {
            options: 3,
            selected: 0,
        }),
        0,
    );
    let [x, y] = centre(&layout.row(0, 0.0).widget(0));
    bar.press(x, y, &layout);
    assert_eq!(bar.back(&layout), Some(ToolbarEvent::Absorbed));
    assert!(bar.popup_open());
    assert_eq!(bar.back(&layout), Some(ToolbarEvent::ClosePopup));
}

#[test]
fn setting_other_pins_closes_a_stale_popup_only() {
    let mut bar = with_pins(&[1, 2]);
    bar.show_popup(1, toggle(), 0);
    assert!(!bar.set_pins(&[1, 2, 3]));
    assert!(bar.popup_open());
    assert!(bar.set_pins(&[1, 9]));
    assert!(!bar.popup_open());
}

#[test]
fn no_more_than_the_most_pins_are_kept() {
    let rows: Vec<u16> = (0..20).collect();
    let bar = with_pins(&rows);
    assert_eq!(bar.pins().len(), MAX_PINS);
    assert_eq!(layout(20).pins().len(), MAX_PINS);
}

#[test]
fn the_frame_carries_the_state() {
    let mut bar = with_pins(&[4, 6]);
    let layout = layout(2);
    let [x, y] = centre(&layout.pins()[1]);
    bar.pointer(x, y, &layout);
    let frame = bar.frame(true, 125);
    assert!(frame.visible);
    assert_eq!(frame.hover, Some(Target::Pin(1)));
    assert_eq!(&frame.pins[..2], &[4, 6]);
    assert_eq!(frame.pin_count, 2);
    assert_eq!(frame.clock, 125);
    assert_eq!(ToolbarFrame::default().clock, NO_CLOCK);
}
