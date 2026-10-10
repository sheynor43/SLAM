//! Data model of a decoded `.osu` file (header sections, timing points and hit objects).

use super::hit_object::HitObject;

/// An opaque 8-bit RGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    /// Red component.
    pub r: u8,
    /// Green component.
    pub g: u8,
    /// Blue component.
    pub b: u8,
}

/// The `[General]` section.
///
/// Types and defaults follow osu!lazer's `Beatmap` / `BeatmapMetadata` after
/// `LegacyBeatmapDecoder.ApplyLegacyDefaults`.
#[derive(Debug, Clone, PartialEq)]
pub struct General {
    /// `AudioFilename`, with `\` replaced by `/`. Default empty.
    pub audio_filename: String,
    /// `AudioLeadIn` (lazer stores it as `double`, but parses it as an integer). Default 0.
    pub audio_lead_in: f64,
    /// `PreviewTime` in ms, shifted by the timing offset unless it is -1. Default -1.
    pub preview_time: i32,
    /// `SampleSet` as the raw `LegacySampleBank` value (0 none, 1 normal, 2 soft, 3 drum).
    /// Lazer does not keep it on the beatmap; it is the default for later timing points.
    pub sample_set: i32,
    /// `SampleVolume`, the default volume for later timing points. Default 100.
    pub sample_volume: i32,
    /// `StackLeniency`. Default 0.7.
    pub stack_leniency: f32,
    /// `Mode` (ruleset id, 0..=3). Default 0.
    pub mode: i32,
    /// `LetterboxInBreaks`. Default false.
    pub letterbox_in_breaks: bool,
    /// `SpecialStyle`. Default false.
    pub special_style: bool,
    /// `WidescreenStoryboard`. Default false (lazer's legacy default).
    pub widescreen_storyboard: bool,
    /// `EpilepsyWarning`. Default false.
    pub epilepsy_warning: bool,
    /// `SamplesMatchPlaybackRate`. Default false.
    pub samples_match_playback_rate: bool,
    /// `Countdown` as the raw `CountdownType` value (0 none, 1 normal, 2 half, 3 double).
    pub countdown: i32,
    /// `CountdownOffset`. Default 0.
    pub countdown_offset: i32,
}

impl Default for General {
    fn default() -> Self {
        Self {
            audio_filename: String::new(),
            audio_lead_in: 0.0,
            preview_time: -1,
            sample_set: 0,
            sample_volume: 100,
            stack_leniency: 0.7,
            mode: 0,
            letterbox_in_breaks: false,
            special_style: false,
            widescreen_storyboard: false,
            epilepsy_warning: false,
            samples_match_playback_rate: false,
            countdown: 0,
            countdown_offset: 0,
        }
    }
}

/// The `[Metadata]` section. Absent strings are empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Metadata {
    /// `Title`.
    pub title: String,
    /// `TitleUnicode`.
    pub title_unicode: String,
    /// `Artist`.
    pub artist: String,
    /// `ArtistUnicode`.
    pub artist_unicode: String,
    /// `Creator`.
    pub creator: String,
    /// `Version` (the difficulty name).
    pub version: String,
    /// `Source`.
    pub source: String,
    /// `Tags` (raw, space separated).
    pub tags: String,
    /// `BeatmapID`. Default -1.
    pub beatmap_id: i32,
    /// `BeatmapSetID`; `None` when the key is absent.
    pub beatmap_set_id: Option<i32>,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            title: String::new(),
            title_unicode: String::new(),
            artist: String::new(),
            artist_unicode: String::new(),
            creator: String::new(),
            version: String::new(),
            source: String::new(),
            tags: String::new(),
            beatmap_id: -1,
            beatmap_set_id: None,
        }
    }
}

/// The `[Difficulty]` section, unclamped.
#[derive(Debug, Clone, PartialEq)]
pub struct Difficulty {
    /// `HPDrainRate`. Default 5.
    pub drain_rate: f32,
    /// `CircleSize`. Default 5.
    pub circle_size: f32,
    /// `OverallDifficulty`. Default 5.
    pub overall_difficulty: f32,
    /// `ApproachRate`. Defaults to the overall difficulty when the key is absent
    /// (as long as no `ApproachRate` line precedes `OverallDifficulty`).
    pub approach_rate: f32,
    /// `SliderMultiplier`. Default 1.4.
    pub slider_multiplier: f64,
    /// `SliderTickRate`. Default 1.
    pub slider_tick_rate: f64,
}

impl Default for Difficulty {
    fn default() -> Self {
        Self {
            drain_rate: 5.0,
            circle_size: 5.0,
            overall_difficulty: 5.0,
            approach_rate: 5.0,
            slider_multiplier: 1.4,
            slider_tick_rate: 1.0,
        }
    }
}

/// A break period from `[Events]`, in ms with the timing offset applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Break {
    /// Start time.
    pub start: f64,
    /// End time (never before `start`).
    pub end: f64,
}

/// The parts of `[Events]` that lazer's beatmap decoder reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Events {
    /// Background image path (standardised with `/`), from a background event, a
    /// non-video "video" event, or the first sprite when nothing else set it.
    /// `None` when absent or empty. Video files themselves are not read by the beatmap decoder.
    pub background: Option<String>,
    /// Break periods in file order.
    pub breaks: Vec<Break>,
}

/// One line of `[TimingPoints]`, exactly as read (no merging or de-duplication).
#[derive(Debug, Clone, PartialEq)]
pub struct TimingPoint {
    /// Time in ms with the timing offset applied.
    pub time: f64,
    /// Raw beat length: positive ms per beat for uninherited points, negative inverse
    /// slider velocity for inherited ones, possibly NaN for inherited points.
    pub beat_length: f64,
    /// Time signature numerator (always >= 1; a leading `0` field means 4).
    pub time_signature: i32,
    /// Raw `LegacySampleBank` value (0 none, 1 normal, 2 soft, 3 drum, anything else kept).
    pub sample_set: i32,
    /// Custom sample bank index.
    pub custom_sample_index: i32,
    /// Sample volume.
    pub volume: i32,
    /// True for timing (red) points, false for inherited (green) points.
    pub uninherited: bool,
    /// Kiai mode (effect bit 0).
    pub kiai: bool,
    /// Omit the first bar line (effect bit 3).
    pub omit_first_bar_line: bool,
}

/// The `[Colours]` section.
///
/// Lazer's `Beatmap` model discards these lines (it implements neither `IHasComboColours` nor
/// `IHasCustomColours`); the semantics come from the same `HandleColours(..., allowAlpha: false)`
/// used for beatmap skins (`LegacySkinDecoder` / `LegacyBeatmapSkin`), reached through
/// `LegacyDecoder.ParseLine`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinDecoder.cs
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Colours {
    /// `Combo1`..`Combo8` in file order.
    pub combo: Vec<Rgb>,
    /// Any other colour key, in first-insertion order; a repeated key replaces its value.
    pub custom: Vec<(String, Rgb)>,
}

/// A decoded beatmap header.
#[derive(Debug, Clone, PartialEq)]
pub struct Beatmap {
    /// Version from the `osu file format vN` line.
    pub format_version: i32,
    /// Offset in ms added to times of old maps: 24 when the version is below 5 and offsets
    /// are applied, otherwise 0. Hit object parsing needs the same value.
    pub timing_offset: f64,
    /// `[General]`.
    pub general: General,
    /// `[Editor]` as raw key/value pairs in file order (not interpreted).
    pub editor: Vec<(String, String)>,
    /// `[Metadata]`.
    pub metadata: Metadata,
    /// `[Difficulty]`.
    pub difficulty: Difficulty,
    /// `[Events]`.
    pub events: Events,
    /// `[TimingPoints]` in file order.
    pub timing_points: Vec<TimingPoint>,
    /// `[Colours]`.
    pub colours: Colours,
    /// `[HitObjects]` in file order, as `ConvertHitObjectParser` produced them. Not applied:
    /// sorting, `applySamples`, `postProcessBreaks`, `ApplyDefaults`, and the zero-length-slider
    /// rule of `createSlider` (all need later steps or `SliderPath` evaluation).
    pub hit_objects: Vec<HitObject>,
}
