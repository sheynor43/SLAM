//! Hit samples with the bank, volume and custom sample bank resolved from sample points.

use slam_formats::osu::{HitSampleName, SampleBank};

/// A sample bank as lazer stores it on sample points and resolved samples.
///
/// Lazer keeps banks as strings. A sample point's bank comes from `LegacySampleBank.ToString()`,
/// so a value outside the enum becomes its number (for example `"4"`); [`Bank::Other`] keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// `sliderslide`: the looping sample while a slider is tracked.
    SliderSlide,
    /// `sliderwhistle`: the looping whistle while a slider is tracked.
    SliderWhistle,
    /// `spinnerspin`: the looping sample while a spinner is spun.
    SpinnerSpin,
    /// `spinnerbonus`: the sample of spinner bonus ticks.
    SpinnerBonus,
    /// `spinnerbonus-max`: the sample of whole spins after the last bonus tick.
    SpinnerBonusMax,
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
            SampleName::SliderSlide => "sliderslide",
            SampleName::SliderWhistle => "sliderwhistle",
            SampleName::SpinnerSpin => "spinnerspin",
            SampleName::SpinnerBonus => "spinnerbonus",
            SampleName::SpinnerBonusMax => "spinnerbonus-max",
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/HitObject.cs (CreateSlidingSamples)
/// The looping samples of a slider made from its body samples: `sliderslide` from the first
/// `hitnormal` and `sliderwhistle` from the first `hitwhistle`.
pub fn sliding_samples(samples: &[HitSample]) -> Vec<HitSample> {
    let normal = samples.iter().find(|s| s.name == SampleName::Normal);
    let whistle = samples.iter().find(|s| s.name == SampleName::Whistle);
    normal
        .map(|s| s.with_name(SampleName::SliderSlide))
        .into_iter()
        .chain(whistle.map(|s| s.with_name(SampleName::SliderWhistle)))
        .collect()
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Spinner.cs (CreateSpinningSamples)
/// The looping sample of a spinner: `spinnerspin` from its first sample.
pub fn spinning_samples(samples: &[HitSample]) -> Vec<HitSample> {
    samples
        .first()
        .map(|s| s.with_name(SampleName::SpinnerSpin))
        .into_iter()
        .collect()
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/HitObject.cs (CreateHitSampleInfo)
/// Lazer's `CreateHitSampleInfo(name)`: a sample of an object's banks under another name. An
/// addition takes the bank of the object's first addition; otherwise the first `hitnormal`
/// is used, and without one a default `normal` sample at full volume.
pub fn create_hit_sample(samples: &[HitSample], name: SampleName) -> HitSample {
    if name != SampleName::Normal
        && let Some(addition) = samples.iter().find(|s| s.name != SampleName::Normal)
    {
        return addition.with_name(name);
    }
    if let Some(normal) = samples.iter().find(|s| s.name == SampleName::Normal) {
        let mut sample = normal.with_name(name);
        // `FileHitSampleInfo.With` drops the flag.
        sample.editor_auto_bank = sample.filename.is_none();
        return sample;
    }
    // `new HitSampleInfo(name)`: a plain sample, never looked up in the beatmap's files.
    HitSample {
        name,
        bank: Bank::Normal,
        suffix: None,
        volume: 100,
        editor_auto_bank: true,
        use_beatmap_samples: false,
        is_layered: false,
        filename: None,
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Spinner.cs (CreateNestedHitObjects)
/// The sample of a spinner's bonus ticks: `CreateHitSampleInfo("spinnerbonus")`.
pub fn spinner_bonus_sample(samples: &[HitSample]) -> HitSample {
    create_hit_sample(samples, SampleName::SpinnerBonus)
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs (LoadSamples, SpinnerBonusMaxSampleInfo)
/// The sample of whole spins after the last bonus tick: `spinnerbonus-max` with the bank,
/// suffix and volume of `CreateHitSampleInfo()`. Lazer makes it a plain `HitSampleInfo`, so it is
/// neither layered nor looked up in the beatmap's files. Its lookup also tries the
/// `spinnerbonus` names.
pub fn spinner_bonus_max_sample(samples: &[HitSample]) -> HitSample {
    let reference = create_hit_sample(samples, SampleName::Normal);
    HitSample {
        name: SampleName::SpinnerBonusMax,
        bank: reference.bank,
        suffix: reference.suffix,
        volume: reference.volume,
        editor_auto_bank: true,
        use_beatmap_samples: false,
        is_layered: false,
        filename: None,
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

    fn sample(name: SampleName, bank: Bank, volume: i32) -> HitSample {
        HitSample {
            name,
            bank,
            suffix: Some(2),
            volume,
            editor_auto_bank: false,
            use_beatmap_samples: true,
            is_layered: false,
            filename: None,
        }
    }

    #[test]
    fn sliding_samples_follow_normal_and_whistle() {
        let samples = [
            sample(SampleName::Clap, Bank::Drum, 50),
            sample(SampleName::Whistle, Bank::Drum, 50),
            sample(SampleName::Normal, Bank::Soft, 50),
        ];
        let sliding = sliding_samples(&samples);
        assert_eq!(sliding.len(), 2);
        assert_eq!(sliding[0].name, SampleName::SliderSlide);
        assert_eq!(sliding[0].bank, Bank::Soft);
        assert_eq!(sliding[1].name, SampleName::SliderWhistle);
        assert_eq!(sliding[1].bank, Bank::Drum);
        assert!(sliding_samples(&samples[..1]).is_empty());
    }

    #[test]
    fn spinning_sample_comes_from_the_first_sample() {
        let samples = [
            sample(SampleName::Finish, Bank::Drum, 40),
            sample(SampleName::Normal, Bank::Soft, 40),
        ];
        let spinning = spinning_samples(&samples);
        assert_eq!(spinning.len(), 1);
        assert_eq!(spinning[0].name, SampleName::SpinnerSpin);
        assert_eq!(spinning[0].bank, Bank::Drum);
        assert!(spinning_samples(&[]).is_empty());
    }

    #[test]
    fn bonus_sample_prefers_the_addition_bank() {
        let samples = [
            sample(SampleName::Normal, Bank::Soft, 40),
            sample(SampleName::Clap, Bank::Drum, 40),
        ];
        let bonus = spinner_bonus_sample(&samples);
        assert_eq!(bonus.name, SampleName::SpinnerBonus);
        assert_eq!(bonus.bank, Bank::Drum);
        let bonus = spinner_bonus_sample(&samples[..1]);
        assert_eq!(bonus.bank, Bank::Soft);
        assert!(bonus.editor_auto_bank);
    }

    #[test]
    fn bonus_sample_without_samples_is_a_plain_default() {
        let bonus = spinner_bonus_sample(&[]);
        assert_eq!(bonus.bank, Bank::Normal);
        assert_eq!(bonus.volume, 100);
        assert!(!bonus.use_beatmap_samples);
    }

    #[test]
    fn bonus_max_sample_takes_the_normal_bank_and_never_uses_beatmap_samples() {
        let mut normal = sample(SampleName::Normal, Bank::Soft, 30);
        normal.is_layered = true;
        let samples = [sample(SampleName::Clap, Bank::Drum, 30), normal];
        let max = spinner_bonus_max_sample(&samples);
        assert_eq!(max.name, SampleName::SpinnerBonusMax);
        assert_eq!(max.bank, Bank::Soft);
        assert_eq!(max.suffix, Some(2));
        assert_eq!(max.volume, 30);
        assert!(!max.use_beatmap_samples);
        assert!(!max.is_layered);
    }

    #[test]
    fn derived_samples_of_a_file_sample_stay_file_samples() {
        let samples = [HitSample::file("hit.wav".to_owned(), 70)];
        assert_eq!(sliding_samples(&samples), samples.to_vec());
        assert_eq!(
            spinner_bonus_sample(&samples).filename.as_deref(),
            Some("hit.wav")
        );
        let max = spinner_bonus_max_sample(&samples);
        assert_eq!(max.filename, None);
        assert_eq!(max.suffix, None);
        assert_eq!(max.volume, 70);
    }
}
