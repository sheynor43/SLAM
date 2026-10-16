use super::SongSelectInput as I;
use super::*;

fn set(key: u64, diffs: usize) -> SetRow {
    SetRow {
        key,
        artist: format!("artist {key}"),
        title: format!("title {key}"),
        creator: "mapper".to_owned(),
        difficulties: (0..diffs)
            .map(|d| DifficultyRow {
                version: format!("diff {d}"),
            })
            .collect(),
    }
}

fn list(sets: &[(u64, usize)]) -> Arc<SongList> {
    Arc::new(SongList {
        sets: sets.iter().map(|&(k, d)| set(k, d)).collect(),
    })
}

fn layout() -> SongSelectLayout {
    SongSelectLayout::new(1280.0, 720.0, &Theme::default())
}

fn screen(sets: &[(u64, usize)]) -> SongSelect {
    let mut s = SongSelect::new();
    s.set_list(list(sets));
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
    s.set_list(list(&[(9, 1), (3, 1), (8, 1), (2, 3), (1, 2)]));
    assert_eq!(s.selected(), Some((3, 1)));
    // the set lost a difficulty: the index is clamped
    s.set_list(list(&[(2, 1)]));
    assert_eq!(s.selected(), Some((0, 0)));
    // the set is gone: the position is kept, clamped
    s.set_list(list(&[(5, 1), (6, 1)]));
    assert_eq!(s.selected(), Some((0, 0)));
    s.set_list(Arc::default());
    assert_eq!(s.selected(), None);
}

#[test]
fn row_mapping_expands_the_selected_set() {
    let l = list(&[(1, 2), (2, 3), (3, 1)]);
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
    assert_eq!(
        l.row_at(x, y, scroll, &s.snapshot().list, 0),
        Some(Row::Set(19))
    );
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
        let row = (snap.selected_set + 1 + snap.selected_difficulty) as f32;
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
