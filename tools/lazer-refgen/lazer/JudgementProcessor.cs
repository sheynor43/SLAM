// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu/osu.Game/Rulesets/Scoring/JudgementProcessor.cs (lines 1-213 without lines 130-139, 202-211); Not a Component; HasCompleted, ResetFromReplayFrame and Update are omitted. Doc comments are removed. See README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
using System;
using System.Collections.Generic;
using osu.Framework.Extensions.TypeExtensions;
using osu.Game.Beatmaps;
using osu.Game.Rulesets.Judgements;
using osu.Game.Rulesets.Objects;

namespace osu.Game.Rulesets.Scoring
{
    public abstract partial class JudgementProcessor
    {
        public event Action<JudgementResult>? NewJudgement;

        public event Action<JudgementResult>? JudgementReverted;

        protected int MaxHits { get; private set; }

        protected bool IsSimulating { get; private set; }

        public int JudgedHits { get; private set; }

        private JudgementResult? lastAppliedResult;

        public virtual void ApplyBeatmap(IBeatmap beatmap)
        {
            Reset(false);
            SimulateAutoplay(beatmap);
            Reset(true);
        }

        public void ApplyResult(JudgementResult result)
        {
#pragma warning disable CS0618
            if (result.Type == HitResult.LegacyComboIncrease)
                throw new ArgumentException(@$"A {nameof(HitResult.LegacyComboIncrease)} hit result cannot be applied.");
#pragma warning restore CS0618

            JudgedHits++;
            lastAppliedResult = result;

            ApplyResultInternal(result);

            NewJudgement?.Invoke(result);
        }

        public void RevertResult(JudgementResult result)
        {
            JudgedHits--;

            RevertResultInternal(result);

            JudgementReverted?.Invoke(result);
        }

        protected abstract void ApplyResultInternal(JudgementResult result);

        protected abstract void RevertResultInternal(JudgementResult result);

        protected virtual void Reset(bool storeResults)
        {
            if (storeResults)
                MaxHits = JudgedHits;

            JudgedHits = 0;
        }

        protected void SimulateAutoplay(IBeatmap beatmap)
        {
            IsSimulating = true;

            foreach (var obj in EnumerateHitObjects(beatmap))
            {
                var judgement = obj.Judgement;

                var result = CreateResult(obj, judgement);
                if (result == null)
                    throw new InvalidOperationException($"{GetType().ReadableName()} must provide a {nameof(JudgementResult)} through {nameof(CreateResult)}.");

                result.Type = GetSimulatedHitResult(judgement);
                ApplyResult(result);
            }

            IsSimulating = false;
        }

        protected virtual IEnumerable<HitObject> EnumerateHitObjects(IBeatmap beatmap)
            => enumerateRecursively(beatmap.HitObjects);

        private IEnumerable<HitObject> enumerateRecursively(IEnumerable<HitObject> hitObjects)
        {
            foreach (var hitObject in hitObjects)
            {
                foreach (var nested in enumerateRecursively(hitObject.NestedHitObjects))
                    yield return nested;

                yield return hitObject;
            }
        }

        protected virtual JudgementResult CreateResult(HitObject hitObject, Judgement judgement) => new JudgementResult(hitObject, judgement);

        protected virtual HitResult GetSimulatedHitResult(Judgement judgement) => judgement.MaxResult;

    }
}
