use super::*;
use crate::song_select::{ModSelect, ModSelectLayout};
use SongSelectInput as I;

fn layout() -> ModSelectLayout {
    ModSelectLayout::new(1280.0, 720.0, &Theme::default())
}

fn number(value: f64) -> SettingRow {
    SettingRow {
        label: "number",
        suffix: "",
        kind: SettingRowKind::Number {
            value,
            min: 0.0,
            max: 10.0,
            step: 0.1,
            decimals: 1,
            unset: false,
        },
    }
}

fn toggle(value: bool) -> SettingRow {
    SettingRow {
        label: "toggle",
        suffix: "",
        kind: SettingRowKind::Toggle { value },
    }
}

fn choice(index: Option<usize>) -> SettingRow {
    SettingRow {
        label: "choice",
        suffix: "",
        kind: SettingRowKind::Choice {
            index,
            count: 3,
            values: ["a", "b", "c"],
        },
    }
}

fn header() -> SettingRow {
    SettingRow {
        label: "mod",
        ..SettingRow::EMPTY
    }
}

/// Rows: 0 header, 1 number, 2 toggle, 3 choice, 4 header, 5 toggle.
fn rows() -> [SettingRow; 6] {
    [
        header(),
        number(5.0),
        toggle(false),
        choice(Some(0)),
        header(),
        toggle(true),
    ]
}

fn panel() -> ModSelect {
    let mut panel = ModSelect::new([&["EZ", "HT"], &["HR"], &[], &["DA"], &[]]);
    panel.open();
    panel.set_setting_rows(&rows());
    panel
}

/// Opens the panel with its header at `(400, 200)`.
fn open(panel: &mut ModSelect) {
    panel.open_customisation_at(400.0, 100.0);
    assert_eq!(panel.customisation().state(), CustomisationState::Expanded);
}

/// The panel's layout where it stands now.
fn placed(panel: &ModSelect, l: &ModSelectLayout) -> CustomisationLayout {
    l.customise.at(
        panel.snapshot().customise.origin,
        panel.customisation().rows().len(),
    )
}

fn far() -> I {
    I::PointerMove { x: 5.0, y: 700.0 }
}

/// Numbers carry float noise from the slider's grid; the application rounds them.
fn r(action: Option<ModSelectAction>) -> Option<ModSelectAction> {
    match action {
        Some(ModSelectAction::SetSetting {
            row,
            value: SettingValue::Number(n),
        }) => set(row, SettingValue::Number((n * 10.0).round() / 10.0)),
        other => other,
    }
}

fn set(row: usize, value: SettingValue) -> Option<ModSelectAction> {
    Some(ModSelectAction::SetSetting { row, value })
}

#[test]
fn nothing_opens_without_a_setting() {
    let l = layout();
    let mut empty = ModSelect::new([&["EZ"], &[], &[], &[], &[]]);
    empty.open();
    empty.set_setting_rows(&[header()]);
    empty.open_customisation_at(400.0, 200.0);
    assert!(!empty.customisation().is_expanded());
    empty.input(I::ToggleCustomisation, &l);
    assert!(!empty.customisation().is_expanded());
    assert!(!empty.snapshot().customise.is_enabled());
    // Nor while the mod select is closed.
    let mut closed = panel();
    closed.close();
    closed.open_customisation_at(400.0, 200.0);
    assert!(!closed.customisation().is_expanded());
}

#[test]
fn rows_without_settings_collapse_the_panel() {
    let mut p = panel();
    open(&mut p);
    p.set_setting_rows(&[header()]);
    assert!(!p.customisation().is_expanded());
}

#[test]
fn the_pointer_wandering_off_does_not_collapse_it() {
    let l = layout();
    let mut p = panel();
    open(&mut p);
    p.input(far(), &l);
    assert!(p.customisation().is_expanded());
}

#[test]
fn right_click_on_a_tile_asks_for_that_mods_settings() {
    let l = layout();
    let mut p = panel();
    let [x, y] = l.tile_rect(0, 1).centre();
    assert_eq!(
        p.input(I::SecondaryPress { x, y }, &l),
        Some(ModSelectAction::Settings {
            column: 0,
            index: 1,
            x,
            y
        })
    );
    // Nowhere near a tile: nothing.
    assert_eq!(p.input(I::SecondaryPress { x: 2.0, y: 2.0 }, &l), None);
    assert!(!p.customisation().is_expanded());
}

#[test]
fn right_click_again_closes_the_open_panel() {
    let l = layout();
    let mut p = panel();
    open(&mut p);
    let [x, y] = placed(&p, &l).row_rect(1).centre();
    assert_eq!(p.input(I::SecondaryPress { x, y }, &l), None);
    assert!(!p.customisation().is_expanded());
    // Also over a tile, and the mod select itself stays.
    open(&mut p);
    let [x, y] = l.tile_rect(0, 0).centre();
    assert_eq!(p.input(I::SecondaryPress { x, y }, &l), None);
    assert!(!p.customisation().is_expanded());
    assert!(p.is_open());
}

#[test]
fn the_panel_opens_at_the_pointer_and_stays_in_the_window() {
    let l = layout();
    let mut p = panel();
    p.open_customisation_at(300.0, 90.0);
    let at = placed(&p, &l);
    assert_eq!((at.header.x, at.header.y), (300.0, 90.0));
    // Opened in a corner, it is moved to lie inside the window.
    p.open_customisation_at(1270.0, 715.0);
    let inside = placed(&p, &l);
    let panel_rect = inside.panel(6);
    assert!(panel_rect.x + panel_rect.width <= 1280.0);
    assert!(panel_rect.y + panel_rect.height <= 720.0);
    assert!(panel_rect.x >= 0.0 && panel_rect.y >= 0.0);
}

#[test]
fn dragging_the_header_moves_the_panel() {
    let l = layout();
    let mut p = panel();
    open(&mut p);
    let start = placed(&p, &l);
    let (hx, hy) = (start.header.x + 30.0, start.header.y + 5.0);
    assert_eq!(p.input(I::PointerPress { x: hx, y: hy }, &l), None);
    assert!(p.snapshot().customise.moving);
    p.input(
        I::PointerMove {
            x: hx - 100.0,
            y: hy + 40.0,
        },
        &l,
    );
    let moved = placed(&p, &l);
    assert_eq!(moved.header.x, start.header.x - 100.0);
    assert_eq!(moved.header.y, start.header.y + 40.0);
    // The panel stays open and a release lets go.
    p.input(I::PointerRelease, &l);
    assert!(!p.snapshot().customise.moving);
    assert!(p.customisation().is_expanded());
    p.input(
        I::PointerMove {
            x: hx - 500.0,
            y: hy,
        },
        &l,
    );
    assert_eq!(placed(&p, &l), moved);
}

#[test]
fn the_panel_cannot_be_dragged_out_of_the_window() {
    let l = layout();
    let mut p = panel();
    open(&mut p);
    let start = placed(&p, &l);
    p.input(
        I::PointerPress {
            x: start.header.x + 5.0,
            y: start.header.y + 5.0,
        },
        &l,
    );
    p.input(
        I::PointerMove {
            x: -5000.0,
            y: -5000.0,
        },
        &l,
    );
    let at = placed(&p, &l);
    assert!(at.header.x >= 0.0 && at.header.y >= 0.0);
}

#[test]
fn reopening_forgets_where_the_panel_was_dragged() {
    let l = layout();
    let mut p = panel();
    open(&mut p);
    let start = placed(&p, &l);
    p.input(
        I::PointerPress {
            x: start.header.x + 5.0,
            y: start.header.y + 5.0,
        },
        &l,
    );
    p.input(
        I::PointerMove {
            x: start.header.x + 205.0,
            y: start.header.y + 105.0,
        },
        &l,
    );
    p.input(I::PointerRelease, &l);
    assert_ne!(placed(&p, &l), start);
    p.input(I::Back, &l);
    assert!(!p.customisation().is_expanded());
    open(&mut p);
    assert_eq!(placed(&p, &l), start);
}

#[test]
fn tab_opens_at_the_pointer_or_the_top_right() {
    let l = layout();
    let mut p = panel();
    p.input(I::ToggleCustomisation, &l);
    assert_eq!(placed(&p, &l), l.customise);
    p.input(I::ToggleCustomisation, &l);
    assert!(!p.customisation().is_expanded());
    p.input(I::PointerMove { x: 100.0, y: 90.0 }, &l);
    p.input(I::ToggleCustomisation, &l);
    assert_eq!(placed(&p, &l).origin(), (100.0, 90.0));
}

#[test]
fn dragging_a_slider_works_in_the_floating_panel() {
    let l = layout();
    let mut p = panel();
    open(&mut p);
    let at = placed(&p, &l);
    let hit = at.slider(&at.row_rect(1)).hit;
    let action = p.input(
        I::PointerPress {
            x: hit.x + hit.width * 0.25,
            y: hit.y + 2.0,
        },
        &l,
    );
    assert_eq!(action, set(1, SettingValue::Number(2.5)));
    let action = p.input(
        I::PointerMove {
            x: hit.x + hit.width * 0.75,
            y: 700.0,
        },
        &l,
    );
    assert_eq!(action, set(1, SettingValue::Number(7.5)));
    p.input(I::PointerRelease, &l);
    assert!(p.customisation().is_expanded());
}

#[test]
fn clicking_outside_collapses_without_toggling_a_tile() {
    let l = layout();
    let mut p = panel();
    open(&mut p);
    let [x, y] = l.tile_rect(0, 0).centre();
    assert_eq!(p.input(I::PointerPress { x, y }, &l), None);
    assert!(!p.customisation().is_expanded());
    // Now the tiles answer again.
    assert_eq!(
        p.input(I::PointerPress { x, y }, &l),
        Some(ModSelectAction::Toggle {
            column: 0,
            index: 0
        })
    );
}

#[test]
fn back_collapses_first_and_closes_second() {
    let l = layout();
    let mut p = panel();
    open(&mut p);
    assert_eq!(p.input(I::Back, &l), None);
    assert!(p.is_open());
    assert!(!p.customisation().is_expanded());
    p.input(I::Back, &l);
    assert!(!p.is_open());
}

#[test]
fn the_expanded_panel_takes_the_mod_keys() {
    let l = layout();
    let mut p = panel();
    p.input(I::ToggleCustomisation, &l);
    assert_eq!(
        p.input(
            I::ModHotkey {
                column: 0,
                index: 0
            },
            &l
        ),
        None
    );
    assert_eq!(p.input(I::DeselectMods, &l), None);
    let [x, y] = l.tile_rect(0, 0).centre();
    p.input(I::PointerMove { x, y }, &l);
    assert_eq!(p.snapshot().hover, None);
}

#[test]
fn up_and_down_select_rows_and_skip_headers() {
    let l = layout();
    let mut p = panel();
    p.input(I::ToggleCustomisation, &l);
    assert_eq!(p.customisation().selected(), 1);
    p.input(I::Down, &l);
    p.input(I::Down, &l);
    assert_eq!(p.customisation().selected(), 3);
    p.input(I::Down, &l);
    assert_eq!(p.customisation().selected(), 5);
    p.input(I::Down, &l);
    assert_eq!(p.customisation().selected(), 5);
    for _ in 0..5 {
        p.input(I::Up, &l);
    }
    assert_eq!(p.customisation().selected(), 1);
}

#[test]
fn left_and_right_step_numbers_flip_toggles_and_cycle_choices() {
    let l = layout();
    let mut p = panel();
    p.input(I::ToggleCustomisation, &l);
    assert_eq!(r(p.input(I::Right, &l)), set(1, SettingValue::Number(5.1)));
    assert_eq!(r(p.input(I::Left, &l)), set(1, SettingValue::Number(5.0)));
    assert_eq!(r(p.input(I::Left, &l)), set(1, SettingValue::Number(4.9)));
    p.input(I::Down, &l);
    assert_eq!(p.input(I::Right, &l), set(2, SettingValue::Toggle(true)));
    assert_eq!(p.input(I::Left, &l), set(2, SettingValue::Toggle(false)));
    assert_eq!(p.input(I::Activate, &l), set(2, SettingValue::Toggle(true)));
    assert_eq!(p.input(I::Space, &l), set(2, SettingValue::Toggle(false)));
    p.input(I::Down, &l);
    assert_eq!(p.input(I::Right, &l), set(3, SettingValue::Choice(1)));
    assert_eq!(p.input(I::Space, &l), set(3, SettingValue::Choice(2)));
    assert_eq!(p.input(I::Right, &l), set(3, SettingValue::Choice(0)));
    assert_eq!(p.input(I::Left, &l), set(3, SettingValue::Choice(2)));
}

#[test]
fn a_number_stops_at_its_bounds() {
    let l = layout();
    let mut p = panel();
    p.set_setting_rows(&[header(), number(10.0)]);
    p.input(I::ToggleCustomisation, &l);
    assert_eq!(p.input(I::Right, &l), None);
    assert_eq!(r(p.input(I::Left, &l)), set(1, SettingValue::Number(9.9)));
}

#[test]
fn delete_resets_the_selected_row() {
    let l = layout();
    let mut p = panel();
    p.input(I::ToggleCustomisation, &l);
    assert_eq!(
        p.input(I::ResetSetting, &l),
        Some(ModSelectAction::ResetSetting { row: 1 })
    );
}

#[test]
fn clicks_change_rows_and_select_them() {
    let l = layout();
    let mut p = panel();
    open(&mut p);
    let at = placed(&p, &l);
    let [x, y] = at.row_rect(2).centre();
    assert_eq!(
        p.input(I::PointerPress { x, y }, &l),
        set(2, SettingValue::Toggle(true))
    );
    assert_eq!(p.customisation().selected(), 2);
    let [x, y] = at.row_rect(3).centre();
    assert_eq!(
        p.input(I::PointerPress { x, y }, &l),
        set(3, SettingValue::Choice(1))
    );
    // A header does nothing.
    let [x, y] = at.row_rect(0).centre();
    assert_eq!(p.input(I::PointerPress { x, y }, &l), None);
    assert!(p.customisation().is_expanded());
}

#[test]
fn the_wheel_scrolls_and_the_selection_stays_in_view() {
    let l = layout();
    let mut p = ModSelect::new([&["EZ"], &[], &[], &[], &[]]);
    p.open();
    let many: Vec<SettingRow> = (0..MAX_SETTING_ROWS).map(|_| toggle(false)).collect();
    p.set_setting_rows(&many);
    p.input(I::ToggleCustomisation, &l);
    let visible = l.customise.visible_rows(MAX_SETTING_ROWS);
    assert!(visible < MAX_SETTING_ROWS);
    p.input(I::Scroll { lines: 3.0 }, &l);
    assert_eq!(p.snapshot().customise.first, 3);
    p.input(I::Scroll { lines: -10.0 }, &l);
    assert_eq!(p.snapshot().customise.first, 0);
    for _ in 0..visible + 2 {
        p.input(I::Down, &l);
    }
    let c = p.snapshot().customise;
    assert!(c.selected >= c.first && c.selected < c.first + visible);
}

#[test]
fn rows_past_the_capacity_are_dropped() {
    let mut p = panel();
    let many = vec![toggle(false); MAX_SETTING_ROWS + 8];
    p.set_setting_rows(&many);
    assert_eq!(p.customisation().rows().len(), MAX_SETTING_ROWS);
}

#[test]
fn closing_the_mod_select_collapses_the_panel() {
    let l = layout();
    let mut p = panel();
    open(&mut p);
    p.input(I::ToggleMods, &l);
    assert!(!p.is_open());
    assert!(!p.customisation().is_expanded());
}
