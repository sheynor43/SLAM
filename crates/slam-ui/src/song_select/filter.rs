//! The song select's search: the query the player types and the list of sets and
//! difficulties that match it.
//!
//! The matching follows osu!lazer's plain-word search. The query is split on whitespace;
//! a difficulty passes when every word is a case-insensitive substring of at least one of
//! its fields (difficulty name, creator, artist, title, their original-script forms,
//! source, tags). A set is shown when one of its difficulties passes, and only the passing
//! ones are listed under it.
//!
//! Case folding is done once off the update thread: [`search_text`] lowercases the fields
//! when the list is built. The words are lowercased the same way (per character, so that
//! both sides agree) into stack buffers when the query changes.

use super::{Row, SongList};

/// Bytes of the query the player can type.
pub const QUERY_CAPACITY: usize = 128;

/// Most words the filter looks at; the query cannot hold more (a word and a space take two
/// bytes of [`QUERY_CAPACITY`]).
const MAX_TERMS: usize = QUERY_CAPACITY / 2;

/// Bytes of the lowercased words. A lowercase form is at most one and a half times as long
/// as the character (`İ`), so the query's words fit; longer would be cut.
const TERM_BYTES: usize = QUERY_CAPACITY * 2;

/// The text the player typed: a fixed-capacity UTF-8 buffer, `Copy` so that the snapshot
/// carries it without allocating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    bytes: [u8; QUERY_CAPACITY],
    len: u8,
}

impl Default for SearchQuery {
    fn default() -> SearchQuery {
        SearchQuery {
            bytes: [0; QUERY_CAPACITY],
            len: 0,
        }
    }
}

impl SearchQuery {
    pub fn as_str(&self) -> &str {
        // The buffer only ever holds whole characters.
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Appends `c`; returns whether it fitted.
    pub fn push(&mut self, c: char) -> bool {
        let len = usize::from(self.len);
        let width = c.len_utf8();
        if len + width > QUERY_CAPACITY {
            return false;
        }
        c.encode_utf8(&mut self.bytes[len..]);
        self.len += width as u8;
        true
    }

    /// Removes the last character; returns whether there was one.
    pub fn pop(&mut self) -> bool {
        let Some(c) = self.as_str().chars().next_back() else {
            return false;
        };
        self.len -= c.len_utf8() as u8;
        true
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

/// The lowercased `fields` joined by `'\n'`, as [`super::SetRow::search`] and
/// [`super::DifficultyRow::search`] hold them. Allocates: call it off the update thread.
pub fn search_text(fields: &[&str]) -> String {
    let mut text = String::with_capacity(fields.iter().map(|f| f.len() + 1).sum());
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            text.push('\n');
        }
        text.extend(field.chars().flat_map(char::to_lowercase));
    }
    text
}

/// The sets and difficulties of a [`SongList`] that pass the query, as indices into it.
/// The buffers are sized for the list when the filter is made ([`SongFilter::for_list`]),
/// off the update thread, so that filtering never allocates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SongFilter {
    /// Indices of the visible sets, ascending.
    sets: Vec<usize>,
    /// Per visible set, where its difficulties end in `difficulties`.
    ends: Vec<usize>,
    /// Indices of the visible difficulties, ascending within a set.
    difficulties: Vec<usize>,
}

impl SongFilter {
    /// A filter with room for every set and difficulty of `list`, showing all of them.
    pub fn for_list(list: &SongList) -> SongFilter {
        let mut filter = SongFilter {
            sets: Vec::with_capacity(list.sets.len()),
            ends: Vec::with_capacity(list.sets.len()),
            difficulties: Vec::with_capacity(list.sets.iter().map(|s| s.difficulties.len()).sum()),
        };
        filter.apply(list, &SearchQuery::default());
        filter
    }

    /// Number of visible sets.
    pub fn set_count(&self) -> usize {
        self.sets.len()
    }

    /// Index into the list of the visible set at `position`.
    pub fn set_index(&self, position: usize) -> Option<usize> {
        self.sets.get(position).copied()
    }

    /// Position among the visible sets of the set at `index` in the list.
    pub fn set_position(&self, index: usize) -> Option<usize> {
        self.sets.binary_search(&index).ok()
    }

    fn range(&self, set: usize) -> std::ops::Range<usize> {
        let end = self.ends.get(set).copied().unwrap_or(0);
        let start = set.checked_sub(1).map_or(0, |prev| self.ends[prev]);
        start..end
    }

    /// Number of visible difficulties of the visible set at `set`.
    pub fn difficulty_count(&self, set: usize) -> usize {
        self.range(set).len()
    }

    /// Index into the set of the visible difficulty at `position` of the visible set `set`;
    /// `position` itself when there is none (a set without difficulties).
    pub fn difficulty_index(&self, set: usize, position: usize) -> usize {
        let range = self.range(set);
        self.difficulties
            .get(range.start + position)
            .filter(|_| range.start + position < range.end)
            .copied()
            .unwrap_or(position)
    }

    /// Position among the visible difficulties of the visible set `set` of the difficulty at
    /// `index` in the list's set.
    pub fn difficulty_position(&self, set: usize, index: usize) -> Option<usize> {
        let range = self.range(set);
        self.difficulties[range].binary_search(&index).ok()
    }

    /// Number of rows when the visible set at `selected_set` is expanded.
    pub fn row_count(&self, selected_set: usize) -> usize {
        self.sets.len() + self.difficulty_count(selected_set)
    }

    /// The row at flat index `index` when the visible set at `selected_set` is expanded,
    /// with indices into the list.
    pub fn row(&self, selected_set: usize, index: usize) -> Option<Row> {
        let below = self.difficulty_count(selected_set);
        let set_row = |position: usize| self.sets.get(position).map(|&set| Row::Set(set));
        if index <= selected_set {
            set_row(index)
        } else if index <= selected_set + below {
            Some(Row::Difficulty {
                set: *self.sets.get(selected_set)?,
                difficulty: self.difficulty_index(selected_set, index - selected_set - 1),
            })
        } else {
            set_row(index - below)
        }
    }

    /// Recomputes the visible sets of `list` for `query`. Does not allocate while the
    /// buffers have the capacity [`SongFilter::for_list`] gave them.
    pub fn apply(&mut self, list: &SongList, query: &SearchQuery) {
        self.sets.clear();
        self.ends.clear();
        self.difficulties.clear();

        // The words, lowercased into stack buffers.
        let mut buf = [0u8; TERM_BYTES];
        let mut len = 0;
        let mut spans = [(0, 0); MAX_TERMS];
        let mut count = 0;
        let mut start = None;
        for c in query.as_str().chars().chain([' ']) {
            if c.is_whitespace() {
                if let Some(start) = start.take()
                    && count < MAX_TERMS
                {
                    spans[count] = (start, len);
                    count += 1;
                }
                continue;
            }
            start.get_or_insert(len);
            for lower in c.to_lowercase() {
                let width = lower.len_utf8();
                if len + width <= TERM_BYTES {
                    lower.encode_utf8(&mut buf[len..]);
                    len += width;
                }
            }
        }
        let text = std::str::from_utf8(&buf[..len]).unwrap_or("");
        let mut terms = [""; MAX_TERMS];
        for (term, &(from, to)) in terms.iter_mut().zip(&spans[..count]) {
            *term = &text[from..to];
        }
        let terms = &terms[..count];

        for (index, set) in list.sets.iter().enumerate() {
            if terms.is_empty() {
                self.difficulties.extend(0..set.difficulties.len());
            } else {
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/BeatmapInfoExtensions.cs (Match)
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/BeatmapMetadataInfoExtensions.cs (Match)
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterCriteria.cs (OptionalTextFilter)
                // Words found in the set's own fields hold for all of its difficulties.
                let mut in_set = 0u64;
                for (i, term) in terms.iter().enumerate() {
                    if set.search.contains(term) {
                        in_set |= 1 << i;
                    }
                }
                let first = self.difficulties.len();
                for (d, difficulty) in set.difficulties.iter().enumerate() {
                    let passes = terms
                        .iter()
                        .enumerate()
                        .all(|(i, term)| in_set >> i & 1 != 0 || difficulty.search.contains(term));
                    if passes {
                        self.difficulties.push(d);
                    }
                }
                if self.difficulties.len() == first {
                    continue;
                }
            }
            self.sets.push(index);
            self.ends.push(self.difficulties.len());
        }
    }
}
