// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu/osu.Game/Rulesets/Scoring/HitResult.cs (lines 16-161, 163, 171-223, 267-278, 308-323, 328-347); the HitResult enum without its attributes ([HasOrderedElements], [Description], [EnumMember], [Order]) and doc comments, and the extension methods IncreasesCombo, BreaksCombo, AffectsCombo, AffectsAccuracy, IsBonus, IsHit and IsScorable (without doc comments). See README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
using System;

namespace osu.Game.Rulesets.Scoring
{
    public enum HitResult
    {
        None,

        Miss,

        Meh,

        Ok,

        Good,

        Great,

        Perfect,

        SmallTickMiss,

        SmallTickHit,

        LargeTickMiss,

        LargeTickHit,

        SmallBonus,

        LargeBonus,

        IgnoreMiss,

        IgnoreHit,

        ComboBreak,

        SliderTailHit,

        [Obsolete("Do not use.")]
        LegacyComboIncrease = 99
    }

#pragma warning disable CS0618
    public static class HitResultExtensions
    {
        public static bool IncreasesCombo(this HitResult result)
            => AffectsCombo(result) && IsHit(result);

        public static bool BreaksCombo(this HitResult result)
            => AffectsCombo(result) && !IsHit(result);

        public static bool AffectsCombo(this HitResult result)
        {
            switch (result)
            {
                case HitResult.Miss:
                case HitResult.Meh:
                case HitResult.Ok:
                case HitResult.Good:
                case HitResult.Great:
                case HitResult.Perfect:
                case HitResult.LargeTickHit:
                case HitResult.LargeTickMiss:
                case HitResult.LegacyComboIncrease:
                case HitResult.ComboBreak:
                case HitResult.SliderTailHit:
                    return true;

                default:
                    return false;
            }
        }

        public static bool AffectsAccuracy(this HitResult result)
        {
            switch (result)
            {
                // LegacyComboIncrease is a special non-gameplay type which is neither a basic, tick, bonus, or accuracy-affecting result.
                case HitResult.LegacyComboIncrease:
                    return false;

                // ComboBreak is a special type that only affects combo. It cannot be considered as basic, tick, bonus, or accuracy-affecting.
                case HitResult.ComboBreak:
                    return false;

                default:
                    return IsScorable(result) && !IsBonus(result);
            }
        }

        public static bool IsScorable(this HitResult result)
        {
            switch (result)
            {
                // LegacyComboIncrease is not actually scorable (in terms of usable by rulesets for that purpose), but needs to be defined as such to be correctly included in statistics output.
                case HitResult.LegacyComboIncrease:
                    return true;

                // ComboBreak is its own type that affects score via combo.
                case HitResult.ComboBreak:
                    return true;

                case HitResult.SliderTailHit:
                    return true;

                default:
                    // Note that IgnoreHit and IgnoreMiss are excluded as they do not affect score.
                    return result >= HitResult.Miss && result < HitResult.IgnoreMiss;
            }
        }

        public static bool IsBonus(this HitResult result)
        {
            switch (result)
            {
                case HitResult.SmallBonus:
                case HitResult.LargeBonus:
                    return true;

                default:
                    return false;
            }
        }

        public static bool IsHit(this HitResult result)
        {
            switch (result)
            {
                case HitResult.None:
                case HitResult.IgnoreMiss:
                case HitResult.Miss:
                case HitResult.SmallTickMiss:
                case HitResult.LargeTickMiss:
                case HitResult.ComboBreak:
                    return false;

                default:
                    return true;
            }
        }
    }
}
