// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu/osu.Game/Utils/PeriodTracker.cs (lines 1-89); Doc comments are removed. See README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
using System;
using System.Collections.Generic;
using System.Diagnostics.CodeAnalysis;
using System.Linq;

namespace osu.Game.Utils
{
    public class PeriodTracker
    {
        private readonly List<Period> periods;
        private int nearestIndex;

        public PeriodTracker(IEnumerable<Period> periods)
        {
            this.periods = periods.OrderBy(period => period.Start).ToList();
        }

        public bool IsInAny(double time) => IsInAny(time, out _);

        public bool IsInAny(double time, [NotNullWhen(true)] out Period? period)
        {
            period = null;

            if (periods.Count == 0)
                return false;

            if (time > periods[nearestIndex].End)
            {
                while (time > periods[nearestIndex].End && nearestIndex < periods.Count - 1)
                    nearestIndex++;
            }
            else
            {
                while (time < periods[nearestIndex].Start && nearestIndex > 0)
                    nearestIndex--;
            }

            var nearest = periods[nearestIndex];
            bool isInAny = time >= nearest.Start && time <= nearest.End;

            if (isInAny)
            {
                period = nearest;
                return true;
            }

            return false;
        }
    }

    public readonly struct Period
    {
        public readonly double Start;

        public readonly double End;

        public double Duration => End - Start;

        public Period(double start, double end)
        {
            if (start >= end)
                throw new ArgumentException($"Invalid period provided, {nameof(start)} must be less than {nameof(end)}");

            Start = start;
            End = end;
        }
    }
}
