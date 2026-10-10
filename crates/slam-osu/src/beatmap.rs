//! A playable osu!standard beatmap built from a decoded `.osu` file.

use std::cmp::Ordering;

use slam_formats::osu::{self as osu_file, Break, HitObjectKind};

use crate::control_points::ControlPoints;
use crate::objects::{ComboInfo, OsuHitObject, OsuHitObjectKind, Slider};

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
/// with combo information.
///
/// Node samples of sliders are not resolved yet (see [`Slider::resolve_node_samples`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Beatmap {
    /// `.osu` format version.
    pub format_version: i32,
    /// Difficulty settings.
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
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs (ParseStreamInto, postProcessBreaks, applyDefaults, applySamples)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmapConverter.cs
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmapProcessor.cs (PreProcess)
    /// Processes a decoded `.osu` file the way lazer's decoder, `OsuBeatmapConverter` and
    /// `OsuBeatmapProcessor.PreProcess` do: clamps the difficulty, builds the control points,
    /// sorts objects and breaks, forces new combos after breaks and spinners, resolves samples
    /// and assigns combo indices.
    pub fn from_file(file: osu_file::Beatmap) -> Result<Beatmap, BeatmapError> {
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

            hit_objects.push(convert(h, new_combo, &control_points, file.format_version));
        }

        assign_combo_info(&mut hit_objects);

        Ok(Beatmap {
            format_version: file.format_version,
            difficulty,
            stack_leniency: file.general.stack_leniency,
            control_points,
            breaks,
            hit_objects,
        })
    }
}

/// Converts one parsed object, applying its difficulty and sample points.
fn convert(
    h: osu_file::HitObject,
    new_combo: bool,
    control_points: &ControlPoints,
    format_version: i32,
) -> OsuHitObject {
    let (kind, sample_time) = match h.kind {
        HitObjectKind::Slider(s) => {
            // applyDefaults
            let difficulty_point = control_points.difficulty_point_at(h.start_time);
            // Prior to v8, speed multipliers don't adjust for how many ticks are generated over
            // the same distance.
            let tick_distance_multiplier = if format_version < 8 {
                1.0 / difficulty_point.slider_velocity
            } else {
                1.0
            };

            let slider = Slider {
                generate_ticks: difficulty_point.generate_ticks,
                slider_velocity_multiplier: difficulty_point.slider_velocity,
                tick_distance_multiplier,
                ..Slider::new(s)
            };
            let sample_time = h.start_time + CONTROL_POINT_LENIENCY + 1.0;
            (OsuHitObjectKind::Slider(slider), sample_time)
        }
        // A hold has a duration, so the converter turns it into a spinner.
        HitObjectKind::Spinner { duration } | HitObjectKind::Hold { duration } => {
            let end_time = h.start_time + duration;
            (
                OsuHitObjectKind::Spinner { end_time },
                end_time + CONTROL_POINT_LENIENCY,
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

/// .NET's `double.CompareTo`: NaN sorts before everything, and -0 equals 0.
fn compare_f64(a: f64, b: f64) -> Ordering {
    match a.partial_cmp(&b) {
        Some(ordering) => ordering,
        None => a.is_nan().cmp(&b.is_nan()).reverse(),
    }
}
