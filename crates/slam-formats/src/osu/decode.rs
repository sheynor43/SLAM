//! `.osu` decoding: file structure, section handlers and diagnostics.

use std::fmt;

use thiserror::Error;

use super::beatmap::{Beatmap, Break, Rgb, TimingPoint};
use super::hit_object_parser::HitObjectParser;
use super::parsing::{self, NumberError};
use crate::text::{
    Lines, SECTION_COLOURS, SECTION_DIFFICULTY, SECTION_EDITOR, SECTION_EVENTS, SECTION_GENERAL,
    SECTION_HIT_OBJECTS, SECTION_METADATA, SECTION_TIMING_POINTS, SECTIONS, decode_encoding,
    enum_try_parse, excerpt, split_key_val,
};

const MAGIC: &str = "osu file format v";

/// Version assumed when the header is missing and `require_header` is off.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs
const LATEST_VERSION: i32 = 14;

/// Offset in milliseconds applied to times of maps older than format version 5 (also to
/// replay frame times, see [`crate::osr`]).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs
pub const EARLY_VERSION_TIMING_OFFSET: i32 = 24;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs
const MAX_COMBO_COLOUR_COUNT: i32 = 8;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Legacy/LegacyEventType.cs
const EVENT_TYPES: &[(&str, i32)] = &[
    ("Background", 0),
    ("Video", 1),
    ("Break", 2),
    ("Colour", 3),
    ("Sprite", 4),
    ("Sample", 5),
    ("Animation", 6),
];
const EVENT_BACKGROUND: i32 = 0;
const EVENT_VIDEO: i32 = 1;
const EVENT_BREAK: i32 = 2;
const EVENT_SPRITE: i32 = 4;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Legacy/LegacySampleBank.cs
const SAMPLE_BANKS: &[(&str, i32)] = &[("None", 0), ("Normal", 1), ("Soft", 2), ("Drum", 3)];

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/CountdownType.cs
const COUNTDOWN_TYPES: &[(&str, i32)] = &[
    ("None", 0),
    ("Normal", 1),
    ("HalfSpeed", 2),
    ("DoubleSpeed", 3),
];

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Legacy/LegacyEffectFlags.cs
const EFFECT_KIAI: i32 = 1;
const EFFECT_OMIT_FIRST_BAR_LINE: i32 = 8;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/SupportedExtensions.cs
const VIDEO_EXTENSIONS: &[&str] = &[".mp4", ".mov", ".avi", ".flv", ".mpg", ".wmv", ".m4v"];

/// Options for [`decode_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeOptions {
    /// Apply the +24 ms offset to maps older than version 5 (lazer's `ApplyOffsets`).
    /// Default true; turn off only for tests.
    pub apply_offsets: bool,
    /// Fail with [`DecodeError::MissingHeader`] when the first non-empty line is not an
    /// `osu file format vN` header. Default false, which is lazer's behaviour: the file is
    /// decoded as version 14 with a warning and the first line is parsed like any other.
    pub require_header: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            apply_offsets: true,
            require_header: false,
        }
    }
}

/// A fatal decoding error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// The input has no non-empty line.
    #[error("unknown file format (no content)")]
    NoContent,
    /// The first non-empty line is not an `osu file format vN` header.
    #[error("missing `osu file format v` header")]
    MissingHeader,
    /// The header's version is not a valid integer.
    #[error("invalid format version: {0}")]
    InvalidVersion(NumberError),
}

/// What went wrong on a line that was dropped (or in the file as a whole).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// The input had sequences invalid for its encoding (UTF-8, or UTF-16/32 chosen by BOM,
    /// including a truncated final unit); they were replaced with U+FFFD (line 0).
    InvalidEncoding,
    /// No format header was found and the file was decoded as the latest version (line 0).
    MissingHeader,
    /// A `[Section]` header with an unknown name. Like lazer, following lines are read as if
    /// they belonged to `[General]`.
    UnknownSection,
    /// A value is not a valid number.
    InvalidNumber,
    /// A number is out of range for its type or beyond the parse limit.
    NumberOutOfRange,
    /// A value is NaN where that is not allowed.
    NotANumber,
    /// A required field is missing or empty.
    MissingField,
    /// A colour is not `R,G,B[,A]` with 8-bit components.
    InvalidColour,
    /// A time signature numerator is not positive.
    InvalidTimeSignature,
    /// An uninherited timing point has a NaN beat length.
    NanBeatLength,
    /// A value is not a known enum name or number.
    InvalidEnumValue,
    /// The `Mode` is not one of the rulesets 0..=3.
    UnsupportedRuleset,
    /// A hit object line whose type bits select no known object type.
    UnknownHitObjectType,
}

impl From<NumberError> for WarningKind {
    fn from(e: NumberError) -> Self {
        match e {
            NumberError::Format => WarningKind::InvalidNumber,
            NumberError::Overflow => WarningKind::NumberOutOfRange,
            NumberError::NotANumber => WarningKind::NotANumber,
        }
    }
}

/// A non-fatal problem found while decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// 1-based physical line number in the file; 0 for file-level warnings.
    ///
    /// This intentionally differs from osu!lazer's log numbering, which does not count the
    /// leading blank lines that `Decoder.GetDecoder` consumes before parsing starts.
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

/// Result of a successful decode.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// The parsed beatmap header.
    pub beatmap: Beatmap,
    /// Problems found, in file order.
    pub warnings: Vec<Warning>,
}

/// Decodes `.osu` bytes with default options. Invalid UTF-8 is replaced lossily with a warning.
pub fn decode(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    decode_with(bytes, &DecodeOptions::default())
}

/// Decodes `.osu` bytes with explicit options.
///
/// The encoding is detected from a byte order mark like lazer's
/// `new StreamReader(stream, Encoding.UTF8, detectEncodingFromByteOrderMarks: true)`:
/// UTF-8, UTF-16 LE/BE and UTF-32 LE/BE; without a BOM the data is UTF-8. Invalid sequences
/// are replaced and reported once as [`WarningKind::InvalidEncoding`].
pub fn decode_with(bytes: &[u8], options: &DecodeOptions) -> Result<Decoded, DecodeError> {
    let (text, lossy) = decode_encoding(bytes);
    let mut decoded = decode_text(&text, options)?;
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
    Ok(decoded)
}

/// Decodes `.osu` text with default options.
pub fn decode_str(text: &str) -> Result<Decoded, DecodeError> {
    decode_str_with(text, &DecodeOptions::default())
}

/// Decodes `.osu` text with explicit options. A leading BOM character is ignored.
pub fn decode_str_with(text: &str, options: &DecodeOptions) -> Result<Decoded, DecodeError> {
    decode_text(text.strip_prefix('\u{feff}').unwrap_or(text), options)
}

fn decode_text(text: &str, options: &DecodeOptions) -> Result<Decoded, DecodeError> {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/Decoder.cs
    let header = Lines(text)
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or(DecodeError::NoContent)?;

    let mut warnings = Vec::new();
    let version = if header.starts_with(MAGIC) {
        // `Parsing.ParseInt(m.Split('v').Last())`
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs
        let last = header.rsplit('v').next().unwrap_or("");
        parsing::int(last).map_err(DecodeError::InvalidVersion)?
    } else if options.require_header {
        return Err(DecodeError::MissingHeader);
    } else {
        warnings.push(Warning {
            line: 0,
            kind: WarningKind::MissingHeader,
            excerpt: excerpt(header),
        });
        LATEST_VERSION
    };

    let offset = if options.apply_offsets && version < 5 {
        f64::from(EARLY_VERSION_TIMING_OFFSET)
    } else {
        0.0
    };

    let mut decoder = Decoder {
        beatmap: Beatmap {
            format_version: version,
            timing_offset: offset,
            general: Default::default(),
            editor: Vec::new(),
            metadata: Default::default(),
            difficulty: Default::default(),
            events: Default::default(),
            timing_points: Vec::new(),
            colours: Default::default(),
            hit_objects: Vec::new(),
        },
        offset,
        hit_object_parser: HitObjectParser::new(offset, version),
        has_approach_rate: false,
    };
    decoder.parse_stream(text, &mut warnings);

    Ok(Decoded {
        beatmap: decoder.beatmap,
        warnings,
    })
}

type LineResult = Result<(), WarningKind>;

/// `CleanFilename`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs
fn clean_filename(path: &str) -> String {
    path.replace("\\\\", "\\")
        .trim_matches('"')
        .replace('\\', "/")
}

/// `Path.GetExtension(path).ToLowerInvariant()` for a standardised (`/`-separated) path.
fn lowercase_extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or("");
    match name.rfind('.') {
        Some(i) if i + 1 < name.len() => name[i..].to_lowercase(),
        _ => String::new(),
    }
}

pub(super) fn field<'a>(split: &[&'a str], i: usize) -> Result<&'a str, WarningKind> {
    split.get(i).copied().ok_or(WarningKind::MissingField)
}

pub(super) fn num<T>(r: Result<T, NumberError>) -> Result<T, WarningKind> {
    r.map_err(WarningKind::from)
}

struct Decoder {
    beatmap: Beatmap,
    offset: f64,
    hit_object_parser: HitObjectParser,
    has_approach_rate: bool,
}

impl Decoder {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs
    fn parse_stream(&mut self, text: &str, warnings: &mut Vec<Warning>) {
        let mut section = SECTION_GENERAL;

        for (index, raw) in Lines(text).enumerate() {
            let line_number = index + 1;

            if raw.trim().is_empty() || raw.trim_start().starts_with("//") {
                continue;
            }

            let mut line = raw;
            if section != SECTION_METADATA {
                // Comments are not stripped from metadata, and only when "//" is not at index 0.
                if let Some(i) = line.find("//")
                    && i > 0
                {
                    line = &line[..i];
                }
            }
            let line = line.trim_end();

            if line.starts_with('[') && line.ends_with(']') && line.len() >= 2 {
                // `Enum.TryParse(name, out section)` assigns default(Section) == General on
                // failure, so lines after an unknown section are read as [General].
                match enum_try_parse(&line[1..line.len() - 1], SECTIONS) {
                    Some(s) => section = s,
                    None => {
                        section = SECTION_GENERAL;
                        warnings.push(Warning {
                            line: line_number,
                            kind: WarningKind::UnknownSection,
                            excerpt: excerpt(line),
                        });
                    }
                }
                continue;
            }

            if let Err(kind) = self.parse_line(section, line) {
                warnings.push(Warning {
                    line: line_number,
                    kind,
                    excerpt: excerpt(line),
                });
            }
        }
    }

    fn parse_line(&mut self, section: i32, line: &str) -> LineResult {
        match section {
            SECTION_GENERAL => self.handle_general(line),
            SECTION_EDITOR => {
                let (k, v) = split_key_val(line);
                self.beatmap.editor.push((k.to_owned(), v.to_owned()));
                Ok(())
            }
            SECTION_METADATA => self.handle_metadata(line),
            SECTION_DIFFICULTY => self.handle_difficulty(line),
            SECTION_EVENTS => self.handle_event(line),
            SECTION_TIMING_POINTS => self.handle_timing_point(line),
            SECTION_COLOURS => self.handle_colours(line),
            SECTION_HIT_OBJECTS => {
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs
                let object = self.hit_object_parser.parse(line)?;
                self.beatmap.hit_objects.push(object);
                Ok(())
            }
            _ => Ok(()), // other sections are not read
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs
    fn handle_general(&mut self, line: &str) -> LineResult {
        let (key, value) = split_key_val(line);
        // C# int arithmetic is unchecked.
        let int_offset = self.offset as i32;
        let preview_offset = |t: i32| {
            if t == -1 {
                t
            } else {
                t.wrapping_add(int_offset)
            }
        };
        let g = &mut self.beatmap.general;
        let flag = |v: &str| num(parsing::int(v)).map(|i| i == 1);

        match key {
            "AudioFilename" => g.audio_filename = value.replace('\\', "/"),
            "AudioLeadIn" => g.audio_lead_in = f64::from(num(parsing::int(value))?),
            "PreviewTime" => {
                let time = num(parsing::int(value))?;
                g.preview_time = preview_offset(time);
            }
            "SampleSet" => {
                g.sample_set =
                    enum_try_parse(value, SAMPLE_BANKS).ok_or(WarningKind::InvalidEnumValue)?;
            }
            "SampleVolume" => g.sample_volume = num(parsing::int(value))?,
            "StackLeniency" => g.stack_leniency = num(parsing::float(value))?,
            "Mode" => {
                let mode = num(parsing::int(value))?;
                if !(0..=3).contains(&mode) {
                    return Err(WarningKind::UnsupportedRuleset);
                }
                g.mode = mode;
            }
            "LetterboxInBreaks" => g.letterbox_in_breaks = flag(value)?,
            "SpecialStyle" => g.special_style = flag(value)?,
            "WidescreenStoryboard" => g.widescreen_storyboard = flag(value)?,
            "EpilepsyWarning" => g.epilepsy_warning = flag(value)?,
            "SamplesMatchPlaybackRate" => g.samples_match_playback_rate = flag(value)?,
            "Countdown" => {
                g.countdown =
                    enum_try_parse(value, COUNTDOWN_TYPES).ok_or(WarningKind::InvalidEnumValue)?;
            }
            "CountdownOffset" => g.countdown_offset = num(parsing::int(value))?,
            _ => {}
        }
        Ok(())
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs
    fn handle_metadata(&mut self, line: &str) -> LineResult {
        let (key, value) = split_key_val(line);
        let m = &mut self.beatmap.metadata;
        match key {
            "Title" => m.title = value.to_owned(),
            "TitleUnicode" => m.title_unicode = value.to_owned(),
            "Artist" => m.artist = value.to_owned(),
            "ArtistUnicode" => m.artist_unicode = value.to_owned(),
            "Creator" => m.creator = value.to_owned(),
            "Version" => m.version = value.to_owned(),
            "Source" => m.source = value.to_owned(),
            "Tags" => m.tags = value.to_owned(),
            "BeatmapID" => m.beatmap_id = num(parsing::int(value))?,
            "BeatmapSetID" => m.beatmap_set_id = Some(num(parsing::int(value))?),
            _ => {}
        }
        Ok(())
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs
    fn handle_difficulty(&mut self, line: &str) -> LineResult {
        let (key, value) = split_key_val(line);
        let d = &mut self.beatmap.difficulty;
        match key {
            "HPDrainRate" => d.drain_rate = num(parsing::float(value))?,
            "CircleSize" => d.circle_size = num(parsing::float(value))?,
            "OverallDifficulty" => {
                d.overall_difficulty = num(parsing::float(value))?;
                if !self.has_approach_rate {
                    d.approach_rate = d.overall_difficulty;
                }
            }
            "ApproachRate" => {
                d.approach_rate = num(parsing::float(value))?;
                self.has_approach_rate = true;
            }
            "SliderMultiplier" => d.slider_multiplier = num(parsing::double(value))?,
            "SliderTickRate" => d.slider_tick_rate = num(parsing::double(value))?,
            _ => {}
        }
        Ok(())
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs
    fn handle_event(&mut self, line: &str) -> LineResult {
        let split: Vec<&str> = line.split(',').collect();
        let Some(kind) = enum_try_parse(field(&split, 0)?, EVENT_TYPES) else {
            return Ok(());
        };
        let events = &mut self.beatmap.events;

        match kind {
            EVENT_SPRITE => {
                // The first sprite acts as the background when none was given.
                if events.background.is_none() {
                    let name = clean_filename(field(&split, 3)?);
                    events.background = (!name.is_empty()).then_some(name);
                }
            }
            EVENT_VIDEO => {
                let filename = clean_filename(field(&split, 2)?);
                // Very old maps used type 1 for backgrounds; handle non-video files as such.
                if !VIDEO_EXTENSIONS.contains(&lowercase_extension(&filename).as_str()) {
                    events.background = (!filename.is_empty()).then_some(filename);
                }
            }
            EVENT_BACKGROUND => {
                let name = clean_filename(field(&split, 2)?);
                events.background = (!name.is_empty()).then_some(name);
            }
            EVENT_BREAK => {
                let start = num(parsing::double(field(&split, 1)?))? + self.offset;
                let end = num(parsing::double(field(&split, 2)?))? + self.offset;
                events.breaks.push(Break {
                    start,
                    end: start.max(end),
                });
            }
            _ => {}
        }
        Ok(())
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs
    fn handle_timing_point(&mut self, line: &str) -> LineResult {
        let split: Vec<&str> = line.split(',').collect();
        let first_char = |i: usize| -> Result<char, WarningKind> {
            field(&split, i)?
                .chars()
                .next()
                .ok_or(WarningKind::MissingField)
        };

        let time = num(parsing::double(field(&split, 0)?.trim()))? + self.offset;

        // beatLength may be NaN (some maps use it to disable slider ticks).
        let beat_length = num(parsing::parse_double(
            field(&split, 1)?.trim(),
            parsing::MAX_PARSE_VALUE,
            true,
        ))?;

        let mut time_signature = 4;
        if split.len() >= 3 {
            time_signature = if first_char(2)? == '0' {
                4
            } else {
                let n = num(parsing::int(split[2]))?;
                if n <= 0 {
                    return Err(WarningKind::InvalidTimeSignature);
                }
                n
            };
        }

        let g = &self.beatmap.general;
        let mut sample_set = g.sample_set;
        if split.len() >= 4 {
            sample_set = num(parsing::int(split[3]))?;
        }

        let mut custom_sample_index = 0;
        if split.len() >= 5 {
            custom_sample_index = num(parsing::int(split[4]))?;
        }

        let mut volume = g.sample_volume;
        if split.len() >= 6 {
            volume = num(parsing::int(split[5]))?;
        }

        let mut uninherited = true;
        if split.len() >= 7 {
            uninherited = first_char(6)? == '1';
        }

        let mut kiai = false;
        let mut omit_first_bar_line = false;
        if split.len() >= 8 {
            let flags = num(parsing::int(split[7]))?;
            kiai = flags & EFFECT_KIAI != 0;
            omit_first_bar_line = flags & EFFECT_OMIT_FIRST_BAR_LINE != 0;
        }

        if uninherited && beat_length.is_nan() {
            return Err(WarningKind::NanBeatLength);
        }

        self.beatmap.timing_points.push(TimingPoint {
            time,
            beat_length,
            time_signature,
            sample_set,
            custom_sample_index,
            volume,
            uninherited,
            kiai,
            omit_first_bar_line,
        });
        Ok(())
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs
    fn handle_colours(&mut self, line: &str) -> LineResult {
        let (key, value) = split_key_val(line);
        let split: Vec<&str> = value.split(',').collect();
        if split.len() != 3 && split.len() != 4 {
            return Err(WarningKind::InvalidColour);
        }
        // Beatmaps do not allow alpha, so a 4th component is never parsed.
        let component =
            |i: usize| parsing::net_parse_u8(split[i]).map_err(|_| WarningKind::InvalidColour);
        let colour = Rgb {
            r: component(0)?,
            g: component(1)?,
            b: component(2)?,
        };

        let is_combo = key.starts_with("Combo")
            && parsing::net_parse_i32(&key[5..])
                .is_ok_and(|i| (1..=MAX_COMBO_COLOUR_COUNT).contains(&i));

        let colours = &mut self.beatmap.colours;
        if is_combo {
            colours.combo.push(colour);
        } else if let Some(slot) = colours.custom.iter_mut().find(|(k, _)| k == key) {
            slot.1 = colour;
        } else {
            colours.custom.push((key.to_owned(), colour));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames() {
        assert_eq!(clean_filename("\"bg\\\\img.JPG\""), "bg/img.JPG");
        assert_eq!(clean_filename(" \"a.png\""), " \"a.png"); // only quotes are trimmed
        assert_eq!(lowercase_extension("a/b.MP4"), ".mp4");
        assert_eq!(lowercase_extension("a.d/b"), "");
        assert_eq!(lowercase_extension("b."), "");
    }
}
