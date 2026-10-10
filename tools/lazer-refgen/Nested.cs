// Writes reference values of slider defaults and nested objects, of the slider event generator
// and of List<T>.Sort, computed by the verbatim lazer sources on the real .NET runtime.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text;
using osu.Game.Beatmaps;
using osu.Game.Beatmaps.ControlPoints;
using osu.Game.Rulesets.Objects;
using osu.Game.Rulesets.Objects.Types;
using osu.Game.Rulesets.Osu.Objects;
using osuTK;

class Difficulty : IBeatmapDifficultyInfo
{
    public float DrainRate { get; set; } = 5;
    public float CircleSize { get; set; } = 5;
    public float OverallDifficulty { get; set; } = 5;
    public float ApproachRate { get; set; } = 5;
    public double SliderMultiplier { get; set; } = 1.4;
    public double SliderTickRate { get; set; } = 1;
}

static class Nested
{
    static readonly StringBuilder sb = new StringBuilder();

    static string F(float f) => BitConverter.SingleToUInt32Bits(f).ToString("x8");
    static string D(double d) => BitConverter.DoubleToUInt64Bits(d).ToString("x16");
    static string I(int i) => i.ToString(CultureInfo.InvariantCulture);

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

    const int full_listing = 24;
    const int listing_edge = 6;

    static ulong fnv(string text)
    {
        ulong h = 0xcbf29ce484222325;
        foreach (byte b in Encoding.UTF8.GetBytes(text))
        {
            h ^= b;
            h *= 0x100000001b3;
        }
        return h;
    }

    // Lists longer than full_listing keep only their first and last listing_edge lines; the
    // count line holds an FNV-1a hash of all lines (each with its '\n').
    static void appendList(string tag, List<string> lines)
    {
        sb.Append("count ").Append(I(lines.Count)).Append(' ').Append(fnv(string.Concat(lines.Select(l => l + "\n"))).ToString("x16")).Append('\n');
        for (int i = 0; i < lines.Count; i++)
        {
            if (lines.Count > full_listing && i >= listing_edge && i < lines.Count - listing_edge)
                continue;
            sb.Append(tag).Append(' ').Append(I(i)).Append(' ').Append(lines[i]).Append('\n');
        }
    }

    struct Case
    {
        public double StartTime, BeatLength, SliderVelocity;
        public int FormatVersion;
        public bool GenerateTicks;
        public Difficulty Difficulty;
        public Vector2 Position;
        public int RepeatCount;
        public PathControlPoint[] Points;
        public double? Expected;
        public bool Optimise;
    }

    static void emit(string name, Case c)
    {
        var slider = new Slider
        {
            StartTime = c.StartTime,
            Position = c.Position,
            RepeatCount = c.RepeatCount,
            Path = new SliderPath(c.Points, c.Expected, c.Optimise),
            SliderVelocityMultiplier = c.SliderVelocity,
            GenerateTicks = c.GenerateTicks,
            // As OsuBeatmapConverter.
            TickDistanceMultiplier = c.FormatVersion < 8 ? 1f / c.SliderVelocity : 1,
        };
        var cpi = new ControlPointInfo();
        cpi.Timing.BeatLength = c.BeatLength;
        slider.ApplyDefaults(cpi, c.Difficulty);

        var d = c.Difficulty;
        sb.Append("slider ").Append(name).Append('\n');
        sb.Append("input ").Append(D(c.StartTime)).Append(' ').Append(D(c.BeatLength)).Append(' ').Append(D(c.SliderVelocity))
          .Append(' ').Append(I(c.FormatVersion)).Append(' ').Append(c.GenerateTicks ? 1 : 0)
          .Append(' ').Append(D(d.SliderMultiplier)).Append(' ').Append(D(d.SliderTickRate)).Append(' ').Append(F(d.ApproachRate)).Append(' ').Append(F(d.CircleSize))
          .Append(' ').Append(F(c.Position.X)).Append(' ').Append(F(c.Position.Y)).Append(' ').Append(I(c.RepeatCount))
          .Append(' ').Append(c.Expected == null ? "-" : D(c.Expected.Value)).Append(' ').Append(c.Optimise ? 1 : 0).Append('\n');
        foreach (var p in c.Points)
            sb.Append("point ").Append(F(p.Position.X)).Append(' ').Append(F(p.Position.Y)).Append(' ').Append(typeCode(p.Type)).Append('\n');
        sb.Append("result ").Append(D(slider.Velocity)).Append(' ').Append(D(slider.TickDistance)).Append(' ').Append(D(slider.TickDistanceMultiplier))
          .Append(' ').Append(D(slider.EndTime)).Append(' ').Append(D(slider.SpanDuration))
          .Append(' ').Append(D(slider.TimePreempt)).Append(' ').Append(D(slider.TimeFadeIn)).Append(' ').Append(F(slider.Scale))
          .Append(' ').Append(F(slider.EndPosition.X)).Append(' ').Append(F(slider.EndPosition.Y)).Append('\n');
        var nestedLines = new List<string>();
        foreach (var h in slider.NestedHitObjects)
        {
            var o = (OsuHitObject)h;
            string kind;
            string extra;
            switch (h)
            {
                case SliderTick t:
                    kind = "tick";
                    extra = I(t.SpanIndex) + " " + D(t.SpanStartTime) + " " + D(t.PathProgress);
                    break;
                case SliderRepeat r:
                    kind = "repeat";
                    extra = I(r.RepeatIndex) + " " + D(r.PathProgress);
                    break;
                case SliderTailCircle tc:
                    kind = "tail";
                    extra = I(tc.RepeatIndex);
                    break;
                case SliderHeadCircle:
                    kind = "head";
                    extra = "";
                    break;
                default:
                    throw new InvalidOperationException();
            }
            string nestedLine = kind + " " + D(o.StartTime) + " " + F(o.Position.X) + " " + F(o.Position.Y)
                                + " " + D(o.TimePreempt) + " " + D(o.TimeFadeIn) + " " + F(o.Scale);
            if (extra.Length > 0) nestedLine += " " + extra;
            nestedLines.Add(nestedLine);
        }
        appendList("nested", nestedLines);
        sb.Append("end\n");
    }

    static PathControlPoint[] pts(PathType type, params float[] xy)
    {
        var result = new PathControlPoint[xy.Length / 2];
        for (int i = 0; i < result.Length; i++)
            result[i] = new PathControlPoint(new Vector2(xy[2 * i], xy[2 * i + 1]), i == 0 ? type : null);
        return result;
    }

    static Case basic(PathControlPoint[] points, double? expected, int repeats = 0) => new Case
    {
        StartTime = 1000,
        BeatLength = 60000 / 180.0,
        SliderVelocity = 1,
        FormatVersion = 14,
        GenerateTicks = true,
        Difficulty = new Difficulty(),
        Position = new Vector2(256, 192),
        RepeatCount = repeats,
        Points = points,
        Expected = expected,
        Optimise = false,
    };

    static void handpicked()
    {
        var line = pts(PathType.LINEAR, 0, 0, 200, 0);
        emit("linear", basic(line, 200));
        emit("linear-repeats", basic(line, 200, 3));
        foreach (double rate in new[] { 0.5, 1, 2, 3, 4, 8 })
        {
            var c = basic(line, 200, 1);
            c.Difficulty = new Difficulty { SliderTickRate = rate };
            emit("tick-rate-" + rate.ToString(CultureInfo.InvariantCulture), c);
        }
        foreach (double sv in new[] { 0.1, 0.25, 0.5, 0.75, 1.3, 2, 3.7, 10 })
        {
            var c = basic(line, 200, 2);
            c.SliderVelocity = sv;
            emit("sv-" + sv.ToString(CultureInfo.InvariantCulture), c);
            c.FormatVersion = 7;
            emit("sv-v7-" + sv.ToString(CultureInfo.InvariantCulture), c);
        }
        foreach (double sm in new[] { 0.4, 1, 1.4, 1.8, 2.6, 3.6 })
        {
            var c = basic(line, 200, 1);
            c.Difficulty = new Difficulty { SliderMultiplier = sm, SliderTickRate = 2 };
            emit("multiplier-" + sm.ToString(CultureInfo.InvariantCulture), c);
        }
        foreach (double beat in new[] { 6, 100, 333.33333333333331, 500, 1000, 60000 })
        {
            var c = basic(line, 200, 1);
            c.BeatLength = beat;
            emit("beat-" + beat.ToString(CultureInfo.InvariantCulture), c);
        }
        foreach (float ar in new[] { 0f, 3.5f, 5f, 8f, 9.3f, 10f, 11f, -2f })
        {
            var c = basic(line, 200, 2);
            c.Difficulty = new Difficulty { ApproachRate = ar };
            emit("ar-" + ar.ToString(CultureInfo.InvariantCulture), c);
        }
        foreach (float cs in new[] { 0f, 2f, 4f, 4.2f, 5f, 7f, 10f })
        {
            var c = basic(line, 200);
            c.Difficulty = new Difficulty { CircleSize = cs };
            emit("cs-" + cs.ToString(CultureInfo.InvariantCulture), c);
        }
        {
            var c = basic(line, 200, 2);
            c.GenerateTicks = false;
            emit("no-ticks", c);
        }
        emit("zero-length", basic(pts(PathType.LINEAR, 0, 0, 0, 0), 0));
        // Zero distance with many repeats: every nested object has the same time, which
        // exercises the unstable sort.
        emit("zero-length-repeats", basic(pts(PathType.LINEAR, 0, 0, 0, 0), 0, 30));
        emit("tiny-repeats", basic(pts(PathType.LINEAR, 0, 0, 1, 0), 1, 20));
        {
            // Times so large that tiny spans round to equal times.
            var c = basic(pts(PathType.LINEAR, 0, 0, 1, 0), 1e-6, 25);
            c.StartTime = 1e9;
            emit("rounded-equal-times", c);
        }
        emit("short-under-72ms", basic(pts(PathType.LINEAR, 0, 0, 20, 0), 20, 1));
        emit("over-max-length", basic(pts(PathType.LINEAR, 0, 0, 512, 0), 150000));
        emit("perfect", basic(pts(PathType.PERFECT_CURVE, 0, 0, 100, 100, 200, 0), 300, 1));
        emit("bezier-even-spans", basic(pts(PathType.BEZIER, 0, 0, 120, -80, 200, 40, 320, 10), 360, 3));
        {
            var c = basic(pts(PathType.CATMULL, 0, 0, 0, 0, 100, 50, 100, 50, 200, 0), 260, 1);
            c.Optimise = true;
            emit("catmull-optimised", c);
        }
        {
            var c = basic(line, 200, 1);
            c.StartTime = -500.5;
            emit("negative-start", c);
        }
    }

    static PathType randomType(Random rng) => rng.Next(5) switch
    {
        0 => PathType.LINEAR,
        1 => PathType.BEZIER,
        2 => PathType.CATMULL,
        _ => PathType.PERFECT_CURVE,
    };

    static void random(int count)
    {
        var rng = new Random(20261011);
        double[] rates = { 0.5, 1, 1, 2, 2, 3, 4, 8 };
        for (int i = 0; i < count; i++)
        {
            int n = rng.Next(2, 6);
            var points = new PathControlPoint[n];
            for (int k = 0; k < n; k++)
            {
                Vector2 pos = k == 0 ? Vector2.Zero : new Vector2(rng.Next(-300, 300), rng.Next(-300, 300));
                points[k] = new PathControlPoint(pos, k == 0 ? randomType(rng) : null);
            }
            bool optimise = rng.Next(2) == 0;
            double raw = new SliderPath(points, null, optimise).Distance;
            double? expected = rng.Next(4) == 0 ? null : Math.Round(raw * (0.5 + rng.NextDouble() * 0.6), rng.Next(3));

            var c = new Case
            {
                StartTime = Math.Round(rng.NextDouble() * 300000, rng.Next(3)),
                BeatLength = rng.Next(3) == 0 ? 60000 / Math.Round(60 + rng.NextDouble() * 300, 2) : Math.Round(150 + rng.NextDouble() * 900, rng.Next(4)),
                SliderVelocity = rng.Next(3) == 0 ? 1 : Math.Clamp(-100 / (double)-rng.Next(10, 1000), 0.1, 10),
                FormatVersion = rng.Next(5) == 0 ? rng.Next(3, 8) : 14,
                GenerateTicks = rng.Next(15) != 0,
                Difficulty = new Difficulty
                {
                    ApproachRate = (float)Math.Round(rng.NextDouble() * 10, 1),
                    CircleSize = (float)Math.Round(rng.NextDouble() * 10, 1),
                    SliderMultiplier = Math.Round(0.4 + rng.NextDouble() * 3.2, 2),
                    SliderTickRate = rates[rng.Next(rates.Length)],
                },
                Position = new Vector2(rng.Next(0, 512), rng.Next(0, 384)),
                RepeatCount = rng.Next(4) == 0 ? rng.Next(1, 8) : 0,
                Points = points,
                Expected = expected,
                Optimise = optimise,
            };
            emit("random-" + I(i), c);
        }
    }

    static void events()
    {
        void gen(string name, double start, double span, double velocity, double tick, double total, int spans)
        {
            sb.Append("events ").Append(name).Append(' ').Append(D(start)).Append(' ').Append(D(span)).Append(' ').Append(D(velocity))
              .Append(' ').Append(D(tick)).Append(' ').Append(D(total)).Append(' ').Append(I(spans)).Append('\n');
            var eventLines = SliderEventGenerator.Generate(start, span, velocity, tick, total, spans)
                .Select(e => e.Type.ToString() + " " + D(e.Time) + " " + I(e.SpanIndex) + " " + D(e.SpanStartTime) + " " + D(e.PathProgress))
                .ToList();
            appendList("event", eventLines);
            sb.Append("end\n");
        }

        gen("nan-distance", 0, 1000, 1, 100, double.NaN, 2);
        gen("nan-tick", 0, 1000, 1, double.NaN, 1000, 2);
        gen("infinite-tick", 0, 1000, 1, double.PositiveInfinity, 1000, 3);
        gen("zero-spans", 0, 1000, 1, 100, 1000, 0);
        gen("negative-spans", 0, 1000, 1, 100, 1000, -3);
        gen("zero-span-duration", 50, 0, 1, 10, 1000, 4);
        gen("over-max-length", 0, 1e7, 0.01, 700.3, 2e5, 2);
        gen("third-ticks", 12.5, 333.3, 0.42, 33.33, 140, 3);
    }

    // Sorts with List<T>.Sort and prints the keys, the original indices in sorted order and
    // the number and FNV-1a hash of the compared index pairs ("a,b;" each), which pins down
    // the whole path of the algorithm, ties or not.
    static void emitSort(List<(double key, int index)> items)
    {
        sb.Append("sort ").Append(string.Join(' ', items.Select(x => I((int)x.key)))).Append('\n');
        var compared = new StringBuilder();
        int count = 0;
        items.Sort((a, b) =>
        {
            compared.Append(I(a.index)).Append(',').Append(I(b.index)).Append(';');
            count++;
            return a.key.CompareTo(b.key);
        });
        sb.Append("sorted ").Append(string.Join(' ', items.Select(x => I(x.index)))).Append('\n');
        sb.Append("compares ").Append(I(count)).Append(' ').Append(fnv(compared.ToString()).ToString("x16")).Append('\n');
    }

    static void sorts()
    {
        // List<T>.Sort with a comparison, over keys with many ties; prints the original
        // indices in sorted order.
        var rng = new Random(424242);
        for (int c = 0; c < 200; c++)
        {
            int n = c < 40 ? c : rng.Next(0, 120);
            int range = rng.Next(1, 6) == 1 ? 1 : rng.Next(1, n + 2);
            emitSort(Enumerable.Range(0, n).Select(i => (key: (double)rng.Next(range), index: i)).ToList());
        }
    }

    // McIlroy's "A Killer Adversary for Quicksort": freezes keys only when a comparison needs
    // them, which drives introsort into its depth limit and so into heapsort.
    static int[] antiQuicksort(int n)
    {
        int gas = n - 1, solid = 0, candidate = 0;
        var val = Enumerable.Repeat(gas, n).ToArray();
        var ptr = Enumerable.Range(0, n).ToList();
        ptr.Sort((x, y) =>
        {
            if (val[x] == gas && val[y] == gas)
            {
                if (x == candidate) val[x] = solid++;
                else val[y] = solid++;
            }
            if (val[x] == gas) candidate = x;
            else if (val[y] == gas) candidate = y;
            return val[x].CompareTo(val[y]);
        });
        return val;
    }

    static void adversarialSorts()
    {
        foreach (int n in new[] { 64, 300, 1000 })
        {
            emitSort(antiQuicksort(n).Select((key, index) => (key: (double)key, index)).ToList());
        }
    }

    public static string Generate()
    {
        sb.Append("# Generated from osu!lazer 2026.1005.0-lazer OsuHitObject.cs, Slider.cs, SliderTick.cs, SliderEndCircle.cs, HitObject.cs (ApplyDefaults), SliderEventGenerator.cs, IBeatmapDifficultyInfo.cs, LegacyRulesetExtensions.cs, IHasPathWithRepeats.cs and the slider path sources on osuTK 1.0.211, .NET ").Append(Environment.Version).Append('\n');
        handpicked();
        random(300);
        events();
        sorts();
        adversarialSorts();
        return sb.ToString();
    }
}
