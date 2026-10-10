// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuScoreMultiplierCalculatorV2.cs (lines 112-152), osu.Game.Rulesets.Osu/Scoring/OsuScoreMultiplierCalculatorV1.cs (lines 87-99), osu.Game/Rulesets/Mods/ModEasyWithExtraLives.cs (lines 19-24), osu.Game.Rulesets.Osu/Mods/OsuModHidden.cs (setting OnlyFadeApproachCircles), osu.Game/Rulesets/Mods/ModDoubleTime.cs, ModHalfTime.cs (SpeedChange and AdjustPitch), ModNightcore.cs, ModDaycore.cs (SpeedChange), osu.Game.Rulesets.Osu/Mods/OsuModClassic.cs (settings). The private multiplier functions are made internal static members of two static classes; the mods are shells holding only their settings; see README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.

using System;
using osu.Framework.Bindables;

namespace osu.Game.Rulesets.Osu.Mods
{
    public class OsuModEasy
    {
        public Bindable<int> Retries { get; } = new BindableInt(2)
        {
            MinValue = 0,
            MaxValue = 10
        };
    }

    public class OsuModHidden
    {
        public Bindable<bool> OnlyFadeApproachCircles { get; } = new BindableBool();
    }

    public class OsuModDoubleTime
    {
        public BindableNumber<double> SpeedChange { get; } = new BindableDouble(1.5)
        {
            MinValue = 1.01,
            MaxValue = 2,
            Precision = 0.01,
        };
        public BindableBool AdjustPitch { get; } = new BindableBool();
    }

    public class OsuModHalfTime
    {
        public BindableNumber<double> SpeedChange { get; } = new BindableDouble(0.75)
        {
            MinValue = 0.5,
            MaxValue = 0.99,
            Precision = 0.01,
        };
        public BindableBool AdjustPitch { get; } = new BindableBool();
    }

    public class OsuModNightcore
    {
        public BindableNumber<double> SpeedChange { get; } = new BindableDouble(1.5)
        {
            MinValue = 1.01,
            MaxValue = 2,
            Precision = 0.01,
        };
    }

    public class OsuModDaycore
    {
        public BindableNumber<double> SpeedChange { get; } = new BindableDouble(0.75)
        {
            MinValue = 0.5,
            MaxValue = 0.99,
            Precision = 0.01,
        };
    }

    public class OsuModClassic
    {
        public Bindable<bool> NoSliderHeadAccuracy { get; } = new BindableBool(true);
        public Bindable<bool> ClassicNoteLock { get; } = new BindableBool(true);
        public Bindable<bool> AlwaysPlayTailSample { get; } = new BindableBool(true);
        public Bindable<bool> FadeHitCircleEarly { get; } = new Bindable<bool>(true);
        public Bindable<bool> ClassicHealth { get; } = new Bindable<bool>(true);
    }
}

namespace osu.Game.Rulesets.Osu.Scoring
{
    using osu.Game.Rulesets.Osu.Mods;

    public static class OsuScoreMultiplierCalculatorV2
    {
        internal static double easyMultiplier(OsuModEasy easy)
        {
            // 0.8x base multiplier
            // Reduce by 0.1x per extra life
            double value = 0.8 - Math.Max(0, 0.1 * (easy.Retries.Value - easy.Retries.Default));

            return Math.Max(0.4, value);
        }

        internal static double halfTimeMultiplier(double speedChange)
        {
            // 0.2x at 0.5x speed, +0.07x per 0.05x speed increment.
            // Default HT (0.75x) = 0.55
            return (int)(speedChange * 20) / 20.0 * 1.4 - 0.5;
        }

        internal static double doubleTimeMultiplier(double speedChange)
        {
            // Floor to the nearest multiple of 0.1.
            double value = (int)(speedChange * 10) / 10.0;

            // 0.01 penalty for non-default rates.
            double penalty = value != 1.5 && value != 1.0 ? 0.01 : 0.0;

            // Linear from 1.0 to 1.46, minus the penalty.
            // Default DT (1.5x) = 1.23
            return (value - 1) * 0.46 + 1 - penalty;
        }

        internal static double hiddenMultiplier(OsuModHidden hidden, bool otherModsProvideTimingInfo)
        {
            double value = 1.04;

            if (hidden.OnlyFadeApproachCircles.Value)
                value -= 0.02;

            if (otherModsProvideTimingInfo)
                value -= 0.02;

            return value;
        }
    }

    public static class OsuScoreMultiplierCalculatorV1
    {
        internal static double rateAdjustMultiplier(double speedChange)
        {
            // Round to the nearest multiple of 0.1.
            double value = (int)(speedChange * 10) / 10.0;

            // Offset back to 0.
            value -= 1;

            if (speedChange >= 1)
                return 1 + value / 5;
            else
                return 0.6 + value;
        }
    }
}
