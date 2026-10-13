//! Hitsound selection: which sample file each sound of each hit object plays, resolved and
//! decoded once when the beatmap loads.
//!
//! Lookup follows lazer: the beatmap's own files (only for samples with a custom sample bank
//! of 1 or more), then the user skin, then the default skin. Every skin is asked with
//! `LegacySkin.getLegacyLookupNames`; a skin whose configuration disables layered hit sounds
//! answers a layered sample with silence, and the lookup stops there.
//!
//! During gameplay [`Hitsounds`] hands out ready samples and gains by object without
//! allocating or hashing.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use slam_audio::Sample;
use slam_osu::Beatmap;
use slam_osu::judgement::Judgement;
use slam_osu::objects::OsuHitObjectKind;
use slam_osu::samples::{
    Bank, HitSample, SampleName, sliding_samples, spinner_bonus_max_sample, spinner_bonus_sample,
    spinning_samples,
};
use slam_skin::{Skin, SkinChain, SkinSource};

/// Lazer's `DrawableHitObject.MINIMUM_SAMPLE_VOLUME`: quieter samples play at this volume.
pub const MINIMUM_SAMPLE_VOLUME: i32 = 5;

/// The skin a sample file was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleSource {
    /// The beatmap's folder (`LegacyBeatmapSkin`).
    Beatmap,
    /// The user's skin.
    User,
    /// The bundled default skin.
    Default,
}

impl From<SkinSource> for SampleSource {
    fn from(source: SkinSource) -> SampleSource {
        match source {
            SkinSource::User => SampleSource::User,
            SkinSource::Default => SampleSource::Default,
        }
    }
}

/// The skins asked for samples, in lookup order.
#[derive(Debug, Clone, Copy)]
pub struct SampleSkins<'a> {
    /// The beatmap's skin ([`Skin::load_beatmap`]); `None` when beatmap hitsounds are turned
    /// off (lazer's `BeatmapHitsounds` setting) or the folder could not be read.
    pub beatmap: Option<&'a Skin>,
    /// The user and default skins.
    pub chain: &'a SkinChain,
}

impl<'a> SampleSkins<'a> {
    /// Skins in lookup order, with whether each requires custom sample bank suffixes
    /// (`UseCustomSampleBanks`: only the beatmap's skin).
    fn sources(self) -> impl Iterator<Item = (SampleSource, &'a Skin, bool)> {
        self.beatmap
            .map(|s| (SampleSource::Beatmap, s, true))
            .into_iter()
            .chain(
                self.chain
                    .sources()
                    .map(|(source, s)| (source.into(), s, false)),
            )
    }
}

/// The result of looking a sample up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// A file was found.
    Found {
        /// The skin that has it.
        source: SampleSource,
        /// The file.
        path: PathBuf,
    },
    /// The sample is layered and this skin disables layered hit sounds (`SampleVirtual` in
    /// lazer): nothing plays and later skins are not asked.
    Silenced(SampleSource),
    /// No skin has the sample.
    Missing,
}

/// Lazer's name of a bank (`LegacySampleBank.ToString().ToLowerInvariant()`).
fn bank_name(bank: Bank) -> String {
    match bank {
        Bank::Normal => "normal".to_owned(),
        Bank::Soft => "soft".to_owned(),
        Bank::Drum => "drum".to_owned(),
        Bank::Other(n) => n.to_string(),
    }
}

/// `Path.ChangeExtension(path, null)`: drops the extension (and a trailing dot) of the last
/// path segment.
fn without_extension(path: &str) -> &str {
    let segment_start = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    match path[segment_start..].rfind('.') {
        Some(dot) => &path[..segment_start + dot],
        None => path,
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Audio/HitSampleInfo.cs (LookupNames)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs (FileHitSampleInfo.LookupNames)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs (SpinnerBonusMaxSampleInfo.LookupNames)
/// The sample's own lookup names, before a skin filters them.
fn sample_lookup_names(sample: &HitSample) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(filename) = &sample.filename {
        names.push(filename.clone());
        names.push(without_extension(filename).to_owned());
    }
    let bank = bank_name(sample.bank);
    let name = sample.name.as_str();
    let mut base = Vec::with_capacity(3);
    if let Some(suffix) = sample.suffix {
        base.push(format!("Gameplay/{bank}-{name}{suffix}"));
    }
    base.push(format!("Gameplay/{bank}-{name}"));
    base.push(format!("Gameplay/{name}"));
    if sample.name == SampleName::SpinnerBonusMax {
        let plain: Vec<String> = base.iter().map(|n| n.replace("-max", "")).collect();
        base.extend(plain);
    }
    names.extend(base);
    names
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkin.cs (getLegacyLookupNames)
/// The names a skin tries for a sample, in order. Each name is tried as given and by its last
/// `/` segment ([`Skin::sample`] does both).
///
/// With a custom sample bank suffix, a skin that uses custom sample banks (the beatmap's) only
/// tries names ending in the suffix, and other skins only names that do not. The bare sample
/// name (`hitnormal`) comes last for every skin.
pub fn lookup_names(sample: &HitSample, custom_sample_banks: bool) -> Vec<String> {
    let mut names = sample_lookup_names(sample);
    if let Some(suffix) = sample.suffix {
        let suffix = suffix.to_string();
        names.retain(|n| n.ends_with(&suffix) == custom_sample_banks);
    }
    names.push(sample.name.as_str().to_owned());
    names
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/SkinProvidingContainer.cs (GetSample)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinTransformer.cs (GetSample)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyBeatmapSkin.cs (GetSample)
/// Looks a sample up through the skins: the first file found wins.
///
/// Each skin first checks the layered rule against its own configuration, then the beatmap's
/// skin skips samples without beatmap samples (custom sample bank 0). A found file that later
/// fails to decode does not continue the lookup: lazer's `SampleStore` returns a sample for any
/// file, and BASS plays nothing for a broken one.
pub fn lookup(sample: &HitSample, skins: SampleSkins<'_>) -> Lookup {
    for (source, skin, custom_sample_banks) in skins.sources() {
        if sample.is_layered && !skin.layered_hit_sounds() {
            return Lookup::Silenced(source);
        }
        if source == SampleSource::Beatmap && !sample.use_beatmap_samples {
            continue;
        }
        for name in lookup_names(sample, custom_sample_banks) {
            if let Some(path) = skin.sample(&name) {
                return Lookup::Found { source, path };
            }
        }
    }
    Lookup::Missing
}

/// Decoded samples by file, converted to the mixer's format. One [`Arc`] per file, so the
/// mixer's per-sample voice limit counts copies of the same file.
///
/// Keep it while the skins and the output format stay the same: beatmaps share the skin's
/// samples. Drop a beatmap's own files with [`SampleCache::remove_dir`] when it is unloaded.
#[derive(Debug)]
pub struct SampleCache {
    sample_rate: u32,
    channels: u16,
    samples: HashMap<PathBuf, Option<Arc<Sample>>>,
}

impl SampleCache {
    /// An empty cache for the mixer's sample rate and channel count.
    pub fn new(sample_rate: u32, channels: u16) -> SampleCache {
        SampleCache {
            sample_rate,
            channels,
            samples: HashMap::new(),
        }
    }

    /// The decoded sample of a file; `None` if it cannot be read or decoded (logged once).
    pub fn get(&mut self, path: &Path) -> Option<Arc<Sample>> {
        if let Some(sample) = self.samples.get(path) {
            return sample.clone();
        }
        let sample = self.decode(path);
        self.samples.insert(path.to_path_buf(), sample.clone());
        sample
    }

    fn decode(&self, path: &Path) -> Option<Arc<Sample>> {
        let bytes = fs::read(path)
            .map_err(|e| tracing::warn!(path = %path.display(), "cannot read sample: {e}"))
            .ok()?;
        let extension = path.extension().and_then(|e| e.to_str());
        Sample::load(&bytes, extension, self.sample_rate, self.channels)
            .map_err(|e| tracing::warn!(path = %path.display(), "cannot decode sample: {e}"))
            .ok()
            .map(Arc::new)
    }

    /// Drops the samples of files below `dir`, for example a beatmap folder after the beatmap
    /// is unloaded.
    pub fn remove_dir(&mut self, dir: &Path) {
        self.samples.retain(|path, _| !path.starts_with(dir));
    }

    /// The number of cached files, including ones that failed to decode.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Drops all samples, for example after a skin change.
    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

/// A sample ready for the mixer.
#[derive(Debug, Clone)]
pub struct Hitsound {
    /// The decoded sample.
    pub sample: Arc<Sample>,
    /// Gain 0..=1.
    pub gain: f32,
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/SkinnableSound.cs (updateSamples)
/// The playback gain of a sample volume: at least [`MINIMUM_SAMPLE_VOLUME`], as a fraction of
/// 100, and at most 1 (the framework's `Volume` range).
pub fn sample_gain(volume: i32) -> f32 {
    (f64::from(volume.max(MINIMUM_SAMPLE_VOLUME)) / 100.0).min(1.0) as f32
}

/// Which sound of an object to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// The object was hit: a circle's or spinner's samples, a slider's tail samples (lazer's
    /// `DrawableSlider` plays them; whether the tail must be hit is the caller's rule).
    Hit,
    /// The nested object with this index was hit: a slider's head, tick or repeat (its tail
    /// has none), or a spinner's bonus tick (its plain ticks have none). The index is
    /// [`ObjectRef::nested`](slam_osu::judgement::ObjectRef::nested): for a spinner, the tick
    /// number.
    Nested(u32),
    /// The looping samples while a slider is tracked (`sliderslide`, `sliderwhistle`).
    Sliding,
    /// The looping sample while a spinner is spun (`spinnerspin`).
    Spinning,
    /// A whole spin after the spinner's last bonus tick (`spinnerbonus-max`).
    SpinnerBonusMax,
}

/// A range of `Hitsounds::voices`.
#[derive(Debug, Clone, Copy, Default)]
struct Span {
    start: u32,
    len: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct ObjectSounds {
    hit: Span,
    looping: Span,
    bonus: Span,
    bonus_max: Span,
    /// Range of `Hitsounds::nested` (slider nested objects).
    nested: Span,
    /// A spinner's bonus ticks by nested index, `bonus_from..bonus_end`
    /// (`SpinsRequiredForBonus..tick count`); they play `bonus`. Empty for other objects.
    bonus_from: u32,
    bonus_end: u32,
}

/// Every sound of a beatmap, resolved and decoded.
#[derive(Debug, Clone, Default)]
pub struct Hitsounds {
    voices: Vec<Hitsound>,
    nested: Vec<Span>,
    objects: Vec<ObjectSounds>,
}

/// Resolves samples once per distinct [`HitSample`] while a beatmap is built.
struct Resolver<'a, 'c> {
    skins: SampleSkins<'a>,
    cache: &'c mut SampleCache,
    resolved: HashMap<HitSample, Option<Hitsound>>,
    voices: Vec<Hitsound>,
}

impl Resolver<'_, '_> {
    fn resolve(&mut self, sample: &HitSample) -> Option<Hitsound> {
        if let Some(found) = self.resolved.get(sample) {
            return found.clone();
        }
        let found = match lookup(sample, self.skins) {
            Lookup::Found { path, .. } => self.cache.get(&path).map(|decoded| Hitsound {
                sample: decoded,
                gain: sample_gain(sample.volume),
            }),
            Lookup::Silenced(_) | Lookup::Missing => None,
        };
        self.resolved.insert(sample.clone(), found.clone());
        found
    }

    /// Appends the playable samples of a list; samples that resolve to nothing are left out.
    fn push<'s>(&mut self, samples: impl IntoIterator<Item = &'s HitSample>) -> Span {
        let start = self.voices.len();
        for sample in samples {
            if let Some(sound) = self.resolve(sample) {
                self.voices.push(sound);
            }
        }
        span(start, self.voices.len())
    }
}

/// `start..end` of a vector as a [`Span`]. Beatmap object and sample counts fit in `u32`.
fn span(start: usize, end: usize) -> Span {
    Span {
        start: start as u32,
        len: (end - start) as u32,
    }
}

impl Hitsounds {
    /// Resolves and decodes every sound of `beatmap`. Reads and decodes files through
    /// `cache`; call it off the hot paths (when the beatmap loads).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (LoadSamples)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs (LoadSamples)
    pub fn new(beatmap: &Beatmap, skins: SampleSkins<'_>, cache: &mut SampleCache) -> Hitsounds {
        let mut resolver = Resolver {
            skins,
            cache,
            resolved: HashMap::new(),
            voices: Vec::new(),
        };
        let mut nested = Vec::new();
        let mut objects = Vec::with_capacity(beatmap.hit_objects.len());
        for object in &beatmap.hit_objects {
            let sounds = match &object.kind {
                OsuHitObjectKind::Circle => ObjectSounds {
                    hit: resolver.push(&object.samples),
                    ..ObjectSounds::default()
                },
                OsuHitObjectKind::Slider(slider) => {
                    let first = nested.len();
                    for n in &slider.nested {
                        nested.push(resolver.push(&n.samples));
                    }
                    ObjectSounds {
                        hit: resolver.push(&slider.tail_samples),
                        looping: resolver.push(&sliding_samples(&object.samples)),
                        nested: span(first, nested.len()),
                        ..ObjectSounds::default()
                    }
                }
                OsuHitObjectKind::Spinner(spinner) => ObjectSounds {
                    hit: resolver.push(&object.samples),
                    looping: resolver.push(&spinning_samples(&object.samples)),
                    bonus: resolver.push([&spinner_bonus_sample(&object.samples)]),
                    bonus_max: resolver.push([&spinner_bonus_max_sample(&object.samples)]),
                    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Spinner.cs (CreateNestedHitObjects)
                    bonus_from: non_negative(spinner.spins_required_for_bonus()),
                    bonus_end: non_negative(spinner.tick_count()),
                    ..ObjectSounds::default()
                },
            };
            objects.push(sounds);
        }
        Hitsounds {
            voices: resolver.voices,
            nested,
            objects,
        }
    }

    /// The samples of one sound of the object with this index; empty if it has none.
    pub fn get(&self, index: u32, sound: Sound) -> &[Hitsound] {
        let Some(object) = self.objects.get(index as usize) else {
            return &[];
        };
        let span = match sound {
            Sound::Hit => object.hit,
            Sound::Nested(k) if (object.bonus_from..object.bonus_end).contains(&k) => object.bonus,
            Sound::Nested(k) if k < object.nested.len => {
                self.nested[(object.nested.start + k) as usize]
            }
            Sound::Nested(_) => Span::default(),
            Sound::Sliding | Sound::Spinning => object.looping,
            Sound::SpinnerBonusMax => object.bonus_max,
        };
        &self.voices[span.start as usize..(span.start + span.len) as usize]
    }

    /// The samples a judgement plays: [`Sound::Hit`] or [`Sound::Nested`] of its object if it
    /// is a hit, nothing otherwise (lazer plays samples only on the transition to `Hit`).
    ///
    /// A slider's own judgement plays its tail samples; lazer plays them only if the tail was
    /// hit too (`SamplePlaysOnlyOnHit`, off with Classic's `AlwaysPlayTailSample`), which is
    /// the caller's rule.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Drawables/DrawableHitObject.cs (UpdateState)
    pub fn for_judgement(&self, judgement: &Judgement) -> &[Hitsound] {
        if !judgement.result.is_hit() {
            return &[];
        }
        let object = judgement.object;
        match object.nested {
            None => self.get(object.index, Sound::Hit),
            Some(k) => self.get(object.index, Sound::Nested(k)),
        }
    }
}

/// An `i32` count as `u32`; negative counts are empty.
fn non_negative(count: i32) -> u32 {
    u32::try_from(count).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: SampleName, bank: Bank, suffix: Option<i32>) -> HitSample {
        HitSample {
            name,
            bank,
            suffix,
            volume: 100,
            editor_auto_bank: false,
            use_beatmap_samples: suffix.is_some(),
            is_layered: false,
            filename: None,
        }
    }

    #[test]
    fn lookup_names_without_suffix() {
        let s = sample(SampleName::Normal, Bank::Soft, None);
        let expected = ["Gameplay/soft-hitnormal", "Gameplay/hitnormal", "hitnormal"];
        assert_eq!(lookup_names(&s, false), expected);
        assert_eq!(lookup_names(&s, true), expected);
    }

    #[test]
    fn suffix_is_required_by_the_beatmap_and_forbidden_to_skins() {
        let s = sample(SampleName::Whistle, Bank::Drum, Some(3));
        assert_eq!(
            lookup_names(&s, true),
            ["Gameplay/drum-hitwhistle3", "hitwhistle"]
        );
        assert_eq!(
            lookup_names(&s, false),
            [
                "Gameplay/drum-hitwhistle",
                "Gameplay/hitwhistle",
                "hitwhistle"
            ]
        );
    }

    #[test]
    fn derived_and_unknown_banks() {
        let s = sample(SampleName::SliderTick, Bank::Normal, Some(2));
        assert_eq!(
            lookup_names(&s, true),
            ["Gameplay/normal-slidertick2", "slidertick"]
        );
        let s = sample(SampleName::Normal, Bank::Other(7), None);
        assert_eq!(lookup_names(&s, false)[0], "Gameplay/7-hitnormal");
    }

    #[test]
    fn file_samples_try_the_file_first() {
        let s = HitSample::file("sounds/Hit.Kick.wav".to_owned(), 70);
        assert_eq!(
            lookup_names(&s, true),
            [
                "sounds/Hit.Kick.wav",
                "sounds/Hit.Kick",
                "Gameplay/normal-hitnormal",
                "Gameplay/hitnormal",
                "hitnormal"
            ]
        );
    }

    #[test]
    fn bonus_max_falls_back_to_bonus_names() {
        let s = sample(SampleName::SpinnerBonusMax, Bank::Soft, Some(4));
        assert_eq!(
            lookup_names(&s, false),
            [
                "Gameplay/soft-spinnerbonus-max",
                "Gameplay/spinnerbonus-max",
                "Gameplay/soft-spinnerbonus",
                "Gameplay/spinnerbonus",
                "spinnerbonus-max"
            ]
        );
    }

    #[test]
    fn change_extension() {
        assert_eq!(without_extension("a.wav"), "a");
        assert_eq!(without_extension("a.b.ogg"), "a.b");
        assert_eq!(without_extension("dir.x/a"), "dir.x/a");
        assert_eq!(without_extension("a."), "a");
        assert_eq!(without_extension("a"), "a");
    }

    #[test]
    fn gain_has_a_floor_and_a_ceiling() {
        assert_eq!(sample_gain(0), 0.05);
        assert_eq!(sample_gain(5), 0.05);
        assert_eq!(sample_gain(60), 0.6);
        assert_eq!(sample_gain(100), 1.0);
        assert_eq!(sample_gain(250), 1.0);
    }
}
