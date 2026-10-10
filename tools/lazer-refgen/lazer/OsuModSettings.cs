// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuScoreMultiplierCalculatorV2.cs (lines 112-152), osu.Game.Rulesets.Osu/Scoring/OsuScoreMultiplierCalculatorV1.cs (lines 87-99), osu.Game/Rulesets/Mods/ModEasyWithExtraLives.cs (lines 19-24), osu.Game.Rulesets.Osu/Mods/OsuModHidden.cs (setting OnlyFadeApproachCircles), osu.Game/Rulesets/Mods/ModDoubleTime.cs, ModHalfTime.cs (SpeedChange and AdjustPitch), ModNightcore.cs, ModDaycore.cs (SpeedChange), osu.Game.Rulesets.Osu/Mods/OsuModClassic.cs (settings), osu.Game/Rulesets/Mods/DifficultyBindable.cs (lines 10-136, without the binding methods and with the extended-limits event as a shell), ModDifficultyAdjust.cs (lines 38-59), OsuModDifficultyAdjust.cs (lines 21-40), OsuModMirror.cs (lines 135-136, 159-164), OsuScoreMultiplierCalculatorV2.cs (lines 61, 165-184), OsuScoreMultiplierCalculatorV1.cs (line 43). The private multiplier functions are made internal static members of two static classes; the mods are shells holding only their settings; see README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.

#pragma warning disable CS8632 // the verbatim sources carry nullable annotations

using System;
using System.Collections.Generic;
using osu.Framework.Bindables;
using osu.Game.Beatmaps;
using osu.Game.Rulesets.Mods;

namespace osu.Game.Rulesets.Osu.Mods
{
    public partial class OsuModEasy : ModEasy
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

    public partial class OsuModMirror : IApplicableToHitObject
    {
        public Bindable<MirrorType> Reflection { get; } = new Bindable<MirrorType>();

        public enum MirrorType
        {
            Horizontal,
            Vertical,
            Both
        }
    }

    public partial class OsuModDifficultyAdjust : ModDifficultyAdjust
    {
        public DifficultyBindable CircleSize { get; } = new DifficultyBindable
        {
            Precision = 0.1f,
            MinValue = 0,
            MaxValue = 10,
            ExtendedMaxValue = 11,
            ReadCurrentFromDifficulty = diff => diff.CircleSize,
        };

        public DifficultyBindable ApproachRate { get; } = new DifficultyBindable
        {
            Precision = 0.1f,
            MinValue = 0,
            MaxValue = 10,
            ExtendedMinValue = -10,
            ExtendedMaxValue = 11,
            ReadCurrentFromDifficulty = diff => diff.ApproachRate,
        };

        protected override IEnumerable<DifficultyBindable> DifficultyBindables
        {
            get
            {
                foreach (var b in base.DifficultyBindables)
                    yield return b;

                yield return CircleSize;
                yield return ApproachRate;
            }
        }
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

namespace osu.Game.Rulesets.Mods
{
    // The settings of the difficulty adjust mods; the reflection over the setting properties in the
    // constructor is replaced by an explicit list, see DifficultyBindables.
    public abstract partial class ModDifficultyAdjust : IApplicableToDifficulty
    {
        public DifficultyBindable DrainRate { get; } = new DifficultyBindable
        {
            Precision = 0.1f,
            MinValue = 0,
            MaxValue = 10,
            ExtendedMaxValue = 11,
            ReadCurrentFromDifficulty = diff => diff.DrainRate,
        };

        public virtual DifficultyBindable OverallDifficulty { get; } = new DifficultyBindable
        {
            Precision = 0.1f,
            MinValue = 0,
            MaxValue = 10,
            ExtendedMaxValue = 11,
            ReadCurrentFromDifficulty = diff => diff.OverallDifficulty,
        };

        public BindableBool ExtendedLimits { get; } = new BindableBool();

        protected virtual IEnumerable<DifficultyBindable> DifficultyBindables
        {
            get
            {
                yield return DrainRate;
                yield return OverallDifficulty;
            }
        }

        protected ModDifficultyAdjust()
        {
            foreach (var diffAdjustBindable in DifficultyBindables)
                diffAdjustBindable.ExtendedLimits.BindTo(ExtendedLimits);
        }
    }

    public class DifficultyBindable : Bindable<float?>
    {
        /// <summary>
        /// Whether the extended limits should be applied to this bindable.
        /// </summary>
        public readonly BindableBool ExtendedLimits = new BindableBool();

        /// <summary>
        /// An internal numeric bindable to hold and propagate min/max/precision.
        /// The value of this bindable should not be set.
        /// </summary>
        internal readonly BindableFloat CurrentNumber = new BindableFloat
        {
            MinValue = 0,
            MaxValue = 10,
        };

        /// <summary>
        /// A function that can extract the current value of this setting from a beatmap difficulty for display purposes.
        /// </summary>
        public Func<IBeatmapDifficultyInfo, float>? ReadCurrentFromDifficulty;

        public float Precision
        {
            set => CurrentNumber.Precision = value;
        }

        private float minValue;

        public float MinValue
        {
            get => minValue;
            set
            {
                if (value == minValue)
                    return;

                minValue = value;
                updateExtents();
            }
        }

        private float maxValue = 10; // matches default max value of `CurrentNumber`

        public float MaxValue
        {
            get => maxValue;
            set
            {
                if (value == maxValue)
                    return;

                maxValue = value;
                updateExtents();
            }
        }

        private float? extendedMinValue;

        /// <summary>
        /// The minimum value to be used when extended limits are applied.
        /// </summary>
        public float? ExtendedMinValue
        {
            get => extendedMinValue;
            set
            {
                if (value == extendedMinValue)
                    return;

                extendedMinValue = value;
                updateExtents();
            }
        }

        private float? extendedMaxValue;

        /// <summary>
        /// The maximum value to be used when extended limits are applied.
        /// </summary>
        public float? ExtendedMaxValue
        {
            get => extendedMaxValue;
            set
            {
                if (value == extendedMaxValue)
                    return;

                extendedMaxValue = value;
                updateExtents();
            }
        }

        public DifficultyBindable()
            : this(null)
        {
        }

        public DifficultyBindable(float? defaultValue = null)
            : base(defaultValue)
        {
            ExtendedLimits.BindValueChanged(_ => updateExtents());
        }

        public override float? Value
        {
            get => base.Value;
            set
            {
                // Ensure that in the case serialisation runs in the wrong order (and limit extensions aren't applied yet) the deserialised value is still propagated.
                if (value != null)
                {
                    CurrentNumber.MinValue = Math.Clamp(MathF.Min(CurrentNumber.MinValue, value.Value), ExtendedMinValue ?? MinValue, MinValue);
                    CurrentNumber.MaxValue = Math.Clamp(MathF.Max(CurrentNumber.MaxValue, value.Value), MaxValue, ExtendedMaxValue ?? MaxValue);

                    base.Value = Math.Clamp(value.Value, CurrentNumber.MinValue, CurrentNumber.MaxValue);
                }
                else
                    base.Value = value;
            }
        }

        private void updateExtents()
        {
            CurrentNumber.MinValue = ExtendedLimits.Value && extendedMinValue != null ? extendedMinValue.Value : minValue;
            CurrentNumber.MaxValue = ExtendedLimits.Value && extendedMaxValue != null ? extendedMaxValue.Value : maxValue;
        }
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

        internal static double difficultyAdjustMultiplier(OsuModDifficultyAdjust difficultyAdjust, IBeatmapDifficultyInfo beatmapDifficulty)
        {
            double selectedCircleSize = difficultyAdjust.CircleSize.Value ?? beatmapDifficulty.CircleSize;
            double selectedDrainRate = difficultyAdjust.DrainRate.Value ?? beatmapDifficulty.DrainRate;
            double selectedOverallDifficulty = difficultyAdjust.OverallDifficulty.Value ?? beatmapDifficulty.OverallDifficulty;
            double selectedApproachRate = difficultyAdjust.ApproachRate.Value ?? beatmapDifficulty.ApproachRate;

            double csDifference = Math.Abs(selectedCircleSize - beatmapDifficulty.CircleSize);
            double hpDifference = Math.Abs(selectedDrainRate - beatmapDifficulty.DrainRate);
            double odDifference = Math.Abs(selectedOverallDifficulty - beatmapDifficulty.OverallDifficulty);
            double arDifference = Math.Abs(selectedApproachRate - beatmapDifficulty.ApproachRate);

            // Per parameter, reduce multiplier by 0.05x per 0.1 change.
            double csMultiplier = Math.Max(0.1, 1.0 - csDifference * 0.5);
            double hpMultiplier = Math.Max(0.1, 1.0 - hpDifference * 0.5);
            double odMultiplier = Math.Max(0.1, 1.0 - odDifference * 0.5);
            double arMultiplier = Math.Max(0.1, 1.0 - arDifference * 0.5);

            return Math.Max(0.1, csMultiplier * hpMultiplier * odMultiplier * arMultiplier);
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
        // Single<OsuModDifficultyAdjust>(hasMultiplier: 0.5);
        internal const double difficultyAdjustMultiplier = 0.5;

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
