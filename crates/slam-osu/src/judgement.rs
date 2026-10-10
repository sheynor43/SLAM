//! Judgement skeleton: hit results, the judgement log and the statistics derived from it.
//!
//! The object rules (hit windows, sliders, spinners) produce [`Judgement`]s; this module only
//! records them and keeps the counts and combo the way lazer's `ScoreProcessor` does.

use slam_formats::osr;

/// The result of judging an object, in lazer's order.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitResult.cs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HitResult {
    /// Not judged yet.
    None,
    /// A miss.
    Miss,
    /// The lowest hit (50).
    Meh,
    /// 100.
    Ok,
    /// Not used by osu!standard.
    Good,
    /// 300.
    Great,
    /// Not used by osu!standard.
    Perfect,
    /// A missed small tick.
    SmallTickMiss,
    /// A hit small tick.
    SmallTickHit,
    /// A missed large tick.
    LargeTickMiss,
    /// A hit large tick.
    LargeTickHit,
    /// A small bonus.
    SmallBonus,
    /// A large bonus.
    LargeBonus,
    /// A miss that affects nothing.
    IgnoreMiss,
    /// A hit that affects nothing.
    IgnoreHit,
    /// Breaks the combo without other effects.
    ComboBreak,
    /// A hit slider tail.
    SliderTailHit,
    /// Increases the combo without other effects (classic slider ticks).
    LegacyComboIncrease,
}

impl HitResult {
    /// Every result, in declaration order; [`HitResult::index`] is the position in this array.
    pub const ALL: [HitResult; 18] = [
        HitResult::None,
        HitResult::Miss,
        HitResult::Meh,
        HitResult::Ok,
        HitResult::Good,
        HitResult::Great,
        HitResult::Perfect,
        HitResult::SmallTickMiss,
        HitResult::SmallTickHit,
        HitResult::LargeTickMiss,
        HitResult::LargeTickHit,
        HitResult::SmallBonus,
        HitResult::LargeBonus,
        HitResult::IgnoreMiss,
        HitResult::IgnoreHit,
        HitResult::ComboBreak,
        HitResult::SliderTailHit,
        HitResult::LegacyComboIncrease,
    ];

    /// The number of results.
    pub const COUNT: usize = HitResult::ALL.len();

    /// The position in [`HitResult::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// Whether the result increases or breaks the combo.
    pub fn affects_combo(self) -> bool {
        matches!(
            self,
            HitResult::Miss
                | HitResult::Meh
                | HitResult::Ok
                | HitResult::Good
                | HitResult::Great
                | HitResult::Perfect
                | HitResult::LargeTickHit
                | HitResult::LargeTickMiss
                | HitResult::LegacyComboIncrease
                | HitResult::ComboBreak
                | HitResult::SliderTailHit
        )
    }

    /// Whether the result increases the combo.
    pub fn increases_combo(self) -> bool {
        self.affects_combo() && self.is_hit()
    }

    /// Whether the result resets the combo to zero.
    pub fn breaks_combo(self) -> bool {
        self.affects_combo() && !self.is_hit()
    }

    /// Whether the result is a miss. Both this and [`HitResult::is_hit`] are false for
    /// [`HitResult::None`].
    pub fn is_miss(self) -> bool {
        matches!(
            self,
            HitResult::IgnoreMiss
                | HitResult::Miss
                | HitResult::SmallTickMiss
                | HitResult::LargeTickMiss
                | HitResult::ComboBreak
        )
    }

    /// Whether the result is a hit.
    pub fn is_hit(self) -> bool {
        !matches!(
            self,
            HitResult::None
                | HitResult::IgnoreMiss
                | HitResult::Miss
                | HitResult::SmallTickMiss
                | HitResult::LargeTickMiss
                | HitResult::ComboBreak
        )
    }

    /// The key of this result in a score's statistics dictionary.
    pub fn to_score_key(self) -> osr::HitResult {
        match self {
            HitResult::None => osr::HitResult::None,
            HitResult::Miss => osr::HitResult::Miss,
            HitResult::Meh => osr::HitResult::Meh,
            HitResult::Ok => osr::HitResult::Ok,
            HitResult::Good => osr::HitResult::Good,
            HitResult::Great => osr::HitResult::Great,
            HitResult::Perfect => osr::HitResult::Perfect,
            HitResult::SmallTickMiss => osr::HitResult::SmallTickMiss,
            HitResult::SmallTickHit => osr::HitResult::SmallTickHit,
            HitResult::LargeTickMiss => osr::HitResult::LargeTickMiss,
            HitResult::LargeTickHit => osr::HitResult::LargeTickHit,
            HitResult::SmallBonus => osr::HitResult::SmallBonus,
            HitResult::LargeBonus => osr::HitResult::LargeBonus,
            HitResult::IgnoreMiss => osr::HitResult::IgnoreMiss,
            HitResult::IgnoreHit => osr::HitResult::IgnoreHit,
            HitResult::ComboBreak => osr::HitResult::ComboBreak,
            HitResult::SliderTailHit => osr::HitResult::SliderTailHit,
            HitResult::LegacyComboIncrease => osr::HitResult::LegacyComboIncrease,
        }
    }
}

/// The judged object: a top-level hit object, or one of its nested objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectRef {
    /// Index into `Beatmap::hit_objects`.
    pub index: u32,
    /// Index into the object's nested objects, `None` for the object itself.
    pub nested: Option<u32>,
}

/// One judgement produced by the rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Judgement {
    /// The judged object.
    pub object: ObjectRef,
    /// Gameplay time of the judgement, in milliseconds.
    pub time: f64,
    /// The result.
    pub result: HitResult,
    /// The best result the object could get; scoring and accuracy count against it.
    pub max_result: HitResult,
}

/// Counts per result and combo, as lazer's `ScoreProcessor` keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Statistics {
    /// Number of judgements per result, indexed by [`HitResult::index`].
    pub counts: [u32; HitResult::COUNT],
    /// Current combo.
    pub combo: u32,
    /// Highest combo reached.
    pub max_combo: u32,
}

impl Statistics {
    /// The number of judgements with this result.
    pub fn count(&self, result: HitResult) -> u32 {
        self.counts[result.index()]
    }

    /// Records a result.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs (ApplyResultInternal)
    pub fn apply(&mut self, result: HitResult) {
        debug_assert!(result != HitResult::None, "judgement without a result");
        self.counts[result.index()] += 1;

        if result.increases_combo() {
            self.combo += 1;
        } else if result.breaks_combo() {
            self.combo = 0;
        }

        self.max_combo = self.max_combo.max(self.combo);
    }
}

/// The judgements of a play in order, with the statistics derived from them.
///
/// The log is allocated once with the capacity the rules ask for, so recording a judgement
/// never allocates while the rules stay within it.
#[derive(Debug, Clone, PartialEq)]
pub struct JudgementLog {
    events: Vec<Judgement>,
    statistics: Statistics,
}

impl JudgementLog {
    /// An empty log with room for `capacity` judgements.
    pub fn with_capacity(capacity: usize) -> JudgementLog {
        JudgementLog {
            events: Vec::with_capacity(capacity),
            statistics: Statistics::default(),
        }
    }

    /// Records a judgement and updates the statistics.
    pub fn apply(&mut self, judgement: Judgement) {
        debug_assert!(
            self.events.len() < self.events.capacity(),
            "judgement log over its capacity"
        );
        self.events.push(judgement);
        self.statistics.apply(judgement.result);
    }

    /// All judgements so far, in order.
    pub fn events(&self) -> &[Judgement] {
        &self.events
    }

    /// The judgements recorded after the first `seen` ones: a consumer keeps `seen` and reads
    /// only the new events each step.
    pub fn events_since(&self, seen: usize) -> &[Judgement] {
        &self.events[seen.min(self.events.len())..]
    }

    /// The statistics so far.
    pub fn statistics(&self) -> &Statistics {
        &self.statistics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_matches_all() {
        for (i, r) in HitResult::ALL.iter().enumerate() {
            assert_eq!(r.index(), i);
        }
    }

    #[test]
    fn combo_classification_matches_lazer() {
        use HitResult::*;
        let increases: Vec<_> = HitResult::ALL
            .into_iter()
            .filter(|r| r.increases_combo())
            .collect();
        assert_eq!(
            increases,
            [
                Meh,
                Ok,
                Good,
                Great,
                Perfect,
                LargeTickHit,
                SliderTailHit,
                LegacyComboIncrease
            ]
        );
        let breaks: Vec<_> = HitResult::ALL
            .into_iter()
            .filter(|r| r.breaks_combo())
            .collect();
        assert_eq!(breaks, [Miss, LargeTickMiss, ComboBreak]);
        assert!(!None.is_hit() && !None.is_miss());
    }

    #[test]
    fn score_keys_round_trip_names() {
        for r in HitResult::ALL {
            let key = r.to_score_key();
            assert_eq!(osr::HitResult::from_name(key.as_str()), key);
            assert!(!matches!(key, osr::HitResult::Unknown(_)));
        }
    }

    #[test]
    fn statistics_track_combo() {
        let mut s = Statistics::default();
        for r in [
            HitResult::Great,
            HitResult::SmallTickHit,
            HitResult::Ok,
            HitResult::Miss,
            HitResult::Great,
            HitResult::IgnoreMiss,
        ] {
            s.apply(r);
        }
        assert_eq!(s.combo, 1);
        assert_eq!(s.max_combo, 2);
        assert_eq!(s.count(HitResult::Great), 2);
        assert_eq!(s.count(HitResult::Miss), 1);
        assert_eq!(s.count(HitResult::SmallTickHit), 1);
    }

    #[test]
    fn events_since_returns_new_events() {
        let mut log = JudgementLog::with_capacity(4);
        let j = |i| Judgement {
            object: ObjectRef {
                index: i,
                nested: None,
            },
            time: f64::from(i),
            result: HitResult::Great,
            max_result: HitResult::Great,
        };
        log.apply(j(0));
        log.apply(j(1));
        assert_eq!(log.events_since(1), &[j(1)]);
        assert!(log.events_since(5).is_empty());
        assert_eq!(log.statistics().max_combo, 2);
    }
}
