use super::*;
use crate::song_select::{ModSelect, ModSelectLayout};
use SongSelectInput as I;

const MS: u64 = 1_000_000;

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
    let mut panel = ModSelect::new([&["EZ", "HT"], &["HR"], &["DA"]]);
    panel.open();
    panel.set_setting_rows(&rows());
    panel
}

fn header_point(l: &ModSelectLayout) -> (f32, f32) {
    let [x, y] = l.customise.header.centre();
    (x, y)
}

fn expand_by_hover(panel: &mut ModSelect, l: &ModSelectLayout) {
    let (x, y) = header_point(l);
    panel.input(I::PointerMove { x, y }, l);
    assert_eq!(panel.customisation().state(), CustomisationState::Expanded);
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
fn hovering_the_header_expands_when_a_setting_exists() {
    let l = layout();
    let mut p = panel();
    expand_by_hover(&mut p, &l);
    let mut empty = ModSelect::new([&["EZ"], &[], &[]]);
    empty.open();
    empty.set_setting_rows(&[header()]);
    let (x, y) = header_point(&l);
    empty.input(I::PointerMove { x, y }, &l);
    assert!(!empty.customisation().is_expanded());
    empty.input(I::ToggleCustomisation, &l);
    assert!(!empty.customisation().is_expanded());
    assert!(!empty.snapshot().customise.is_enabled());
}

#[test]
fn rows_without_settings_collapse_the_panel() {
    let l = layout();
    let mut p = panel();
    expand_by_hover(&mut p, &l);
    p.set_setting_rows(&[header()]);
    assert!(!p.customisation().is_expanded());
}

#[test]
fn the_pointer_collapses_after_the_grace_time() {
    let l = layout();
    let mut p = panel();
    expand_by_hover(&mut p, &l);
    p.input(far(), &l);
    p.tick(1000 * MS, &l);
    p.tick(1179 * MS, &l);
    assert!(p.customisation().is_expanded());
    p.tick(1180 * MS, &l);
    assert!(!p.customisation().is_expanded());
}

#[test]
fn coming_back_restarts_the_grace_time() {
    let l = layout();
    let mut p = panel();
    expand_by_hover(&mut p, &l);
    p.input(far(), &l);
    p.tick(0, &l);
    p.tick(100 * MS, &l);
    let (x, y) = header_point(&l);
    p.input(I::PointerMove { x, y }, &l);
    p.tick(150 * MS, &l);
    p.input(far(), &l);
    p.tick(200 * MS, &l);
    p.tick(379 * MS, &l);
    assert!(p.customisation().is_expanded());
    p.tick(380 * MS, &l);
    assert!(!p.customisation().is_expanded());
}

#[test]
fn a_pointer_near_the_panel_does_not_collapse_it() {
    let l = layout();
    let mut p = panel();
    expand_by_hover(&mut p, &l);
    let panel_rect = l.customise.panel(6);
    p.input(
        I::PointerMove {
            x: panel_rect.x - COLLAPSE_DISTANCE * l.customise.scale + 1.0,
            y: panel_rect.y + 5.0,
        },
        &l,
    );
    p.tick(0, &l);
    p.tick(10_000 * MS, &l);
    assert!(p.customisation().is_expanded());
}

#[test]
fn tab_expands_and_the_pointer_collapses_only_after_it_moves() {
    let l = layout();
    let mut p = panel();
    p.input(far(), &l);
    p.input(I::ToggleCustomisation, &l);
    assert_eq!(p.customisation().state(), CustomisationState::Expanded);
    p.tick(0, &l);
    p.tick(10_000 * MS, &l);
    assert!(p.customisation().is_expanded());
    p.input(far(), &l);
    p.tick(10_001 * MS, &l);
    p.tick(10_200 * MS, &l);
    assert!(!p.customisation().is_expanded());
    p.input(I::ToggleCustomisation, &l);
    p.input(I::ToggleCustomisation, &l);
    assert!(!p.customisation().is_expanded());
}

#[test]
fn the_pointer_does_not_collapse_a_panel_expanded_by_a_mod() {
    let l = layout();
    let mut p = panel();
    p.expand_customisation_by_mod();
    p.input(far(), &l);
    p.tick(0, &l);
    p.tick(10_000 * MS, &l);
    assert_eq!(p.customisation().state(), CustomisationState::ExpandedByMod);
    p.collapse_customisation();
    assert!(!p.customisation().is_expanded());
}

#[test]
fn dragging_a_slider_holds_the_panel_open() {
    let l = layout();
    let mut p = panel();
    expand_by_hover(&mut p, &l);
    let row = l.customise.row_rect(1);
    let hit = l.customise.slider(&row).hit;
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
    p.tick(0, &l);
    p.tick(10_000 * MS, &l);
    assert!(p.customisation().is_expanded());
    p.input(I::PointerRelease, &l);
    // The release does not move the pointer, so the clock starts at the next tick.
    p.tick(10_001 * MS, &l);
    p.tick(10_181 * MS, &l);
    assert!(!p.customisation().is_expanded());
}

#[test]
fn clicking_outside_collapses_without_toggling_a_tile() {
    let l = layout();
    let mut p = panel();
    expand_by_hover(&mut p, &l);
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
    expand_by_hover(&mut p, &l);
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
    expand_by_hover(&mut p, &l);
    let [x, y] = l.customise.row_rect(2).centre();
    assert_eq!(
        p.input(I::PointerPress { x, y }, &l),
        set(2, SettingValue::Toggle(true))
    );
    assert_eq!(p.customisation().selected(), 2);
    let [x, y] = l.customise.row_rect(3).centre();
    assert_eq!(
        p.input(I::PointerPress { x, y }, &l),
        set(3, SettingValue::Choice(1))
    );
    // A header does nothing.
    let [x, y] = l.customise.row_rect(0).centre();
    assert_eq!(p.input(I::PointerPress { x, y }, &l), None);
    assert!(p.customisation().is_expanded());
}

#[test]
fn the_wheel_scrolls_and_the_selection_stays_in_view() {
    let l = layout();
    let mut p = ModSelect::new([&["EZ"], &[], &[]]);
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
    expand_by_hover(&mut p, &l);
    p.input(I::ToggleMods, &l);
    assert!(!p.is_open());
    assert!(!p.customisation().is_expanded());
}
