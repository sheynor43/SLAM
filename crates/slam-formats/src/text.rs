//! Text helpers shared by the line-based legacy decoders (`.osu`, `skin.ini`).

use std::borrow::Cow;

use crate::osu::parsing;

/// Maximum number of characters of a line kept in a warning.
const EXCERPT_LIMIT: usize = 50;

// Section enum values, as in lazer's `LegacyDecoder.Section`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs
pub(crate) const SECTIONS: &[(&str, i32)] = &[
    ("General", 0),
    ("Editor", 1),
    ("Metadata", 2),
    ("Difficulty", 3),
    ("Events", 4),
    ("TimingPoints", 5),
    ("Colours", 6),
    ("HitObjects", 7),
    ("Variables", 8),
    ("Fonts", 9),
    ("CatchTheBeat", 10),
    ("Mania", 11),
];
pub(crate) const SECTION_GENERAL: i32 = 0;
pub(crate) const SECTION_EDITOR: i32 = 1;
pub(crate) const SECTION_METADATA: i32 = 2;
pub(crate) const SECTION_DIFFICULTY: i32 = 3;
pub(crate) const SECTION_EVENTS: i32 = 4;
pub(crate) const SECTION_TIMING_POINTS: i32 = 5;
pub(crate) const SECTION_COLOURS: i32 = 6;
pub(crate) const SECTION_HIT_OBJECTS: i32 = 7;

pub(crate) const SECTION_CATCH_THE_BEAT: i32 = 10;

/// Decodes bytes to text according to the BOM; returns whether anything was replaced.
pub(crate) fn decode_encoding(bytes: &[u8]) -> (Cow<'_, str>, bool) {
    // Same detection order as .NET's `StreamReader`: UTF-32 LE is checked before UTF-16 LE.
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE, 0x00, 0x00]) {
        decode_utf32(rest, u32::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(&[0x00, 0x00, 0xFE, 0xFF]) {
        decode_utf32(rest, u32::from_be_bytes)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        decode_utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        decode_utf16(rest, u16::from_be_bytes)
    } else {
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        let text = String::from_utf8_lossy(bytes);
        let lossy = matches!(text, Cow::Owned(_));
        (text, lossy)
    }
}

pub(crate) fn decode_utf16(bytes: &[u8], read: fn([u8; 2]) -> u16) -> (Cow<'static, str>, bool) {
    let (chunks, remainder) = bytes.as_chunks::<2>();
    let dangling = !remainder.is_empty();
    let mut lossy = dangling;
    let units = chunks.iter().map(|&c| read(c));
    let mut text: String = char::decode_utf16(units)
        .map(|r| {
            r.unwrap_or_else(|_| {
                lossy = true;
                char::REPLACEMENT_CHARACTER
            })
        })
        .collect();
    if dangling {
        // A dangling byte decodes to one replacement character, like .NET.
        text.push(char::REPLACEMENT_CHARACTER);
    }
    (Cow::Owned(text), lossy)
}

pub(crate) fn decode_utf32(bytes: &[u8], read: fn([u8; 4]) -> u32) -> (Cow<'static, str>, bool) {
    let (chunks, remainder) = bytes.as_chunks::<4>();
    let dangling = !remainder.is_empty();
    let mut lossy = dangling;
    let mut text: String = chunks
        .iter()
        .map(|&c| read(c))
        .map(|v| {
            char::from_u32(v).unwrap_or_else(|| {
                lossy = true;
                char::REPLACEMENT_CHARACTER
            })
        })
        .collect();
    if dangling {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    (Cow::Owned(text), lossy)
}

/// Splits like .NET `StreamReader.ReadLine`: on `\r\n`, `\r` or `\n`, with no final empty line.
pub(crate) struct Lines<'a>(pub(crate) &'a str);

impl<'a> Iterator for Lines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.0.is_empty() {
            return None;
        }
        match self.0.find(['\r', '\n']) {
            Some(i) => {
                let line = &self.0[..i];
                let rest = &self.0[i..];
                self.0 = rest
                    .strip_prefix("\r\n")
                    .or_else(|| rest.strip_prefix(['\r', '\n']))
                    .unwrap_or("");
                Some(line)
            }
            None => {
                let line = self.0;
                self.0 = "";
                Some(line)
            }
        }
    }
}

pub(crate) fn excerpt(line: &str) -> String {
    if line.chars().count() <= EXCERPT_LIMIT {
        line.to_owned()
    } else {
        let mut s: String = line.chars().take(EXCERPT_LIMIT).collect();
        s.push('…');
        s
    }
}

/// Emulates `Enum.TryParse` / `Enum.Parse` for a non-negative-valued enum.
///
/// Like .NET, a string starting with a digit or sign is parsed as a whole as an integer (which
/// may be undefined in the enum); any other string is a comma-separated list of case-sensitive
/// names, OR-ed together. Numbers inside a list are not accepted, so `"1,2"` and `"Video,2"`
/// both fail.
pub(crate) fn enum_try_parse(s: &str, names: &[(&str, i32)]) -> Option<i32> {
    let s = s.trim();
    let first = *s.as_bytes().first()?;
    if first.is_ascii_digit() || first == b'-' || first == b'+' {
        return parsing::net_parse_i32(s).ok();
    }
    let mut acc = 0;
    for token in s.split(',') {
        let (_, v) = names.iter().find(|(n, _)| *n == token.trim())?;
        acc |= v;
    }
    Some(acc)
}

/// `SplitKeyVal`: split on the first `:` and trim both parts.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs
pub(crate) fn split_key_val(line: &str) -> (&str, &str) {
    match line.split_once(':') {
        Some((k, v)) => (k.trim(), v.trim()),
        None => (line.trim(), ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENT_TYPES: &[(&str, i32)] = &[("Background", 0), ("Video", 1), ("Break", 2)];

    #[test]
    fn lines_split_like_readline() {
        let v: Vec<_> = Lines("a\r\nb\rc\n\nd\n").collect();
        assert_eq!(v, ["a", "b", "c", "", "d"]);
        assert_eq!(Lines("").count(), 0);
    }

    #[test]
    fn enum_parse() {
        assert_eq!(enum_try_parse("Break", EVENT_TYPES), Some(2));
        assert_eq!(enum_try_parse(" 2 ", EVENT_TYPES), Some(2));
        assert_eq!(enum_try_parse("99", EVENT_TYPES), Some(99));
        assert_eq!(enum_try_parse("-1", EVENT_TYPES), Some(-1));
        assert_eq!(enum_try_parse("break", EVENT_TYPES), None); // case-sensitive
        assert_eq!(enum_try_parse("", EVENT_TYPES), None);
        assert_eq!(enum_try_parse("Video,Break", EVENT_TYPES), Some(3));
        assert_eq!(enum_try_parse("Video,", EVENT_TYPES), None);
        assert_eq!(enum_try_parse("1,2", EVENT_TYPES), None);
        assert_eq!(enum_try_parse("Video,2", EVENT_TYPES), None);
        assert_eq!(enum_try_parse("99999999999", EVENT_TYPES), None);
    }

    #[test]
    fn excerpt_truncates() {
        assert_eq!(excerpt("short"), "short");
        let long = "x".repeat(60);
        assert_eq!(excerpt(&long), format!("{}…", "x".repeat(50)));
    }
}
