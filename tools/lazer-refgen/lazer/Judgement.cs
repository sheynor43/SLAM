// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu/osu.Game/Rulesets/Judgements/Judgement.cs (lines 1-152); Doc comments are removed. See README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
using osu.Game.Rulesets.Objects;
using osu.Game.Rulesets.Scoring;

namespace osu.Game.Rulesets.Judgements
{
    public class Judgement
    {
        protected const double DEFAULT_MAX_HEALTH_INCREASE = 0.05;

        public virtual HitResult MaxResult => HitResult.Perfect;

        public virtual HitResult MinResult
        {
            get
            {
                switch (MaxResult)
                {
                    case HitResult.SmallBonus:
                    case HitResult.LargeBonus:
                    case HitResult.IgnoreHit:
                        return HitResult.IgnoreMiss;

                    case HitResult.SmallTickHit:
                        return HitResult.SmallTickMiss;

                    case HitResult.LargeTickHit:
                        return HitResult.LargeTickMiss;

                    case HitResult.SliderTailHit:
                        return HitResult.IgnoreMiss;

                    default:
                        return HitResult.Miss;
                }
            }
        }

        public double MaxHealthIncrease => HealthIncreaseFor(MaxResult);

        protected virtual double HealthIncreaseFor(HitResult result)
        {
            switch (result)
            {
                default:
                    return 0;

                case HitResult.SmallTickHit:
                    return DEFAULT_MAX_HEALTH_INCREASE * 0.5;

                case HitResult.SmallTickMiss:
                    return -DEFAULT_MAX_HEALTH_INCREASE * 0.5;

                case HitResult.SliderTailHit:
                case HitResult.LargeTickHit:
                    return DEFAULT_MAX_HEALTH_INCREASE;

                case HitResult.LargeTickMiss:
                    return -DEFAULT_MAX_HEALTH_INCREASE;

                case HitResult.Miss:
                    return -DEFAULT_MAX_HEALTH_INCREASE * 2;

                case HitResult.Meh:
                    return DEFAULT_MAX_HEALTH_INCREASE * 0.05;

                case HitResult.Ok:
                    return DEFAULT_MAX_HEALTH_INCREASE * 0.5;

                case HitResult.Good:
                    return DEFAULT_MAX_HEALTH_INCREASE * 0.75;

                case HitResult.Great:
                    return DEFAULT_MAX_HEALTH_INCREASE;

                case HitResult.Perfect:
                    return DEFAULT_MAX_HEALTH_INCREASE * 1.05;

                case HitResult.SmallBonus:
                    return DEFAULT_MAX_HEALTH_INCREASE * 0.5;

                case HitResult.LargeBonus:
                    return DEFAULT_MAX_HEALTH_INCREASE;
            }
        }

        public double HealthIncreaseFor(JudgementResult result) => HealthIncreaseFor(result.Type);

        public override string ToString() => $"MaxResult:{MaxResult}";
    }
}
