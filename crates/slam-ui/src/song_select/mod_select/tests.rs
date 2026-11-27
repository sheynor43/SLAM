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
        l.tile_at(
            r.x + 1.0,
            l.column_titles[1].y - 5.0,
            column_counts(&COLUMNS)
        ),
        None
    );
}

#[test]
fn tiles_fit_the_window_and_do_not_overlap() {
    let l = layout();
    let counts = column_counts(&COLUMNS);
    for column in (0..MAX_COLUMNS).filter(|&c| l.is_shown(c)) {
        let a = l.tile_rect(column, counts[column] - 1);
        assert!(a.y + a.height < l.hint.y);
        let b = l.tile_rect(column, counts[column]);
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
    // The headers share a top, hence one lean offset: compare them, not the plates.
    let (first, last) = (l.column_titles[0], l.column_titles[3]);
    let shift = l.column_titles[0].x - l.column_rect(0, 4).x;
    let left = first.x - shift;
    let right = 1280.0 - (last.x - shift + last.width);
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
    // The columns hang near the top, and lean.
    assert!(l.column_rect(0, 4).y < 720.0 * 0.3);
    assert!(l.lean() > 0.0);
    assert!(l.tile_rect(1, 0).x > l.tile_rect(1, 5).x);
}

#[test]
fn multipliers_are_formatted_as_lazer_does() {
    let show = |x: f64| {
        let mut text = String::new();
        push_multiplier(&mut text, x);
        text
    };
    assert_eq!(show(1.0), "1.00x");
    assert_eq!(show(1.0001), "1.01x");
    assert_eq!(show(1.055), "1.06x");
    assert_eq!(show(0.9999), "0.99x");
    assert_eq!(show(1.23), "1.23x");
    assert_eq!(show(0.5), "0.50x");
}

#[test]
fn multiplier_tones_follow_the_value() {
    assert_eq!(MultiplierTone::of(1.12), MultiplierTone::Up);
    assert_eq!(MultiplierTone::of(0.5), MultiplierTone::Down);
    assert_eq!(MultiplierTone::of(1.0), MultiplierTone::Neutral);
}

#[test]
fn the_panel_hands_its_multipliers_to_the_snapshot() {
    let mut panel = open_panel();
    assert_eq!(panel.snapshot().total_multiplier, 1.0);
    panel.set_multiplier(1, 0, 1.06);
    panel.set_multiplier(1, 9, 5.0);
    panel.set_total_multiplier(1.12);
    let snap = panel.snapshot();
    assert_eq!(snap.multipliers[1][0], 1.06);
    assert_eq!(snap.multipliers[1][9], 1.0);
    assert_eq!(snap.total_multiplier, 1.12);
}

#[test]
fn the_bigger_tiles_fit_the_window() {
    let theme = Theme::default();
    let l = layout();
    let tile = l.tile_rect(0, 0);
    assert!(tile.width <= theme.mod_tile_width * theme.scale(720.0) + 0.01);
    assert!(tile.height > 50.0 * theme.scale(720.0) - 0.01);
    let last = l.tile_rect(1, 2);
    assert!(last.y + last.height <= 720.0);
}

#[test]
fn the_columns_are_as_tall_as_their_content_and_hang_under_the_header() {
    let theme = Theme::default();
    for (width, height) in [(1920.0, 1080.0), (1280.0, 720.0)] {
        let mut l = ModSelectLayout::new(width, height, &theme);
        let counts = column_counts(&COLUMNS);
        l.place(counts);
        let scale = theme.scale(height);
        let tallest = (0..MAX_COLUMNS).max_by_key(|&c| counts[c]).unwrap();
        let last = l.tile_rect(tallest, counts[tallest] - 1);
        let plate = l.column_rect(tallest, counts[tallest]);
        // No band under the last tile but the padding.
        let slack = plate.y + plate.height - (last.y + last.height);
        assert!(
            slack <= theme.mod_column_pad * scale + 0.01,
            "{height}: {slack}"
        );
        assert_eq!(l.content_bottom(), plate.y + plate.height);
        // The block hangs right under the title, clear of the toolbar and the footer.
        let title_bottom = l.title.y + l.title.height;
        assert!(plate.y >= title_bottom, "{height}");
        assert!(plate.y - title_bottom <= 0.12 * height, "{height}");
        assert!(plate.y >= (theme.toolbar_height + theme.song_top_gap) * scale);
        assert!(l.content_bottom() <= l.hint.y, "{height}");
        // A shorter column's plate ends after its own last tile, not the tallest's.
        let short = (0..MAX_COLUMNS).find(|&c| counts[c] == 2).unwrap();
        let end = l.tile_rect(short, 1);
        let short_plate = l.column_rect(short, 2);
        assert!(short_plate.y + short_plate.height < plate.y + plate.height);
        assert!(
            short_plate.y + short_plate.height - (end.y + end.height)
                <= theme.mod_column_pad * scale + 0.01
        );
        // The settings panel's header is clear of the columns.
        assert!(l.customise.header.y + l.customise.header.height <= plate.y);
    }
}

#[test]
fn the_columns_lean_on_one_common_line() {
    let l = layout();
    // Headers share a top, so the shared lean puts them at one x per column slot spacing:
    // each column's left edge at the header is its slot's x plus the same offset.
    let counts = column_counts(&COLUMNS);
    let shift = |c: usize| l.column_titles[c].x - l.column_rect(c, counts[c]).x;
    // A taller plate leans further at the header; the plate and header edges agree.
    assert!(shift(0) > 0.0);
    // Tiles at the same height in different columns are one column pitch apart: no stagger.
    let pitch = l.tile_rect(1, 0).x - l.tile_rect(0, 0).x;
    let pitch2 = l.tile_rect(2, 0).x - l.tile_rect(1, 0).x;
    assert!((pitch - pitch2).abs() < 0.01, "{pitch} vs {pitch2}");
}
