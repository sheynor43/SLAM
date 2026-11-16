use super::*;
use SongSelectInput as I;

const COLUMNS: ModColumns = [
    &["EZ", "NF", "HT", "DC"],
    &["HR", "SD", "PF"],
    &["AT", "RX"],
    &["DA", "CL"],
    &[],
];

fn layout() -> ModSelectLayout {
    let mut layout = ModSelectLayout::new(1280.0, 720.0, &Theme::default());
    layout.place(column_counts(&COLUMNS));
    layout
}

fn open_panel() -> ModSelect {
    let mut panel = ModSelect::new(COLUMNS);
    panel.open();
    panel
}

fn toggle(column: usize, index: usize) -> Option<ModSelectAction> {
    Some(ModSelectAction::Toggle { column, index })
}

#[test]
fn hotkeys_toggle_by_index_among_the_column() {
    let mut panel = open_panel();
    let l = layout();
    assert_eq!(
        panel.input(
            I::ModHotkey {
                column: 0,
                index: 0
            },
            &l
        ),
        toggle(0, 0)
    );
    assert_eq!(
        panel.input(
            I::ModHotkey {
                column: 0,
                index: 3
            },
            &l
        ),
        toggle(0, 3)
    );
    assert_eq!(
        panel.input(
            I::ModHotkey {
                column: 1,
                index: 2
            },
            &l
        ),
        toggle(1, 2)
    );
    // Past the visible mods of the column.
    assert_eq!(
        panel.input(
            I::ModHotkey {
                column: 0,
                index: 4
            },
            &l
        ),
        None
    );
    assert_eq!(
        panel.input(
            I::ModHotkey {
                column: 1,
                index: 3
            },
            &l
        ),
        None
    );
    assert_eq!(
        panel.input(
            I::ModHotkey {
                column: 1,
                index: 9
            },
            &l
        ),
        None
    );
    // The automation column has hotkeys, as in lazer; the conversion column has none.
    assert_eq!(
        panel.input(
            I::ModHotkey {
                column: 2,
                index: 1
            },
            &l
        ),
        toggle(2, 1)
    );
    assert_eq!(
        panel.input(
            I::ModHotkey {
                column: 3,
                index: 0
            },
            &l
        ),
        None
    );
    assert!(panel.is_open());
}

#[test]
fn close_keys_close_and_backspace_deselects() {
    for key in [I::Back, I::Activate, I::ToggleMods] {
        let mut panel = open_panel();
        assert_eq!(panel.input(key, &layout()), None);
        assert!(!panel.is_open(), "{key:?}");
    }
    let mut panel = open_panel();
    assert_eq!(
        panel.input(I::DeselectMods, &layout()),
        Some(ModSelectAction::DeselectAll)
    );
    assert!(panel.is_open());
    // Navigation keys do nothing.
    for key in [
        I::Up,
        I::Down,
        I::Left,
        I::Right,
        I::Home,
        I::End,
        I::PageUp,
    ] {
        assert_eq!(panel.input(key, &layout()), None);
    }
}

#[test]
fn mouse_toggles_the_tile_under_the_cursor_and_hovers() {
    let mut panel = open_panel();
    let l = layout();
    let r = l.tile_rect(1, 2);
    let [x, y] = r.centre();
    assert_eq!(panel.input(I::PointerMove { x, y }, &l), None);
    assert_eq!(panel.snapshot().hover, Some((1, 2)));
    assert_eq!(panel.input(I::PointerPress { x, y }, &l), toggle(1, 2));
    // Mouse works in the conversion column.
    let [x, y] = l.tile_rect(3, 1).centre();
    assert_eq!(panel.input(I::PointerPress { x, y }, &l), toggle(3, 1));
    // The gap between two tiles, a tile that does not exist, and the outside.
    let gap_y = r.y + r.height + 0.5;
    assert_eq!(l.tile_at(r.x + 1.0, gap_y, column_counts(&COLUMNS)), None);
    let [x, y] = l.tile_rect(1, 3).centre();
    assert_eq!(panel.input(I::PointerPress { x, y }, &l), None);
    assert_eq!(panel.snapshot().hover, None);
    assert_eq!(l.tile_at(0.0, 0.0, column_counts(&COLUMNS)), None);
    assert_eq!(
        l.tile_at(r.x + 1.0, r.y - 100.0, column_counts(&COLUMNS)),
        None
    );
}

#[test]
fn tiles_fit_the_window_and_do_not_overlap() {
    let l = layout();
    for column in (0..MAX_COLUMNS).filter(|&c| l.is_shown(c)) {
        let a = l.tile_rect(column, 6);
        assert!(a.y + a.height < l.hint.y);
        let b = l.tile_rect(column, 7);
        assert!(b.y >= a.y + a.height);
    }
    assert!(l.tile_rect(0, 0).x + l.tile_rect(0, 0).width <= l.tile_rect(1, 0).x);
}

#[test]
fn flags_are_set_by_the_application() {
    let mut panel = ModSelect::new(COLUMNS);
    panel.set_active(0, 2, true);
    panel.set_active(1, 0, true);
    panel.set_active(1, 9, true);
    panel.set_active(7, 0, true);
    assert!(panel.is_active(0, 2) && panel.is_active(1, 0));
    assert!(!panel.is_active(0, 0) && !panel.is_active(1, 9));
    let on: Vec<_> = panel.iter().filter(|m| m.3).map(|m| m.2).collect();
    assert_eq!(on, ["HT", "HR"]);
    panel.set_active(0, 2, false);
    assert!(!panel.is_active(0, 2));
    assert_eq!(panel.snapshot().active, [0, 1, 0, 0, 0]);
}

#[test]
fn closing_clears_the_hover() {
    let mut panel = open_panel();
    let l = layout();
    let [x, y] = l.tile_rect(0, 0).centre();
    panel.input(I::PointerMove { x, y }, &l);
    assert!(panel.snapshot().hover.is_some());
    panel.input(I::Back, &l);
    assert_eq!(panel.snapshot(), ModSelectSnapshot::default());
}

#[test]
fn empty_columns_are_hidden_and_the_rest_centred() {
    let l = layout();
    assert!((0..4).all(|c| l.is_shown(c)) && !l.is_shown(4));
    let first = l.column_rect(0, 4);
    let last = l.column_rect(3, 2);
    let left = first.x;
    let right = 1280.0 - (last.x + last.width);
    assert!((left - right).abs() < 0.01, "{left} vs {right}");
    assert_eq!(l.column_titles[4], Rect::default());
    // Nothing under the hidden column's place, even at x 0.
    let mut all = ModSelectLayout::new(1280.0, 720.0, &Theme::default());
    all.place([1, 1, 1, 1, 0]);
    assert_eq!(
        all.tile_at(0.0, all.tile_rect(0, 0).y + 1.0, [1, 1, 1, 1, 0]),
        None
    );
    // Only two columns: they move to the middle.
    all.place([2, 0, 0, 3, 0]);
    assert!(all.tile_rect(0, 0).x > l.tile_rect(0, 0).x);
    assert!(all.tile_rect(3, 0).x < l.tile_rect(3, 0).x);
}

#[test]
fn a_column_plate_holds_its_header_and_tiles() {
    let l = layout();
    for (column, count) in column_counts(&COLUMNS).into_iter().enumerate() {
        if count == 0 {
            continue;
        }
        let plate = l.column_rect(column, count);
        // The plate leans about its middle: its edges at a height.
        let middle = plate.y + plate.height / 2.0;
        let edges = |y: f32| {
            let x = plate.x + (middle - y) * l.lean();
            (x, x + plate.width)
        };
        let header = l.column_titles[column];
        assert_eq!(plate.y, header.y);
        let (left, right) = edges(header.y + header.height / 2.0);
        assert!((header.x - left).abs() < 0.01 && (header.x + header.width - right).abs() < 0.01);
        for index in 0..count {
            let tile = l.tile_rect(column, index);
            let (left, right) = edges(tile.y + tile.height / 2.0);
            assert!(
                tile.x >= left && tile.x + tile.width <= right,
                "{column} {index}"
            );
        }
        let last = l.tile_rect(column, count - 1);
        assert!(plate.y + plate.height >= last.y + last.height);
        assert!(header.y + header.height <= l.tile_rect(column, 0).y);
    }
    // The columns stand in the lower part of the window, and lean.
    assert!(l.column_rect(0, 4).y > 720.0 * 0.2);
    assert!(l.lean() > 0.0);
    assert!(l.tile_rect(1, 0).x > l.tile_rect(1, 5).x);
}
