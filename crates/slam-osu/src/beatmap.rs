//! A playable osu!standard beatmap built from a decoded `.osu` file.

use slam_formats::osu::{self as osu_file, Break, HitObjectKind};

use crate::control_points::ControlPoints;
use crate::dotnet::compare_f64;
use crate::mods::{GameplayMod, Mod};
use crate::objects::{ComboInfo, ObjectDefaults, OsuHitObject, OsuHitObjectKind, Slider};
use crate::stacking;

/// Lazer's `LegacyBeatmapDecoder.CONTROL_POINT_LENIENCY`: sample points are looked up this many
/// ms after a sample's time, so that a point placed slightly late still applies.
pub const CONTROL_POINT_LENIENCY: f64 = 5.0;

/// Why a decoded beatmap cannot be played.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BeatmapError {
    /// The map is for another ruleset (`Mode` in `[General]`); only osu!standard (0) is played.
    #[error("beatmap mode {0} is not osu!standard")]
    UnsupportedMode(i32),
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/BeatmapDifficulty.cs
/// Difficulty settings, clamped to the ranges lazer allows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Difficulty {
    /// HP drain rate, 0..=10.
    pub drain_rate: f32,
    /// Circle size, 0..=10.
    pub circle_size: f32,
    /// Overall difficulty, 0..=10.
    pub overall_difficulty: f32,
    /// Approach rate, 0..=10.
    pub approach_rate: f32,
    /// Base slider velocity in hundreds of osu!pixels per beat, 0.4..=3.6.
    pub slider_multiplier: f64,
    /// Slider ticks per beat, 0.5..=8.
    pub slider_tick_rate: f64,
}

impl Difficulty {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs (applyDifficultyRestrictions)
    /// Lazer's `applyDifficultyRestrictions` for an osu!standard map.
    pub fn from_file(d: &osu_file::Difficulty) -> Difficulty {
        Difficulty {
            drain_rate: d.drain_rate.clamp(0.0, 10.0),
            circle_size: d.circle_size.clamp(0.0, 10.0),
            overall_difficulty: d.overall_difficulty.clamp(0.0, 10.0),
            approach_rate: d.approach_rate.clamp(0.0, 10.0),
            slider_multiplier: d.slider_multiplier.clamp(0.4, 3.6),
            slider_tick_rate: d.slider_tick_rate.clamp(0.5, 8.0),
        }
    }
}

/// A playable osu!standard beatmap: control points, difficulty, breaks and converted objects
/// with combo information, difficulty defaults, the mods' changes and stacking applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Beatmap {
    /// `.osu` format version.
    pub format_version: i32,
    /// Difficulty settings, with the mods' changes.
    pub difficulty: Difficulty,
    /// Stack leniency from `[General]`.
    pub stack_leniency: f32,
    /// Control points.
    pub control_points: ControlPoints,
    /// Breaks sorted by start, then end time.
    pub breaks: Vec<Break>,
    /// Hit objects sorted by start time (stable: objects with equal times keep file order).
    pub hit_objects: Vec<OsuHitObject>,
}

impl Beatmap {
    /// Processes a decoded `.osu` file the way lazer's decoder, `OsuBeatmapConverter` and
    /// `OsuBeatmapProcessor.PreProcess` do: clamps the difficulty, builds the control points,
    /// sorts objects and breaks, forces new combos after breaks and spinners, resolves samples
    /// and assigns combo indices. Then applies the difficulty defaults to every object
    /// ([`OsuHitObject::apply_defaults`]), as lazer does after `PreProcess`, and stacking
    /// ([`Beatmap::apply_stacking`]), as `PostProcess` does.
    ///
    /// This is [`Beatmap::from_file_with_mods`] without mods.
    pub fn from_file(file: osu_file::Beatmap) -> Result<Beatmap, BeatmapError> {
        Beatmap::from_file_with_mods(file, &mut [] as &mut [Mod])
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs (ParseStreamInto, postProcessBreaks, applyDefaults, applySamples)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmapConverter.cs
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmapProcessor.cs (PreProcess)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/WorkingBeatmap.cs (GetPlayableBeatmap)
    /// Builds the beatmap as [`Beatmap::from_file`] does, applying the mods' beatmap hooks in
    /// lazer's order: every mod's [`apply_to_difficulty`](GameplayMod::apply_to_difficulty)
    /// in mod order, the objects' defaults with the changed difficulty, each mod's
    /// [`apply_to_hit_object`](GameplayMod::apply_to_hit_object) on every object (one mod
    /// after another), stacking, then every mod's
    /// [`apply_to_beatmap`](GameplayMod::apply_to_beatmap).
    ///
    /// Objects are converted with the difficulty of the file: lazer's decoder resolves the
    /// samples before any mod applies.
    pub fn from_file_with_mods<M: GameplayMod>(
        file: osu_file::Beatmap,
        mods: &mut [M],
    ) -> Result<Beatmap, BeatmapError> {
        if file.general.mode != 0 {
            return Err(BeatmapError::UnsupportedMode(file.general.mode));
        }

        let difficulty = Difficulty::from_file(&file.difficulty);
        let control_points = ControlPoints::from_legacy(&file.timing_points);

        let mut breaks = file.events.breaks;
        breaks.sort_by(|a, b| compare_f64(a.start, b.start).then(compare_f64(a.end, b.end)));

        let mut parsed = file.hit_objects;
        // Objects are out of order only in hand-edited files; a stable sort keeps the file
        // order of objects with equal start times.
        parsed.sort_by(|a, b| compare_f64(a.start_time, b.start_time));

        let mut hit_objects: Vec<OsuHitObject> = Vec::with_capacity(parsed.len());
        let mut current_break = 0;

        for h in parsed {
            // postProcessBreaks: the first object after a break starts a new combo.
            let mut force_new_combo = false;
            while current_break < breaks.len() && breaks[current_break].end < h.start_time {
                force_new_combo = true;
                current_break += 1;
            }
            let new_combo = h.new_combo | force_new_combo;

            hit_objects.push(convert(
                h,
                new_combo,
                &control_points,
                &difficulty,
                file.format_version,
            ));
        }

        let mut difficulty = difficulty;
        for m in mods.iter() {
            m.apply_to_difficulty(&mut difficulty);
        }

        assign_combo_info(&mut hit_objects);

        for obj in &mut hit_objects {
            obj.apply_defaults(&control_points, &difficulty);
        }

        for m in mods.iter() {
            for obj in &mut hit_objects {
                m.apply_to_hit_object(obj);
            }
        }

        let mut beatmap = Beatmap {
            format_version: file.format_version,
            difficulty,
            stack_leniency: file.general.stack_leniency,
            control_points,
            breaks,
            hit_objects,
        };
        beatmap.apply_stacking();

        for m in mods.iter_mut() {
            m.apply_to_beatmap(&mut beatmap);
        }
        Ok(beatmap)
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmapProcessor.cs (PostProcess)
    /// Recomputes the stack heights of all objects, as lazer's `OsuBeatmapProcessor.PostProcess`
    /// does after the defaults and the mods are applied. Call again after changing the objects'
    /// defaults, times or positions.
    pub fn apply_stacking(&mut self) {
        stacking::apply_stacking(
            &mut self.hit_objects,
            self.format_version,
            self.stack_leniency,
        );
    }
}

/// Converts one parsed object, applying its difficulty and sample points.
fn convert(
    h: osu_file::HitObject,
    new_combo: bool,
    control_points: &ControlPoints,
    difficulty: &Difficulty,
    format_version: i32,
) -> OsuHitObject {
    let (kind, sample_time) = match h.kind {
        HitObjectKind::Slider(s) => {
            let slider = Slider::new(s, h.start_time, control_points, difficulty, format_version);
            let sample_time = h.start_time + CONTROL_POINT_LENIENCY + 1.0;
            (OsuHitObjectKind::Slider(Box::new(slider)), sample_time)
        }
        // A hold has a duration, so the converter turns it into a spinner.
        HitObjectKind::Spinner { duration } | HitObjectKind::Hold { duration } => {
            // The decoder looks up samples at the convert's end time. The converter then sets
            // the spinner's `EndTime` after its `StartTime`, which stores
            // `Duration = EndTime - StartTime` and reads back `StartTime + Duration`.
            let convert_end_time = h.start_time + duration;
            let end_time = h.start_time + (convert_end_time - h.start_time);
            (
                OsuHitObjectKind::Spinner { end_time },
                convert_end_time + CONTROL_POINT_LENIENCY,
            )
        }
        HitObjectKind::Circle => (
            OsuHitObjectKind::Circle,
            h.start_time + CONTROL_POINT_LENIENCY,
        ),
    };

    let sample_point = control_points.sample_point_at(sample_time);
    let samples = h.samples.iter().map(|s| sample_point.apply_to(s)).collect();

    OsuHitObject {
        start_time: h.start_time,
        position: h.position,
        new_combo,
        combo_offset: h.combo_offset,
        combo: ComboInfo::default(),
        stack_height: 0,
        defaults: ObjectDefaults::default(),
        samples,
        kind,
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmapProcessor.cs (PreProcess)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/BeatmapProcessor.cs (PreProcess)
/// Forces a new combo on the first object and on the first object after a spinner (spinners
/// themselves excluded), then assigns combo info to every object in order.
fn assign_combo_info(objects: &mut [OsuHitObject]) {
    let mut last_is_spinner = None;
    for obj in objects.iter_mut() {
        if !obj.is_spinner() && last_is_spinner.is_none_or(|spinner| spinner) {
            obj.new_combo = true;
        }
        last_is_spinner = Some(obj.is_spinner());
    }

    for i in 0..objects.len() {
        let (before, rest) = objects.split_at_mut(i);
        update_combo_info(&mut rest[0], before.last_mut());
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/OsuHitObject.cs (UpdateComboInformation)
/// Sets the combo info of `obj` from the previous object `last`. For combo colours spinners
/// never start a new combo even when flagged, and the first object and the first object after
/// a spinner always do.
fn update_combo_info(obj: &mut OsuHitObject, last: Option<&mut OsuHitObject>) {
    let mut index = last.as_ref().map_or(0, |l| l.combo.combo_index);
    let mut index_with_offsets = last
        .as_ref()
        .map_or(0, |l| l.combo.combo_index_with_offsets);
    let mut in_current_combo = last
        .as_ref()
        .map_or(0, |l| l.combo.index_in_current_combo.wrapping_add(1));

    let last_is_spinner = last.as_ref().is_some_and(|l| l.is_spinner());
    if !obj.is_spinner() && (obj.new_combo || last.is_none() || last_is_spinner) {
        in_current_combo = 0;
        index = index.wrapping_add(1);
        index_with_offsets = index_with_offsets.wrapping_add(obj.combo_offset.wrapping_add(1));

        if let Some(last) = last {
            last.combo.last_in_combo = true;
        }
    }

    obj.combo = ComboInfo {
        combo_index: index,
        combo_index_with_offsets: index_with_offsets,
        index_in_current_combo: in_current_combo,
        last_in_combo: false,
    };
}
