use super::SongSelectInput as I;
use super::*;

fn set(key: u64, diffs: usize) -> SetRow {
    SetRow {
        key,
        artist: format!("artist {key}"),
        title: format!("title {key}"),
        creator: "mapper".to_owned(),
        difficulties: (0..diffs)
            .map(|d| {
                DifficultyRow::new(SearchSource {
                    difficulty: &format!("diff {d}"),
                    creator: "mapper",
                    artist: &format!("artist {key}"),
                    title: &format!("title {key}"),
                    ..SearchSource::default()
                })
            })
            .collect(),
    }
}

fn list(sets: &[(u64, usize)]) -> Arc<SongList> {
    Arc::new(SongList {
        sets: sets.iter().map(|&(k, d)| set(k, d)).collect(),
    })
}

/// Gives `s` `list` with filter buffers made for it, as the application does.
fn put(s: &mut SongSelect, list: Arc<SongList>) {
    let filter = SongFilter::for_list(&list);
    s.set_list(list, filter);
}

fn layout() -> SongSelectLayout {
    SongSelectLayout::new(1280.0, 720.0, &Theme::default())
}

fn screen(sets: &[(u64, usize)]) -> SongSelect {
    let mut s = SongSelect::new();
    put(&mut s, list(sets));
    s
}

fn press(s: &mut SongSelect, input: SongSelectInput) -> Option<SongSelectAction> {
    s.input(input, &layout())
}

/// The primary button pressed and released at `(x, y)` without moving.
fn click(s: &mut SongSelect, x: f32, y: f32) -> Option<SongSelectAction> {
    assert_eq!(press(s, I::PointerPress { x, y }), None);
    press(s, I::PointerRelease)
}

#[test]
fn keyboard_navigation_table() {
    // (sets as (key, difficulties), inputs, expected selection)
    type Case<'a> = (&'a [(u64, usize)], &'a [SongSelectInput], (usize, usize));
    let cases: &[Case] = &[
        (&[(1, 2), (2, 3), (3, 1)], &[I::Down], (0, 1)),
        (&[(1, 2), (2, 3), (3, 1)], &[I::Down, I::Down], (1, 0)),
        (&[(1, 2), (2, 3), (3, 1)], &[I::Down; 6], (2, 0)),
        // clamps at the end and at the start
        (&[(1, 2), (2, 3), (3, 1)], &[I::Down; 20], (2, 0)),
        (&[(1, 2), (2, 3), (3, 1)], &[I::Up], (0, 0)),
        // Up crosses into the last difficulty of the previous set
        (&[(1, 2), (2, 3)], &[I::Down, I::Down, I::Up], (0, 1)),
        (&[(1, 2), (2, 3)], &[I::Right, I::Down, I::Left], (0, 0)),
        (
            &[(1, 2), (2, 3), (3, 1)],
            &[I::Right, I::Right, I::Right],
            (2, 0),
        ),
        (&[(1, 2), (2, 3), (3, 1)], &[I::End], (2, 0)),
        (&[(1, 2), (2, 3), (3, 1)], &[I::End, I::Home], (0, 0)),
        (&[(1, 2), (2, 3), (3, 1)], &[I::PageDown], (2, 0)),
        (&[(1, 2), (2, 3), (3, 1)], &[I::End, I::PageUp], (0, 0)),
        // a set without difficulties is passed over
        (&[(1, 1), (2, 0), (3, 1)], &[I::Down, I::Down], (2, 0)),
    ];
    for (sets, inputs, expected) in cases {
        let mut s = screen(sets);
        for input in *inputs {
            assert_eq!(press(&mut s, *input), None);
        }
        assert_eq!(s.selected(), Some(*expected), "{sets:?} {inputs:?}");
    }
}

#[test]
fn page_moves_by_visible_rows() {
    let sets: Vec<(u64, usize)> = (0..100).map(|k| (k, 1)).collect();
    let mut s = screen(&sets);
    let page = layout().viewport_rows() as usize;
    press(&mut s, I::PageDown);
    assert_eq!(s.selected(), Some((page, 0)));
    press(&mut s, I::PageUp);
    assert_eq!(s.selected(), Some((0, 0)));
}

#[test]
fn activate_returns_indices() {
    let mut s = screen(&[(1, 2), (2, 3)]);
    press(&mut s, I::Right);
    press(&mut s, I::Down);
    assert_eq!(
        press(&mut s, I::Activate),
        Some(SongSelectAction::Play {
            set: 1,
            difficulty: 1
        })
    );
}

#[test]
fn empty_list_and_back() {
    let mut s = SongSelect::new();
    // Scanning: no button, so Enter has nothing to do either.
    s.set_status(SongSelectStatus::Scanning { sets: 0 });
    for input in [
        I::Up,
        I::Down,
        I::Left,
        I::Right,
        I::PageDown,
        I::End,
        I::Activate,
    ] {
        assert_eq!(press(&mut s, input), None);
    }
    assert_eq!(s.selected(), None);
    assert_eq!(press(&mut s, I::Back), Some(SongSelectAction::Back));
}

#[test]
fn loading_ignores_everything_but_back() {
    let mut s = screen(&[(1, 2), (2, 2)]);
    s.set_status(SongSelectStatus::Loading);
    for input in [I::Down, I::Right, I::Activate, I::Scroll { lines: 3.0 }] {
        assert_eq!(press(&mut s, input), None);
    }
    assert_eq!(s.selected(), Some((0, 0)));
    assert_eq!(press(&mut s, I::Back), Some(SongSelectAction::Back));
}

#[test]
fn selection_follows_the_key_across_set_list() {
    let mut s = screen(&[(1, 2), (2, 3), (3, 1)]);
    press(&mut s, I::Right);
    press(&mut s, I::Down);
    assert_eq!(s.selected(), Some((1, 1)));
    // new sets sorted before, the old ones reordered
    put(&mut s, list(&[(9, 1), (3, 1), (8, 1), (2, 3), (1, 2)]));
    assert_eq!(s.selected(), Some((3, 1)));
    // the set lost a difficulty: the index is clamped
    put(&mut s, list(&[(2, 1)]));
    assert_eq!(s.selected(), Some((0, 0)));
    // the set is gone: the position is kept, clamped
    put(&mut s, list(&[(5, 1), (6, 1)]));
    assert_eq!(s.selected(), Some((0, 0)));
    put(&mut s, Arc::default());
    assert_eq!(s.selected(), None);
}

#[test]
fn row_mapping_expands_the_selected_set() {
    let l = SongFilter::for_list(&list(&[(1, 2), (2, 3), (3, 1)]));
    assert_eq!(l.row_count(1), 6);
    let rows: Vec<Row> = (0..6).map(|i| l.row(1, i).unwrap()).collect();
    assert_eq!(
        rows,
        vec![
            Row::Set(0),
            Row::Set(1),
            Row::Difficulty {
                set: 1,
                difficulty: 0
            },
            Row::Difficulty {
                set: 1,
                difficulty: 1
            },
            Row::Difficulty {
                set: 1,
                difficulty: 2
            },
            Row::Set(2),
        ]
    );
    assert_eq!(l.row(1, 6), None);
}

fn centre_of(layout: &SongSelectLayout, index: usize, scroll: f32) -> (f32, f32) {
    (
        layout.list.x + layout.list.width / 2.0,
        layout.row_y(index, scroll) + layout.row_height / 2.0,
    )
}

#[test]
fn pointer_selects_and_second_press_activates() {
    let mut s = screen(&[(1, 2), (2, 3), (3, 1)]);
    let l = layout();
    // set 0 is expanded: its difficulties are rows 1 and 2, set 1 is row 3
    let (x, y) = centre_of(&l, 3, 0.0);
    assert_eq!(click(&mut s, x, y), None);
    assert_eq!(s.selected(), Some((1, 0)));
    // its second difficulty is row 3 again (set 1 is now row 1, difficulties 2 to 4)
    let (x, y) = centre_of(&l, 3, 0.0);
    assert_eq!(click(&mut s, x, y), None);
    assert_eq!(s.selected(), Some((1, 1)));
    assert_eq!(
        click(&mut s, x, y),
        Some(SongSelectAction::Play {
            set: 1,
            difficulty: 1
        })
    );
    // outside the list: nothing
    assert_eq!(click(&mut s, 1.0, 1.0), None);
    assert_eq!(s.selected(), Some((1, 1)));
}

#[test]
fn pointer_move_only_hovers() {
    let mut s = screen(&[(1, 2), (2, 3)]);
    let (x, y) = centre_of(&layout(), 3, 0.0);
    press(&mut s, I::PointerMove { x, y });
    assert_eq!(s.snapshot().hovered, Some(Row::Set(1)));
    assert_eq!(s.selected(), Some((0, 0)));
    press(&mut s, I::PointerMove { x: 1.0, y: 1.0 });
    assert_eq!(s.snapshot().hovered, None);
}

#[test]
fn hit_testing_follows_the_scroll() {
    let sets: Vec<(u64, usize)> = (0..50).map(|k| (k, 1)).collect();
    let mut s = screen(&sets);
    let l = layout();
    press(&mut s, I::Scroll { lines: 10.5 });
    // The shown scroll eases there (ADR-0121); hit tests follow what is shown.
    s.tick(1, &l);
    s.tick(1_000_000_000, &l);
    let scroll = s.snapshot().shown_scroll;
    assert_eq!(scroll, 10.5);
    assert_eq!(s.selected(), Some((0, 0)));
    let (x, y) = centre_of(&l, 20, scroll);
    assert_eq!(l.row_at(x, y, scroll, s.filter(), 0), Some(Row::Set(19)));
    click(&mut s, x, y);
    assert_eq!(s.selected(), Some((19, 0)));
}

#[test]
fn scroll_is_clamped() {
    let mut s = screen(&[(1, 2), (2, 3)]);
    press(&mut s, I::Scroll { lines: 100.0 });
    assert_eq!(s.snapshot().scroll, 0.0);
    let sets: Vec<(u64, usize)> = (0..50).map(|k| (k, 1)).collect();
    let mut s = screen(&sets);
    press(&mut s, I::Scroll { lines: 1000.0 });
    let max = 51.0 - layout().viewport_rows();
    assert!((s.snapshot().scroll - max).abs() < 1e-3);
    press(&mut s, I::Scroll { lines: -2000.0 });
    assert_eq!(s.snapshot().scroll, 0.0);
}

#[test]
fn keyboard_keeps_the_selection_visible() {
    let sets: Vec<(u64, usize)> = (0..50).map(|k| (k, 2)).collect();
    let mut s = screen(&sets);
    let l = layout();
    for _ in 0..80 {
        press(&mut s, I::Down);
        let snap = s.snapshot();
        let row = s.selected_row() as f32;
        assert!(row >= snap.scroll && row + 1.0 <= snap.scroll + l.viewport_rows() + 1e-3);
    }
    press(&mut s, I::Home);
    assert_eq!(s.snapshot().scroll, 0.0);
    // the wheel moves away, then a key brings the selection back
    press(&mut s, I::Scroll { lines: 30.0 });
    press(&mut s, I::Down);
    assert!(s.snapshot().scroll < 3.0);
}

#[test]
fn visible_rows_cover_the_list_area() {
    let l = layout();
    let rows = l.visible_rows(0.0, 1000);
    assert_eq!(rows.start, 0);
    assert!(rows.end as f32 >= l.viewport_rows());
    assert_eq!(l.visible_rows(0.0, 3), 0..3);
    assert_eq!(l.visible_rows(5.5, 1000).start, 5);
}

const MOD_COLUMNS: ModColumns = [&["EZ", "NF"], &["HR", "SD"], &["DA"]];

fn with_mods() -> SongSelect {
    let mut s = screen(&[(1, 2), (2, 3)]);
    s.mods_mut().clone_from(&ModSelect::new(MOD_COLUMNS));
    s
}

#[test]
fn f1_opens_the_panel_which_takes_every_key() {
    let mut s = with_mods();
    assert_eq!(press(&mut s, I::ToggleMods), None);
    assert!(s.snapshot().mods.open);
    // The carousel does not move and Enter does not play.
    for key in [
        I::Down,
        I::Right,
        I::End,
        I::PageDown,
        I::Scroll { lines: 3.0 },
    ] {
        assert_eq!(press(&mut s, key), None);
    }
    assert_eq!(s.selected(), Some((0, 0)));
    // Esc closes the panel and stays in the song select.
    assert_eq!(press(&mut s, I::Back), None);
    assert!(!s.snapshot().mods.open);
    assert_eq!(
        press(&mut s, I::Activate),
        Some(SongSelectAction::Play {
            set: 0,
            difficulty: 0
        })
    );
    // Enter and F1 close it too; the next Esc leaves.
    for key in [I::Activate, I::ToggleMods] {
        press(&mut s, I::ToggleMods);
        assert_eq!(press(&mut s, key), None);
        assert!(!s.mods().is_open());
    }
    assert_eq!(press(&mut s, I::Back), Some(SongSelectAction::Back));
}

#[test]
fn the_panel_turns_toggles_and_deselect_into_actions() {
    let mut s = with_mods();
    // Closed: hotkeys do nothing, Backspace deselects.
    assert_eq!(
        press(
            &mut s,
            I::ModHotkey {
                column: 0,
                index: 0
            }
        ),
        None
    );
    assert_eq!(
        press(&mut s, I::DeselectMods),
        Some(SongSelectAction::DeselectMods)
    );
    press(&mut s, I::ToggleMods);
    assert_eq!(
        press(
            &mut s,
            I::ModHotkey {
                column: 1,
                index: 1
            }
        ),
        Some(SongSelectAction::ToggleMod {
            column: 1,
            index: 1
        })
    );
    assert_eq!(
        press(
            &mut s,
            I::ModHotkey {
                column: 2,
                index: 0
            }
        ),
        None
    );
    assert_eq!(
        press(&mut s, I::DeselectMods),
        Some(SongSelectAction::DeselectMods)
    );
    // The application writes the flags back.
    s.mods_mut().set_active(1, 1, true);
    assert_eq!(s.snapshot().mods.active, [0, 2, 0]);
    // A click on a tile toggles it.
    let [x, y] = layout().mods.tile_rect(0, 1).centre();
    assert_eq!(
        press(&mut s, I::PointerPress { x, y }),
        Some(SongSelectAction::ToggleMod {
            column: 0,
            index: 1
        })
    );
}

#[test]
fn the_panel_does_not_open_while_loading() {
    let mut s = with_mods();
    s.set_status(SongSelectStatus::Loading);
    assert_eq!(press(&mut s, I::ToggleMods), None);
    assert!(!s.mods().is_open());
    assert_eq!(press(&mut s, I::DeselectMods), None);
}

#[test]
fn choose_folder_button_sits_in_the_second_list_row() {
    let l = layout();
    let b = l.choose_folder;
    assert!((b.y - (l.list.y + l.row_height)).abs() < 1e-3);
    assert!((b.x - l.list.x).abs() < 1e-3 && (b.width - l.list.width).abs() < 1e-3);
    assert!(b.height > 0.0 && b.height < l.row_height);
    assert!(b.y + b.height <= l.list.y + l.list.height);
}

#[test]
fn choose_folder_is_offered_only_when_nothing_can_play() {
    use SongSelectStatus as S;
    let mut s = SongSelect::new();
    for (status, shown) in [
        (S::NoFolder, true),
        (S::Failed, true),
        (S::Ready, true),
        (S::Scanning { sets: 0 }, false),
        (S::Loading, false),
    ] {
        s.set_status(status);
        assert_eq!(s.shows_choose_folder(), shown, "{status:?}");
    }
    s.set_status(S::Ready);
    put(&mut s, list(&[(1, 1)]));
    assert!(!s.shows_choose_folder());
}

#[test]
fn choose_folder_by_click_and_enter() {
    let l = layout();
    let b = l.choose_folder;
    let (x, y) = (b.x + b.width / 2.0, b.y + b.height / 2.0);
    let mut s = SongSelect::new();
    s.set_status(SongSelectStatus::NoFolder);
    assert_eq!(
        s.input(I::Activate, &l),
        Some(SongSelectAction::ChooseFolder)
    );
    assert_eq!(s.input(I::PointerMove { x, y }, &l), None);
    assert!(s.snapshot().folder_hovered);
    assert_eq!(click(&mut s, x, y), Some(SongSelectAction::ChooseFolder));
    // Outside the button: nothing.
    assert_eq!(click(&mut s, 1.0, 1.0), None);
    assert!(!s.snapshot().folder_hovered);
    // While scanning the button is absent.
    s.set_status(SongSelectStatus::Scanning { sets: 0 });
    assert_eq!(s.input(I::Activate, &l), None);
    assert_eq!(click(&mut s, x, y), None);
}

#[test]
fn enter_with_a_list_still_plays() {
    let mut s = screen(&[(1, 2)]);
    s.set_status(SongSelectStatus::Ready);
    assert_eq!(
        press(&mut s, I::Activate),
        Some(SongSelectAction::Play {
            set: 0,
            difficulty: 0
        })
    );
}

const MS: u64 = 1_000_000;

/// 50 sets of one difficulty, ticked at time 0.
fn long_list() -> (SongSelect, SongSelectLayout) {
    let sets: Vec<(u64, usize)> = (0..50).map(|k| (k, 1)).collect();
    let mut s = screen(&sets);
    let l = layout();
    s.tick(0, &l);
    (s, l)
}

/// Drags from the centre of row `from` by `rows` row heights upwards, one tick of `step`
/// ms per row height, starting at `now`; returns the time after the last tick.
fn drag_up(s: &mut SongSelect, l: &SongSelectLayout, from: usize, rows: usize, now: u64) -> u64 {
    let (x, y) = centre_of(l, from, 0.0);
    press(s, I::PointerPress { x, y });
    let mut now = now;
    for i in 1..=rows {
        let y = y - i as f32 * l.row_height;
        press(s, I::PointerMove { x, y });
        now += 16 * MS;
        s.tick(now, l);
    }
    now
}

#[test]
fn dragging_scrolls_and_does_not_select() {
    let (mut s, l) = long_list();
    let (x, y) = centre_of(&l, 5, 0.0);
    press(&mut s, I::PointerPress { x, y });
    // Up to the click distance nothing moves.
    press(&mut s, I::PointerMove { x, y: y - 9.0 });
    assert_eq!(s.snapshot().scroll, 0.0);
    // Past it the list follows the pointer from there on: 3 rows up scroll 3 rows.
    let start = y - 11.0;
    press(&mut s, I::PointerMove { x, y: start });
    assert_eq!(s.snapshot().scroll, 0.0);
    press(
        &mut s,
        I::PointerMove {
            x,
            y: start - 3.0 * l.row_height,
        },
    );
    assert!((s.snapshot().scroll - 3.0).abs() < 1e-4);
    // The release is no click: the selection stays.
    assert_eq!(press(&mut s, I::PointerRelease), None);
    assert_eq!(s.selected(), Some((0, 0)));
}

#[test]
fn a_small_move_is_still_a_click() {
    let (mut s, l) = long_list();
    let (x, y) = centre_of(&l, 5, 0.0);
    press(&mut s, I::PointerPress { x, y });
    press(
        &mut s,
        I::PointerMove {
            x: x + 4.0,
            y: y + 6.0,
        },
    );
    assert_eq!(press(&mut s, I::PointerRelease), None);
    assert_eq!(s.selected(), Some((4, 0)));
    // A sideways drag does not block the click either, as lazer's (only the scroll axis).
    let (x, y) = centre_of(&l, 8, 0.0);
    press(&mut s, I::PointerPress { x, y });
    press(
        &mut s,
        I::PointerMove {
            x: x - 40.0,
            y: y + 2.0,
        },
    );
    press(&mut s, I::PointerRelease);
    assert_eq!(s.selected(), Some((7, 0)));
}

#[test]
fn a_release_off_the_pressed_row_does_not_click() {
    let (mut s, l) = long_list();
    let (x, y) = centre_of(&l, 5, 0.0);
    press(&mut s, I::PointerPress { x, y });
    // Leaves the list sideways without scrolling: the press drags, but does not block.
    press(&mut s, I::PointerMove { x: 1.0, y });
    assert_eq!(press(&mut s, I::PointerRelease), None);
    assert_eq!(s.selected(), Some((0, 0)));
}

#[test]
fn a_fast_release_flings_and_slows_down() {
    let (mut s, l) = long_list();
    let now = drag_up(&mut s, &l, 8, 4, 0);
    let dragged = s.snapshot().scroll;
    press(&mut s, I::PointerRelease);
    let mut last = dragged;
    let mut steps = Vec::new();
    let mut now = now;
    for _ in 0..60 {
        now += 16 * MS;
        s.tick(now, &l);
        let scroll = s.snapshot().scroll;
        steps.push(scroll - last);
        last = scroll;
    }
    assert!(last > dragged + 1.0, "flings on: {dragged} -> {last}");
    // Each step moves less than the one before, and it settles.
    assert!(steps.windows(2).all(|w| w[1] <= w[0] + 1e-6), "{steps:?}");
    for _ in 0..600 {
        now += 16 * MS;
        s.tick(now, &l);
    }
    let settled = s.snapshot().scroll;
    now += 16 * MS;
    s.tick(now, &l);
    assert_eq!(s.snapshot().scroll, settled);
}

#[test]
fn a_held_still_release_does_not_fling() {
    let (mut s, l) = long_list();
    let now = drag_up(&mut s, &l, 8, 4, 0);
    // Held for half a second: 0.95^500 of the speed is left, under the cutoff.
    s.tick(now + 500 * MS, &l);
    let held = s.snapshot().scroll;
    press(&mut s, I::PointerRelease);
    s.tick(now + 516 * MS, &l);
    s.tick(now + 1000 * MS, &l);
    assert_eq!(s.snapshot().scroll, held);
}

#[test]
fn the_fling_stops_at_the_end_of_the_list() {
    let (mut s, l) = long_list();
    let max = 51.0 - l.viewport_rows();
    press(&mut s, I::Scroll { lines: max - 1.0 });
    let mut now = drag_up(&mut s, &l, 8, 4, 0);
    press(&mut s, I::PointerRelease);
    for _ in 0..600 {
        now += 16 * MS;
        s.tick(now, &l);
        assert!(s.snapshot().scroll <= max + 1e-4);
    }
    assert!((s.snapshot().scroll - max).abs() < 1e-3);
    // Dragging past the start stops there too.
    press(&mut s, I::Scroll { lines: -1000.0 });
    let (x, y) = centre_of(&l, 2, 0.0);
    press(&mut s, I::PointerPress { x, y });
    press(&mut s, I::PointerMove { x, y: y + 400.0 });
    assert_eq!(s.snapshot().scroll, 0.0);
}

#[test]
fn a_press_or_the_keyboard_stops_the_fling() {
    let (mut s, l) = long_list();
    let mut now = drag_up(&mut s, &l, 8, 4, 0);
    press(&mut s, I::PointerRelease);
    now += 16 * MS;
    s.tick(now, &l);
    let (x, y) = centre_of(&l, 3, 0.0);
    press(&mut s, I::PointerPress { x, y });
    let stopped = s.snapshot().scroll;
    now += 16 * MS;
    s.tick(now, &l);
    assert_eq!(s.snapshot().scroll, stopped);
    // The press that stopped it still clicks.
    press(&mut s, I::PointerRelease);
    assert!(s.selected().is_some_and(|(set, _)| set > 0));

    let (mut s, l) = long_list();
    let mut now = drag_up(&mut s, &l, 8, 4, 0);
    press(&mut s, I::PointerRelease);
    now += 16 * MS;
    s.tick(now, &l);
    press(&mut s, I::Down);
    let kept = s.snapshot().scroll;
    now += 16 * MS;
    s.tick(now, &l);
    assert_eq!(s.snapshot().scroll, kept);
}

#[test]
fn hover_follows_the_fling() {
    let (mut s, l) = long_list();
    let mut now = drag_up(&mut s, &l, 8, 4, 0);
    press(&mut s, I::PointerRelease);
    let before = s.snapshot().hovered;
    // The fling starts on the tick after the release.
    now += MS;
    s.tick(now, &l);
    now += 100 * MS;
    s.tick(now, &l);
    let scroll = s.snapshot().scroll;
    let (x, y) = s.pointer.unwrap();
    assert_eq!(s.snapshot().hovered, l.row_at(x, y, scroll, s.filter(), 0));
    assert_ne!(s.snapshot().hovered, before);
}

#[test]
fn the_scroll_eases_towards_its_target_and_lands() {
    use crate::song_select::{SCROLL_EASE_NS, ease_scroll};
    let step = ease_scroll(0.0, 10.0, SCROLL_EASE_NS as u64);
    assert!(
        (step - 10.0 * (1.0 - (-1.0f32).exp())).abs() < 1e-3,
        "{step}"
    );
    assert_eq!(ease_scroll(9.9995, 10.0, 1), 10.0);
    assert_eq!(ease_scroll(3.0, 3.0, 1_000), 3.0);
    // Going back up eases the same way.
    assert!(ease_scroll(10.0, 0.0, 1_000_000) < 10.0);
}

#[test]
fn the_list_leans_and_hit_tests_with_it() {
    let layout = layout();
    let middle = (layout.header.y + layout.hint.y + layout.hint.height) / 2.0;
    assert_eq!(layout.lean_x(middle), 0.0);
    assert!(
        layout.lean_x(layout.list.y) > 0.0,
        "the top sits further right"
    );
    assert!(layout.lean_x(layout.list.y + layout.list.height) < 0.0);
    // Just inside the list's left edge at its top, shifted right: outside unless leaned.
    let y = layout.list.y + 1.0;
    let x = layout.list.x + layout.lean_x(y) + 1.0;
    assert!(layout.list.contains(x - layout.lean_x(y), y));
}

#[test]
fn a_dragged_list_follows_the_hand_without_easing() {
    let sets: Vec<(u64, usize)> = (0..50).map(|k| (k, 1)).collect();
    let mut s = screen(&sets);
    let l = layout();
    let (x, y) = centre_of(&l, 5, 0.0);
    press(&mut s, I::PointerPress { x, y });
    // The first move past the threshold starts the drag; the next one scrolls.
    press(&mut s, I::PointerMove { x, y: y - 20.0 });
    press(&mut s, I::PointerMove { x, y: y - 300.0 });
    s.tick(1, &l);
    let snap = s.snapshot();
    assert!(snap.scroll > 0.0);
    assert_eq!(snap.shown_scroll, snap.scroll);
}

#[test]
fn the_leaning_backing_reaches_past_the_right_edge() {
    let theme = Theme::default();
    for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (800.0, 1600.0)] {
        let l = SongSelectLayout::new(w, h, &theme);
        let b = l.backing(w, theme.song_margin * l.scale);
        // The bottom right corner, sheared left by the lean about the middle.
        let bottom = b.y + b.height;
        let corner = b.x + b.width - (bottom - (b.y + b.height / 2.0)) * l.lean;
        assert!(corner > w, "{w}x{h}: {corner}");
        // The lean is half the buttons' skew.
        let half = (theme.button_skew() * crate::song_select::LEAN_OF_SKEW).tan();
        assert!((l.lean - half).abs() < 1e-6);
    }
}

mod thumbnails {
    use slam_render::hal::Limits;
    use slam_render::upload::{TextureHandle, TextureKind, TextureLoaderConfig, texture_loader};

    use super::super::view::{FADES, ThumbGeometry, fade_alpha};
    use super::*;

    fn handles(n: usize) -> Vec<TextureHandle> {
        let limits = Limits {
            max_texture_size: 4096,
            max_uniform_block_size: 16384,
            max_uniform_buffer_slots: 12,
            max_texture_slots: 16,
        };
        let (mut requests, _uploads) = texture_loader(&TextureLoaderConfig {
            max_textures: 256,
            ..TextureLoaderConfig::new(limits)
        })
        .unwrap();
        (0..n)
            .map(|_| requests.load(Vec::new(), TextureKind::Standalone).unwrap())
            .collect()
    }

    const MS: u64 = 1_000_000;

    #[test]
    fn a_thumbnail_fades_in_from_its_first_frame_and_stays() {
        let h = handles(2);
        let mut fades = [None; FADES];
        let mut next = 0;
        assert_eq!(
            fade_alpha(&mut fades, &mut next, h[0], 100 * MS, 200.0),
            0.0
        );
        let half = fade_alpha(&mut fades, &mut next, h[0], 200 * MS, 200.0);
        assert!((half - 0.5).abs() < 1e-6, "{half}");
        assert_eq!(
            fade_alpha(&mut fades, &mut next, h[0], 900 * MS, 200.0),
            1.0
        );
        // Another thumbnail starts on its own.
        assert_eq!(
            fade_alpha(&mut fades, &mut next, h[1], 900 * MS, 200.0),
            0.0
        );
        assert_eq!(
            fade_alpha(&mut fades, &mut next, h[0], 901 * MS, 200.0),
            1.0
        );
        // No fade time: whole at once.
        let mut fades = [None; FADES];
        assert_eq!(fade_alpha(&mut fades, &mut 0, h[0], 0, 0.0), 1.0);
    }

    #[test]
    fn the_fade_table_replaces_the_oldest_when_full() {
        let h = handles(FADES + 1);
        let mut fades = [None; FADES];
        let mut next = 0;
        for handle in &h[..FADES] {
            fade_alpha(&mut fades, &mut next, *handle, 0, 100.0);
        }
        // One more: the first is forgotten, so it starts over.
        fade_alpha(&mut fades, &mut next, h[FADES], 500 * MS, 100.0);
        assert_eq!(
            fade_alpha(&mut fades, &mut next, h[0], 600 * MS, 100.0),
            0.0
        );
        // The third is still known (it was shown at 0 and is whole).
        assert_eq!(
            fade_alpha(&mut fades, &mut next, h[2], 600 * MS, 100.0),
            1.0
        );
    }

    #[test]
    fn the_thumbnail_sits_inside_the_row_and_the_text_makes_room() {
        let theme = Theme::default();
        let layout = SongSelectLayout::new(1280.0, 720.0, &theme);
        let g = ThumbGeometry::new(&theme, &layout);
        let row = layout.row_height - theme.song_row_gap * layout.scale;
        assert!(g.height > 0.0 && g.height < row);
        assert!((g.height + 2.0 * g.inset - row).abs() < 1e-3);
        assert!((g.width / g.height - theme.song_thumbnail_aspect).abs() < 1e-3);
        assert!((g.text_shift - (g.inset + g.width)).abs() < 1e-3);
        // Cut at the list's edges; nothing outside gives none.
        let area = Rect {
            x: 0.0,
            y: 100.0,
            width: 300.0,
            height: row,
        };
        let (top, bottom) = g.span(&area, 0.0, 1000.0).unwrap();
        assert!((top - (100.0 + g.inset)).abs() < 1e-3 && (bottom - top - g.height).abs() < 1e-3);
        let (top, _) = g.span(&area, 120.0, 1000.0).unwrap();
        assert_eq!(top, 120.0);
        assert!(g.span(&area, 0.0, 100.0).is_none());
    }
}

/// A list of one set whose difficulties have the given keys, with star ratings in the
/// order of `stars` (`None` for unknown).
fn keyed_list(keys_and_stars: &[(u64, Option<f32>)]) -> Arc<SongList> {
    let mut one = set(1, keys_and_stars.len());
    for (row, &(key, stars)) in one.difficulties.iter_mut().zip(keys_and_stars) {
        row.key = key;
        row.stars = stars;
        row.version = format!("key {key}");
    }
    Arc::new(SongList { sets: vec![one] })
}

#[test]
fn reordering_a_set_keeps_the_selected_difficulty_and_scroll() {
    let mut s = SongSelect::new();
    put(&mut s, keyed_list(&[(10, None), (11, None), (12, None)]));
    press(&mut s, I::Down);
    press(&mut s, I::Down);
    assert_eq!(s.selected(), Some((0, 2)));
    let scroll = s.snapshot().scroll;
    // Stars arrive and put key 12 first.
    put(
        &mut s,
        keyed_list(&[(12, Some(1.0)), (10, Some(2.0)), (11, Some(3.0))]),
    );
    assert_eq!(s.selected(), Some((0, 0)));
    assert_eq!(s.snapshot().scroll, scroll);
    assert_eq!(s.snapshot().selected_stars(), Some(1.0));
}

#[test]
fn zero_keys_fall_back_to_the_position() {
    let mut s = screen(&[(1, 3)]);
    press(&mut s, I::Down);
    put(&mut s, list(&[(1, 3)]));
    assert_eq!(s.selected(), Some((0, 1)));
}

#[test]
fn view_focus_names_the_first_set_in_view() {
    let mut s = screen(&[(7, 2), (8, 2)]);
    assert_eq!(s.view_focus().map(|f| f.0), Some(7));
    assert!(s.view_focus().is_some_and(|f| f.1 >= 2));
    put(&mut s, Arc::default());
    assert_eq!(s.view_focus(), None);
}

#[test]
fn stars_are_written_as_two_decimals() {
    let mut text = String::from("Insane");
    push_stars(&mut text, 5.4321);
    assert_eq!(text, "Insane \u{2605}5.43");
}
