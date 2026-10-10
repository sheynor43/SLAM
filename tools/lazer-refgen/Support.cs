// Minimal stand-ins for the lazer/framework infrastructure around the verbatim sources.
using System;
using System.Collections.Generic;
using osuTK;

namespace osu.Framework.Utils
{
    public static class Precision
    {
        // Verbatim from osu.Framework/Utils/Precision.cs (2026.921.1).
        public const float FLOAT_EPSILON = 1e-3f;
        public const double DOUBLE_EPSILON = 1e-7;
        public static bool AlmostEquals(float value1, float value2, float acceptableDifference = FLOAT_EPSILON) => Math.Abs(value1 - value2) <= acceptableDifference;
        public static bool AlmostEquals(double value1, double value2, double acceptableDifference = DOUBLE_EPSILON) => Math.Abs(value1 - value2) <= acceptableDifference;
    }
}

namespace osu.Game.Rulesets.Objects.Types
{
    public enum SplineType { Catmull, BSpline, Linear, PerfectCurve }

    public readonly struct PathType
    {
        public static readonly PathType CATMULL = new PathType(SplineType.Catmull);
        public static readonly PathType BEZIER = new PathType(SplineType.BSpline);
        public static readonly PathType LINEAR = new PathType(SplineType.Linear);
        public static readonly PathType PERFECT_CURVE = new PathType(SplineType.PerfectCurve);
        public SplineType Type { get; init; }
        public int? Degree { get; init; }
        public PathType(SplineType splineType) { Type = splineType; Degree = null; }
        public static PathType BSpline(int degree) => new PathType { Type = SplineType.BSpline, Degree = degree };
    }
}

namespace osu.Game.Rulesets.Objects
{
    using osu.Game.Rulesets.Objects.Types;

    public class PathControlPoint
    {
        public Vector2 Position;
        public PathType? Type;
        public PathControlPoint(Vector2 position, PathType? type) { Position = position; Type = type; }
    }

    public class ValueHolder<T> { public T Value; }

    public partial class SliderPath
    {
        public readonly List<PathControlPoint> ControlPoints = new List<PathControlPoint>();
        public readonly ValueHolder<double?> ExpectedDistance = new ValueHolder<double?>();
        public bool OptimiseCatmull;

        private readonly List<Vector2> calculatedPath = new List<Vector2>();
        private readonly List<double> cumulativeLength = new List<double>();
        private double optimisedLength;
        private double calculatedLength;
        private readonly List<int> segmentEnds = new List<int>();
        private double[] segmentEndDistances = Array.Empty<double>();

        public SliderPath(PathControlPoint[] controlPoints, double? expectedDistance, bool optimiseCatmull)
        {
            ControlPoints.AddRange(controlPoints);
            ExpectedDistance.Value = expectedDistance;
            OptimiseCatmull = optimiseCatmull;
            calculatePath();
            calculateLength();
        }

        public double Distance => cumulativeLength.Count == 0 ? 0 : cumulativeLength[^1];
        public double CalculatedDistance => calculatedLength;
        public IReadOnlyList<Vector2> CalculatedPath => calculatedPath;
        public IReadOnlyList<double> CumulativeLength => cumulativeLength;
        public IEnumerable<double> GetSegmentEnds() { foreach (double d in segmentEndDistances) yield return d / Distance; }

        public Vector2 PositionAt(double progress)
        {
            double d = progressToDistance(progress);
            return interpolateVertices(indexOfDistance(d), d);
        }

        public void GetPathToProgress(List<Vector2> path, double p0, double p1)
        {
            double d0 = progressToDistance(p0);
            double d1 = progressToDistance(p1);
            path.Clear();
            int i = 0;
            for (; i < calculatedPath.Count && cumulativeLength[i] < d0; ++i) { }
            path.Add(interpolateVertices(i, d0));
            for (; i < calculatedPath.Count && cumulativeLength[i] <= d1; ++i)
                path.Add(calculatedPath[i]);
            path.Add(interpolateVertices(i, d1));
        }
    }
}
