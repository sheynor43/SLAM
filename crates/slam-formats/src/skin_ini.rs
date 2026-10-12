//! `skin.ini` decoding.
//!
//! Port of osu!lazer's `LegacySkinDecoder` on top of the shared legacy line machinery: the same
//! BOM detection, comment stripping and `Enum.TryParse` section semantics as the `.osu`
//! decoder. The decoder is infallible; malformed lines are dropped with a [`Warning`].
//!
//! The result is what the file says: fallbacks such as the default combo colours or the
//! "no `skin.ini` means latest version" rule belong to the skin loader, not here.
//!
//! ```
//! let decoded = slam_formats::skin_ini::decode_str("[General]\nName: Test\nVersion: 2.5\n");
//! assert_eq!(decoded.skin.name, "Test");
//! assert_eq!(decoded.skin.version, slam_formats::skin_ini::SkinVersion::new(25, 1));
//! ```

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

use crate::osu::parsing;
use crate::text::{
    Lines, SECTION_CATCH_THE_BEAT, SECTION_COLOURS, SECTION_GENERAL, SECTION_METADATA, SECTIONS,
    decode_encoding, enum_try_parse, excerpt, split_key_val,
};

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs
const MAX_COMBO_COLOUR_COUNT: i32 = 8;

/// Maximum number of fractional digits of a .NET `decimal`.
const MAX_SCALE: u32 = 28;
/// Largest mantissa of a .NET `decimal` (96 bits).
const MAX_MANTISSA: u128 = (1 << 96) - 1;

/// An exact decimal skin version, compared by value (`2 == 2.0 == 2.00`).
///
/// Models the subset of .NET `decimal` that `skin.ini` versions use: a non-negative value of
/// up to 96 bits of mantissa and 28 fractional digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SkinVersion {
    mantissa: u128,
    scale: u32,
}

impl SkinVersion {
    /// Version of a skin without a `Version` key (1.0).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinDecoder.cs
    pub const V1: Self = Self::new(1, 0);
    /// `SkinConfiguration.LATEST_VERSION`, the value of `Version: latest` (2.7).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/SkinConfiguration.cs
    pub const LATEST: Self = Self::new(27, 1);

    /// Creates the value `mantissa / 10^scale`, normalised so that equal values are equal
    /// structurally. Values beyond 28 fractional digits are not representable.
    ///
    /// # Panics
    /// If `scale` is greater than 28 or `mantissa` does not fit in 96 bits.
    pub const fn new(mut mantissa: u128, mut scale: u32) -> Self {
        assert!(scale <= MAX_SCALE && mantissa <= MAX_MANTISSA);
        while scale > 0 && mantissa.is_multiple_of(10) {
            mantissa /= 10;
            scale -= 1;
        }
        Self { mantissa, scale }
    }

    /// Parses like `decimal.TryParse(s, NumberStyles.AllowDecimalPoint, InvariantCulture)`:
    /// ASCII digits with at most one `.`, no sign, white space or exponent, and at least one
    /// digit. Fractional digits beyond the 28th are truncated (.NET rounds); `None` when the
    /// integer part does not fit a `decimal`.
    pub fn parse(s: &str) -> Option<Self> {
        let (int_part, frac_part) = match s.split_once('.') {
            Some((i, f)) => (i, f),
            None => (s, ""),
        };
        if int_part.is_empty() && frac_part.is_empty() {
            return None;
        }
        if !int_part.bytes().all(|b| b.is_ascii_digit())
            || !frac_part.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let mut mantissa: u128 = 0;
        for b in int_part.bytes() {
            mantissa = mantissa
                .checked_mul(10)?
                .checked_add(u128::from(b - b'0'))?;
            if mantissa > MAX_MANTISSA {
                return None;
            }
        }
        let mut scale = 0;
        for b in frac_part.bytes() {
            if scale == MAX_SCALE {
                break;
            }
            let next = mantissa * 10 + u128::from(b - b'0');
            if next > MAX_MANTISSA {
                break;
            }
            mantissa = next;
            scale += 1;
        }
        Some(Self::new(mantissa, scale))
    }

    /// The value as `f64` (for display and coarse comparisons only).
    pub fn to_f64(self) -> f64 {
        self.mantissa as f64 / 10f64.powi(self.scale as i32)
    }

    fn split(self) -> (u128, u128) {
        let unit = 10u128.pow(self.scale);
        let frac = self.mantissa % unit * 10u128.pow(MAX_SCALE - self.scale);
        (self.mantissa / unit, frac)
    }
}

impl Ord for SkinVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.split().cmp(&other.split())
    }
}

impl PartialOrd for SkinVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for SkinVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let unit = 10u128.pow(self.scale);
        if self.scale == 0 {
            write!(f, "{}", self.mantissa)
        } else {
            write!(
                f,
                "{}.{:0width$}",
                self.mantissa / unit,
                self.mantissa % unit,
                width = self.scale as usize
            )
        }
    }
}

/// An 8-bit RGBA colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha (255 when the file gave none or alpha is not read).
    pub a: u8,
}

impl Rgba {
    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}

/// `SkinConfiguration.DefaultComboColours`, the fallback when a skin gives no combo colours.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/SkinConfiguration.cs
pub const DEFAULT_COMBO_COLOURS: [Rgba; 4] = [
    Rgba::rgb(255, 192, 0),
    Rgba::rgb(0, 202, 0),
    Rgba::rgb(18, 124, 255),
    Rgba::rgb(242, 24, 57),
];

/// Flat key/value settings in insertion order; the last write to a key wins.
///
/// Mirrors `SkinConfiguration.ConfigDictionary`: it holds the non-colour keys of every
/// section. Keys are case-sensitive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    entries: Vec<(String, String)>,
    index: HashMap<String, usize>,
}

impl Settings {
    /// The value of `key`, if present (case-sensitive).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.index.get(key).map(|&i| self.entries[i].1.as_str())
    }

    /// Iterates over `(key, value)` in order of first insertion.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Number of distinct keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no keys.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Sets `key` (replacing an earlier value, keeping its position).
    pub fn set(&mut self, key: &str, value: &str) {
        match self.index.get(key) {
            Some(&i) => self.entries[i].1 = value.to_owned(),
            None => {
                self.index.insert(key.to_owned(), self.entries.len());
                self.entries.push((key.to_owned(), value.to_owned()));
            }
        }
    }
}

/// The data of a `skin.ini` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinIni {
    /// `[General] Name`.
    pub name: String,
    /// `[General] Author`.
    pub author: String,
    /// `[General] Version` (1.0 when absent or unparsable).
    pub version: SkinVersion,
    /// Whether the version was given as `latest`.
    pub is_latest: bool,
    /// `Combo1..Combo8` colours in file order (the index does not choose the position).
    /// Empty means "use [`DEFAULT_COMBO_COLOURS`]", which the loader decides.
    pub combo_colours: Vec<Rgba>,
    /// Other colour keys in insertion order; a repeated key replaces the value.
    pub custom_colours: Vec<(String, Rgba)>,
    /// All other keys of all sections.
    pub settings: Settings,
}

impl Default for SkinIni {
    fn default() -> Self {
        Self {
            name: String::new(),
            author: String::new(),
            version: SkinVersion::V1,
            is_latest: false,
            combo_colours: Vec::new(),
            custom_colours: Vec::new(),
            settings: Settings::default(),
        }
    }
}

impl SkinIni {
    /// A custom colour by exact key (e.g. `SliderBorder`).
    pub fn custom_colour(&self, key: &str) -> Option<Rgba> {
        self.custom_colours
            .iter()
            .find(|(k, _)| k == key)
            .map(|&(_, c)| c)
    }

    /// Sets a custom colour, replacing an existing one of the same key.
    pub fn set_custom_colour(&mut self, key: &str, colour: Rgba) {
        match self.custom_colours.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = colour,
            None => self.custom_colours.push((key.to_owned(), colour)),
        }
    }
}

/// What went wrong on a dropped line (or in the file as a whole).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// The input had sequences invalid for its encoding; they were replaced (line 0).
    InvalidEncoding,
    /// A `[Section]` header with an unknown name. Like lazer, following lines are read as if
    /// they belonged to `[General]`.
    UnknownSection,
    /// A colour is not `R,G,B[,A]` with 8-bit components.
    InvalidColour,
    /// A `Version` value is not a decimal number; lazer ignores it silently and so does the
    /// decoder (the previous value stays).
    InvalidVersion,
}

/// A non-fatal problem found while decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// 1-based physical line number; 0 for file-level warnings.
    pub line: usize,
    /// What went wrong.
    pub kind: WarningKind,
    /// The offending line (after comment stripping), truncated to 50 characters plus `…`.
    pub excerpt: String,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}: {:?} \"{}\"",
            self.line, self.kind, self.excerpt
        )
    }
}

/// Result of decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The parsed file.
    pub skin: SkinIni,
    /// Problems found, in file order.
    pub warnings: Vec<Warning>,
}

/// Decodes `skin.ini` bytes. The encoding is detected from a BOM like lazer's
/// `StreamReader`; invalid sequences are replaced and reported once.
pub fn decode(bytes: &[u8]) -> Decoded {
    let (text, lossy) = decode_encoding(bytes);
    let mut decoded = decode_text(&text);
    if lossy {
        decoded.warnings.insert(
            0,
            Warning {
                line: 0,
                kind: WarningKind::InvalidEncoding,
                excerpt: String::new(),
            },
        );
    }
    decoded
}

/// Decodes `skin.ini` text. A leading BOM character is ignored.
pub fn decode_str(text: &str) -> Decoded {
    decode_text(text.strip_prefix('\u{feff}').unwrap_or(text))
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs
fn decode_text(text: &str) -> Decoded {
    let mut skin = SkinIni::default();
    let mut warnings = Vec::new();
    let mut section = SECTION_GENERAL;

    for (index, raw) in Lines(text).enumerate() {
        let line_number = index + 1;
        if raw.trim().is_empty() || raw.trim_start().starts_with("//") {
            continue;
        }
        let mut line = raw;
        if section != SECTION_METADATA
            && let Some(i) = line.find("//")
            && i > 0
        {
            line = &line[..i];
        }
        let line = line.trim_end();

        if line.starts_with('[') && line.ends_with(']') && line.len() >= 2 {
            match enum_try_parse(&line[1..line.len() - 1], SECTIONS) {
                Some(s) => section = s,
                None => {
                    section = SECTION_GENERAL;
                    warnings.push(warning(line_number, WarningKind::UnknownSection, line));
                }
            }
            continue;
        }

        if let Some(kind) = parse_line(&mut skin, section, line) {
            warnings.push(warning(line_number, kind, line));
        }
    }
    Decoded { skin, warnings }
}

fn warning(line: usize, kind: WarningKind, text: &str) -> Warning {
    Warning {
        line,
        kind,
        excerpt: excerpt(text),
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinDecoder.cs
fn parse_line(skin: &mut SkinIni, section: i32, line: &str) -> Option<WarningKind> {
    if section == SECTION_COLOURS {
        return handle_colours(skin, line, false);
    }
    let (key, value) = split_key_val(line);
    match section {
        SECTION_GENERAL => match key {
            "Name" => {
                skin.name = value.to_owned();
                return None;
            }
            "Author" => {
                skin.author = value.to_owned();
                return None;
            }
            "Version" => {
                if value == "latest" {
                    skin.version = SkinVersion::LATEST;
                    skin.is_latest = true;
                } else if let Some(v) = SkinVersion::parse(value) {
                    skin.version = v;
                    skin.is_latest = false;
                } else {
                    return Some(WarningKind::InvalidVersion);
                }
                return None;
            }
            _ => {}
        },
        // osu!catch section only has colour settings.
        SECTION_CATCH_THE_BEAT => return handle_colours(skin, line, true),
        _ => {}
    }
    if !key.is_empty() {
        skin.settings.set(key, value);
    }
    None
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs
fn handle_colours(skin: &mut SkinIni, line: &str, allow_alpha: bool) -> Option<WarningKind> {
    let (key, value) = split_key_val(line);
    let split: Vec<&str> = value.split(',').collect();
    if split.len() != 3 && split.len() != 4 {
        return Some(WarningKind::InvalidColour);
    }
    let component = |i: usize| parsing::net_parse_u8(split[i]).ok();
    let alpha = if allow_alpha && split.len() == 4 {
        component(3)
    } else {
        Some(255)
    };
    let (Some(r), Some(g), Some(b), Some(a)) = (component(0), component(1), component(2), alpha)
    else {
        return Some(WarningKind::InvalidColour);
    };
    let colour = Rgba { r, g, b, a };

    let is_combo = key.starts_with("Combo")
        && parsing::net_parse_i32(&key[5..])
            .is_ok_and(|i| (1..=MAX_COMBO_COLOUR_COUNT).contains(&i));
    if is_combo {
        skin.combo_colours.push(colour);
    } else {
        skin.set_custom_colour(key, colour);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse_and_order() {
        assert_eq!(SkinVersion::parse("2"), Some(SkinVersion::new(2, 0)));
        assert_eq!(SkinVersion::parse("2.00"), SkinVersion::parse("2"));
        assert_eq!(SkinVersion::parse(".5"), Some(SkinVersion::new(5, 1)));
        assert_eq!(SkinVersion::parse("5."), Some(SkinVersion::new(5, 0)));
        for bad in ["", ".", "-1", "+1", " 1", "1 ", "1e2", "1.2.3", "a", "1,5"] {
            assert_eq!(SkinVersion::parse(bad), None, "{bad:?}");
        }
        assert!(SkinVersion::new(19, 1) < SkinVersion::new(2, 0));
        assert!(SkinVersion::LATEST > SkinVersion::new(2, 0));
        assert!(SkinVersion::parse("2.70").unwrap() == SkinVersion::LATEST);
        assert_eq!(SkinVersion::LATEST.to_string(), "2.7");
        assert_eq!(SkinVersion::new(105, 2).to_string(), "1.05");
    }

    #[test]
    fn version_overflow_and_precision() {
        assert_eq!(SkinVersion::parse("99999999999999999999999999999"), None);
        let long = format!("1.{}", "1".repeat(40));
        assert!(SkinVersion::parse(&long).unwrap() > SkinVersion::new(1, 0));
    }
}
