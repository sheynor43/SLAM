// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu/osu.Game/Rulesets/Scoring/HealthProcessor.cs (lines 1-113); Doc comments are removed. See README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
using System;
using osu.Framework.Bindables;
using osu.Framework.Utils;
using osu.Game.Rulesets.Judgements;

namespace osu.Game.Rulesets.Scoring
{
    public abstract partial class HealthProcessor : JudgementProcessor
    {
        public event Func<bool>? Failed;

        public event Func<HealthProcessor, JudgementResult, bool>? FailConditions;

        public readonly BindableDouble Health = new BindableDouble(1) { MinValue = 0, MaxValue = 1 };

        public bool HasFailed { get; private set; }

        public void TriggerFailure()
        {
            if (HasFailed)
                return;

            if (Failed?.Invoke() != false)
                HasFailed = true;
        }

        protected override void ApplyResultInternal(JudgementResult result)
        {
            result.HealthAtJudgement = Health.Value;
            result.FailedAtJudgement = HasFailed;

            if (HasFailed)
                return;

            Health.Value += GetHealthIncreaseFor(result);

            if (meetsAnyFailCondition(result))
                TriggerFailure();
        }

        protected override void RevertResultInternal(JudgementResult result)
        {
            HasFailed = result.FailedAtJudgement;

            if (HasFailed)
                return;

            Health.Value = result.HealthAtJudgement;
        }

        protected virtual double GetHealthIncreaseFor(JudgementResult result) => result.HealthIncrease;

        protected virtual bool CheckDefaultFailCondition(JudgementResult result) => Precision.AlmostBigger(Health.MinValue, Health.Value);

        private bool meetsAnyFailCondition(JudgementResult result)
        {
            if (CheckDefaultFailCondition(result))
                return true;

            if (FailConditions != null)
            {
                foreach (var condition in FailConditions.GetInvocationList())
                {
                    bool conditionResult = (bool)condition.Method.Invoke(condition.Target, new object[] { this, result })!;
                    if (conditionResult)
                        return true;
                }
            }

            return false;
        }

        protected override void Reset(bool storeResults)
        {
            base.Reset(storeResults);

            Health.Value = 1;
            HasFailed = false;
        }
    }
}
