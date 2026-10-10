// Extracted verbatim from osu/osu.Game/Rulesets/Objects/SliderPath.cs (osu!lazer 2026.1005.0-lazer, lines 287-522 wrapped in a partial class); only the parts slider paths need, see README.md.
using osu.Game.Rulesets.Objects.Types; using System; using System.Linq; using System.Diagnostics; using System.Collections.Generic; using osu.Framework.Utils; using osuTK; namespace osu.Game.Rulesets.Objects { public partial class SliderPath {
        private void calculatePath()
        {
            calculatedPath.Clear();
            segmentEnds.Clear();
            optimisedLength = 0;

            if (ControlPoints.Count == 0)
                return;

            Vector2[] vertices = new Vector2[ControlPoints.Count];
            for (int i = 0; i < ControlPoints.Count; i++)
                vertices[i] = ControlPoints[i].Position;

            int start = 0;

            for (int i = 0; i < ControlPoints.Count; i++)
            {
                if (ControlPoints[i].Type == null && i < ControlPoints.Count - 1)
                    continue;

                // The current vertex ends the segment
                var segmentVertices = vertices.AsSpan().Slice(start, i - start + 1);
                var segmentType = ControlPoints[start].Type ?? PathType.LINEAR;

                // No need to calculate path when there is only 1 vertex
                if (segmentVertices.Length == 1)
                    calculatedPath.Add(segmentVertices[0]);
                else if (segmentVertices.Length > 1)
                {
                    List<Vector2> subPath = calculateSubPath(segmentVertices, segmentType);

                    // Skip the first vertex if it is the same as the last vertex from the previous segment
                    bool skipFirst = calculatedPath.Count > 0 && subPath.Count > 0 && calculatedPath.Last() == subPath[0];

                    for (int j = skipFirst ? 1 : 0; j < subPath.Count; j++)
                        calculatedPath.Add(subPath[j]);
                }

                if (i > 0)
                {
                    // Remember the index of the segment end
                    segmentEnds.Add(calculatedPath.Count - 1);
                }

                // Start the new segment at the current vertex
                start = i;
            }
        }

        private List<Vector2> calculateSubPath(ReadOnlySpan<Vector2> subControlPoints, PathType type)
        {
            switch (type.Type)
            {
                case SplineType.Linear:
                    return PathApproximator.LinearToPiecewiseLinear(subControlPoints);

                case SplineType.PerfectCurve:
                {
                    if (subControlPoints.Length != 3)
                        break;

                    CircularArcProperties circularArcProperties = new CircularArcProperties(subControlPoints);

                    // `PathApproximator` will already internally revert to B-spline if the arc isn't valid.
                    if (!circularArcProperties.IsValid)
                        break;

                    // taken from https://github.com/ppy/osu-framework/blob/1201e641699a1d50d2f6f9295192dad6263d5820/osu.Framework/Utils/PathApproximator.cs#L181-L186
                    int subPoints = (2f * circularArcProperties.Radius <= 0.1f) ? 2 : Math.Max(2, (int)Math.Ceiling(circularArcProperties.ThetaRange / (2.0 * Math.Acos(1f - (0.1f / circularArcProperties.Radius)))));

                    // 1000 subpoints requires an arc length of at least ~120 thousand to occur
                    // See here for calculations https://www.desmos.com/calculator/umj6jvmcz7
                    if (subPoints >= 1000)
                        break;

                    List<Vector2> subPath = PathApproximator.CircularArcToPiecewiseLinear(subControlPoints);

                    // If for some reason a circular arc could not be fit to the 3 given points, fall back to a numerically stable bezier approximation.
                    if (subPath.Count == 0)
                        break;

                    return subPath;
                }

                case SplineType.Catmull:
                {
                    List<Vector2> subPath = PathApproximator.CatmullToPiecewiseLinear(subControlPoints);

                    if (!OptimiseCatmull)
                        return subPath;

                    // At draw time, osu!stable optimises paths by only keeping piecewise segments that are 6px apart.
                    // For the most part we don't care about this optimisation, and its additional heuristics are hard to reproduce in every implementation.
                    //
                    // However, it matters for Catmull paths which form "bulbs" around sequential knots with identical positions,
                    // so we'll apply a very basic form of the optimisation here and return a length representing the optimised portion.
                    // The returned length is important so that the optimisation doesn't cause the path to get extended to match the value of ExpectedDistance.

                    List<Vector2> optimisedPath = new List<Vector2>(subPath.Count);

                    Vector2? lastStart = null;
                    double lengthRemovedSinceStart = 0;

                    for (int i = 0; i < subPath.Count; i++)
                    {
                        if (lastStart == null)
                        {
                            optimisedPath.Add(subPath[i]);
                            lastStart = subPath[i];
                            continue;
                        }

                        Debug.Assert(i > 0);

                        double distFromStart = Vector2.Distance(lastStart.Value, subPath[i]);
                        lengthRemovedSinceStart += Vector2.Distance(subPath[i - 1], subPath[i]);

                        // See PathApproximator.catmull_detail.
                        const int catmull_detail = 50;
                        const int catmull_segment_length = catmull_detail * 2;

                        // Either 6px from the start, the last vertex at every knot, or the end of the path.
                        if (distFromStart > 6 || (i + 1) % catmull_segment_length == 0 || i == subPath.Count - 1)
                        {
                            optimisedPath.Add(subPath[i]);
                            optimisedLength += lengthRemovedSinceStart - distFromStart;

                            lastStart = null;
                            lengthRemovedSinceStart = 0;
                        }
                    }

                    return optimisedPath;
                }
            }

            return PathApproximator.BSplineToPiecewiseLinear(subControlPoints, type.Degree ?? subControlPoints.Length);
        }

        private void calculateLength()
        {
            calculatedLength = optimisedLength;
            cumulativeLength.Clear();
            cumulativeLength.Add(0);

            for (int i = 0; i < calculatedPath.Count - 1; i++)
            {
                Vector2 diff = calculatedPath[i + 1] - calculatedPath[i];
                calculatedLength += diff.Length;
                cumulativeLength.Add(calculatedLength);
            }

            // Store the distances of the segment ends now, because after shortening the indices may be out of range
            segmentEndDistances = new double[segmentEnds.Count];

            for (int i = 0; i < segmentEnds.Count; i++)
            {
                segmentEndDistances[i] = cumulativeLength[segmentEnds[i]];
            }

            if (ExpectedDistance.Value is double expectedDistance && calculatedLength != expectedDistance)
            {
                // In osu-stable, if the last two path points of a slider are equal, extension is not performed.
                if (calculatedPath.Count >= 2 && calculatedPath[^1] == calculatedPath[^2] && expectedDistance > calculatedLength)
                {
                    cumulativeLength.Add(calculatedLength);
                    return;
                }

                // The last length is always incorrect
                cumulativeLength.RemoveAt(cumulativeLength.Count - 1);

                int pathEndIndex = calculatedPath.Count - 1;

                if (calculatedLength > expectedDistance)
                {
                    // The path will be shortened further, in which case we should trim any more unnecessary lengths and their associated path segments
                    while (cumulativeLength.Count > 0 && cumulativeLength[^1] >= expectedDistance)
                    {
                        cumulativeLength.RemoveAt(cumulativeLength.Count - 1);
                        calculatedPath.RemoveAt(pathEndIndex--);
                    }
                }

                if (pathEndIndex <= 0)
                {
                    // The expected distance is negative or zero
                    // TODO: Perhaps negative path lengths should be disallowed altogether
                    cumulativeLength.Add(0);
                    return;
                }

                // The direction of the segment to shorten or lengthen
                Vector2 dir = (calculatedPath[pathEndIndex] - calculatedPath[pathEndIndex - 1]).Normalized();

                calculatedPath[pathEndIndex] = calculatedPath[pathEndIndex - 1] + dir * (float)(expectedDistance - cumulativeLength[^1]);
                cumulativeLength.Add(expectedDistance);
            }
        }

        private int indexOfDistance(double d)
        {
            int i = cumulativeLength.BinarySearch(d);
            if (i < 0) i = ~i;

            return i;
        }

        private double progressToDistance(double progress)
        {
            return Math.Clamp(progress, 0, 1) * Distance;
        }

        private Vector2 interpolateVertices(int i, double d)
        {
            if (calculatedPath.Count == 0)
                return Vector2.Zero;

            if (i <= 0)
                return calculatedPath.First();
            if (i >= calculatedPath.Count)
                return calculatedPath.Last();

            Vector2 p0 = calculatedPath[i - 1];
            Vector2 p1 = calculatedPath[i];

            double d0 = cumulativeLength[i - 1];
            double d1 = cumulativeLength[i];

            // Avoid division by and almost-zero number in case two points are extremely close to each other.
            if (Precision.AlmostEquals(d0, d1))
                return p0;

            double w = (d - d0) / (d1 - d0);
            return p0 + (p1 - p0) * (float)w;
        }
} }
