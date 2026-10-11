// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu/osu.Game/Beatmaps/Timing/BreakPeriod.cs (lines 1-92 without lines 69-69); Contains is omitted (it needs BreakOverlay). Doc comments are removed. See README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
using System;

namespace osu.Game.Beatmaps.Timing
{
    public class BreakPeriod : IEquatable<BreakPeriod>, IComparable<BreakPeriod>
    {
        public const double GAP_BEFORE_BREAK = 200;

        public const double GAP_AFTER_BREAK = 450;

        public const double MIN_BREAK_DURATION = 650;

        public const double MIN_GAP_DURATION = GAP_BEFORE_BREAK + MIN_BREAK_DURATION + GAP_AFTER_BREAK;

        public double StartTime { get; }

        public double EndTime { get; }

        public double Duration => EndTime - StartTime;

        public bool HasEffect => Duration >= MIN_BREAK_DURATION;

        public BreakPeriod(double startTime, double endTime)
        {
            StartTime = startTime;
            EndTime = endTime;
        }

        public bool Intersects(BreakPeriod other) => StartTime <= other.EndTime && EndTime >= other.StartTime;

        public virtual bool Equals(BreakPeriod? other) =>
            other != null
            && StartTime == other.StartTime
            && EndTime == other.EndTime;

        public override int GetHashCode() => HashCode.Combine(StartTime, EndTime);

        public int CompareTo(BreakPeriod? other)
        {
            if (ReferenceEquals(this, other)) return 0;
            if (ReferenceEquals(null, other)) return 1;

            int result = StartTime.CompareTo(other.StartTime);
            if (result != 0)
                return result;

            return EndTime.CompareTo(other.EndTime);
        }
    }
}
