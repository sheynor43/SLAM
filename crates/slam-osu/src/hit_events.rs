//! Hit errors of a play: which judgements have a timing offset, and the average, median and
//! unstable rate of those offsets, as lazer's `HitEventExtensions` (ADR-0074).
//!
//! [`HitEventTargets`] maps judgements to their offsets; [`HitEvents`] collects the offsets of
//! a play without allocating after it is built and keeps the unstable rate as they come.

use crate::Beatmap;
use crate::hit_windows::MISS_WINDOW;
use crate::judgement::{HitResult, Judgement};
use crate::objects::{NestedKind, OsuHitObjectKind};

/// The judgement of an object that has hit windows: lazer gives them to hit circles and
/// slider heads only (the others' are `HitWindows.Empty`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Windowed {
    None,
    /// A hit circle at this time.
    Circle(f64),
    /// A slider whose head is this nested object at this time.
    Head(u32, f64),
}

/// Which judgements of a beatmap have hit windows, and when their objects end. Built at load.
#[derive(Debug, Clone)]
pub struct HitEventTargets {
    objects: Vec<Windowed>,
    /// Objects with hit windows: at most one hit event each.
    windowed: usize,
}

impl HitEventTargets {
    /// The targets of `beatmap`'s objects.
    pub fn new(beatmap: &Beatmap) -> Self {
        let objects: Vec<Windowed> = beatmap
            .hit_objects
            .iter()
            .map(|o| match &o.kind {
                OsuHitObjectKind::Circle => Windowed::Circle(o.start_time),
                OsuHitObjectKind::Slider(slider) => slider
                    .nested
                    .iter()
                    .position(|n| n.kind == NestedKind::Head)
                    .map_or(Windowed::None, |i| {
                        Windowed::Head(i as u32, slider.nested[i].start_time)
                    }),
                OsuHitObjectKind::Spinner(_) => Windowed::None,
            })
            .collect();
        let windowed = objects.iter().filter(|w| **w != Windowed::None).count();
        HitEventTargets { objects, windowed }
    }

    /// The number of objects with hit windows.
    pub fn windowed(&self) -> usize {
        self.windowed
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Judgements/JudgementResult.cs (TimeOffset)
    /// How late `judgement` was, in milliseconds (negative when early), if its object has hit
    /// windows. Its result may be a miss.
    pub fn offset(&self, judgement: &Judgement) -> Option<f64> {
        let time = match (
            self.objects.get(judgement.object.index as usize)?,
            judgement.object.nested,
        ) {
            (Windowed::Circle(time), None) => *time,
            (Windowed::Head(head, time), Some(n)) if n == *head => *time,
            _ => return None,
        };
        // Circles end where they start; their `MaximumJudgementOffset` is the miss window.
        Some((judgement.time - time).min(MISS_WINDOW))
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitEventExtensions.cs (AffectsUnstableRate)
    /// The offset of `judgement` if it counts for the hit error statistics: a hit of an object
    /// with hit windows.
    pub fn hit_offset(&self, judgement: &Judgement) -> Option<f64> {
        if !affects_unstable_rate(judgement.result) {
            return None;
        }
        self.offset(judgement)
    }
}

/// Whether a result of an object with hit windows counts for the statistics (`IsHit`).
pub fn affects_unstable_rate(result: HitResult) -> bool {
    result.is_hit()
}

/// The unstable rate, counted one offset at a time with Welford's algorithm.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitEventExtensions.cs (UnstableRateCalculationResult, CalculateUnstableRate)
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UnstableRate {
    event_count: i32,
    sum_of_squares: f64,
    mean: f64,
}

impl UnstableRate {
    /// Counts a hit `offset` taken at gameplay `rate`.
    pub fn push(&mut self, offset: f64, rate: f64) {
        self.event_count += 1;
        // Division by gameplay rate is to account for the offset scaling with gameplay rate.
        let current_value = offset / rate;
        let next_mean = self.mean + (current_value - self.mean) / f64::from(self.event_count);
        self.sum_of_squares += (current_value - self.mean) * (current_value - next_mean);
        self.mean = next_mean;
    }

    /// The offsets counted.
    pub fn count(&self) -> i32 {
        self.event_count
    }

    /// The unstable rate: ten times the standard deviation of the rate-adjusted offsets.
    /// `None` without offsets.
    pub fn value(&self) -> Option<f64> {
        (self.event_count > 0)
            // `sqrt` is correctly rounded, as .NET's `Math.Sqrt`.
            .then(|| 10.0 * (self.sum_of_squares / f64::from(self.event_count)).sqrt())
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitEventExtensions.cs (CalculateAverageHitError)
/// The mean of `offsets`, not adjusted for the gameplay rate. `None` if empty.
pub fn average_hit_error(offsets: &[f64]) -> Option<f64> {
    if offsets.is_empty() {
        return None;
    }
    // .NET's `Average` of doubles sums in order, then divides.
    let mut sum = 0.0;
    for offset in offsets {
        sum += offset;
    }
    Some(sum / offsets.len() as f64)
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitEventExtensions.cs (CalculateMedianHitError)
/// The median of `offsets`, not adjusted for the gameplay rate: the mean of the two central
/// values if their number is even. `None` if empty. Sorts a copy.
pub fn median_hit_error(offsets: &[f64]) -> Option<f64> {
    if offsets.is_empty() {
        return None;
    }
    let mut sorted = offsets.to_vec();
    sorted.sort_by(f64::total_cmp);
    let center = sorted.len() / 2;
    // Use average of the 2 central values if length is even.
    Some(if sorted.len().is_multiple_of(2) {
        (sorted[center - 1] + sorted[center]) / 2.0
    } else {
        sorted[center]
    })
}

/// The hit error statistics of a play: lazer's `ScoreProcessor.HitEvents` as far as the
/// statistics need them.
///
/// Allocates in [`HitEvents::new`] only: each object with hit windows is judged once.
#[derive(Debug, Clone)]
pub struct HitEvents {
    targets: HitEventTargets,
    /// The gameplay rate of every judgement (lazer's `GetTrueGameplayRate`).
    rate: f64,
    offsets: Vec<f64>,
    unstable_rate: UnstableRate,
}

impl HitEvents {
    /// Statistics for a play of `beatmap` at gameplay `rate`.
    pub fn new(beatmap: &Beatmap, rate: f64) -> Self {
        let targets = HitEventTargets::new(beatmap);
        let offsets = Vec::with_capacity(targets.windowed());
        HitEvents {
            targets,
            rate,
            offsets,
            unstable_rate: UnstableRate::default(),
        }
    }

    /// Takes a judgement the play counts (not one after the player failed).
    pub fn apply(&mut self, judgement: &Judgement) {
        if let Some(offset) = self.targets.hit_offset(judgement) {
            debug_assert!(
                self.offsets.len() < self.offsets.capacity(),
                "one hit event per object with hit windows"
            );
            self.offsets.push(offset);
            self.unstable_rate.push(offset, self.rate);
        }
    }

    /// Forgets the play's judgements, keeping the memory.
    pub fn restart(&mut self) {
        self.offsets.clear();
        self.unstable_rate = UnstableRate::default();
    }

    /// The offsets counted, in judgement order.
    pub fn offsets(&self) -> &[f64] {
        &self.offsets
    }

    /// The unstable rate, `None` without hits.
    pub fn unstable_rate(&self) -> Option<f64> {
        self.unstable_rate.value()
    }

    /// The average hit error, `None` without hits.
    pub fn average(&self) -> Option<f64> {
        average_hit_error(&self.offsets)
    }

    /// The median hit error, `None` without hits. Allocates.
    pub fn median(&self) -> Option<f64> {
        median_hit_error(&self.offsets)
    }

    /// The statistics so far. Allocates for the median.
    pub fn summary(&self) -> HitErrorSummary {
        HitErrorSummary {
            unstable_rate: self.unstable_rate(),
            average: self.average(),
            median: self.median(),
            hits: self.offsets.len(),
        }
    }
}

/// The hit error statistics of a finished play.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HitErrorSummary {
    /// Unstable rate, adjusted for the gameplay rate.
    pub unstable_rate: Option<f64>,
    /// Average hit error in milliseconds, negative when early.
    pub average: Option<f64>,
    /// Median hit error in milliseconds, negative when early.
    pub median: Option<f64>,
    /// The hits counted.
    pub hits: usize,
}

impl std::fmt::Display for HitErrorSummary {
    /// `UR 123.45, average -1.23 ms, median -1.00 ms (456 hits)`, `n/a` for missing values.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UR ")?;
        match self.unstable_rate {
            Some(ur) => write!(f, "{ur:.2}")?,
            None => write!(f, "n/a")?,
        }
        for (name, value) in [("average", self.average), ("median", self.median)] {
            match value {
                Some(ms) => write!(f, ", {name} {ms:.2} ms")?,
                None => write!(f, ", {name} n/a")?,
            }
        }
        write!(f, " ({} hits)", self.hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f64 = 1e-7;

    struct Event {
        offset: f64,
        rate: f64,
        result: HitResult,
        windowed: bool,
    }

    fn great(offset: f64, rate: f64) -> Event {
        Event {
            offset,
            rate,
            result: HitResult::Great,
            windowed: true,
        }
    }

    fn unstable_rate(events: &[Event]) -> Option<f64> {
        let mut ur = UnstableRate::default();
        for e in events {
            if e.windowed && affects_unstable_rate(e.result) {
                ur.push(e.offset, e.rate);
            }
        }
        ur.value()
    }

    /// `Enumerable.Range(-5, 11).Select(t => t - 5)` at rate 1.
    fn distributed() -> Vec<Event> {
        (-5..=5).map(|t| great(f64::from(t - 5), 1.0)).collect()
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/NonVisual/Ranking/UnstableRateTest.cs (TestDistributedHits)
    #[test]
    fn distributed_hits() {
        let ur = unstable_rate(&distributed()).unwrap();
        assert!((ur - 10.0 * 10f64.sqrt()).abs() < EPSILON, "{ur}");
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/NonVisual/Ranking/UnstableRateTest.cs (TestDistributedHitsIncremental)
    #[test]
    fn distributed_hits_with_misses_and_empty_windows() {
        let mut events = distributed();
        events.insert(
            4,
            Event {
                offset: 200.0,
                rate: 1.0,
                result: HitResult::Meh,
                windowed: false,
            },
        );
        events.insert(
            8,
            Event {
                offset: -100.0,
                rate: 1.0,
                result: HitResult::Miss,
                windowed: true,
            },
        );
        let ur = unstable_rate(&events).unwrap();
        assert!((ur - 10.0 * 10f64.sqrt()).abs() < EPSILON, "{ur}");
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/NonVisual/Ranking/UnstableRateTest.cs (TestMissesAndEmptyWindows)
    #[test]
    fn misses_and_empty_windows() {
        let events = [
            Event {
                offset: -100.0,
                rate: 1.0,
                result: HitResult::Miss,
                windowed: true,
            },
            great(0.0, 1.0),
            Event {
                offset: 200.0,
                rate: 1.0,
                result: HitResult::Meh,
                windowed: false,
            },
        ];
        assert_eq!(unstable_rate(&events), Some(0.0));
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/NonVisual/Ranking/UnstableRateTest.cs (TestStaticRateChange)
    #[test]
    fn static_rate_change() {
        let events = [
            great(-150.0, 1.5),
            great(-150.0, 1.5),
            great(150.0, 1.5),
            great(150.0, 1.5),
        ];
        assert_eq!(unstable_rate(&events), Some(10.0 * 100.0));
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/NonVisual/Ranking/UnstableRateTest.cs (TestDynamicRateChange)
    #[test]
    fn dynamic_rate_change() {
        let events = [
            great(-50.0, 0.5),
            great(75.0, 0.75),
            great(-100.0, 1.0),
            great(125.0, 1.25),
        ];
        assert_eq!(unstable_rate(&events), Some(10.0 * 100.0));
    }

    fn judgement(index: u32, nested: Option<u32>, time: f64, result: HitResult) -> Judgement {
        Judgement {
            object: crate::judgement::ObjectRef { index, nested },
            time,
            result,
            max_result: HitResult::Great,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SliderHeadCircle.cs (ClassicSliderBehaviour)
    #[test]
    fn circles_and_slider_heads_have_hit_windows() {
        let text = "osu file format v14\n\n[Difficulty]\nCircleSize:4\nOverallDifficulty:8\n\
                    ApproachRate:9\nSliderMultiplier:1.4\nSliderTickRate:1\n\n[TimingPoints]\n\
                    0,300,4,2,0,100,1,0\n\n[HitObjects]\n100,100,500,1,0\n\
                    120,120,800,2,0,L|300:120,1,180\n256,192,2000,12,0,3000\n";
        let map = Beatmap::from_file(slam_formats::osu::decode_str(text).unwrap().beatmap).unwrap();
        let OsuHitObjectKind::Slider(slider) = &map.hit_objects[1].kind else {
            panic!("a slider");
        };
        let head = slider
            .nested
            .iter()
            .position(|n| n.kind == NestedKind::Head)
            .unwrap() as u32;
        let tail = slider
            .nested
            .iter()
            .position(|n| matches!(n.kind, NestedKind::Tail { .. }))
            .unwrap() as u32;
        let targets = HitEventTargets::new(&map);
        assert_eq!(targets.windowed(), 2);

        let circle = judgement(0, None, 490.0, HitResult::Great);
        assert_eq!(targets.hit_offset(&circle), Some(-10.0));
        // The offset is capped at the miss window; a miss has an offset but no hit event.
        let late_miss = judgement(0, None, 1000.0, HitResult::Miss);
        assert_eq!(targets.offset(&late_miss), Some(MISS_WINDOW));
        assert_eq!(targets.hit_offset(&late_miss), None);
        // A Classic head is judged as a large tick and still has hit windows.
        let classic_head = judgement(1, Some(head), 815.0, HitResult::LargeTickHit);
        assert_eq!(targets.hit_offset(&classic_head), Some(15.0));
        let classic_head_miss = judgement(1, Some(head), 1200.0, HitResult::LargeTickMiss);
        assert_eq!(targets.hit_offset(&classic_head_miss), None);
        // Tails, sliders and spinners have none.
        let tail = judgement(1, Some(tail), 1000.0, HitResult::SliderTailHit);
        assert_eq!(targets.offset(&tail), None);
        assert_eq!(
            targets.offset(&judgement(1, None, 1000.0, HitResult::Great)),
            None
        );
        assert_eq!(
            targets.offset(&judgement(2, None, 3000.0, HitResult::Great)),
            None
        );

        let mut events = HitEvents::new(&map, 1.0);
        for j in [circle, late_miss, classic_head, classic_head_miss, tail] {
            events.apply(&j);
        }
        assert_eq!(events.offsets(), &[-10.0, 15.0]);
        events.restart();
        assert_eq!(events.summary(), HitErrorSummary::default());
    }

    #[test]
    fn no_hits_have_no_values() {
        assert_eq!(UnstableRate::default().value(), None);
        assert_eq!(average_hit_error(&[]), None);
        assert_eq!(median_hit_error(&[]), None);
        let summary = HitErrorSummary::default();
        assert_eq!(
            summary.to_string(),
            "UR n/a, average n/a, median n/a (0 hits)"
        );
    }

    #[test]
    fn average_is_not_rate_adjusted() {
        assert_eq!(
            average_hit_error(&[-150.0, -150.0, 150.0, 90.0]),
            Some(-15.0)
        );
    }

    #[test]
    fn median_of_odd_and_even_counts() {
        assert_eq!(median_hit_error(&[5.0, -3.0, 1.0]), Some(1.0));
        assert_eq!(median_hit_error(&[5.0, -3.0, 1.0, 2.0]), Some(1.5));
        assert_eq!(median_hit_error(&[7.0]), Some(7.0));
    }

    #[test]
    fn summary_display() {
        let summary = HitErrorSummary {
            unstable_rate: Some(123.456),
            average: Some(-1.234),
            median: Some(-1.0),
            hits: 456,
        };
        assert_eq!(
            summary.to_string(),
            "UR 123.46, average -1.23 ms, median -1.00 ms (456 hits)"
        );
    }
}
