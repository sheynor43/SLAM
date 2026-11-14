//! Searching with the text typed on the other keyboard layout (ADR-0137).

use super::query::Criteria;
use super::*;

fn titled(title: &str, artist: &str) -> DifficultyRow {
    DifficultyRow::new(SearchSource {
        title,
        artist,
        ..SearchSource::default()
    })
}

fn kept(rows: &[DifficultyRow], query: &str) -> Vec<usize> {
    let criteria = Criteria::parse(query);
    (0..rows.len())
        .filter(|&i| criteria.matches(&rows[i]))
        .collect()
}

fn passes(row: &DifficultyRow, query: &str) -> bool {
    Criteria::parse(query).matches(row)
}

#[test]
fn other_keyboard_layout_finds_the_same_text() {
    let rows = [
        titled("Through the Fire and Flames", "DragonForce"),
        titled("привет", "Артист"),
    ];
    assert_eq!(kept(&rows, "еркщгпр еру ашку фтв адфьуы"), [0]);
    assert_eq!(kept(&rows, "ghbdtn"), [1]);
    assert_eq!(kept(&rows, "GHBDTN"), [1]);
    // A punctuation key that carries a letter.
    assert_eq!(kept(&[titled("орех", "")], "jht["), [0]);
    // Typed correctly, nothing changes.
    assert_eq!(kept(&rows, "through fire"), [0]);
    assert_eq!(kept(&rows, "привет"), [1]);
    assert_eq!(kept(&rows, "zzzz"), Vec::<usize>::new());
}

#[test]
fn layout_swap_is_per_term_and_leaves_filters_alone() {
    let mut row = titled("Through the Fire", "Artist");
    row.approach_rate = 9.5;
    // One term typed on the wrong layout, the other right.
    assert!(passes(&row, "еркщгпр fire"));
    // Filters typed correctly still apply and are not swapped.
    assert!(passes(&row, "ar>9 еркщгпр"));
    assert!(!passes(&row, "ar<9 еркщгпр"));
    // A keyword value is not swapped.
    assert!(!passes(&row, "artist=Фкешые"));
    assert!(passes(&row, "artist=artist"));
}

#[test]
fn quoted_phrases_are_swapped_too() {
    let row = titled("Through the Fire", "");
    assert!(passes(&row, "\"еру аШку\""));
    assert!(!passes(&row, "\"ер\""));
}
