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

/// Name of a resolved sample: one of the four hit sounds of the file, or a name lazer gives
/// derived samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleName {
    /// `hitnormal`.
    Normal,
    /// `hitwhistle`.
    Whistle,
    /// `hitfinish`.
    Finish,
    /// `hitclap`.
    Clap,
    /// `slidertick`: the sample of slider ticks.
    SliderTick,
}

impl SampleName {
    /// Lazer's sample name string.
    pub fn as_str(self) -> &'static str {
        match self {
            SampleName::Normal => "hitnormal",
            SampleName::Whistle => "hitwhistle",
            SampleName::Finish => "hitfinish",
            SampleName::Clap => "hitclap",
            SampleName::SliderTick => "slidertick",
        }
    }
}

impl From<HitSampleName> for SampleName {
    fn from(name: HitSampleName) -> SampleName {
        match name {
            HitSampleName::Normal => SampleName::Normal,
            HitSampleName::Whistle => SampleName::Whistle,
            HitSampleName::Finish => SampleName::Finish,
            HitSampleName::Clap => SampleName::Clap,
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs (LegacyHitSampleInfo, FileHitSampleInfo)
/// A hit sample after its sample point was applied (lazer's `LegacyHitSampleInfo` returned by
/// `LegacySampleControlPoint.ApplyTo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitSample {
    /// Sample name.
    pub name: SampleName,
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
            name: SampleName::Normal,
            bank: Bank::Normal,
            suffix: None,
            volume,
            editor_auto_bank: false,
            use_beatmap_samples: true,
            is_layered: false,
            filename: Some(filename),
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs (LegacyHitSampleInfo.With, FileHitSampleInfo.With)
    /// Lazer's `With(newName)`: the same sample under another name. A file sample ignores the
    /// name and stays a `hitnormal` file sample.
    pub fn with_name(&self, name: SampleName) -> HitSample {
        match &self.filename {
            Some(filename) => HitSample::file(filename.clone(), self.volume),
            None => HitSample {
                name,
                ..self.clone()
            },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_name_renames_a_bank_sample() {
        let sample = HitSample {
            name: SampleName::Normal,
            bank: Bank::Soft,
            suffix: Some(3),
            volume: 60,
            editor_auto_bank: true,
            use_beatmap_samples: true,
            is_layered: true,
            filename: None,
        };
        let tick = sample.with_name(SampleName::SliderTick);
        assert_eq!(
            tick,
            HitSample {
                name: SampleName::SliderTick,
                ..sample
            }
        );
    }

    #[test]
    fn with_name_keeps_a_file_sample() {
        let sample = HitSample::file("tick.wav".to_owned(), 40);
        assert_eq!(sample.with_name(SampleName::SliderTick), sample);
    }
}
