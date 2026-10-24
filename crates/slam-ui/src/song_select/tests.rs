use super::SongSelectInput as I;
use super::*;

fn set(key: u64, diffs: usize) -> SetRow {
    SetRow {
        key,
        artist: format!("artist {key}"),
        title: format!("title {key}"),
        creator: "mapper".to_owned(),
        search: search_text(&["mapper", &format!("artist {key}"), &format!("title {key}")]),
        difficulties: (0..diffs)
            .map(|d| DifficultyRow {
                version: format!("diff {d}"),
                search: format!("diff {d}"),
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
    assert_eq!(press(&mut s, I::PointerPress { x, y }), None);
    assert_eq!(s.selected(), Some((1, 0)));
    // its second difficulty is row 3 again (set 1 is now row 1, difficulties 2 to 4)
    let (x, y) = centre_of(&l, 3, 0.0);
    assert_eq!(press(&mut s, I::PointerPress { x, y }), None);
    assert_eq!(s.selected(), Some((1, 1)));
    assert_eq!(
        press(&mut s, I::PointerPress { x, y }),
        Some(SongSelectAction::Play {
            set: 1,
            difficulty: 1
        })
    );
    // outside the list: nothing
    assert_eq!(press(&mut s, I::PointerPress { x: 1.0, y: 1.0 }), None);
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
    let scroll = s.snapshot().scroll;
    assert_eq!(scroll, 10.5);
    assert_eq!(s.selected(), Some((0, 0)));
    let (x, y) = centre_of(&l, 20, scroll);
    assert_eq!(l.row_at(x, y, scroll, s.filter(), 0), Some(Row::Set(19)));
    press(&mut s, I::PointerPress { x, y });
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
    assert_eq!(
        s.input(I::PointerPress { x, y }, &l),
        Some(SongSelectAction::ChooseFolder)
    );
    // Outside the button: nothing.
    assert_eq!(s.input(I::PointerPress { x: 1.0, y: 1.0 }, &l), None);
    assert!(!s.snapshot().folder_hovered);
    // While scanning the button is absent.
    s.set_status(SongSelectStatus::Scanning { sets: 0 });
    assert_eq!(s.input(I::Activate, &l), None);
    assert_eq!(s.input(I::PointerPress { x, y }, &l), None);
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
