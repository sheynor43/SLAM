// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu/osu.Game/Rulesets/Judgements/JudgementResult.cs (lines 19, 24, 29, 38, 43-47, 85, 90, 100, 105, 112-115, 117); only the members the health processors read: Type, HitObject, Judgement, RawTime, TimeOffset, HealthAtJudgement, FailedAtJudgement, IsHit, HealthIncrease and the constructor without the Reset call. See README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
using System;
using osu.Game.Rulesets.Objects;
using osu.Game.Rulesets.Scoring;

namespace osu.Game.Rulesets.Judgements
{
    public class JudgementResult
    {
        public HitResult Type;
        public readonly HitObject HitObject;
        public readonly Judgement Judgement;
        internal double? RawTime { get; set; }
        public double TimeOffset
        {
            get => RawTime != null ? Math.Min(RawTime.Value - HitObject.GetEndTime(), HitObject.MaximumJudgementOffset) : 0;
            internal set => RawTime = HitObject.GetEndTime() + value;
        }
        public double HealthAtJudgement { get; internal set; }
        public bool FailedAtJudgement { get; internal set; }
        public bool IsHit => Type.IsHit();
        public double HealthIncrease => Judgement.HealthIncreaseFor(this);
        public JudgementResult(HitObject hitObject, Judgement judgement)
        {
            HitObject = hitObject;
            Judgement = judgement;
        }
    }
}
