//! Standardised scoring: score, accuracy, combo and rank, as lazer's `OsuScoreProcessor`.
//!
//! The maxima are settled once from a simulated perfect play ([`ScoreProcessor::new`]);
//! [`ScoreProcessor::apply`] then takes the judgements of the play without allocating.

use crate::autoplay;
use crate::beatmap::Beatmap;
use crate::dotnet;
use crate::judgement::{HitResult, Judgement, Statistics};
use crate::math;
use crate::mods::{GameplayMod, ModSet, MultiplierContext, MultiplierVersion};
use crate::rules::RulesOptions;

use slam_formats::osr;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs
/// The exponent of the combo in the combo portion.
pub const COMBO_EXPONENT: f64 = 0.5;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs
/// The score of a perfect play without bonus and mods.
pub const MAX_SCORE: f64 = 1_000_000.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs
const ACCURACY_CUTOFF_X: f64 = 1.0;
const ACCURACY_CUTOFF_S: f64 = 0.95;
const ACCURACY_CUTOFF_A: f64 = 0.9;
const ACCURACY_CUTOFF_B: f64 = 0.8;
const ACCURACY_CUTOFF_C: f64 = 0.7;

/// The rank of a score, from worst to best.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/ScoreRank.cs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScoreRank {
    /// A failed play.
    F,
    /// Below 70% accuracy.
    D,
    /// From 70%.
    C,
    /// From 80%.
    B,
    /// From 90%, or S and SS with a miss.
    A,
    /// From 95% without a miss.
    S,
    /// Silver S (Hidden).
    SH,
    /// 100% accuracy (SS).
    X,
    /// Silver SS (Hidden).
    XH,
}

impl ScoreRank {
    /// The rank as a score's `rank` field names it.
    pub fn to_score_key(self) -> osr::ScoreRank {
        match self {
            ScoreRank::F => osr::ScoreRank::F,
            ScoreRank::D => osr::ScoreRank::D,
            ScoreRank::C => osr::ScoreRank::C,
            ScoreRank::B => osr::ScoreRank::B,
            ScoreRank::A => osr::ScoreRank::A,
            ScoreRank::S => osr::ScoreRank::S,
            ScoreRank::SH => osr::ScoreRank::SH,
            ScoreRank::X => osr::ScoreRank::X,
            ScoreRank::XH => osr::ScoreRank::XH,
        }
    }
}

impl HitResult {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitResult.cs (IsScorable)
    /// Whether the result counts towards the score.
    pub fn is_scorable(self) -> bool {
        match self {
            HitResult::LegacyComboIncrease | HitResult::ComboBreak | HitResult::SliderTailHit => {
                true
            }
            // `IgnoreHit` and `IgnoreMiss` do not affect the score.
            _ => self >= HitResult::Miss && self < HitResult::IgnoreMiss,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitResult.cs (AffectsAccuracy)
    /// Whether the result counts towards the accuracy.
    pub fn affects_accuracy(self) -> bool {
        match self {
            HitResult::LegacyComboIncrease | HitResult::ComboBreak => false,
            _ => self.is_scorable() && !self.is_bonus(),
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs (GetBaseScoreForResult)
    /// The score the result is worth.
    pub fn base_score(self) -> i32 {
        match self {
            HitResult::SmallTickHit => 10,
            HitResult::LargeTickHit => 30,
            HitResult::SliderTailHit => 150,
            HitResult::Meh => 50,
            HitResult::Ok => 100,
            HitResult::Good => 200,
            // Perfect gives no more score or accuracy than Great.
            HitResult::Great | HitResult::Perfect => 300,
            HitResult::SmallBonus => 10,
            HitResult::LargeBonus => 50,
            _ => 0,
        }
    }
}

/// What a [`ScoreProcessor`] needs to know about the play beyond the beatmap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoringOptions {
    /// Lazer's `Slider.ClassicSliderBehaviour`, which decides the best results of sliders.
    pub classic_slider_behaviour: bool,
    /// The mods' score multiplier.
    pub score_multiplier: f64,
}

impl ScoringOptions {
    /// The options of a play with these mods, scored with the current multipliers
    /// ([`MultiplierVersion::V2`], as lazer's `ScoreProcessor`).
    pub fn for_play(beatmap: &Beatmap, mods: &ModSet) -> ScoringOptions {
        ScoringOptions::with_multipliers(beatmap, mods, MultiplierVersion::V2)
    }

    /// The options of a play with these mods and a given table of multipliers: a score set
    /// before the rebalance of the multipliers was scored with [`MultiplierVersion::V1`].
    pub fn with_multipliers(
        beatmap: &Beatmap,
        mods: &ModSet,
        version: MultiplierVersion,
    ) -> ScoringOptions {
        ScoringOptions {
            classic_slider_behaviour: RulesOptions::for_mods(mods).classic_slider_behaviour,
            score_multiplier: mods.score_multiplier(&MultiplierContext {
                version,
                difficulty_without_mods: &beatmap.difficulty_without_mods,
            }),
        }
    }
}

/// The score of a play so far: lazer's `ScoreProcessor` with `OsuScoreProcessor`'s rank,
/// without hit events and rewinding.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreProcessor {
    score_multiplier: f64,

    maximum_statistics: Statistics,
    maximum_base_score: f64,
    maximum_combo_portion: f64,
    maximum_accuracy_judgement_count: i32,
    maximum_total_score: i64,

    statistics: Statistics,
    current_maximum_base_score: f64,
    current_base_score: f64,
    current_accuracy_judgement_count: i32,
    current_combo_portion: f64,
    current_bonus_portion: f64,

    accuracy: f64,
    minimum_accuracy: f64,
    maximum_accuracy: f64,
    total_score_without_mods: i64,
    total_score: i64,
    failed: bool,
}

impl ScoreProcessor {
    /// A processor for a play of `beatmap` with these mods (see [`ScoringOptions::for_play`]).
    pub fn for_play(beatmap: &Beatmap, mods: &ModSet) -> ScoreProcessor {
        ScoreProcessor::new(beatmap, ScoringOptions::for_play(beatmap, mods))
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs (ApplyBeatmap, Reset)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/JudgementProcessor.cs (ApplyBeatmap, SimulateAutoplay)
    /// A processor for `beatmap` before the first judgement, with its maxima settled by a
    /// simulated perfect play.
    pub fn new(beatmap: &Beatmap, options: ScoringOptions) -> ScoreProcessor {
        let mut processor = ScoreProcessor::empty(options.score_multiplier);
        let classic = options.classic_slider_behaviour;
        autoplay::for_each_judgement(beatmap, |_, target| {
            let max_result = target.max_result(classic);
            processor.apply_result(max_result, max_result);
        });
        processor.store_maxima();
        processor
    }

    /// A processor before `ApplyBeatmap`.
    fn empty(score_multiplier: f64) -> ScoreProcessor {
        ScoreProcessor {
            score_multiplier,
            maximum_statistics: Statistics::default(),
            maximum_base_score: 0.0,
            maximum_combo_portion: 0.0,
            maximum_accuracy_judgement_count: 0,
            maximum_total_score: 0,
            statistics: Statistics::default(),
            current_maximum_base_score: 0.0,
            current_base_score: 0.0,
            current_accuracy_judgement_count: 0,
            current_combo_portion: 0.0,
            current_bonus_portion: 0.0,
            accuracy: 1.0,
            minimum_accuracy: 0.0,
            maximum_accuracy: 1.0,
            total_score_without_mods: 0,
            total_score: 0,
            failed: false,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs (Reset)
    /// `Reset(storeResults: true)` after the simulated perfect play: keeps its values as the
    /// maxima and starts over.
    fn store_maxima(&mut self) {
        // The simulated play is scored once more, against the maxima of the empty processor.
        self.update_score();
        self.maximum_base_score = self.current_base_score;
        self.maximum_combo_portion = self.current_combo_portion;
        self.maximum_accuracy_judgement_count = self.current_accuracy_judgement_count;
        self.maximum_statistics = self.statistics;
        self.maximum_total_score = self.total_score;

        self.statistics = Statistics::default();
        self.current_base_score = 0.0;
        self.current_maximum_base_score = 0.0;
        self.current_accuracy_judgement_count = 0;
        self.current_combo_portion = 0.0;
        self.current_bonus_portion = 0.0;
        // Lazer resets only `TotalScore` and `Accuracy` here; the other values are stale
        // until the first judgement, which recomputes them all.
        self.total_score = 0;
        self.accuracy = 1.0;
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs (ApplyResultInternal)
    /// Applies a judgement. `failed` is lazer's `JudgementResult.FailedAtJudgement`: whether
    /// the player had already failed before it (the health processor takes each judgement
    /// first); such a judgement changes nothing.
    pub fn apply(&mut self, judgement: &Judgement, failed: bool) {
        debug_assert!(
            judgement.result != HitResult::LegacyComboIncrease,
            "lazer rejects a LegacyComboIncrease judgement"
        );
        if failed {
            return;
        }
        self.apply_result(judgement.result, judgement.max_result);
        self.update_score();
    }

    /// The accounting of `ApplyResultInternal`, without updating the score (as while lazer
    /// simulates).
    fn apply_result(&mut self, result: HitResult, max_result: HitResult) {
        self.statistics.apply(result);

        if max_result.affects_accuracy() {
            self.current_maximum_base_score += f64::from(max_result.base_score());
            self.current_accuracy_judgement_count += 1;
        }

        if result.affects_accuracy() {
            self.current_base_score += f64::from(result.base_score());
        }

        if result.is_bonus() {
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs (GetBonusScoreChange)
            self.current_bonus_portion += f64::from(result.base_score());
        } else if result.is_scorable() {
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs (GetComboScoreChange)
            self.current_combo_portion += f64::from(max_result.base_score())
                * math::pow(f64::from(self.statistics.combo), COMBO_EXPONENT);
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs (updateScore, ComputeTotalScore)
    fn update_score(&mut self) {
        // The accuracies are bindables clamped to 0..=1; the scores are clamped from 0.
        self.accuracy = clamp_unit(if self.current_maximum_base_score > 0.0 {
            self.current_base_score / self.current_maximum_base_score
        } else {
            1.0
        });
        self.minimum_accuracy = clamp_unit(if self.maximum_base_score > 0.0 {
            self.current_base_score / self.maximum_base_score
        } else {
            0.0
        });
        self.maximum_accuracy = clamp_unit(if self.maximum_base_score > 0.0 {
            (self.current_base_score + (self.maximum_base_score - self.current_maximum_base_score))
                / self.maximum_base_score
        } else {
            1.0
        });

        let combo_progress = if self.maximum_combo_portion > 0.0 {
            self.current_combo_portion / self.maximum_combo_portion
        } else {
            1.0
        };
        let accuracy_progress = if self.maximum_accuracy_judgement_count > 0 {
            f64::from(self.current_accuracy_judgement_count)
                / f64::from(self.maximum_accuracy_judgement_count)
        } else {
            1.0
        };

        let total = 500_000.0 * self.accuracy * combo_progress
            + 500_000.0 * math::pow(self.accuracy, 5.0) * accuracy_progress
            + self.current_bonus_portion;
        self.total_score_without_mods = round_to_long(total).max(0);
        self.total_score =
            round_to_long(self.total_score_without_mods as f64 * self.score_multiplier).max(0);
    }

    /// Marks the play as failed: the rank is F from now on (lazer's `FailScore`).
    pub fn fail(&mut self) {
        self.failed = true;
    }

    /// Whether the play has failed.
    pub fn has_failed(&self) -> bool {
        self.failed
    }

    /// The score with the mods' multiplier (`TotalScore`).
    pub fn total_score(&self) -> i64 {
        self.total_score
    }

    /// The score without the mods' multiplier (`TotalScoreWithoutMods`).
    pub fn total_score_without_mods(&self) -> i64 {
        self.total_score_without_mods
    }

    /// The accuracy of the judgements so far, 0..=1 (`Accuracy`).
    pub fn accuracy(&self) -> f64 {
        self.accuracy
    }

    /// The accuracy if every remaining judgement were missed (`MinimumAccuracy`).
    pub fn minimum_accuracy(&self) -> f64 {
        self.minimum_accuracy
    }

    /// The accuracy if every remaining judgement were perfect (`MaximumAccuracy`).
    pub fn maximum_accuracy(&self) -> f64 {
        self.maximum_accuracy
    }

    /// Counts per result and combo of the applied judgements.
    pub fn statistics(&self) -> &Statistics {
        &self.statistics
    }

    /// Counts per result and combo of a perfect play (`MaximumResultCounts`, `MaximumCombo`).
    pub fn maximum_statistics(&self) -> &Statistics {
        &self.maximum_statistics
    }

    /// The score of a perfect play, with the multiplier (`MaximumTotalScore`).
    pub fn maximum_total_score(&self) -> i64 {
        self.maximum_total_score
    }

    /// Lazer's private maxima: `maximumBaseScore`, `maximumComboPortion` and
    /// `maximumAccuracyJudgementCount`. For parity tests.
    #[doc(hidden)]
    pub fn internal_maxima(&self) -> (f64, f64, i32) {
        (
            self.maximum_base_score,
            self.maximum_combo_portion,
            self.maximum_accuracy_judgement_count,
        )
    }

    /// The mods' score multiplier.
    pub fn score_multiplier(&self) -> f64 {
        self.score_multiplier
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs (updateRank)
    /// The rank, adjusted by `mods` (Hidden turns S and SS silver).
    ///
    /// Lazer updates the rank when the accuracy or the mods change; the misses it reads only
    /// matter from the first one on, which changes the accuracy unless it is already 0, where
    /// the rank is D either way, so computing the rank from the current state gives the same
    /// result.
    pub fn rank(&self, mods: &ModSet) -> ScoreRank {
        if self.failed {
            return ScoreRank::F;
        }
        let mut rank = rank_from_score(self.accuracy, &self.statistics);
        for m in mods.mods() {
            rank = m.adjust_rank(rank, self.accuracy);
        }
        rank
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs (RankFromScore)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuScoreProcessor.cs (RankFromScore)
/// The rank of an accuracy: a miss keeps S and SS at A.
pub fn rank_from_score(accuracy: f64, statistics: &Statistics) -> ScoreRank {
    let rank = if accuracy == ACCURACY_CUTOFF_X {
        ScoreRank::X
    } else if accuracy >= ACCURACY_CUTOFF_S {
        ScoreRank::S
    } else if accuracy >= ACCURACY_CUTOFF_A {
        ScoreRank::A
    } else if accuracy >= ACCURACY_CUTOFF_B {
        ScoreRank::B
    } else if accuracy >= ACCURACY_CUTOFF_C {
        ScoreRank::C
    } else {
        ScoreRank::D
    };
    match rank {
        ScoreRank::S | ScoreRank::X if statistics.count(HitResult::Miss) > 0 => ScoreRank::A,
        _ => rank,
    }
}

/// `BindableDouble { MinValue = 0, MaxValue = 1 }`: `Math.Clamp`, which keeps a NaN.
fn clamp_unit(value: f64) -> f64 {
    dotnet::clamp(value, 0.0, 1.0)
}

/// `(long)Math.Round(value)`: to even on ties.
fn round_to_long(value: f64) -> i64 {
    value.round_ties_even() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_to_even() {
        assert_eq!(round_to_long(0.5), 0);
        assert_eq!(round_to_long(1.5), 2);
        assert_eq!(round_to_long(2.5), 2);
        assert_eq!(round_to_long(-0.5), 0);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Rulesets/Scoring/ScoreProcessorTest.cs (TestAffectsAccuracy, TestIsScorable)
    #[test]
    fn result_classes_match_lazer() {
        use HitResult::*;
        let cases = [
            // (result, scorable, affects accuracy)
            (None, false, false),
            (Miss, true, true),
            (Meh, true, true),
            (Ok, true, true),
            (Good, true, true),
            (Great, true, true),
            (Perfect, true, true),
            (SmallTickMiss, true, true),
            (SmallTickHit, true, true),
            (LargeTickMiss, true, true),
            (LargeTickHit, true, true),
            (SmallBonus, true, false),
            (LargeBonus, true, false),
            (IgnoreMiss, false, false),
            (IgnoreHit, false, false),
            (ComboBreak, true, false),
            (SliderTailHit, true, true),
            (LegacyComboIncrease, true, false),
        ];
        for (result, scorable, accuracy) in cases {
            assert_eq!(result.is_scorable(), scorable, "{result:?} scorable");
            assert_eq!(result.affects_accuracy(), accuracy, "{result:?} accuracy");
        }
    }

    /// A processor for objects with these best results, in judgement order.
    fn processor(max_results: &[HitResult], score_multiplier: f64) -> ScoreProcessor {
        let mut p = ScoreProcessor::empty(score_multiplier);
        for &m in max_results {
            p.apply_result(m, m);
        }
        p.store_maxima();
        p
    }

    fn judge(p: &mut ScoreProcessor, result: HitResult, max_result: HitResult) {
        let judgement = Judgement {
            object: crate::judgement::ObjectRef {
                index: 0,
                nested: Option::None,
            },
            time: 0.0,
            result,
            max_result,
        };
        p.apply(&judgement, false);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Rulesets/Scoring/ScoreProcessorTest.cs (TestSingleOsuHit)
    #[test]
    fn single_osu_hit_matches_lazer() {
        use HitResult::*;
        for (result, expected) in [(Meh, 83_398), (Ok, 168_724), (Great, 1_000_000)] {
            let mut p = processor(&[Great], 1.0);
            judge(&mut p, result, Great);
            assert_eq!(p.total_score(), expected, "{result:?}");
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Rulesets/Scoring/ScoreProcessorTest.cs (TestFourVariousResultsOneMiss)
    #[test]
    fn four_various_results_one_miss_match_lazer() {
        use HitResult::*;
        let cases = [
            (Miss, Great, 0),
            (Meh, Great, 34_734),
            (Ok, Great, 69_925),
            (Good, Perfect, 154_499),
            (Great, Great, 326_963),
            (Perfect, Perfect, 326_963),
            (SmallTickMiss, SmallTickHit, 0),
            (SmallTickHit, SmallTickHit, 493_652),
            (LargeTickMiss, LargeTickHit, 0),
            (LargeTickHit, LargeTickHit, 326_963),
            (SliderTailHit, SliderTailHit, 371_627),
            (SmallBonus, SmallBonus, 1_000_030),
            (LargeBonus, LargeBonus, 1_000_150),
        ];
        for (result, max_result, expected) in cases {
            let mut p = processor(&[max_result; 4], 1.0);
            // The judgement of the third object is a `TestJudgement(hitResult)`'s minimum.
            for i in 0..4 {
                let r = if i == 2 { result.min_result() } else { result };
                judge(&mut p, r, max_result);
            }
            assert_eq!(p.total_score(), expected, "{result:?} of {max_result:?}");
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Rulesets/Scoring/ScoreProcessorTest.cs (TestEmptyBeatmap)
    #[test]
    fn empty_beatmap_scores_zero() {
        let p = processor(&[], 1.0);
        assert_eq!(p.total_score(), 0);
        assert_eq!(p.accuracy(), 1.0);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Rulesets/Scoring/ScoreProcessorTest.cs (TestAccuracyWhenNearPerfect)
    #[test]
    fn accuracy_when_near_perfect() {
        use HitResult::*;
        let mut p = processor(&[Great; 1000], 1.0);
        for i in 0..1000 {
            judge(&mut p, if i == 0 { Miss } else { Great }, Great);
        }
        assert_eq!(p.accuracy(), 999.0 / 1000.0);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Rulesets/Scoring/ScoreProcessorTest.cs (TestAccuracyUpdateFromIgnoreMiss)
    #[test]
    fn ignore_miss_of_an_accuracy_judgement_lowers_accuracy() {
        use HitResult::*;
        for max_result in [Great, LargeTickHit] {
            let mut p = processor(&[max_result], 1.0);
            judge(&mut p, IgnoreMiss, max_result);
            assert_ne!(p.accuracy(), 1.0, "{max_result:?}");
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Rulesets/Scoring/ScoreProcessorTest.cs (TestNormalGrades, TestSilverGrades)
    #[test]
    fn hidden_turns_ranks_silver() {
        use HitResult::*;
        let hidden = ModSet::new(vec![crate::mods::Mod::Hidden {
            only_fade_approach_circles: false,
        }])
        .unwrap();
        let none = ModSet::default();
        // 99 Greats and an Ok: 0.99667 accuracy.
        let mut p = processor(&[Great; 100], 1.0);
        assert_eq!(p.rank(&none), ScoreRank::X);
        assert_eq!(p.rank(&hidden), ScoreRank::XH);
        judge(&mut p, Ok, Great);
        for _ in 0..99 {
            judge(&mut p, Great, Great);
        }
        assert_eq!(p.rank(&none), ScoreRank::S);
        assert_eq!(p.rank(&hidden), ScoreRank::SH);
        p.fail();
        assert_eq!(p.rank(&hidden), ScoreRank::F);
    }

    #[test]
    fn failed_judgements_score_nothing() {
        use HitResult::*;
        let mut p = processor(&[Great; 2], 1.0);
        judge(&mut p, Great, Great);
        let before = p.clone();
        let judgement = Judgement {
            object: crate::judgement::ObjectRef {
                index: 1,
                nested: Option::None,
            },
            time: 0.0,
            result: Great,
            max_result: Great,
        };
        p.apply(&judgement, true);
        assert_eq!(p, before);
    }

    #[test]
    fn multiplier_applies_to_the_rounded_score() {
        use HitResult::*;
        let mut p = processor(&[Great], 1.09);
        assert_eq!(p.maximum_total_score(), 1_090_000);
        judge(&mut p, Ok, Great);
        assert_eq!(p.total_score_without_mods(), 168_724);
        assert_eq!(p.total_score(), round_to_long(168_724.0 * 1.09));
    }

    #[test]
    fn misses_keep_s_and_ss_at_a() {
        let mut statistics = Statistics::default();
        assert_eq!(rank_from_score(1.0, &statistics), ScoreRank::X);
        assert_eq!(rank_from_score(0.95, &statistics), ScoreRank::S);
        assert_eq!(rank_from_score(0.9499, &statistics), ScoreRank::A);
        assert_eq!(rank_from_score(0.8, &statistics), ScoreRank::B);
        assert_eq!(rank_from_score(0.7, &statistics), ScoreRank::C);
        assert_eq!(rank_from_score(0.6999, &statistics), ScoreRank::D);
        statistics.apply(HitResult::Miss);
        assert_eq!(rank_from_score(1.0, &statistics), ScoreRank::A);
        assert_eq!(rank_from_score(0.96, &statistics), ScoreRank::A);
        assert_eq!(rank_from_score(0.85, &statistics), ScoreRank::B);
    }
}
