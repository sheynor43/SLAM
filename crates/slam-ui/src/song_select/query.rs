//! The search query language of the song select, as osu!lazer's (ADR-0099).
//!
//! [`Criteria::parse`] turns the typed text into keyword filters (`ar>9`, `artist="xi"!`,
//! `status=ranked`, ...), a difficulty name in brackets, quoted phrases and plain words.
//! [`Criteria::matches`] checks one difficulty against them. lazer does this with regular
//! expressions and lists; here the expressions are followed by hand over the query (at most
//! [`QUERY_CAPACITY`] bytes) into fixed buffers, so that parsing on every key press does not
//! allocate.
//!
//! Text is compared lowercased: the difficulty's fields are lowercased when the list is
//! built, the filter terms here, per character, the same way. .NET's `\w` (a word
//! character) is approximated by "alphanumeric or `_`".
//!
//! Keys whose data the library does not have yet (stars, BPM, length, beat divisor, online
//! status, play and rank dates) are parsed and taken out of the text as in lazer, but do
//! not hide difficulties.

use std::cmp::Ordering;
use std::ops::{Add, Sub};

use super::DifficultyRow;
use super::filter::{Field, QUERY_CAPACITY};

/// Most search terms: a term and a space take at least two bytes of the query.
pub const MAX_TERMS: usize = QUERY_CAPACITY / 2;

/// Most `tag` filters kept; further ones are parsed and dropped.
pub const MAX_TAGS: usize = 8;

/// Bytes of lowercased text the filters hold. Each byte of the query can end up in a
/// keyword value, a bracketed difficulty name and a quoted phrase at once, and a lowercase
/// form is at most one and a half times as long as its character (`İ`).
const TEXT_BYTES: usize = QUERY_CAPACITY * 5;

/// A comparison operator of a keyword filter.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/Filter/Operator.cs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    Less,
    LessOrEqual,
    Equal,
    NotEqual,
    GreaterOrEqual,
    Greater,
}

/// How a text filter compares.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterCriteria.cs (MatchMode)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatchMode {
    /// A substring anywhere: a plain word.
    #[default]
    Substring,
    /// A substring with a word boundary or the field's edge on each side: `"phrase"`.
    IsolatedPhrase,
    /// The whole field: `"phrase"!`.
    FullPhrase,
}

/// A text filter: a lowercased term in [`Criteria`]'s buffer. No term, no filter.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterCriteria.cs (OptionalTextFilter)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextFilter {
    start: u16,
    end: u16,
    /// The term as read on the other keyboard layout (free-text terms only); empty if the
    /// swap changes nothing.
    alt_start: u16,
    alt_end: u16,
    pub mode: MatchMode,
    /// Whether the filter keeps what does not match (`!=`).
    pub exclude: bool,
}

impl TextFilter {
    pub fn has_filter(&self) -> bool {
        self.end > self.start
    }

    /// The filter for the other-layout form of the term, if there is one.
    fn alternative(&self) -> Option<TextFilter> {
        (self.alt_end > self.alt_start).then_some(TextFilter {
            start: self.alt_start,
            end: self.alt_end,
            alt_start: 0,
            alt_end: 0,
            ..*self
        })
    }
}

/// A range a value has to fall in; unbounded sides are `None`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterCriteria.cs (OptionalRange)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range<T> {
    pub min: Option<T>,
    pub max: Option<T>,
    pub lower_inclusive: bool,
    pub upper_inclusive: bool,
    /// Whether the filter keeps what is outside the range (`!=`).
    pub invert: bool,
}

/// A value a [`Range`] compares, as .NET's `Comparer<T>.Default` does.
pub trait Compare: Copy {
    fn compare(self, other: Self) -> Ordering;
}

impl Compare for f32 {
    fn compare(self, other: f32) -> Ordering {
        // `Single.CompareTo`: NaN is below everything and equal to itself.
        self.partial_cmp(&other)
            .unwrap_or_else(|| self.is_nan().cmp(&other.is_nan()).reverse())
    }
}

impl Compare for f64 {
    fn compare(self, other: f64) -> Ordering {
        self.partial_cmp(&other)
            .unwrap_or_else(|| self.is_nan().cmp(&other.is_nan()).reverse())
    }
}

impl Compare for i32 {
    fn compare(self, other: i32) -> Ordering {
        self.cmp(&other)
    }
}

impl Compare for Date {
    fn compare(self, other: Date) -> Ordering {
        self.cmp(&other)
    }
}

impl<T> Default for Range<T> {
    fn default() -> Range<T> {
        Range {
            min: None,
            max: None,
            lower_inclusive: false,
            upper_inclusive: false,
            invert: false,
        }
    }
}

impl<T> Range<T> {
    pub fn has_filter(&self) -> bool {
        self.min.is_some() || self.max.is_some()
    }
}

impl<T: Compare> Range<T> {
    /// Whether `value` passes the range.
    pub fn contains(&self, value: T) -> bool {
        let lower = self.min.is_none_or(|min| match value.compare(min) {
            Ordering::Greater => true,
            Ordering::Equal => self.lower_inclusive,
            Ordering::Less => false,
        });
        let upper = self.max.is_none_or(|max| match value.compare(max) {
            Ordering::Less => true,
            Ordering::Equal => self.upper_inclusive,
            Ordering::Greater => false,
        });
        (lower && upper) != self.invert
    }
}

/// A calendar date (UTC midnight), as lazer's ranked and submitted filters bound them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl Date {
    /// The date, if it exists in years 1 ..= 9999 as .NET's `DateTimeOffset` allows.
    pub fn new(year: u64, month: u64, day: u64) -> Option<Date> {
        let date = Date {
            year: u16::try_from(year)
                .ok()
                .filter(|y| (1..=9999).contains(y))?,
            month: u8::try_from(month).ok().filter(|m| (1..=12).contains(m))?,
            day: u8::try_from(day).ok()?,
        };
        (1..=date.days_in_month())
            .contains(&date.day)
            .then_some(date)
    }

    fn days_in_month(self) -> u8 {
        match self.month {
            2 if self.year.is_multiple_of(4)
                && (!self.year.is_multiple_of(100) || self.year.is_multiple_of(400)) =>
            {
                29
            }
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }

    fn add_years(self, years: u16) -> Option<Date> {
        let year = self.year.checked_add(years)?;
        let day = self.day.min(Date { year, ..self }.days_in_month());
        Date::new(year.into(), self.month.into(), day.into())
    }

    fn add_months(self, months: u8) -> Option<Date> {
        let index = u16::from(self.month) - 1 + u16::from(months);
        let (year, month) = (self.year.checked_add(index / 12)?, (index % 12) as u8 + 1);
        let day = self.day.min(
            Date {
                year,
                month,
                ..self
            }
            .days_in_month(),
        );
        Date::new(year.into(), month.into(), day.into())
    }

    fn add_day(self) -> Option<Date> {
        if self.day < self.days_in_month() {
            Some(Date {
                day: self.day + 1,
                ..self
            })
        } else {
            Date { day: 1, ..self }.add_months(1)
        }
    }
}

/// A time some amount back from now: the value of a `lastplayed` filter. Years and months
/// are whole as lazer takes them; the rest may be fractional. `None` parts were not given.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ago {
    pub years: Option<u64>,
    pub months: Option<u64>,
    pub days: Option<f64>,
    pub hours: Option<f64>,
    pub minutes: Option<f64>,
    pub seconds: Option<f64>,
}

/// A bound of the last-played range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlayedBound {
    /// `DateTimeOffset.MinValue`: what lazer gives a beatmap never played.
    Never,
    /// `DateTimeOffset.MaxValue`.
    Last,
    Ago(Ago),
}

/// The online statuses of a beatmap, in lazer's `BeatmapOnlineStatus` order of values.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/BeatmapOnlineStatus.cs
const STATUSES: [(&str, i64); 9] = [
    ("LocallyModified", -4),
    ("None", -3),
    ("Graveyard", -2),
    ("WIP", -1),
    ("Pending", 0),
    ("Ranked", 1),
    ("Approved", 2),
    ("Qualified", 3),
    ("Loved", 4),
];

/// The statuses .NET's `Enum.GetNames` lists, sorted by the unsigned value: a prefix
/// picks the first of these that starts with it, so `l` is `Loved`.
const STATUS_NAME_ORDER: [usize; 9] = [4, 5, 6, 7, 8, 0, 1, 2, 3];

/// A set of online statuses, one bit per entry of [`STATUSES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusSet(u16);

impl Default for StatusSet {
    fn default() -> StatusSet {
        StatusSet::ALL
    }
}

impl StatusSet {
    const ALL: StatusSet = StatusSet((1 << STATUSES.len()) - 1);

    /// Whether the status named `name` (as in lazer's enum) is in the set.
    pub fn contains(&self, name: &str) -> bool {
        STATUSES
            .iter()
            .position(|(n, _)| *n == name)
            .is_some_and(|i| self.0 >> i & 1 != 0)
    }

    pub fn len(&self) -> u32 {
        self.0.count_ones()
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
}

/// The filters of a query.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterCriteria.cs
#[derive(Clone, Debug)]
pub struct Criteria {
    text: [u8; TEXT_BYTES],
    text_len: usize,
    terms: [TextFilter; MAX_TERMS],
    term_count: usize,
    /// The only term, when it is an integer: also an online beatmap or set ID.
    pub search_number: Option<i32>,
    pub star_difficulty: Range<f64>,
    pub approach_rate: Range<f32>,
    pub drain_rate: Range<f32>,
    pub circle_size: Range<f32>,
    pub overall_difficulty: Range<f32>,
    pub length: Range<f64>,
    pub bpm: Range<f64>,
    pub beat_divisor: Range<i32>,
    pub online_status: StatusSet,
    pub last_played: Range<PlayedBound>,
    pub date_ranked: Range<Date>,
    pub date_submitted: Range<Date>,
    pub creator: TextFilter,
    pub artist: TextFilter,
    pub title: TextFilter,
    pub difficulty_name: TextFilter,
    pub source: TextFilter,
    tags: [TextFilter; MAX_TAGS],
    tag_count: usize,
}

impl Default for Criteria {
    fn default() -> Criteria {
        Criteria {
            text: [0; TEXT_BYTES],
            text_len: 0,
            terms: [TextFilter::default(); MAX_TERMS],
            term_count: 0,
            search_number: None,
            star_difficulty: Range::default(),
            approach_rate: Range::default(),
            drain_rate: Range::default(),
            circle_size: Range::default(),
            overall_difficulty: Range::default(),
            length: Range::default(),
            bpm: Range::default(),
            beat_divisor: Range::default(),
            online_status: StatusSet::default(),
            last_played: Range::default(),
            date_ranked: Range::default(),
            date_submitted: Range::default(),
            creator: TextFilter::default(),
            artist: TextFilter::default(),
            title: TextFilter::default(),
            difficulty_name: TextFilter::default(),
            source: TextFilter::default(),
            tags: [TextFilter::default(); MAX_TAGS],
            tag_count: 0,
        }
    }
}

/// A fixed-capacity copy of the query that keyword filters are cut out of.
struct Work {
    bytes: [u8; QUERY_CAPACITY],
    len: usize,
}

impl Work {
    fn new(s: &str) -> Work {
        let mut work = Work {
            bytes: [0; QUERY_CAPACITY],
            len: 0,
        };
        let len = floor_char_boundary(s, QUERY_CAPACITY);
        work.bytes[..len].copy_from_slice(&s.as_bytes()[..len]);
        work.len = len;
        work
    }

    fn as_str(&self) -> &str {
        // Only ever whole characters are copied in or cut out.
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }

    /// Removes every occurrence of `old`, scanning left to right as .NET's
    /// `string.Replace(old, "")`.
    fn remove_all(&mut self, old: &str) {
        if old.is_empty() {
            return;
        }
        let old = old.as_bytes();
        let (mut read, mut write) = (0, 0);
        while read < self.len {
            if self.bytes[read..self.len].starts_with(old) {
                read += old.len();
            } else {
                self.bytes[write] = self.bytes[read];
                write += 1;
                read += 1;
            }
        }
        self.len = write;
    }
}

fn floor_char_boundary(s: &str, at: usize) -> usize {
    if at >= s.len() {
        return s.len();
    }
    (0..=at).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0)
}

/// .NET's `\w`, approximately.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// One `key op value` match of lazer's `query_syntax_regex`, as byte ranges of the query.
struct KeywordMatch {
    start: usize,
    end: usize,
    key: std::ops::Range<usize>,
    op: Operator,
    value: std::ops::Range<usize>,
}

/// The next match at or after `from` of
/// `\b(?<key>\w+)(?<op>(!?(:|=)|(>|<)(:|=)?))(?<value>("".*?""[!]?)|(\S*))`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (query_syntax_regex)
fn next_keyword(query: &str, from: usize) -> Option<KeywordMatch> {
    let mut prev = query[..from].chars().next_back();
    for (start, c) in query[from..].char_indices().map(|(i, c)| (i + from, c)) {
        let at_word_start = is_word(c) && !prev.is_some_and(is_word);
        prev = Some(c);
        if !at_word_start {
            continue;
        }
        // The key is the whole run of word characters: the operator starts with a
        // non-word character, so no shorter key can be followed by one.
        let key_end = query[start..]
            .char_indices()
            .find(|&(_, c)| !is_word(c))
            .map_or(query.len(), |(i, _)| start + i);
        let rest = &query.as_bytes()[key_end..];
        let eq = |i: usize| matches!(rest.get(i), Some(b':' | b'='));
        let (op, op_len) = match rest.first() {
            Some(b'!') if eq(1) => (Operator::NotEqual, 2),
            Some(b':' | b'=') => (Operator::Equal, 1),
            Some(b'<') if eq(1) => (Operator::LessOrEqual, 2),
            Some(b'<') => (Operator::Less, 1),
            Some(b'>') if eq(1) => (Operator::GreaterOrEqual, 2),
            Some(b'>') => (Operator::Greater, 1),
            _ => continue,
        };
        let value_start = key_end + op_len;
        let value_end = quoted_value_end(query, value_start).unwrap_or_else(|| {
            query[value_start..]
                .char_indices()
                .find(|&(_, c)| c.is_whitespace())
                .map_or(query.len(), |(i, _)| value_start + i)
        });
        return Some(KeywordMatch {
            start,
            end: value_end,
            key: start..key_end,
            op,
            value: value_start..value_end,
        });
    }
    None
}

/// The end of `".*?"[!]?` at `at`, if it matches there.
fn quoted_value_end(query: &str, at: usize) -> Option<usize> {
    let rest = query.get(at..)?.strip_prefix('"')?;
    // `.` stops at a line feed.
    let close = rest
        .find(['"', '\n'])
        .filter(|&i| rest.as_bytes()[i] == b'"')?;
    let end = at + 1 + close + 1;
    Some(if query[end..].starts_with('!') {
        end + 1
    } else {
        end
    })
}

/// `float.TryParse(value, NumberStyles.AllowDecimalPoint, InvariantCulture)`: digits with at
/// most one point, nothing else.
fn parse_decimal<T: std::str::FromStr>(value: &str) -> Option<T> {
    let digits = value.bytes().filter(u8::is_ascii_digit).count();
    let points = value.bytes().filter(|&b| b == b'.').count();
    if digits == 0 || points > 1 || digits + points != value.len() {
        return None;
    }
    value.parse().ok()
}

/// `int.TryParse(value, NumberStyles.None, InvariantCulture)`: digits only.
fn parse_digits(value: &str) -> Option<i32> {
    (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

/// `int.TryParse(value)` (`NumberStyles.Integer`): optional white space and a sign around
/// the digits.
fn parse_integer(value: &str) -> Option<i32> {
    let value = value.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r'));
    let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// A run of ASCII digits, saturated: years and dates beyond range are rejected later.
fn parse_whole(value: &str) -> Option<u64> {
    (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())).then(|| {
        value.bytes().fold(0u64, |n, b| {
            n.saturating_mul(10).saturating_add(u64::from(b - b'0'))
        })
    })
}

/// `\d+(\.\d+)?` at the start of `s`: the length of the number, if there is one.
fn number_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let int = b.iter().take_while(|b| b.is_ascii_digit()).count();
    if int == 0 {
        return None;
    }
    if b.get(int) == Some(&b'.') {
        let frac = b[int + 1..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if frac > 0 {
            return Some(int + 1 + frac);
        }
    }
    Some(int)
}

/// Matches `s` against a sequence of optional `<number><unit>` parts in `units` order, as
/// `^((?<a>\d+(\.\d+)?)a)?((?<b>...)b)?...$` does; `whole[i]` restricts part `i` to `\d+`.
/// Returns the number of each part found.
fn unit_parts<const N: usize>(
    s: &str,
    units: [u8; N],
    whole: [bool; N],
) -> Option<[Option<&str>; N]> {
    let mut parts = [None; N];
    let mut rest = s;
    for (i, unit) in units.into_iter().enumerate() {
        let len = if whole[i] {
            let n = rest.bytes().take_while(u8::is_ascii_digit).count();
            (n > 0).then_some(n)
        } else {
            number_len(rest)
        };
        // A number not followed by this unit may still be a later part's.
        if let Some(len) = len
            && rest.as_bytes().get(len) == Some(&unit)
        {
            parts[i] = Some(&rest[..len]);
            rest = &rest[len + 1..];
        }
    }
    rest.is_empty().then_some(parts)
}

impl Criteria {
    /// The filters of `query`. Does not allocate.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (ApplyQueries)
    pub fn parse(query: &str) -> Criteria {
        let mut criteria = Criteria::default();
        let query = &query[..floor_char_boundary(query, QUERY_CAPACITY)];
        let mut work = Work::new(query);
        let mut from = 0;
        while let Some(m) = next_keyword(query, from) {
            from = m.end;
            let key = &query[m.key];
            let value = &query[m.value];
            if criteria.keyword(key, m.op, value) {
                work.remove_all(&query[m.start..m.end]);
            }
        }
        criteria.set_search_text(work.as_str());
        criteria
    }

    /// The lowercased term of `filter`.
    pub fn term(&self, filter: &TextFilter) -> &str {
        let (start, end) = (usize::from(filter.start), usize::from(filter.end));
        std::str::from_utf8(&self.text[start..end]).unwrap_or("")
    }

    /// The search terms: quoted phrases, then plain words.
    pub fn terms(&self) -> &[TextFilter] {
        &self.terms[..self.term_count]
    }

    /// The `tag` filters.
    pub fn tags(&self) -> &[TextFilter] {
        &self.tags[..self.tag_count]
    }

    /// Appends `s` lowercased to the text buffer; returns its span. What does not fit is
    /// cut at a character.
    fn push_lower(&mut self, s: &str) -> (u16, u16) {
        let start = self.text_len;
        for lower in s.chars().flat_map(char::to_lowercase) {
            let width = lower.len_utf8();
            if self.text_len + width > TEXT_BYTES {
                break;
            }
            lower.encode_utf8(&mut self.text[self.text_len..]);
            self.text_len += width;
        }
        // TEXT_BYTES fits in u16.
        (start as u16, self.text_len as u16)
    }

    /// Sets the term of `filter` from `value`, keeping its `exclude`: quotes make a phrase.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterCriteria.cs (OptionalTextFilter.SearchTerm)
    fn set_term(&mut self, mut filter: TextFilter, value: &str) -> TextFilter {
        let (term, mode) = if value.starts_with('"') {
            // The length check keeps the opening quote from also being the closing one.
            if value.ends_with("\"!") && value.len() >= 3 {
                (
                    value.trim_end_matches('!').trim_matches('"'),
                    MatchMode::FullPhrase,
                )
            } else {
                (value.trim_matches('"'), MatchMode::IsolatedPhrase)
            }
        } else {
            (value, MatchMode::Substring)
        };
        (filter.start, filter.end) = self.push_lower(term);
        filter.mode = mode;
        filter
    }

    /// Applies one keyword filter; returns whether it was understood (and is to be taken
    /// out of the text).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (tryParseKeywordCriteria)
    fn keyword(&mut self, key: &str, op: Operator, value: &str) -> bool {
        let is = |name: &str| key.eq_ignore_ascii_case(name);
        if is("star") || is("stars") || is("sr") {
            update_float_text(&mut self.star_difficulty, op, value, 0.0)
        } else if is("ar") {
            update_float_text(&mut self.approach_rate, op, value, 0.05)
        } else if is("dr") || is("hp") {
            update_float_text(&mut self.drain_rate, op, value, 0.05)
        } else if is("cs") {
            update_float_text(&mut self.circle_size, op, value, 0.05)
        } else if is("od") {
            update_float_text(&mut self.overall_difficulty, op, value, 0.05)
        } else if is("bpm") {
            update_float_text(&mut self.bpm, op, value, 0.5)
        } else if is("length") {
            self.update_length(op, value)
        } else if is("lastplayed") {
            self.update_date_ago(op, value)
        } else if is("ranked") {
            update_date_range(&mut self.date_ranked, op, value)
        } else if is("created") || is("submitted") {
            update_date_range(&mut self.date_submitted, op, value)
        } else if is("played") {
            self.update_played(op, value)
        } else if is("divisor") {
            parse_digits(value).is_some_and(|v| update_range(&mut self.beat_divisor, op, v))
        } else if is("status") {
            update_status(&mut self.online_status, op, value)
        } else if is("creator") || is("author") || is("mapper") {
            self.update_text(|c| &mut c.creator, op, value)
        } else if is("artist") {
            self.update_text(|c| &mut c.artist, op, value)
        } else if is("title") {
            self.update_text(|c| &mut c.title, op, value)
        } else if is("diff") {
            self.update_text(|c| &mut c.difficulty_name, op, value)
        } else if is("source") {
            self.update_text(|c| &mut c.source, op, value)
        } else if is("tag") {
            let mut tag = TextFilter::default();
            match op {
                Operator::NotEqual => tag.exclude = true,
                Operator::Equal => {}
                _ => return true,
            }
            let tag = self.set_term(tag, value);
            if self.tag_count < MAX_TAGS {
                self.tags[self.tag_count] = tag;
                self.tag_count += 1;
            }
            true
        } else {
            false
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (TryUpdateCriteriaText)
    fn update_text(
        &mut self,
        field: fn(&mut Criteria) -> &mut TextFilter,
        op: Operator,
        value: &str,
    ) -> bool {
        let mut filter = *field(self);
        match op {
            Operator::NotEqual => filter.exclude = true,
            Operator::Equal => {}
            _ => return false,
        }
        let filter = self.set_term(filter, value);
        *field(self) = filter;
        true
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (tryUpdateLengthRange)
    fn update_length(&mut self, op: Operator, value: &str) -> bool {
        // (seconds, minutes, hours) as the expressions name them.
        let parts = {
            let mut colon = value.split(':');
            let fields: [Option<&str>; 3] = [colon.next(), colon.next(), colon.next()];
            let digits = |s: &&str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
            match fields {
                // ^((?<hours>\d+):)?(?<minutes>\d+):(?<seconds>\d+)$
                [Some(m), Some(s), None] if digits(&m) && digits(&s) => {
                    Some([Some(s), Some(m), None])
                }
                [Some(h), Some(m), Some(s)]
                    if colon.next().is_none() && digits(&h) && digits(&m) && digits(&s) =>
                {
                    Some([Some(s), Some(m), Some(h)])
                }
                _ => None,
            }
        }
        .or_else(|| {
            // ^((?<hours>...)h)?((?<minutes>...)m)?((?<seconds>...)s)?$
            unit_parts(value, *b"hms", [false; 3]).map(|[h, m, s]| [s, m, h])
        })
        .or_else(|| {
            // ^(?<seconds>\d+(\.\d+)?)$
            (number_len(value) == Some(value.len())).then_some([Some(value), None, None])
        });
        let Some(parts) = parts else {
            return false;
        };
        const SCALES: [u32; 3] = [1000, 60_000, 3_600_000];
        let present = parts.iter().filter(|p| p.is_some()).count();
        let mut total = 0.0;
        let mut min_scale = 3_600_000;
        for (i, (part, scale)) in parts
            .iter()
            .zip(SCALES)
            .filter_map(|(p, s)| p.map(|p| (p, s)))
            .enumerate()
        {
            let Some(length) = parse_decimal::<f64>(part) else {
                return false;
            };
            if i != present - 1 && length >= 60.0 {
                return false;
            }
            if i != 0 && part.contains('.') {
                return false;
            }
            total += length * f64::from(scale);
            min_scale = min_scale.min(scale);
        }
        update_float_range(&mut self.length, op, total, f64::from(min_scale) / 2.0)
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (tryUpdateDateAgoRange)
    fn update_date_ago(&mut self, op: Operator, value: &str) -> bool {
        // "More than five days ago" is *before* five days ago.
        let op = match op {
            Operator::Equal | Operator::NotEqual => return false,
            Operator::Less => Operator::Greater,
            Operator::LessOrEqual => Operator::GreaterOrEqual,
            Operator::Greater => Operator::Less,
            Operator::GreaterOrEqual => Operator::LessOrEqual,
        };
        let parts = unit_parts(value, *b"yMdhms", [true, true, false, false, false, false])
            .or_else(|| {
                (number_len(value) == Some(value.len())).then_some([
                    None,
                    None,
                    Some(value),
                    None,
                    None,
                    None,
                ])
            });
        let Some([years, months, days, hours, minutes, seconds]) = parts else {
            return false;
        };
        let fraction = |p: Option<&str>| p.map(parse_decimal::<f64>);
        let whole = |p: Option<&str>| p.map(parse_whole);
        let ago = Ago {
            years: whole(years).flatten(),
            months: whole(months).flatten(),
            days: fraction(days).flatten(),
            hours: fraction(hours).flatten(),
            minutes: fraction(minutes).flatten(),
            seconds: fraction(seconds).flatten(),
        };
        if ago == Ago::default() {
            return false;
        }
        update_range(&mut self.last_played, op, PlayedBound::Ago(ago))
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (tryParseKeywordCriteria, "played")
    fn update_played(&mut self, op: Operator, value: &str) -> bool {
        let Some(mut played) = parse_bool(value) else {
            return false;
        };
        // Unplayed beatmaps are filtered on `DateTimeOffset.MinValue`.
        if op == Operator::NotEqual {
            played = !played;
        }
        let range = &mut self.last_played;
        range.min = Some(PlayedBound::Never);
        if played {
            range.max = Some(PlayedBound::Last);
            range.lower_inclusive = false;
        } else {
            range.max = Some(PlayedBound::Never);
            range.lower_inclusive = true;
            range.upper_inclusive = true;
        }
        true
    }

    /// Splits what is left of the query after the keyword filters into the bracketed
    /// difficulty name, quoted phrases and plain words.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterCriteria.cs (SearchText)
    fn set_search_text(&mut self, value: &str) {
        let mut remaining = Work::new(value);

        // (^|\s)\[(.*)(\]\s|$): `.*` is greedy, so the match runs to the end of the text.
        let bracket = value.char_indices().find_map(|(i, c)| {
            if i == 0 && c == '[' {
                Some((0, 1))
            } else if c.is_whitespace() && value[i + c.len_utf8()..].starts_with('[') {
                Some((i, i + c.len_utf8() + 1))
            } else {
                None
            }
        });
        if let Some((start, inner)) = bracket {
            let name = value[inner..].trim_matches(']');
            self.difficulty_name = self.set_term(TextFilter::default(), name);
            remaining.remove_all(&value[start..]);
        }

        // Quoted segments first, so that the spaces inside them stay: ("[^"]+"[!]?)
        let mut from = 0;
        while let Some(open) = value[from..].find('"').map(|i| from + i) {
            let inner = &value[open + 1..];
            match inner.find('"') {
                Some(0) => from = open + 1,
                Some(close) => {
                    let mut end = open + 1 + close + 1;
                    if value[end..].starts_with('!') {
                        end += 1;
                    }
                    let segment = &value[open..end];
                    self.push_term(segment);
                    remaining.remove_all(segment);
                    from = end;
                }
                None => break,
            }
        }

        // Then the rest, split on spaces.
        let rest = remaining;
        for word in rest.as_str().split(' ').filter(|w| !w.is_empty()) {
            self.push_term(word);
        }

        self.search_number = None;
        if let [term] = self.terms() {
            self.search_number = parse_integer(self.term(term));
        }
    }

    fn push_term(&mut self, value: &str) {
        if self.term_count < MAX_TERMS {
            let mut term = self.set_term(TextFilter::default(), value);
            // Free text only: keyword values (`ar>9`, `artist=...`) are never swapped. The
            // lowercased term is swapped through a stack buffer into the text buffer; an
            // alternative that does not fit whole is dropped.
            let mut buf = [0u8; 2 * TEXT_BYTES / MAX_TERMS + 2 * QUERY_CAPACITY];
            let original = &self.text[usize::from(term.start)..usize::from(term.end)];
            let original = std::str::from_utf8(original).unwrap_or("");
            let swapped = crate::layout_swap::swap_into(original, &mut buf);
            if swapped != original && !swapped.is_empty() {
                let at = self.text_len;
                let (start, end) = self.push_lower(swapped);
                if usize::from(end - start) == swapped.len() {
                    (term.alt_start, term.alt_end) = (start, end);
                } else {
                    self.text_len = at;
                }
            }
            self.terms[self.term_count] = term;
            self.term_count += 1;
        }
    }

    /// Whether `filter` passes `value`, a lowercased field.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterCriteria.cs (OptionalTextFilter.Matches)
    pub fn text_matches(&self, filter: &TextFilter, value: &str) -> bool {
        if !filter.has_filter() {
            return true;
        }
        // The term is not empty, so an empty field does not match.
        if value.is_empty() {
            return filter.exclude;
        }
        let term = self.term(filter);
        let found = match filter.mode {
            MatchMode::Substring => value.contains(term),
            MatchMode::IsolatedPhrase => isolated_match(value, term),
            MatchMode::FullPhrase => value == term,
        };
        found != filter.exclude
    }

    /// Whether `difficulty` passes the filters the library has data for.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/BeatmapCarouselFilterMatching.cs (CheckCriteriaMatch)
    pub fn matches(&self, difficulty: &DifficultyRow) -> bool {
        let field = |f: Field| difficulty.search.get(f);
        let mut matched = true;

        if self.term_count > 0 {
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/BeatmapInfoExtensions.cs (Match)
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/BeatmapMetadataInfoExtensions.cs (Match)
            // A term also passes in its other-layout form (ADR-0137).
            matched = self.terms().iter().all(|term| {
                let alt = term.alternative();
                Field::ALL.iter().any(|&f| {
                    self.text_matches(term, field(f))
                        || alt.is_some_and(|alt| self.text_matches(&alt, field(f)))
                })
            });
            // A number found nowhere in the text may be an online ID.
            if !matched && let Some(number) = self.search_number {
                matched = difficulty.beatmap_id == number || difficulty.beatmap_set_id == number;
            }
        }

        let in_range =
            |range: &Range<f32>, value: f32| !range.has_filter() || range.contains(value);
        matched &= in_range(&self.approach_rate, difficulty.approach_rate);
        matched &= in_range(&self.drain_rate, difficulty.drain_rate);
        matched &= in_range(&self.circle_size, difficulty.circle_size);
        matched &= in_range(&self.overall_difficulty, difficulty.overall_difficulty);

        matched &= self.text_matches(&self.creator, field(Field::Creator));
        matched &= self.either_matches(
            &self.artist,
            field(Field::Artist),
            field(Field::ArtistUnicode),
        );
        matched &=
            self.either_matches(&self.title, field(Field::Title), field(Field::TitleUnicode));
        matched &= self.text_matches(&self.difficulty_name, field(Field::Difficulty));
        matched &= self.text_matches(&self.source, field(Field::Source));

        // lazer matches `tag` against the website's user tags, which the library does not
        // have; the words of the file's `Tags` stand in for them (ADR-0099).
        let tags = || field(Field::Tags).split(' ').filter(|t| !t.is_empty());
        for tag in self.tags() {
            if tag.exclude {
                matched &= tags().all(|t| self.text_matches(tag, t));
            } else {
                matched &= tags().any(|t| self.text_matches(tag, t));
            }
        }
        matched
    }

    /// An artist or title filter: the romanised or the original form has to match, or, for
    /// an excluding filter, neither may.
    fn either_matches(&self, filter: &TextFilter, romanised: &str, original: &str) -> bool {
        if !filter.has_filter() {
            return true;
        }
        let (a, b) = (
            self.text_matches(filter, romanised),
            self.text_matches(filter, original),
        );
        if filter.exclude { a && b } else { a || b }
    }
}

/// `(^|\b)term($|\b)`: an occurrence of `term` in `value` with a word boundary or an edge
/// of `value` on each side.
fn isolated_match(value: &str, term: &str) -> bool {
    let (Some(first), Some(last)) = (term.chars().next(), term.chars().next_back()) else {
        return false;
    };
    value.char_indices().any(|(start, _)| {
        if !value[start..].starts_with(term) {
            return false;
        }
        let end = start + term.len();
        let left = value[..start]
            .chars()
            .next_back()
            .is_none_or(|prev| is_word(prev) != is_word(first));
        let right = value[end..]
            .chars()
            .next()
            .is_none_or(|next| is_word(last) != is_word(next));
        left && right
    })
}

/// `bool` from `1`/`yes`/`0`/`no` or .NET's `bool.TryParse`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (tryParseBool)
fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "yes" => Some(true),
        "0" | "no" => Some(false),
        _ => {
            let trimmed = value.trim();
            if trimmed.eq_ignore_ascii_case("true") {
                Some(true)
            } else if trimmed.eq_ignore_ascii_case("false") {
                Some(false)
            } else {
                None
            }
        }
    }
}

/// A float range with a tolerance around the value.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (TryUpdateCriteriaRange, double)
fn update_float_range<T>(range: &mut Range<T>, op: Operator, value: T, tolerance: T) -> bool
where
    T: Copy + PartialEq + Add<Output = T> + Sub<Output = T> + From<u8>,
{
    // The `float` overload sets the inclusive flags only for `=`; the filters that use it
    // never have a zero tolerance, so the `double` overload's behaviour stands for both.
    let zero = tolerance == T::from(0);
    range.invert = false;
    match op {
        Operator::NotEqual | Operator::Equal => {
            range.invert = op == Operator::NotEqual;
            range.min = Some(value - tolerance);
            range.max = Some(value + tolerance);
            if zero {
                range.lower_inclusive = true;
                range.upper_inclusive = true;
            }
        }
        Operator::Greater => range.min = Some(value + tolerance),
        Operator::GreaterOrEqual => {
            range.min = Some(value - tolerance);
            if zero {
                range.lower_inclusive = true;
            }
        }
        Operator::Less => range.max = Some(value - tolerance),
        Operator::LessOrEqual => {
            range.max = Some(value + tolerance);
            if zero {
                range.upper_inclusive = true;
            }
        }
    }
    true
}

/// [`update_float_range`] with `value` parsed as lazer's `tryParseFloatWithPoint` does.
fn update_float_text<T>(range: &mut Range<T>, op: Operator, value: &str, tolerance: T) -> bool
where
    T: Copy + PartialEq + Add<Output = T> + Sub<Output = T> + From<u8> + std::str::FromStr,
{
    parse_decimal(value).is_some_and(|v| update_float_range(range, op, v, tolerance))
}

/// A range without tolerance.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (tryUpdateCriteriaRange<T>)
fn update_range<T: Copy>(range: &mut Range<T>, op: Operator, value: T) -> bool {
    match op {
        Operator::NotEqual | Operator::Equal => {
            if op == Operator::NotEqual {
                range.invert = true;
            }
            range.lower_inclusive = true;
            range.upper_inclusive = true;
            range.min = Some(value);
            range.max = Some(value);
        }
        Operator::Greater => {
            range.lower_inclusive = false;
            range.min = Some(value);
        }
        Operator::GreaterOrEqual => {
            range.lower_inclusive = true;
            range.min = Some(value);
        }
        Operator::Less => {
            range.upper_inclusive = false;
            range.max = Some(value);
        }
        Operator::LessOrEqual => {
            range.upper_inclusive = true;
            range.max = Some(value);
        }
    }
    true
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (tryUpdateRankedDateRange)
fn update_date_range(range: &mut Range<Date>, op: Operator, value: &str) -> bool {
    // ^(?<year>\d+)([-/.](?<month>\d+)([-/.](?<day>\d+))?)?$
    let mut parts = value.split(['-', '/', '.']);
    let (Some(year), month, day) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    let Some(year) = parse_whole(year) else {
        return false;
    };
    let (month, day) = match (month.map(parse_whole), day.map(parse_whole)) {
        (Some(None), _) | (_, Some(None)) => return false,
        (m, d) => (m.flatten(), d.flatten()),
    };

    range.invert = false;
    let date = |m: Option<u64>, d: Option<u64>| Date::new(year, m.unwrap_or(1), d.unwrap_or(1));
    // The first day after the period the value names: its year, month or day.
    let after = || {
        let start = date(month, day)?;
        match (month, day) {
            (None, _) => start.add_years(1),
            (Some(_), None) => start.add_months(1),
            (Some(_), Some(_)) => start.add_day(),
        }
    };
    let bounds = match op {
        Operator::Less | Operator::GreaterOrEqual => date(month, day).map(|d| (op, d, None)),
        Operator::LessOrEqual => after().map(|d| (Operator::Less, d, None)),
        Operator::Greater => after().map(|d| (Operator::GreaterOrEqual, d, None)),
        Operator::NotEqual | Operator::Equal => {
            if op == Operator::NotEqual {
                range.invert = true;
            }
            date(month, day)
                .zip(after())
                .map(|(min, max)| (Operator::GreaterOrEqual, min, Some(max)))
        }
    };
    let Some((op, bound, max)) = bounds else {
        return false;
    };
    update_range(range, op, bound);
    if let Some(max) = max {
        update_range(range, Operator::Less, max);
    }
    true
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/FilterQueryParser.cs (TryUpdateCriteriaSet, tryParseEnum)
fn update_status(set: &mut StatusSet, op: Operator, value: &str) -> bool {
    let mut matching = 0u16;
    if value.contains(',') {
        let mut parsed = 0u16;
        for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let Some(status) = parse_status(part) else {
                return false;
            };
            parsed |= status_bit(status);
        }
        matching = match op {
            Operator::Equal => parsed,
            Operator::NotEqual => StatusSet::ALL.0 & !parsed,
            _ => return false,
        };
    } else {
        let Some(pivot) = parse_status(value) else {
            return false;
        };
        for (i, &(_, status)) in STATUSES.iter().enumerate() {
            let keep = match op {
                Operator::Less => status < pivot,
                Operator::LessOrEqual => status <= pivot,
                Operator::Equal => status == pivot,
                Operator::GreaterOrEqual => status >= pivot,
                Operator::Greater => status > pivot,
                Operator::NotEqual => status != pivot,
            };
            if keep {
                matching |= 1 << i;
            }
        }
    }
    set.0 &= matching;
    true
}

/// The bit of a status value; none for a value lazer's enum does not define.
fn status_bit(status: i64) -> u16 {
    STATUSES
        .iter()
        .position(|&(_, s)| s == status)
        .map_or(0, |i| 1 << i)
}

/// `Enum.TryParse(value, ignoreCase: true)`, then the first name (in `Enum.GetNames` order)
/// that starts with `value`.
fn parse_status(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return trimmed.parse::<i32>().ok().map(i64::from);
    }
    if let Some(&(_, status)) = STATUSES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(trimmed))
    {
        return Some(status);
    }
    STATUS_NAME_ORDER
        .iter()
        .map(|&i| STATUSES[i])
        .find_map(|(name, status)| {
            let prefix = name.get(..value.len())?;
            prefix.eq_ignore_ascii_case(value).then_some(status)
        })
}
