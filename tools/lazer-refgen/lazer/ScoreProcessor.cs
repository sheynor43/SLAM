// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu/osu.Game/Rulesets/Scoring/ScoreProcessor.cs (lines 22-475 and 559-573 without doc comments; the NOTE comment of ApplyBeatmap, which is about the omitted Beatmap bindable, is dropped), omitting: the hit events (TrackHitEvents, hitEvents, lastHitObject, HitEvents, CreateHitEvent and their statements in ApplyResultInternal and Reset), OnResetFromReplayFrame, the Ruleset (constructor parameter and field) and Beatmap bindable, MaximumStatistics and beatmapApplied, RevertResultInternal (a stand-in that throws is in Scoring.cs), updateScoreMultiplier (and its calls: the multiplier is the public field scoreMultiplier, set directly by the generator; the multipliers of mods are covered by lazer-mods.txt), and PopulateScore, FailScore, ResetFromReplayFrame, Get/SetScoreProcessorStatistics and AccuracyCutoffFromRank (lines 476-558, 574-). maximumBaseScore, maximumComboPortion, maximumAccuracyJudgementCount and scoreMultiplier are public instead of private so that the generator can read and set them. See README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
#nullable enable
using System;
using System.Collections.Generic;
using System.Linq;
using osu.Framework.Bindables;
using osu.Game.Beatmaps;
using osu.Game.Rulesets.Judgements;
using osu.Game.Rulesets.Mods;
using osu.Game.Rulesets.Objects;
using osu.Game.Scoring;

namespace osu.Game.Rulesets.Scoring
{
    public partial class ScoreProcessor : JudgementProcessor
    {
        public const double COMBO_EXPONENT = 0.5;

        public const double MAX_SCORE = 1000000;

        private const double accuracy_cutoff_x = 1;
        private const double accuracy_cutoff_s = 0.95;
        private const double accuracy_cutoff_a = 0.9;
        private const double accuracy_cutoff_b = 0.8;
        private const double accuracy_cutoff_c = 0.7;
        private const double accuracy_cutoff_d = 0;

        public readonly BindableLong TotalScore = new BindableLong { MinValue = 0 };

        public readonly BindableLong TotalScoreWithoutMods = new BindableLong { MinValue = 0 };

        public readonly BindableDouble Accuracy = new BindableDouble(1) { MinValue = 0, MaxValue = 1 };

        public readonly BindableDouble MinimumAccuracy = new BindableDouble { MinValue = 0, MaxValue = 1 };

        public readonly BindableDouble MaximumAccuracy = new BindableDouble(1) { MinValue = 0, MaxValue = 1 };

        public readonly BindableInt Combo = new BindableInt();

        public readonly Bindable<IReadOnlyList<Mod>> Mods = new Bindable<IReadOnlyList<Mod>>(Array.Empty<Mod>());

        public IBindable<ScoreRank> Rank => rank;

        private readonly Bindable<ScoreRank> rank = new Bindable<ScoreRank>(ScoreRank.X);

        public readonly BindableInt HighestCombo = new BindableInt();

        public long MaximumTotalScore { get; private set; }

        public int MaximumCombo { get; private set; }

        private double currentMaximumBaseScore;

        private double currentBaseScore;

        public double maximumBaseScore;

        public int maximumAccuracyJudgementCount;

        private int currentAccuracyJudgementCount;

        public double maximumComboPortion;

        private double currentComboPortion;

        private double currentBonusPortion;

        public double scoreMultiplier = 1;

        protected readonly Dictionary<HitResult, int> ScoreResultCounts = new Dictionary<HitResult, int>();
        protected readonly Dictionary<HitResult, int> MaximumResultCounts = new Dictionary<HitResult, int>();

        public bool ApplyNewJudgementsWhenFailed { get; set; }

        public ScoreProcessor()
        {
            Accuracy.ValueChanged += _ => updateRank();

            Mods.ValueChanged += mods =>
            {
                updateScore();
                updateRank();
            };

        }

        public override void ApplyBeatmap(IBeatmap beatmap)
        {
            base.ApplyBeatmap(beatmap);
        }

        protected sealed override void ApplyResultInternal(JudgementResult result)
        {
            result.ComboAtJudgement = Combo.Value;
            result.HighestComboAtJudgement = HighestCombo.Value;

            if (result.FailedAtJudgement && !ApplyNewJudgementsWhenFailed)
                return;

            ScoreResultCounts[result.Type] = ScoreResultCounts.GetValueOrDefault(result.Type) + 1;

            if (result.Type.IncreasesCombo())
                Combo.Value++;
            else if (result.Type.BreaksCombo())
                Combo.Value = 0;

            HighestCombo.Value = Math.Max(HighestCombo.Value, Combo.Value);

            result.ComboAfterJudgement = Combo.Value;
            result.HighestComboAfterJudgement = HighestCombo.Value;

            if (result.Judgement.MaxResult.AffectsAccuracy())
            {
                currentMaximumBaseScore += GetBaseScoreForResult(result.Judgement.MaxResult);
                currentAccuracyJudgementCount++;
            }

            if (result.Type.AffectsAccuracy())
                currentBaseScore += GetBaseScoreForResult(result.Type);

            if (result.Type.IsBonus())
                currentBonusPortion += GetBonusScoreChange(result);
            else if (result.Type.IsScorable())
                currentComboPortion += GetComboScoreChange(result);

            ApplyScoreChange(result);

            if (!IsSimulating)
            {
                updateScore();
            }
        }

        protected virtual double GetBonusScoreChange(JudgementResult result) => GetBaseScoreForResult(result.Type);

        protected virtual double GetComboScoreChange(JudgementResult result) => GetBaseScoreForResult(result.Judgement.MaxResult) * Math.Pow(result.ComboAfterJudgement, COMBO_EXPONENT);

        public virtual int GetBaseScoreForResult(HitResult result)
        {
            switch (result)
            {
                default:
                    return 0;

                case HitResult.SmallTickHit:
                    return 10;

                case HitResult.LargeTickHit:
                    return 30;

                case HitResult.SliderTailHit:
                    return 150;

                case HitResult.Meh:
                    return 50;

                case HitResult.Ok:
                    return 100;

                case HitResult.Good:
                    return 200;

                case HitResult.Great:
                case HitResult.Perfect: // Perfect doesn't actually give more score / accuracy directly.
                    return 300;

                case HitResult.SmallBonus:
                    return 10;

                case HitResult.LargeBonus:
                    return 50;
            }
        }

        protected virtual void ApplyScoreChange(JudgementResult result)
        {
        }

        protected virtual void RemoveScoreChange(JudgementResult result)
        {
        }

        private void updateScore()
        {
            Accuracy.Value = currentMaximumBaseScore > 0 ? currentBaseScore / currentMaximumBaseScore : 1;
            MinimumAccuracy.Value = maximumBaseScore > 0 ? currentBaseScore / maximumBaseScore : 0;
            MaximumAccuracy.Value = maximumBaseScore > 0 ? (currentBaseScore + (maximumBaseScore - currentMaximumBaseScore)) / maximumBaseScore : 1;

            double comboProgress = maximumComboPortion > 0 ? currentComboPortion / maximumComboPortion : 1;
            double accuracyProgress = maximumAccuracyJudgementCount > 0 ? (double)currentAccuracyJudgementCount / maximumAccuracyJudgementCount : 1;

            TotalScoreWithoutMods.Value = (long)Math.Round(ComputeTotalScore(comboProgress, accuracyProgress, currentBonusPortion));
            TotalScore.Value = (long)Math.Round(TotalScoreWithoutMods.Value * scoreMultiplier);
        }

        private void updateRank()
        {
            // Once failed, we shouldn't update the rank anymore.
            if (rank.Value == ScoreRank.F)
                return;

            ScoreRank newRank = RankFromScore(Accuracy.Value, ScoreResultCounts);

            foreach (var mod in Mods.Value.OfType<IApplicableToScoreProcessor>())
                newRank = mod.AdjustRank(newRank, Accuracy.Value);

            rank.Value = newRank;
        }

        protected virtual double ComputeTotalScore(double comboProgress, double accuracyProgress, double bonusPortion)
        {
            return 500000 * Accuracy.Value * comboProgress +
                   500000 * Math.Pow(Accuracy.Value, 5) * accuracyProgress +
                   bonusPortion;
        }

        protected override void Reset(bool storeResults)
        {
            // Run one last time to store max values.
            updateScore();

            base.Reset(storeResults);

            if (storeResults)
            {
                maximumBaseScore = currentBaseScore;

                maximumComboPortion = currentComboPortion;
                maximumAccuracyJudgementCount = currentAccuracyJudgementCount;

                MaximumResultCounts.Clear();
                MaximumResultCounts.AddRange(ScoreResultCounts);

                MaximumTotalScore = TotalScore.Value;
                MaximumCombo = HighestCombo.Value;
            }

            ScoreResultCounts.Clear();

            currentBaseScore = 0;
            currentMaximumBaseScore = 0;
            currentAccuracyJudgementCount = 0;
            currentComboPortion = 0;
            currentBonusPortion = 0;

            TotalScore.Value = 0;
            Accuracy.Value = 1;
            Combo.Value = 0;
            HighestCombo.Value = 0;
            updateRank();
        }

        public virtual ScoreRank RankFromScore(double accuracy, IReadOnlyDictionary<HitResult, int> results)
        {
            if (accuracy == accuracy_cutoff_x)
                return ScoreRank.X;
            if (accuracy >= accuracy_cutoff_s)
                return ScoreRank.S;
            if (accuracy >= accuracy_cutoff_a)
                return ScoreRank.A;
            if (accuracy >= accuracy_cutoff_b)
                return ScoreRank.B;
            if (accuracy >= accuracy_cutoff_c)
                return ScoreRank.C;

            return ScoreRank.D;
        }
    }
}
