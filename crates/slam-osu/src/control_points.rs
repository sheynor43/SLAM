//! Control points (timing, difficulty, effect, sample) and their lookup by time.
//!
//! [`ControlPoints`] keeps one sorted list per point type, like lazer's
//! `LegacyControlPointInfo`. [`ControlPoints::from_legacy`] builds it from the `[TimingPoints]`
//! lines of a `.osu` file with the same merging and de-duplication rules as lazer's
//! `LegacyBeatmapDecoder`.

use slam_formats::osu::TimingPoint as LegacyTimingPoint;

use crate::samples::{Bank, HitSample};

/// Something placed on the timeline.
pub trait ControlPoint {
    /// Time in ms.
    fn time(&self) -> f64;
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/TimingControlPoint.cs
/// A timing (red) point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimingPoint {
    /// Time in ms.
    pub time: f64,
    /// Milliseconds per beat, within [`Self::MIN_BEAT_LENGTH`]..=[`Self::MAX_BEAT_LENGTH`].
    pub beat_length: f64,
    /// Time signature numerator (beats per bar).
    pub time_signature: i32,
    /// Whether the first bar line of this point is not drawn.
    pub omit_first_bar_line: bool,
}

impl TimingPoint {
    /// Lazer's `DEFAULT_BEAT_LENGTH`.
    pub const DEFAULT_BEAT_LENGTH: f64 = 1000.0;
    /// Lower bound of `BeatLengthBindable`.
    pub const MIN_BEAT_LENGTH: f64 = 6.0;
    /// Upper bound of `BeatLengthBindable`.
    pub const MAX_BEAT_LENGTH: f64 = 60000.0;

    /// Lazer's `TimingControlPoint.DEFAULT`, used when a map has no timing points.
    pub const DEFAULT: TimingPoint = TimingPoint {
        time: 0.0,
        beat_length: Self::DEFAULT_BEAT_LENGTH,
        time_signature: 4,
        omit_first_bar_line: false,
    };

    /// Creates a point, clamping the beat length like lazer's bindable does.
    pub fn new(
        time: f64,
        beat_length: f64,
        time_signature: i32,
        omit_first_bar_line: bool,
    ) -> Self {
        TimingPoint {
            time,
            beat_length: beat_length.clamp(Self::MIN_BEAT_LENGTH, Self::MAX_BEAT_LENGTH),
            time_signature,
            omit_first_bar_line,
        }
    }
}

impl ControlPoint for TimingPoint {
    fn time(&self) -> f64 {
        self.time
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/DifficultyControlPoint.cs
/// A difficulty point: slider velocity changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DifficultyPoint {
    /// Time in ms.
    pub time: f64,
    /// Slider velocity multiplier, within [`Self::MIN_SLIDER_VELOCITY`]..=[`Self::MAX_SLIDER_VELOCITY`].
    pub slider_velocity: f64,
    /// False when the file's beat length was NaN: some maps use that to disable slider ticks.
    pub generate_ticks: bool,
}

impl DifficultyPoint {
    /// Lower bound of `SliderVelocityBindable`.
    pub const MIN_SLIDER_VELOCITY: f64 = 0.1;
    /// Upper bound of `SliderVelocityBindable`.
    pub const MAX_SLIDER_VELOCITY: f64 = 10.0;

    /// Lazer's `DifficultyControlPoint.DEFAULT`.
    pub const DEFAULT: DifficultyPoint = DifficultyPoint {
        time: 0.0,
        slider_velocity: 1.0,
        generate_ticks: true,
    };

    /// Creates a point, clamping the slider velocity like lazer's bindable does.
    pub fn new(time: f64, slider_velocity: f64, generate_ticks: bool) -> Self {
        DifficultyPoint {
            time,
            slider_velocity: slider_velocity
                .clamp(Self::MIN_SLIDER_VELOCITY, Self::MAX_SLIDER_VELOCITY),
            generate_ticks,
        }
    }

    /// Lazer's `IsRedundant`.
    pub fn is_redundant(&self, existing: &DifficultyPoint) -> bool {
        self.generate_ticks == existing.generate_ticks
            && self.slider_velocity == existing.slider_velocity
    }
}

impl ControlPoint for DifficultyPoint {
    fn time(&self) -> f64 {
        self.time
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/EffectControlPoint.cs
/// An effect point.
///
/// Lazer's `ScrollSpeed` is left out: the decoder sets it only for osu!taiko and osu!mania maps,
/// so for osu!standard it is always the default 1 and never affects redundancy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectPoint {
    /// Time in ms.
    pub time: f64,
    /// Kiai mode.
    pub kiai: bool,
}

impl EffectPoint {
    /// Lazer's `EffectControlPoint.DEFAULT`.
    pub const DEFAULT: EffectPoint = EffectPoint {
        time: 0.0,
        kiai: false,
    };

    /// Lazer's `IsRedundant`.
    pub fn is_redundant(&self, existing: &EffectPoint) -> bool {
        self.kiai == existing.kiai
    }
}

impl ControlPoint for EffectPoint {
    fn time(&self) -> f64 {
        self.time
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/SampleControlPoint.cs
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs (LegacySampleControlPoint)
/// A sample point: default bank, volume and custom sample bank of hit sounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplePoint {
    /// Time in ms.
    pub time: f64,
    /// Bank used by samples that do not specify one.
    pub bank: Bank,
    /// Volume 0..=100 used by samples with volume 0.
    pub volume: i32,
    /// Custom sample bank used by samples without one.
    pub custom_sample_bank: i32,
}

impl SamplePoint {
    /// Lazer's `SampleControlPoint.DEFAULT`, used when a map has no sample points.
    ///
    /// It is a plain `SampleControlPoint` in lazer, not a legacy one, but applying it gives the
    /// same result as applying a legacy point with these values.
    pub const DEFAULT: SamplePoint = SamplePoint {
        time: 0.0,
        bank: Bank::Normal,
        volume: 100,
        custom_sample_bank: 0,
    };

    /// Creates a point, clamping the volume to 0..=100 like lazer's bindable does.
    pub fn new(time: f64, bank: Bank, volume: i32, custom_sample_bank: i32) -> Self {
        SamplePoint {
            time,
            bank,
            volume: volume.clamp(0, 100),
            custom_sample_bank,
        }
    }

    /// Lazer's `LegacySampleControlPoint.IsRedundant`.
    pub fn is_redundant(&self, existing: &SamplePoint) -> bool {
        self.bank == existing.bank
            && self.volume == existing.volume
            && self.custom_sample_bank == existing.custom_sample_bank
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyDecoder.cs (LegacySampleControlPoint.ApplyTo)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs (LegacyHitSampleInfo, FileHitSampleInfo)
    /// Resolves a parsed hit sample: fills in the bank, volume and custom sample bank that the
    /// object left unspecified.
    pub fn apply_to(&self, sample: &slam_formats::osu::HitSample) -> HitSample {
        let volume = if sample.volume > 0 {
            sample.volume
        } else {
            self.volume
        };

        if let Some(filename) = &sample.filename {
            // FileHitSampleInfo.With keeps only the file name and the new volume.
            return HitSample::file(filename.clone(), volume);
        }

        let own_bank = sample.custom_sample_bank();
        let custom_sample_bank = if own_bank > 0 {
            own_bank
        } else {
            self.custom_sample_bank
        };
        let bank = if sample.bank_specified {
            Bank::from(sample.bank)
        } else {
            self.bank
        };

        HitSample {
            name: sample.name,
            bank,
            suffix: (custom_sample_bank >= 2).then_some(custom_sample_bank),
            volume,
            editor_auto_bank: sample.editor_auto_bank,
            use_beatmap_samples: custom_sample_bank >= 1,
            is_layered: sample.is_layered,
            filename: None,
        }
    }
}

impl ControlPoint for SamplePoint {
    fn time(&self) -> f64 {
        self.time
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/ControlPointInfo.cs
/// How [`binary_search`] picks among points with the same time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqualitySelection {
    /// Whichever equal point the search reaches first.
    FirstFound,
    /// The first of the equal points.
    Leftmost,
    /// The last of the equal points.
    Rightmost,
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/ControlPointInfo.cs
/// Lazer's `ControlPointInfo.BinarySearch(list, time, equalitySelection)`.
///
/// Returns the index of a point at `time`, or the bitwise complement (`!i`) of the index where
/// such a point would be inserted. Returns -1 for an empty list or a time before the first point.
pub fn binary_search<T: ControlPoint>(
    list: &[T],
    time: f64,
    equality_selection: EqualitySelection,
) -> isize {
    // Slice lengths never exceed isize::MAX.
    let n = list.len() as isize;

    if n == 0 {
        return -1;
    }

    if time < list[0].time() {
        return -1;
    }

    if time > list[list.len() - 1].time() {
        return !n;
    }

    let mut l: isize = 0;
    let mut r: isize = n - 1;
    let mut equality_found = false;

    while l <= r {
        let pivot = l + ((r - l) >> 1);
        let pivot_time = list[pivot as usize].time();

        if pivot_time < time {
            l = pivot + 1;
        } else if pivot_time > time {
            r = pivot - 1;
        } else {
            equality_found = true;

            match equality_selection {
                EqualitySelection::Leftmost => r = pivot - 1,
                EqualitySelection::Rightmost => l = pivot + 1,
                EqualitySelection::FirstFound => return pivot,
            }
        }
    }

    if !equality_found {
        return !l;
    }

    match equality_selection {
        EqualitySelection::Leftmost => l,
        _ => l - 1,
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/ControlPointInfo.cs
/// Lazer's `ControlPointInfo.BinarySearch(list, time)`: the last point at or before `time`.
pub fn active_point<T: ControlPoint>(list: &[T], time: f64) -> Option<&T> {
    let mut index = binary_search(list, time, EqualitySelection::Rightmost);

    if index < 0 {
        index = !index - 1;
    }

    if index >= 0 {
        Some(&list[index as usize])
    } else {
        None
    }
}

/// Puts `point` into the sorted `list`, replacing a point at the same time.
///
/// Lazer keeps at most one point of each type per time (`ControlPointGroup.Add` removes the
/// existing one) and inserts into a sorted list.
fn insert_point<T: ControlPoint>(list: &mut Vec<T>, point: T) {
    let time = point.time();
    let index = list.partition_point(|p| p.time() < time);

    if index < list.len() && list[index].time() == time {
        list[index] = point;
    } else {
        list.insert(index, point);
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/ControlPoints/ControlPointInfo.cs
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Legacy/LegacyControlPointInfo.cs
/// All control points of a beatmap, one sorted list per type.
///
/// Times are finite (the `.osu` parser rejects NaN and infinite times).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ControlPoints {
    timing: Vec<TimingPoint>,
    difficulty: Vec<DifficultyPoint>,
    effect: Vec<EffectPoint>,
    sample: Vec<SamplePoint>,
}

impl ControlPoints {
    /// No control points.
    pub fn new() -> Self {
        Self::default()
    }

    /// Timing points in time order.
    pub fn timing_points(&self) -> &[TimingPoint] {
        &self.timing
    }

    /// Difficulty points in time order.
    pub fn difficulty_points(&self) -> &[DifficultyPoint] {
        &self.difficulty
    }

    /// Effect points in time order.
    pub fn effect_points(&self) -> &[EffectPoint] {
        &self.effect
    }

    /// Sample points in time order.
    pub fn sample_points(&self) -> &[SamplePoint] {
        &self.sample
    }

    /// Lazer's `TimingPointAt`: the active timing point, or the first one before it starts, or
    /// [`TimingPoint::DEFAULT`] if there are none.
    pub fn timing_point_at(&self, time: f64) -> TimingPoint {
        let fallback = self.timing.first().copied().unwrap_or(TimingPoint::DEFAULT);
        active_point(&self.timing, time)
            .copied()
            .unwrap_or(fallback)
    }

    /// Lazer's `DifficultyPointAt`: the active difficulty point or [`DifficultyPoint::DEFAULT`].
    pub fn difficulty_point_at(&self, time: f64) -> DifficultyPoint {
        active_point(&self.difficulty, time)
            .copied()
            .unwrap_or(DifficultyPoint::DEFAULT)
    }

    /// Lazer's `EffectPointAt`: the active effect point or [`EffectPoint::DEFAULT`].
    pub fn effect_point_at(&self, time: f64) -> EffectPoint {
        active_point(&self.effect, time)
            .copied()
            .unwrap_or(EffectPoint::DEFAULT)
    }

    /// Lazer's `SamplePointAt`: the active sample point, or the first one before it starts, or
    /// [`SamplePoint::DEFAULT`] if there are none.
    pub fn sample_point_at(&self, time: f64) -> SamplePoint {
        let fallback = self.sample.first().copied().unwrap_or(SamplePoint::DEFAULT);
        active_point(&self.sample, time)
            .copied()
            .unwrap_or(fallback)
    }

    /// Adds a timing point. Timing points are never redundant; one at the same time is replaced.
    pub fn add_timing(&mut self, point: TimingPoint) {
        insert_point(&mut self.timing, point);
    }

    /// Adds a difficulty point unless it equals the point active at its time (including the
    /// default). Returns whether it was added.
    pub fn add_difficulty(&mut self, point: DifficultyPoint) -> bool {
        if point.is_redundant(&self.difficulty_point_at(point.time)) {
            return false;
        }
        insert_point(&mut self.difficulty, point);
        true
    }

    /// Adds an effect point unless it equals the point active at its time (including the
    /// default). Returns whether it was added.
    pub fn add_effect(&mut self, point: EffectPoint) -> bool {
        if point.is_redundant(&self.effect_point_at(point.time)) {
            return false;
        }
        insert_point(&mut self.effect, point);
        true
    }

    /// Adds a sample point unless it equals the point active at its time. Unlike difficulty
    /// and effect points there is no default to compare with: the first sample point is always added.
    /// Returns whether it was added.
    pub fn add_sample(&mut self, point: SamplePoint) -> bool {
        if let Some(existing) = active_point(&self.sample, point.time)
            && point.is_redundant(existing)
        {
            return false;
        }
        insert_point(&mut self.sample, point);
        true
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs (handleTimingPoint, addControlPoint, flushPendingPoints)
    /// Builds the control points from `[TimingPoints]` lines in file order, the way lazer's
    /// legacy decoder does.
    ///
    /// Every line yields a difficulty, an effect and a sample point, plus a timing point if it
    /// is uninherited. Points of consecutive lines with the same time are merged: per type, an
    /// inherited line overrides an uninherited one, a later inherited line overrides an earlier
    /// one, and among uninherited lines the first one wins.
    pub fn from_legacy(lines: &[LegacyTimingPoint]) -> Self {
        let mut points = ControlPoints::new();
        let mut pending = Pending::default();

        for line in lines {
            let time = line.time;
            let beat_length = line.beat_length;

            // If beatLength is NaN, speedMultiplier should still be 1 because all comparisons
            // against NaN are false.
            let speed_multiplier = if beat_length < 0.0 {
                100.0 / -beat_length
            } else {
                1.0
            };

            let timing_change = line.uninherited;

            // The parser drops uninherited lines with a NaN beat length, as lazer does by
            // throwing before anything is added.
            if timing_change {
                let timing = TimingPoint::new(
                    time,
                    beat_length,
                    line.time_signature,
                    line.omit_first_bar_line,
                );
                pending.add(&mut points, time, PendingPoint::Timing(timing), true);
            }

            let difficulty = DifficultyPoint::new(time, speed_multiplier, !beat_length.is_nan());
            pending.add(
                &mut points,
                time,
                PendingPoint::Difficulty(difficulty),
                timing_change,
            );

            let effect = EffectPoint {
                time,
                kiai: line.kiai,
            };
            pending.add(
                &mut points,
                time,
                PendingPoint::Effect(effect),
                timing_change,
            );

            let sample = SamplePoint::new(
                time,
                Bank::from_legacy(line.sample_set),
                line.volume,
                line.custom_sample_index,
            );
            pending.add(
                &mut points,
                time,
                PendingPoint::Sample(sample),
                timing_change,
            );
        }

        pending.flush(&mut points);
        points
    }
}

#[derive(Debug, Clone, Copy)]
enum PendingPoint {
    Timing(TimingPoint),
    Difficulty(DifficultyPoint),
    Effect(EffectPoint),
    Sample(SamplePoint),
}

/// The decoder's pending points of the current time (`pendingControlPoints`).
#[derive(Debug, Default)]
struct Pending {
    points: Vec<PendingPoint>,
    time: f64,
}

impl Pending {
    fn add(
        &mut self,
        target: &mut ControlPoints,
        time: f64,
        point: PendingPoint,
        timing_change: bool,
    ) {
        if time != self.time {
            self.flush(target);
        }

        if timing_change {
            self.points.insert(0, point);
        } else {
            self.points.push(point);
        }

        self.time = time;
    }

    fn flush(&mut self, target: &mut ControlPoints) {
        // Changes from non-timing points are at the end of the list and override changes from
        // timing points at the start, so the list is walked backwards and the first point of
        // each type wins.
        let mut seen = [false; 4];

        for point in self.points.iter().rev() {
            let slot = match point {
                PendingPoint::Timing(_) => 0,
                PendingPoint::Difficulty(_) => 1,
                PendingPoint::Effect(_) => 2,
                PendingPoint::Sample(_) => 3,
            };
            if std::mem::replace(&mut seen[slot], true) {
                continue;
            }

            // Points carry their own time, which equals `self.time`.
            match *point {
                PendingPoint::Timing(p) => target.add_timing(p),
                PendingPoint::Difficulty(p) => {
                    target.add_difficulty(p);
                }
                PendingPoint::Effect(p) => {
                    target.add_effect(p);
                }
                PendingPoint::Sample(p) => {
                    target.add_sample(p);
                }
            }
        }

        self.points.clear();
    }
}
