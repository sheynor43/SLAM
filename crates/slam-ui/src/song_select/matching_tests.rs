//! Matching a difficulty against a query, against lazer's `FilterMatchingTest` and the
//! matching half of `FilterQueryParserTest`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/NonVisual/Filtering/FilterMatchingTest.cs
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/NonVisual/Filtering/FilterQueryParserTest.cs (TestDifficultySearch)

use super::query::Criteria;
use super::search_tests::{example_difficulty, example_source};
use super::*;

/// Whether lazer's test would call the example beatmap `Filtered` for `query`.
fn filtered(query: &str) -> bool {
    filtered_row(&example_difficulty("version as well"), query)
}

fn filtered_row(row: &DifficultyRow, query: &str) -> bool {
    !Criteria::parse(query).matches(row)
}

/// The indices of `rows` that `query` keeps.
fn kept(rows: &[DifficultyRow], query: &str) -> Vec<usize> {
    let criteria = Criteria::parse(query);
    (0..rows.len())
        .filter(|&i| criteria.matches(&rows[i]))
        .collect()
}

#[test]
fn ranges_follow_inclusiveness() {
    // TestCriteriaMatchingRangeMin: AR 5 against `Min = 5`.
    assert!(filtered("ar>5"));
    assert!(!filtered("ar>=5"));
    assert!(!filtered("ar=5"));
    assert!(filtered("ar!=5"));
    assert!(!filtered("ar<5.1 ar>4.9"));
    assert!(!filtered("hp=3 cs=2 od=5"));
    assert!(filtered("cs>2.05"));
}

#[test]
fn terms() {
    for (query, expected) in [
        ("artist", false),
        ("artist title author", false),
        ("an artist", true),
        ("tags too", false),
        ("version", false),
        ("an auteur", true),
        ("unit", false),
    ] {
        assert_eq!(filtered(query), expected, "{query}");
    }
}

#[test]
fn exact_terms() {
    for (query, expected) in [
        ("\"artist\"", false),
        ("\"arti\"", true),
        ("\"artist title author\"", true),
        ("\"artist\" \"title\" \"author\"", false),
        ("\"an artist\"", true),
        ("\"tags too\"", false),
        ("\"tags to\"", true),
        ("\"version\"", false),
        ("\"an auteur\"", true),
        ("\"Artist\"!", true),
        ("\"The Artist\"!", false),
        ("\"the artist\"!", false),
        ("\"unit tests\"!", false),
        ("\"\\\"", true),
    ] {
        assert_eq!(filtered(query), expected, "{query}");
    }
}

#[test]
fn creator() {
    for (term, expected) in [
        ("The", false),
        ("THE", false),
        ("author", false),
        ("\"the author\"", false),
        ("unknown", true),
    ] {
        assert_eq!(filtered(&format!("creator={term}")), expected, "{term}");
    }
    // An empty value is no filter.
    assert!(!filtered("creator="));
}

#[test]
fn terms_next_to_punctuation_and_symbols() {
    for title in ["the artist \"quoted words\"", "the artist ~quoted words~"] {
        let row = DifficultyRow::new(SearchSource {
            difficulty: "d",
            title,
            ..SearchSource::default()
        });
        let (open, close) = if title.contains('~') {
            ("~", "~")
        } else {
            ("\"", "\"")
        };
        let quoted = format!("{open}quoted words{close}");
        let unknown = format!("{open}unknown{close}");
        assert!(!filtered_row(&row, &quoted), "{title}");
        assert!(!filtered_row(&row, "\"the artist\""), "{title}");
        assert!(
            !filtered_row(&row, &format!("the artist {quoted}")),
            "{title}"
        );
        assert!(filtered_row(&row, &unknown), "{title}");
    }
}

#[test]
fn title_and_artist() {
    for (query, expected) in [
        ("title=Goes", false),
        ("title=GOES", false),
        ("title=\"title goes\"", false),
        ("title=titleunicode", false),
        ("title=unknown", true),
        ("artist=The", false),
        ("artist=artist", false),
        ("artist=\"the artist\"", false),
        ("artist=\"unicode too\"", false),
        ("artist=unknown", true),
        ("artist=\"Artist\"!", true),
        ("artist=\"The Artist\"!", false),
        ("artist=\"the artist\"!", false),
    ] {
        assert_eq!(filtered(query), expected, "{query}");
    }
    let no_unicode = DifficultyRow::new(SearchSource {
        artist_unicode: "",
        ..example_source("v")
    });
    assert!(!filtered_row(&no_unicode, "artist=artist"));
    assert!(filtered_row(&no_unicode, "artist=unknown"));
}

#[test]
fn excluding_artist_needs_both_forms_to_differ() {
    // TestCriteriaNotMatchingArtist
    assert!(filtered("artist!=artist"));
    assert!(filtered("artist!=unicode"));
    assert!(!filtered("artist!=unknown"));
}

#[test]
fn beatmap_ids() {
    let row = DifficultyRow {
        beatmap_id: 20_201_010,
        beatmap_set_id: 1535,
        ..example_difficulty("v")
    };
    for (query, expected) in [
        ("202010", true),
        ("20201010", false),
        ("153", true),
        ("1535", false),
    ] {
        assert_eq!(filtered_row(&row, query), expected, "{query}");
    }
    // A number found in the text matches by text.
    let numbered = DifficultyRow::new(SearchSource {
        title: "Song 2",
        ..SearchSource::default()
    });
    assert!(!filtered_row(&numbered, "2"));
}

#[test]
fn tags_stand_in_for_user_tags() {
    // lazer's user tags are "song representation/simple" and "style/clean"; the file's
    // tags are words (ADR-0099).
    let row = DifficultyRow::new(SearchSource {
        tags: "simple clean-style",
        ..example_source("v")
    });
    for (query, expected) in [
        ("tag=simple", false),
        ("tag=\"clean-style\"", false),
        ("tag=\"clean-style\"!", false),
        ("tag=\"clean\"!", true),
        ("tag=iNiS-style", true),
        ("tag=\"simple\"! tag=\"clean-style\"!", false),
        ("tag=\"simple\"! tag=\"dirty\"!", true),
        ("tag!=\"simple\"!", true),
        ("tag=\"simple\"! tag!=\"clean-style\"!", true),
        ("tag!=dirty", false),
    ] {
        assert_eq!(filtered_row(&row, query), expected, "{query}");
    }
    let untagged = DifficultyRow::new(SearchSource {
        tags: "",
        ..example_source("v")
    });
    assert!(filtered_row(&untagged, "tag=simple"));
    assert!(!filtered_row(&untagged, "tag!=simple"));
}

#[test]
fn not_equal_text_filters() {
    let rows: Vec<_> = [
        "Title1",
        "Title1",
        "My[Favourite]Song",
        "Title",
        "Another One",
        "Diff in title",
        "a",
    ]
    .iter()
    .map(|title| {
        DifficultyRow::new(SearchSource {
            difficulty: "d",
            title,
            ..SearchSource::default()
        })
    })
    .collect();
    assert_eq!(kept(&rows, "title!=Title"), [2, 4, 6]);
    assert_eq!(kept(&rows, "title!=\"Title1\""), [2, 3, 4, 5, 6]);
    assert_eq!(kept(&rows, "title!=\"Title1\"!"), [2, 3, 4, 5, 6]);
}

#[test]
fn filters_without_data_hide_nothing() {
    for query in [
        "star>5",
        "bpm=180",
        "length<1:00",
        "divisor=4",
        "status=ranked",
        "lastplayed<5d",
        "played=1",
        "ranked>2012",
        "submitted<2020",
    ] {
        let c = Criteria::parse(query);
        assert!(c.terms().is_empty(), "{query}");
        assert!(!filtered(query), "{query}");
    }
}

#[test]
fn difficulty_search() {
    // (title, difficulty name)
    let rows: Vec<_> = [
        ("Title1", "Diff1"),
        ("Title1", "Diff2"),
        ("My[Favourite]Song", "Expert"),
        ("Title", "My Favourite Diff"),
        ("Another One", "diff ]with [[ brackets]]]"),
        ("Diff in title", "a"),
        ("a", "Diff in diff"),
    ]
    .iter()
    .map(|(title, difficulty)| {
        DifficultyRow::new(SearchSource {
            difficulty,
            title,
            ..SearchSource::default()
        })
    })
    .collect();
    let cases: [(&str, &[usize]); 19] = [
        ("[1]", &[0]),
        ("[1", &[0]),
        ("My[Favourite", &[2]),
        ("My[Favourite]", &[2]),
        ("My[Favourite]Song", &[2]),
        ("Favourite]", &[2]),
        ("[Diff", &[0, 1, 3, 4, 6]),
        ("[Diff]", &[0, 1, 3, 4, 6]),
        ("[Favourite]", &[3]),
        ("Title1 [Diff]", &[0, 1]),
        ("Title1[Diff]", &[]),
        ("[diff ]with]", &[4]),
        ("[diff ]with [[ brackets]]]]", &[4]),
        ("[Diff in title]", &[]),
        ("[Diff in diff]", &[6]),
        ("diff=Diff", &[0, 1, 3, 4, 6]),
        ("diff=Diff1", &[0]),
        ("diff=\"Diff\"", &[3, 4, 6]),
        ("diff=!\"Diff\"", &[]),
    ];
    for (query, expected) in cases {
        assert_eq!(kept(&rows, query), expected, "{query}");
    }
}
