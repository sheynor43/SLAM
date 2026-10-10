//! Hit samples with the bank, volume and custom sample bank resolved from sample points.

use slam_formats::osu::{HitSampleName, SampleBank};

/// A sample bank as lazer stores it on sample points and resolved samples.
///
/// Lazer keeps banks as strings. A sample point's bank comes from `LegacySampleBank.ToString()`,
/// so a value outside the enum becomes its number (for example `"4"`); [`Bank::Other`] keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bank {
    /// `normal`.
    Normal,
    /// `soft`.
    Soft,
    /// `drum`.
    Drum,
    /// A `LegacySampleBank` value outside the enum, named by its number.
    Other(i32),
}

impl Bank {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs (handleTimingPoint)
    /// The bank of a timing point's raw `LegacySampleBank` value; `None` (0) means `normal`.
    pub fn from_legacy(value: i32) -> Bank {
        match value {
            0 | 1 => Bank::Normal,
            2 => Bank::Soft,
            3 => Bank::Drum,
            other => Bank::Other(other),
        }
    }
}

impl From<SampleBank> for Bank {
    fn from(bank: SampleBank) -> Bank {
        match bank {
            SampleBank::Normal => Bank::Normal,
            SampleBank::Soft => Bank::Soft,
            SampleBank::Drum => Bank::Drum,
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs (LegacyHitSampleInfo, FileHitSampleInfo)
/// A hit sample after its sample point was applied (lazer's `LegacyHitSampleInfo` returned by
/// `LegacySampleControlPoint.ApplyTo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitSample {
    /// Sample name.
    pub name: HitSampleName,
    /// Bank.
    pub bank: Bank,
    /// Lookup suffix: the custom sample bank when it is 2 or above.
    pub suffix: Option<i32>,
    /// Volume 0..=100.
    pub volume: i32,
    /// Whether the addition bank follows the normal bank in the editor.
    pub editor_auto_bank: bool,
    /// Whether the sample can be looked up in the beatmap's own samples (custom sample bank >= 1).
    pub use_beatmap_samples: bool,
    /// Whether this is a layered normal sample.
    pub is_layered: bool,
    /// Custom sample file name; replaces all bank-based lookups.
    pub filename: Option<String>,
}

impl HitSample {
    /// Lazer's `FileHitSampleInfo(filename, volume)`: `hitnormal` in the normal bank with custom
    /// sample bank 1, so that beatmap samples are never replaced by the user skin.
    pub fn file(filename: String, volume: i32) -> HitSample {
        HitSample {
            name: HitSampleName::Normal,
            bank: Bank::Normal,
            suffix: None,
            volume,
            editor_auto_bank: false,
            use_beatmap_samples: true,
            is_layered: false,
            filename: Some(filename),
        }
    }

    /// Lazer's `LegacyHitSampleInfo.CustomSampleBank`.
    pub fn custom_sample_bank(&self) -> i32 {
        match self.suffix {
            Some(s) => s,
            None => i32::from(self.use_beatmap_samples),
        }
    }
}
