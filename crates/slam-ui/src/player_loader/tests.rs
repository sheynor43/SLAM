use super::*;
use crate::widgets::{Slider, Toggle};

const MS: u64 = 1_000_000;

fn rows() -> [QuickRow; 3] {
    [
        QuickRow {
            row: 40,
            group: QuickGroup::Visual,
            widget: RowWidget::Slider(Slider::new(0.0, 100.0, 1.0, 50.0)),
        },
        QuickRow {
            row: 41,
            group: QuickGroup::Visual,
            widget: RowWidget::Toggle(Toggle::new(true)),
        },
        QuickRow {
            row: 50,
            group: QuickGroup::Input,
            widget: RowWidget::Toggle(Toggle::new(false)),
        },
    ]
}

fn layout() -> LoaderLayout {
    LoaderLayout::new(1920.0, 1080.0, &Theme::default(), &rows())
}

fn centre(rect: Rect) -> [f32; 2] {
    rect.centre()
}

#[test]
fn the_column_is_on_the_right_with_groups_in_order_and_empty_ones_skipped() {
    let l = layout();
    let column = l.column();
    assert!(column.x > 1920.0 / 2.0, "{column:?}");
    assert!(column.x + column.width <= 1920.0);
    // Visual above input, audio (empty) has no plate.
    assert!(l.groups[0].height > 0.0);
    assert_eq!(l.groups[1].height, 0.0);
    assert!(l.groups[2].y > l.groups[0].y + l.groups[0].height);
    // Rows inside their groups, one under another.
    assert!(l.slots[1].y > l.slots[0].y);
    assert!(l.groups[2].contains(centre(l.slots[2])[0], centre(l.slots[2])[1]));
    // The card is in the middle of the window.
    let [cx, cy] = l.card_centre();
    assert!((cx - 960.0).abs() < 1e-3);
    assert!((cy - 540.0).abs() < 1e-3);
}

#[test]
fn play_waits_for_the_delay_and_the_preparation() {
    let theme = Theme::default();
    let delay = (theme.loader_push_delay_ms as u64) * MS;
    let mut s = PlayerLoader::new(&rows(), 0);
    assert!(!s.may_leave(delay, &theme), "not prepared");
    s.set_prepared(true);
    assert!(!s.may_leave(delay - MS, &theme), "too soon");
    assert!(s.may_leave(delay, &theme));
    s.leave(delay);
    assert!(!s.may_leave(delay + MS, &theme), "already going");
    let out = (theme.loader_out_ms as u64) * MS;
    assert!(!s.gone(delay + out - MS, &theme));
    assert!(s.gone(delay + out, &theme));
}

#[test]
fn without_effects_the_screen_goes_at_once() {
    let theme = Theme {
        effects: false,
        ..Theme::default()
    };
    let mut s = PlayerLoader::new(&rows(), 0);
    s.leave(5);
    assert!(s.gone(5, &theme));
}

#[test]
fn the_pointer_on_the_quick_settings_holds_play_back() {
    let theme = Theme::default();
    let l = layout();
    let mut s = PlayerLoader::new(&rows(), 0);
    s.set_prepared(true);
    let [x, y] = centre(l.slots[1]);
    s.pointer(1919.0, y, &l);
    s.pointer(x, y, &l);
    assert!(s.holds());
    assert!(!s.may_leave(10_000 * MS, &theme));
    assert!(s.frame().held);
    s.pointer(100.0, 100.0, &l);
    assert!(!s.holds());
    assert!(s.may_leave(10_000 * MS, &theme));
}

#[test]
fn a_press_on_a_toggle_changes_its_settings_row() {
    let l = layout();
    let mut s = PlayerLoader::new(&rows(), 0);
    let toggle = l.row(1).widget(0);
    let [x, y] = toggle.centre();
    s.pointer(1919.0, y, &l);
    s.pointer(x, y, &l);
    let event = s.press(x, y, &l);
    assert_eq!(
        event,
        Some(LoaderEvent::Changed {
            row: 41,
            value: RowValue::Toggle(false),
        })
    );
    // Elsewhere a press is not the screen's.
    assert_eq!(s.press(10.0, 10.0, &l), None);
}

#[test]
fn arrows_step_the_slider_under_the_pointer() {
    let l = layout();
    let mut s = PlayerLoader::new(&rows(), 0);
    assert_eq!(
        s.key(SettingsInput::Right, &l),
        None,
        "nothing under the pointer"
    );
    let [x, y] = centre(l.slots[0]);
    s.pointer(1919.0, y, &l);
    s.pointer(x, y, &l);
    assert_eq!(
        s.key(SettingsInput::Right, &l),
        Some(LoaderEvent::Changed {
            row: 40,
            value: RowValue::Slider(51.0),
        })
    );
    assert_eq!(
        s.key(SettingsInput::Left, &l),
        Some(LoaderEvent::Changed {
            row: 40,
            value: RowValue::Slider(50.0),
        })
    );
}

#[test]
fn back_leaves_the_screen_and_nothing_is_taken_once_it_goes() {
    let l = layout();
    let mut s = PlayerLoader::new(&rows(), 0);
    assert_eq!(s.key(SettingsInput::Back, &l), Some(LoaderEvent::Back));
    s.leave(1);
    assert_eq!(s.key(SettingsInput::Back, &l), None);
    let [x, y] = centre(l.slots[1]);
    s.pointer(1919.0, y, &l);
    assert_eq!(s.press(x, y, &l), None);
}

#[test]
fn rows_past_the_limit_are_dropped() {
    let many = [rows()[1]; MAX_QUICK_ROWS + 3];
    let s = PlayerLoader::new(&many, 0);
    assert_eq!(s.rows().count(), MAX_QUICK_ROWS);
    assert_eq!(s.frame().count, MAX_QUICK_ROWS);
}

#[test]
fn the_quick_settings_hide_until_the_pointer_reaches_the_right_edge() {
    let l = layout();
    let mut s = PlayerLoader::new(&rows(), 0);
    let [x, y] = centre(l.slots[1]);
    // Hidden: the column's place is not theirs.
    s.pointer(x, y, &l);
    assert!(!s.frame().revealed);
    assert_eq!(s.press(x, y, &l), None);
    // The right edge shows them; they stay while the pointer is by them.
    s.pointer(1919.0, y, &l);
    assert!(s.frame().revealed);
    s.pointer(x, y, &l);
    assert!(s.frame().revealed);
    assert!(s.press(x, y, &l).is_some());
    // Away from them, they hide again.
    s.pointer(200.0, y, &l);
    assert!(!s.frame().revealed);
    assert!(!s.holds());
}
