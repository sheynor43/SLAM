// Writes slider path reference values computed by the verbatim lazer/osu-framework sources.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Text;
using osu.Game.Rulesets.Objects;
using osu.Game.Rulesets.Objects.Types;
using osuTK;

static class Program
{
    static readonly StringBuilder sb = new StringBuilder();

    static string F(float f) => BitConverter.SingleToUInt32Bits(f).ToString("x8");
    static string D(double d) => BitConverter.DoubleToUInt64Bits(d).ToString("x16");

    static ulong fnv(IEnumerable<ulong> words)
    {
        ulong h = 0xcbf29ce484222325;
        foreach (ulong w in words)
            for (int i = 0; i < 8; i++)
            {
                h ^= (w >> (8 * i)) & 0xff;
                h *= 0x100000001b3;
            }
        return h;
    }

    static ulong hashVertices(IEnumerable<Vector2> v) => fnv(v.Select(p => ((ulong)BitConverter.SingleToUInt32Bits(p.X) << 32) | BitConverter.SingleToUInt32Bits(p.Y)));

    static string typeCode(PathType? t)
    {
        if (t == null) return "-";
        var v = t.Value;
        return v.Type switch
        {
            SplineType.Linear => "L",
            SplineType.Catmull => "C",
            SplineType.PerfectCurve => "P",
            _ => v.Degree == null ? "B" : "B" + v.Degree.Value.ToString(CultureInfo.InvariantCulture),
        };
    }

    static void emit(string name, PathControlPoint[] points, double? expected, bool optimise)
    {
        var path = new SliderPath(points, expected, optimise);

        sb.Append("case ").Append(name).Append('\n');
        sb.Append("optimise ").Append(optimise ? 1 : 0).Append('\n');
        sb.Append("expected ").Append(expected == null ? "-" : D(expected.Value)).Append('\n');
        foreach (var p in points)
            sb.Append("point ").Append(F(p.Position.X)).Append(' ').Append(F(p.Position.Y)).Append(' ').Append(typeCode(p.Type)).Append('\n');
        sb.Append("vertices ").Append(path.CalculatedPath.Count).Append(' ').Append(hashVertices(path.CalculatedPath).ToString("x16")).Append('\n');
        sb.Append("lengths ").Append(path.CumulativeLength.Count).Append(' ').Append(fnv(path.CumulativeLength.Select(BitConverter.DoubleToUInt64Bits)).ToString("x16")).Append('\n');
        sb.Append("calculated ").Append(D(path.CalculatedDistance)).Append('\n');
        sb.Append("distance ").Append(D(path.Distance)).Append('\n');
        sb.Append("ends");
        foreach (double e in path.GetSegmentEnds()) sb.Append(' ').Append(D(e));
        sb.Append('\n');
        sb.Append("positions");
        for (int i = 0; i <= 16; i++)
        {
            var p = path.PositionAt(i / 16.0);
            sb.Append(' ').Append(F(p.X)).Append(' ').Append(F(p.Y));
        }
        sb.Append('\n');
        var part = new List<Vector2>();
        path.GetPathToProgress(part, 0.25, 0.75);
        sb.Append("partial ").Append(part.Count).Append(' ').Append(hashVertices(part).ToString("x16")).Append('\n');
        sb.Append("end\n");
    }

    static PathControlPoint[] pts(PathType type, params float[] xy)
    {
        var result = new PathControlPoint[xy.Length / 2];
        for (int i = 0; i < result.Length; i++)
            result[i] = new PathControlPoint(new Vector2(xy[2 * i], xy[2 * i + 1]), i == 0 ? type : null);
        return result;
    }

    static void probes()
    {
        // osuTK operator semantics the port relies on.
        var rng = new Random(7);
        for (int i = 0; i < 64; i++)
        {
            var v = new Vector2((float)(rng.NextDouble() * 2000 - 1000), (float)(rng.NextDouble() * 2000 - 1000));
            float s = (float)(rng.NextDouble() * 10 - 5);
            if (i == 0) s = 3;
            var div = v / s;
            var norm = v.Normalized();
            sb.Append("probe ").Append(F(v.X)).Append(' ').Append(F(v.Y)).Append(' ').Append(F(s))
              .Append(' ').Append(F(div.X)).Append(' ').Append(F(div.Y))
              .Append(' ').Append(F(norm.X)).Append(' ').Append(F(norm.Y))
              .Append(' ').Append(F(v.Length)).Append(' ').Append(F(Vector2.Distance(v, norm)))
              .Append('\n');
        }
    }

    static void handpicked()
    {
        foreach (bool opt in new[] { false, true })
        {
            string o = opt ? "-opt" : "";
            emit("empty" + o, Array.Empty<PathControlPoint>(), 100, opt);
            emit("single" + o, pts(PathType.LINEAR, 5, 5), 100, opt);
            emit("linear" + o, pts(PathType.LINEAR, 0, 0, 30, 40, 30, 140), null, opt);
            emit("linear-trim" + o, pts(PathType.LINEAR, 0, 0, 100, 0, 100, 100), 50, opt);
            emit("linear-extend" + o, pts(PathType.LINEAR, 0, 0, 33, 71), 150.5, opt);
            emit("linear-dup-end" + o, pts(PathType.LINEAR, 0, 0, 100, 0, 100, 0), 150, opt);
            emit("linear-zero" + o, pts(PathType.LINEAR, 0, 0, 100, 0), 0, opt);
            emit("linear-negative" + o, pts(PathType.LINEAR, 0, 0, 100, 0), -20, opt);
            emit("perfect" + o, pts(PathType.PERFECT_CURVE, 0, 0, 100, 100, 200, 0), 300, opt);
            emit("perfect-cw" + o, pts(PathType.PERFECT_CURVE, 0, 0, 57, -23, 140, 12), 160, opt);
            emit("perfect-collinear" + o, pts(PathType.PERFECT_CURVE, 0, 0, 50, 50, 100, 100), 141, opt);
            emit("perfect-huge" + o, pts(PathType.PERFECT_CURVE, 0, 0, 5000, 0.5f, 10000, 0), 10000, opt);
            emit("perfect-tiny" + o, pts(PathType.PERFECT_CURVE, 0, 0, 0.02f, 0.01f, 0.04f, 0), null, opt);
            emit("perfect-four" + o, pts(PathType.PERFECT_CURVE, 0, 0, 50, 50, 100, 0, 150, 50), 200, opt);
            emit("bezier" + o, pts(PathType.BEZIER, 0, 0, 120, -80, 200, 40, 320, 10), 360, opt);
            emit("bspline3" + o, pts(PathType.BSpline(3), 0, 0, 1, 0, 1, -1, -1, -1, -1, 1, 3, 2, 3, 0), null, opt);
            emit("bspline4-framework-test" + o, pts(PathType.BSpline(4), 0, 0, 1, 0, 1, -1, -1, -1, -1, 1, 3, 2, 3, 0), null, opt);
            emit("bspline2-big" + o, pts(PathType.BSpline(2), 0, 0, 100, -50, 160, 90, 40, 200, -80, 130, -20, 10), 500, opt);
            emit("catmull" + o, pts(PathType.CATMULL, 0, 0, 0, 0, 100, 50, 100, 50, 200, 0), 260, opt);
            emit("catmull-straight" + o, pts(PathType.CATMULL, 0, 386, 0, 386, 34, 673), null, opt);
            var multi = new[]
            {
                new PathControlPoint(new Vector2(0, 0), PathType.BEZIER),
                new PathControlPoint(new Vector2(60, -40), null),
                new PathControlPoint(new Vector2(120, 0), PathType.LINEAR),
                new PathControlPoint(new Vector2(180, 30), PathType.PERFECT_CURVE),
                new PathControlPoint(new Vector2(220, 90), null),
                new PathControlPoint(new Vector2(200, 160), PathType.CATMULL),
                new PathControlPoint(new Vector2(150, 200), null),
                new PathControlPoint(new Vector2(150, 200), null),
                new PathControlPoint(new Vector2(90, 230), null),
            };
            emit("multi-segment" + o, multi, 520, opt);
            emit("multi-segment-trim" + o, multi, 150, opt);
        }
    }

    static PathType randomType(Random rng) => rng.Next(6) switch
    {
        0 => PathType.LINEAR,
        1 => PathType.BEZIER,
        2 => PathType.CATMULL,
        3 => PathType.PERFECT_CURVE,
        4 => PathType.PERFECT_CURVE,
        _ => PathType.BSpline(rng.Next(1, 6)),
    };

    static float coordinate(Random rng) => rng.Next(4) == 0
        ? (float)(rng.NextDouble() * 900 - 300)
        : rng.Next(-300, 600);

    static void random(int count)
    {
        var rng = new Random(20261010);
        for (int c = 0; c < count; c++)
        {
            int n = rng.Next(2, 9);
            var points = new PathControlPoint[n];
            for (int i = 0; i < n; i++)
            {
                Vector2 pos = i == 0 ? (rng.Next(3) == 0 ? new Vector2(coordinate(rng), coordinate(rng)) : Vector2.Zero)
                    : rng.Next(5) == 0 ? points[i - 1].Position
                    : new Vector2(coordinate(rng), coordinate(rng));
                PathType? type = i == 0 || rng.Next(6) == 0 ? randomType(rng) : null;
                points[i] = new PathControlPoint(pos, type);
            }
            // A three-point perfect curve is the most common slider in real maps.
            if (rng.Next(4) == 0)
                points = new[] { new PathControlPoint(Vector2.Zero, PathType.PERFECT_CURVE), new PathControlPoint(new Vector2(coordinate(rng), coordinate(rng)), null), new PathControlPoint(new Vector2(coordinate(rng), coordinate(rng)), null) };

            bool optimise = rng.Next(2) == 0;
            double? expected = rng.Next(5) switch
            {
                0 => null,
                1 => Math.Round(rng.NextDouble() * 1000, 2),
                _ => new SliderPath(points, null, optimise).Distance * (0.8 + rng.NextDouble() * 0.4),
            };
            emit("random-" + c.ToString(CultureInfo.InvariantCulture), points, expected, optimise);
        }
    }

    static void Main(string[] args)
    {
        sb.Append("# Generated from osu!lazer 2026.1005.0-lazer SliderPath.cs and osu-framework 2026.921.1 PathApproximator.cs, CircularArcProperties.cs on osuTK 1.0.211, .NET ").Append(Environment.Version).Append('\n');
        probes();
        handpicked();
        random(400);
        File.WriteAllText(args[0], sb.ToString());
        if (args.Length > 1)
            File.WriteAllText(args[1], Nested.Generate());
        if (args.Length > 2)
            File.WriteAllText(args[2], Stacking.Generate());
    }
}
