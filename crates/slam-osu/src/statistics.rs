//! What the song select shows of a beatmap: length, BPM, object counts and the difficulty
//! with mods, as lazer's `BeatmapTitleWedge` and `OsuRuleset.GetAdjustedDisplayDifficulty`.

use crate::beatmap::{Beatmap, Difficulty};
use crate::control_points::{ControlPoints, TimingPoint};
use crate::hit_windows::GREAT_WINDOW_RANGE;
use crate::mods::{GameplayMod, ModSet};
use crate::objects::{OsuHitObject, OsuHitObjectKind, PREEMPT_RANGE};

/// A beatmap's statistics without mods, in milliseconds and beats per minute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeatmapStatistics {
    /// From the first object's start to the last object's end (lazer's `BeatmapInfo.Length`).
    pub length: f64,
    /// [`length`](Self::length) without the breaks, at least 0.
    pub drain_length: f64,
    /// The BPM of the longest beat.
    pub bpm_minimum: f64,
    /// The BPM of the shortest beat.
    pub bpm_maximum: f64,
    /// The BPM that lasts longest.
    pub bpm_most_common: f64,
    /// Hit circles.
    pub circles: u32,
    /// Sliders.
    pub sliders: u32,
    /// Spinners.
    pub spinners: u32,
}

impl BeatmapStatistics {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/BeatmapInfoExtensions.cs (UpdateStatisticsFromBeatmap)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmap.cs (GetStatistics)
    /// The statistics of `beatmap`, built without mods.
    pub fn of(beatmap: &Beatmap) -> Self {
        let length = playable_length(&beatmap.hit_objects);
        let breaks: f64 = beatmap.breaks.iter().map(|b| b.end - b.start).sum();
        let count = |f: fn(&OsuHitObjectKind) -> bool| {
            beatmap.hit_objects.iter().filter(|o| f(&o.kind)).count() as u32
        };
        BeatmapStatistics {
            length,
            drain_length: (length - breaks).max(0.0),
            bpm_minimum: bpm_minimum(&beatmap.control_points),
            bpm_maximum: bpm_maximum(&beatmap.control_points),
            bpm_most_common: 60000.0 / most_common_beat_length(beatmap),
            circles: count(|k| matches!(k, OsuHitObjectKind::Circle)),
            sliders: count(|k| matches!(k, OsuHitObjectKind::Slider(_))),
            spinners: count(|k| matches!(k, OsuHitObjectKind::Spinner(_))),
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/IBeatmap.cs (CalculatePlayableBounds)
/// The start of the first object and the latest end of any object; `(0, 0)` without objects.
pub fn playable_bounds(objects: &[OsuHitObject]) -> (f64, f64) {
    let Some(first) = objects.first() else {
        return (0.0, 0.0);
    };
    let last = objects
        .iter()
        .map(OsuHitObject::end_time)
        .fold(f64::NEG_INFINITY, f64::max);
    (first.start_time, last)
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/IBeatmap.cs (CalculatePlayableLength)
/// From the first object's start to the latest end of any object.
pub fn playable_length(objects: &[OsuHitObject]) -> f64 {
    let (start, end) = playable_bounds(objects);
    end - start
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/ControlPointInfo.cs (BPMMinimum)
/// The BPM of the longest beat of any timing point; 60 without timing points.
pub fn bpm_minimum(points: &ControlPoints) -> f64 {
    let longest = points
        .timing_points()
        .iter()
        .map(|t| t.beat_length)
        .reduce(|a, b| if b > a { b } else { a })
        .unwrap_or(TimingPoint::DEFAULT_BEAT_LENGTH);
    60000.0 / longest
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/ControlPointInfo.cs (BPMMaximum)
/// The BPM of the shortest beat of any timing point; 60 without timing points.
pub fn bpm_maximum(points: &ControlPoints) -> f64 {
    let shortest = points
        .timing_points()
        .iter()
        .map(|t| t.beat_length)
        .reduce(|a, b| if b < a { b } else { a })
        .unwrap_or(TimingPoint::DEFAULT_BEAT_LENGTH);
    60000.0 / shortest
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Beatmap.cs (GetMostCommonBeatLength)
/// The beat length that lasts longest up to the last object's end, the first timing point
/// counted from 0 as osu!stable did.
pub fn most_common_beat_length(beatmap: &Beatmap) -> f64 {
    let timing = beatmap.control_points.timing_points();
    let last_time = if beatmap.hit_objects.is_empty() {
        timing.last().map_or(0.0, |t| t.time)
    } else {
        playable_bounds(&beatmap.hit_objects).1
    };
    // (rounded beat length, duration) in order of first appearance, as `GroupBy` keeps it.
    let mut groups: Vec<(f64, f64)> = Vec::new();
    for (i, t) in timing.iter().enumerate() {
        let duration = if t.time > last_time {
            0.0
        } else {
            let current = if i == 0 { 0.0 } else { t.time };
            let next = timing.get(i + 1).map_or(last_time, |n| n.time);
            next - current
        };
        let key = (t.beat_length * 1000.0).round_ties_even() / 1000.0;
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, sum)) => *sum += duration,
            None => groups.push((key, duration)),
        }
    }
    // `OrderByDescending` is stable: the first of equal durations wins.
    let most_common = groups
        .iter()
        .copied()
        .reduce(|best, g| if g.1 > best.1 { g } else { best })
        .map_or(0.0, |(key, _)| key);
    if most_common == 0.0 {
        return TimingPoint::DEFAULT_BEAT_LENGTH;
    }
    let min = timing
        .iter()
        .map(|t| t.beat_length)
        .fold(f64::INFINITY, f64::min);
    let max = timing
        .iter()
        .map(|t| t.beat_length)
        .fold(f64::NEG_INFINITY, f64::max);
    most_common.clamp(min, max)
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/FormatUtils.cs (RoundBPM)
/// A BPM at `rate`, rounded as lazer shows it (`Math.Round`: half to even).
pub fn round_bpm(bpm: f64, rate: f64) -> i32 {
    (bpm * rate).round_ties_even() as i32
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Ruleset.cs (GetAdjustedDisplayDifficulty)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/OsuRuleset.cs (GetAdjustedDisplayDifficulty)
/// `difficulty` with the mods' changes, and the approach rate and overall difficulty a
/// player perceives at the mods' rate.
pub fn adjusted_display_difficulty(difficulty: Difficulty, mods: &ModSet) -> Difficulty {
    let mut adjusted = difficulty;
    for m in mods.mods() {
        m.apply_to_difficulty(&mut adjusted);
    }
    let rate = mods.rate().rate;
    let preempt = PREEMPT_RANGE.value(f64::from(adjusted.approach_rate)) / rate;
    adjusted.approach_rate = PREEMPT_RANGE.inverse(preempt) as f32;
    let great = GREAT_WINDOW_RANGE.value(f64::from(adjusted.overall_difficulty)) / rate;
    adjusted.overall_difficulty = GREAT_WINDOW_RANGE.inverse(great) as f32;
    adjusted
}

#[cfg(test)]
mod tests;
