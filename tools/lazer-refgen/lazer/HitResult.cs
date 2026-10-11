// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu/osu.Game/Rulesets/Scoring/HitResult.cs (lines 16-161, 267-278, 308-323); the HitResult enum without its attributes ([HasOrderedElements], [Description], [EnumMember], [Order]) and doc comments, and the extension methods IsBonus and IsHit. See README.md.
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

    public static class HitResultExtensions
    {
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
