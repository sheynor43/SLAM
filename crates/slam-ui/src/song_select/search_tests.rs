use super::SongSelectInput as I;
use super::*;

/// The example beatmap of lazer's `FilterMatchingTest` plus a second difficulty.
fn example_set(key: u64) -> SetRow {
    let diff = |version: &str| DifficultyRow {
        version: version.to_owned(),
        search: search_text(&[version]),
    };
    SetRow {
        key,
        artist: "The Artist".to_owned(),
        title: "Title goes here".to_owned(),
        creator: "The Author".to_owned(),
        search: search_text(&[
            "The Author",
            "The Artist",
            "check unicode too",
            "Title goes here",
            "TitleUnicode goes here",
            "unit tests",
            "look for tags too",
        ]),
        difficulties: vec![diff("version as well"), diff("Extra")],
    }
}

fn plain_set(key: u64, artist: &str, title: &str, versions: &[&str]) -> SetRow {
    SetRow {
        key,
        artist: artist.to_owned(),
        title: title.to_owned(),
        creator: String::new(),
        search: search_text(&["", artist, title]),
        difficulties: versions
            .iter()
            .map(|v| DifficultyRow {
                version: (*v).to_owned(),
                search: search_text(&[v]),
            })
            .collect(),
    }
}

fn query(text: &str) -> SearchQuery {
    let mut q = SearchQuery::default();
    for c in text.chars() {
        assert!(q.push(c));
    }
    q
}

/// The visible `(set index, difficulty indices)` of `sets` for `text`.
fn visible(sets: Vec<SetRow>, text: &str) -> Vec<(usize, Vec<usize>)> {
    let list = SongList { sets };
    let mut filter = SongFilter::for_list(&list);
    filter.apply(&list, &query(text));
    (0..filter.set_count())
        .map(|set| {
            let diffs = (0..filter.difficulty_count(set))
                .map(|d| filter.difficulty_index(set, d))
                .collect();
            (filter.set_index(set).unwrap(), diffs)
        })
        .collect()
}

#[test]
fn query_is_a_bounded_char_buffer() {
    let mut q = SearchQuery::default();
    assert!(q.is_empty());
    assert!(!q.pop());
    for c in "a\u{44b}\u{1f600}".chars() {
        assert!(q.push(c));
    }
    assert_eq!(q.as_str(), "a\u{44b}\u{1f600}");
    assert!(q.pop());
    assert_eq!(q.as_str(), "a\u{44b}");
    assert!(q.pop() && q.pop() && q.is_empty());
    // Full: further characters are dropped, a wide one that does not fit too.
    for _ in 0..QUERY_CAPACITY - 1 {
        assert!(q.push('x'));
    }
    assert!(!q.push('\u{44b}'));
    assert!(q.push('y'));
    assert!(!q.push('z'));
    assert_eq!(q.as_str().len(), QUERY_CAPACITY);
    q.clear();
    assert_eq!(q.as_str(), "");
}

#[test]
fn matching_terms_follow_lazer() {
    // Ported from osu!lazer 2026.1005.0-lazer:
    // osu.Game.Tests/NonVisual/Filtering/FilterMatchingTest.cs (TestCriteriaMatchingTerms)
    // (terms, shown)
    let cases = [
        ("artist", true),
        ("artist title author", true),
        ("an artist", false),
        ("tags too", true),
        ("version", true),
        ("an auteur", false),
        ("unit", true),
        // empty and blank queries show everything
        ("", true),
        ("   ", true),
        // case-insensitive, any field, any order
        ("ARTIST", true),
        ("TITLEUNICODE goes", true),
        ("tests author", true),
        ("CHECK Unicode", true),
        // a word does not match across fields
        ("authorthe", false),
        ("hereunit", false),
        // a part of a word is enough
        ("goe", true),
    ];
    for (terms, shown) in cases {
        let got = !visible(vec![example_set(1)], terms).is_empty();
        assert_eq!(got, shown, "{terms:?}");
    }
}

#[test]
fn case_folding_covers_cyrillic_and_unicode_fields() {
    let mut set = plain_set(
        1,
        "\u{41f}\u{440}\u{438}\u{43c}\u{435}\u{440}",
        "Title",
        &["Easy"],
    );
    set.search = search_text(&[
        "",
        "\u{41f}\u{440}\u{438}\u{43c}\u{435}\u{440}",
        "\u{418}\u{43c}\u{44f} \u{43f}\u{435}\u{441}\u{43d}\u{438}",
    ]);
    for terms in [
        "\u{43f}\u{440}\u{438}",
        "\u{41f}\u{420}\u{418}",
        "\u{418}\u{41c}\u{42f} \u{43f}\u{420}\u{418}\u{43c}",
    ] {
        assert_eq!(visible(vec![set.clone()], terms).len(), 1, "{terms:?}");
    }
    assert!(visible(vec![set], "\u{43f}\u{440}\u{43e}").is_empty());
    // The final sigma folds like any other, on both sides.
    let greek = plain_set(
        1,
        "\u{39f}\u{394}\u{3a5}\u{3a3}\u{3a3}\u{395}\u{3a5}\u{3a3}",
        "x",
        &["d"],
    );
    assert_eq!(visible(vec![greek.clone()], "\u{3a3}").len(), 1);
    assert_eq!(
        visible(
            vec![greek],
            "\u{3bf}\u{3b4}\u{3c5}\u{3c3}\u{3c3}\u{3b5}\u{3c5}\u{3c3}"
        )
        .len(),
        1
    );
}

#[test]
fn a_difficulty_name_hides_the_other_difficulties() {
    let sets = vec![
        example_set(1),
        plain_set(2, "Other", "Song", &["Insane", "Easy"]),
    ];
    assert_eq!(visible(sets.clone(), "insane"), vec![(1, vec![0])]);
    assert_eq!(visible(sets.clone(), "extra"), vec![(0, vec![1])]);
    assert_eq!(visible(sets.clone(), "easy"), vec![(1, vec![1])]);
    // A word of the set shows all of its difficulties; a word of a difficulty narrows them.
    assert_eq!(visible(sets.clone(), "artist"), vec![(0, vec![0, 1])]);
    assert_eq!(visible(sets.clone(), "artist well"), vec![(0, vec![0])]);
    assert_eq!(visible(sets.clone(), "artist extra"), vec![(0, vec![1])]);
    // Words of two difficulties never pass together.
    assert!(visible(sets.clone(), "well extra").is_empty());
    assert_eq!(visible(sets.clone(), "").len(), 2);
    assert_eq!(visible(sets, "other song easy"), vec![(1, vec![1])]);
}

#[test]
fn sets_without_difficulties_show_only_without_a_query() {
    let sets = vec![plain_set(1, "a", "b", &[])];
    assert_eq!(visible(sets.clone(), ""), vec![(0, vec![])]);
    assert!(visible(sets, "a").is_empty());
}

fn searchable(sets: Vec<SetRow>) -> SongSelect {
    let mut s = SongSelect::new();
    let list = Arc::new(SongList { sets });
    let filter = SongFilter::for_list(&list);
    s.set_list(list, filter);
    s
}

fn songs() -> Vec<SetRow> {
    vec![
        plain_set(1, "Alpha", "One", &["Easy", "Hard"]),
        plain_set(2, "Beta", "Two", &["Easy", "Insane"]),
        plain_set(3, "Gamma", "Three", &["Hard"]),
        plain_set(4, "Delta", "Hardcore", &["Normal", "Hard"]),
    ]
}

fn press(s: &mut SongSelect, input: SongSelectInput) -> Option<SongSelectAction> {
    s.input(
        input,
        &SongSelectLayout::new(1280.0, 720.0, &Theme::default()),
    )
}

fn type_text(s: &mut SongSelect, text: &str) {
    for c in text.chars() {
        assert_eq!(press(s, I::Type(c)), None);
    }
}

#[test]
fn typing_filters_and_space_needs_a_word() {
    let mut s = searchable(songs());
    assert_eq!(press(&mut s, I::Type(' ')), None);
    assert!(s.query().is_empty());
    type_text(&mut s, "hard");
    assert_eq!(s.query().as_str(), "hard");
    // Sets 1, 3 and 4 match; 4 by its title, with all its difficulties.
    assert_eq!(s.filter().set_count(), 3);
    assert_eq!(s.snapshot().row_count, 3 + 1);
    type_text(&mut s, " \t\u{7}");
    assert_eq!(s.query().as_str(), "hard ");
    type_text(&mut s, "two");
    assert_eq!(s.filter().set_count(), 0);
    assert_eq!(s.selected(), None);
    assert_eq!(press(&mut s, I::Activate), None);
    assert_eq!(s.snapshot().row_count, 0);
}

#[test]
fn erase_and_backspace_remove_characters_before_mods() {
    let mut s = searchable(songs());
    type_text(&mut s, "ab");
    assert_eq!(press(&mut s, I::Erase), None);
    assert_eq!(s.query().as_str(), "a");
    assert_eq!(press(&mut s, I::DeselectMods), None);
    assert!(s.query().is_empty());
    assert_eq!(s.filter().set_count(), 4);
    // With no query Backspace deselects the mods, and a repeat does nothing.
    assert_eq!(
        press(&mut s, I::DeselectMods),
        Some(SongSelectAction::DeselectMods)
    );
    assert_eq!(press(&mut s, I::Erase), None);
}

#[test]
fn back_clears_the_query_before_leaving() {
    let mut s = searchable(songs());
    type_text(&mut s, "beta");
    assert_eq!(s.filter().set_count(), 1);
    assert_eq!(press(&mut s, I::Back), None);
    assert!(s.query().is_empty());
    assert_eq!(s.filter().set_count(), 4);
    assert_eq!(press(&mut s, I::Back), Some(SongSelectAction::Back));
    // Loading: typing is ignored, Back leaves.
    type_text(&mut s, "x");
    s.set_status(SongSelectStatus::Loading);
    assert_eq!(press(&mut s, I::Type('y')), None);
    assert_eq!(press(&mut s, I::Erase), None);
    assert_eq!(s.query().as_str(), "x");
    assert_eq!(press(&mut s, I::Back), Some(SongSelectAction::Back));
}

#[test]
fn typed_text_types_like_its_characters_and_filters_once() {
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let mut by_chars = searchable(songs());
    type_text(&mut by_chars, "hard ins");
    let mut by_text = searchable(songs());
    by_text.type_text(" ", &layout);
    by_text.type_text("hard ", &layout);
    by_text.type_text("ins", &layout);
    assert_eq!(by_text.query(), by_chars.query());
    assert_eq!(by_text.filter().set_count(), by_chars.filter().set_count());
    assert_eq!(by_text.selected(), by_chars.selected());
    // The open panel and a load take no text.
    press(&mut by_text, I::ToggleMods);
    by_text.type_text("x", &layout);
    assert_eq!(by_text.query().as_str(), "hard ins");
    press(&mut by_text, I::Back);
    by_text.set_status(SongSelectStatus::Loading);
    by_text.type_text("x", &layout);
    assert_eq!(by_text.query().as_str(), "hard ins");
}

#[test]
fn the_mod_panel_takes_the_letters() {
    let mut s = searchable(songs());
    press(&mut s, I::ToggleMods);
    assert!(s.mods().is_open());
    assert_eq!(press(&mut s, I::Type('a')), None);
    assert_eq!(press(&mut s, I::Erase), None);
    assert!(s.query().is_empty());
    press(&mut s, I::Back);
    type_text(&mut s, "a");
    press(&mut s, I::ToggleMods);
    // Backspace is the panel's own then, and Back closes it without touching the query.
    assert_eq!(
        press(&mut s, I::DeselectMods),
        Some(SongSelectAction::DeselectMods)
    );
    assert_eq!(s.query().as_str(), "a");
    assert_eq!(press(&mut s, I::Back), None);
    assert!(!s.mods().is_open());
    assert_eq!(s.query().as_str(), "a");
}

#[test]
fn selection_stays_or_moves_when_the_query_changes() {
    let mut s = searchable(songs());
    // Select Delta / Hard: set 3, difficulty 1.
    press(&mut s, I::End);
    press(&mut s, I::Down);
    assert_eq!(s.selected(), Some((3, 1)));
    type_text(&mut s, "h");
    assert_eq!(s.selected(), Some((3, 1)), "still visible");
    type_text(&mut s, "ard");
    assert_eq!(s.selected(), Some((3, 1)));
    type_text(&mut s, "core");
    // Only Delta matches, and Hard is not hidden by "hardcore" (it is in the title).
    assert_eq!(s.selected(), Some((3, 1)));
    // Only Delta's Normal difficulty has "normal": the set stays, the difficulty moves.
    for _ in 0..8 {
        press(&mut s, I::Erase);
    }
    type_text(&mut s, "normal");
    assert_eq!(s.filter().set_count(), 1);
    assert_eq!(s.selected(), Some((3, 0)));
    // The set goes away: the first visible set is selected.
    press(&mut s, I::Back);
    type_text(&mut s, "beta");
    assert_eq!(s.selected(), Some((1, 0)));
    // Clearing keeps the selection where it is.
    press(&mut s, I::Back);
    assert_eq!(s.selected(), Some((1, 0)));
}

#[test]
fn the_selection_stays_in_view_after_filtering() {
    let sets: Vec<SetRow> = (0..60)
        .map(|k| plain_set(k, &format!("artist {k}"), &format!("t{}", k % 3), &["d"]))
        .collect();
    let mut s = searchable(sets);
    press(&mut s, I::End);
    assert!(s.snapshot().scroll > 10.0);
    type_text(&mut s, "t1");
    let snap = s.snapshot();
    assert_eq!(snap.row_count, 20 + 1);
    assert!(snap.scroll + 12.0 >= s.selected_row() as f32);
    assert!(s.selected_row() as f32 >= snap.scroll);
}

#[test]
fn set_list_keeps_the_query() {
    let mut s = searchable(songs());
    type_text(&mut s, "gamma");
    assert_eq!(s.selected(), Some((2, 0)));
    let list = Arc::new(SongList {
        sets: {
            let mut v = vec![plain_set(9, "Gamma Ray", "New", &["a"])];
            v.extend(songs());
            v
        },
    });
    let filter = SongFilter::for_list(&list);
    let old = s.set_list(list, filter);
    assert_eq!(old.set_count(), 1);
    assert_eq!(s.query().as_str(), "gamma");
    assert_eq!(s.filter().set_count(), 2);
    // The selected set, now at index 3, is still the selected one.
    assert_eq!(s.selected(), Some((3, 0)));
    assert_eq!(
        s.snapshot().rows(),
        [
            Row::Set(0),
            Row::Set(3),
            Row::Difficulty {
                set: 3,
                difficulty: 0
            }
        ]
    );
}

#[test]
fn play_and_pointer_use_indices_into_the_list() {
    let mut s = searchable(songs());
    type_text(&mut s, "hard");
    // Visible: Alpha (Hard only? no: "hard" is not in the set fields, only in the version).
    assert_eq!(s.snapshot().rows().len(), 4);
    assert_eq!(
        press(&mut s, I::Activate),
        Some(SongSelectAction::Play {
            set: 0,
            difficulty: 1
        })
    );
    press(&mut s, I::Down);
    press(&mut s, I::Down);
    assert_eq!(s.selected(), Some((3, 0)));
    assert_eq!(
        press(&mut s, I::Activate),
        Some(SongSelectAction::Play {
            set: 3,
            difficulty: 0
        })
    );
    // Clicking the row of a difficulty selects it and a second click plays it.
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let rows: Vec<Row> = s.snapshot().rows().to_vec();
    let at = rows
        .iter()
        .position(|r| {
            *r == Row::Difficulty {
                set: 3,
                difficulty: 1,
            }
        })
        .unwrap();
    let (x, y) = (
        layout.list.x + layout.list.width / 2.0,
        layout.row_y(at, 0.0) + layout.row_height / 2.0,
    );
    assert_eq!(press(&mut s, I::PointerMove { x, y }), None);
    assert_eq!(
        s.snapshot().hovered,
        Some(Row::Difficulty {
            set: 3,
            difficulty: 1
        })
    );
    press(&mut s, I::PointerPress { x, y });
    assert_eq!(s.selected(), Some((3, 1)));
    assert_eq!(
        press(&mut s, I::PointerPress { x, y }),
        Some(SongSelectAction::Play {
            set: 3,
            difficulty: 1
        })
    );
}

#[test]
fn snapshot_rows_cover_the_list_area() {
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    for (w, h) in [
        (640.0, 360.0),
        (1280.0, 720.0),
        (1920.0, 1080.0),
        (3840.0, 2160.0),
        (800.0, 1600.0),
    ] {
        let l = SongSelectLayout::new(w, h, &Theme::default());
        assert!(
            (l.viewport_rows().ceil() as usize) < MAX_VISIBLE_ROWS,
            "{w}x{h}: {}",
            l.viewport_rows()
        );
    }
    let sets: Vec<SetRow> = (0..100).map(|k| plain_set(k, "a", "b", &["d"])).collect();
    let mut s = searchable(sets);
    s.tick(0, &layout);
    press(&mut s, I::Scroll { lines: 7.5 });
    let snap = s.snapshot();
    assert_eq!(snap.first_row, 7);
    let shown = layout.visible_rows(snap.scroll, snap.row_count);
    assert!(shown.start >= snap.first_row);
    assert!(shown.end <= snap.first_row + snap.rows().len());
    assert_eq!(snap.rows()[0], Row::Set(6), "set 0 is expanded");
}
