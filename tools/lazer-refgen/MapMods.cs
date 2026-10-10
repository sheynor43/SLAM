// Writes reference objects after the map-changing mods (Difficulty Adjust, Hard Rock, Easy, Mirror), computed by
// the verbatim lazer mods on objects built from small `.osu` maps in the order of
// WorkingBeatmap.GetPlayableBeatmap: difficulty mods, ApplyDefaults of every object, hit object mods (the outer
// loop over the mods, the inner one over the objects), then the stacking of the beatmap processor.
//
// Every case is a small `.osu` map, as in Stacking.cs; the Rust test decodes the same text.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text;
using osu.Game.Beatmaps;
using osu.Game.Beatmaps.ControlPoints;
using osu.Game.Rulesets.Mods;
using osu.Game.Rulesets.Objects;
using osu.Game.Rulesets.Objects.Types;
using osu.Game.Rulesets.Osu.Beatmaps;
using osu.Game.Rulesets.Osu.Mods;
using osu.Game.Rulesets.Osu.Objects;
using osuTK;

static class MapMods
{
    static readonly StringBuilder sb = new StringBuilder();
    static readonly CultureInfo inv = CultureInfo.InvariantCulture;

    static string F(float f) => BitConverter.SingleToUInt32Bits(f).ToString("x8");
    static string D(double d) => BitConverter.DoubleToUInt64Bits(d).ToString("x16");
    static string S(double d) => d.ToString("R", inv);
    static string I(int i) => i.ToString(inv);

    // `HR`, `EZ`, `MR:<int>` or `DA:<cs>,<ar>,<hp>,<od>,<extended limits 0/1>` with each value as JSON text or `null`.
    // The settings of Difficulty Adjust are applied through the conversion of an `APIMod`; the order of the extended limits varies per case.
    static object createMod(string spec, bool extendedLast)
    {
        if (spec == "HR")
            return new OsuModHardRock();
        if (spec == "EZ")
            return new OsuModEasy();

        if (spec.StartsWith("MR:"))
        {
            var mirror = new OsuModMirror();
            mirror.Reflection.Value = (OsuModMirror.MirrorType)int.Parse(spec.Substring(3), inv);
            return mirror;
        }

        if (spec.StartsWith("DA:"))
        {
            string[] v = spec.Substring(3).Split(',');
            var da = new OsuModDifficultyAdjust();
            string extended = v[4] == "1" ? "true" : "false";
            if (!extendedLast)
                Mods.parse(da.ExtendedLimits, extended);
            Mods.parse(da.CircleSize, v[0]);
            Mods.parse(da.ApproachRate, v[1]);
            Mods.parse(da.DrainRate, v[2]);
            Mods.parse(da.OverallDifficulty, v[3]);
            if (extendedLast)
                Mods.parse(da.ExtendedLimits, extended);
            return da;
        }

        throw new InvalidOperationException(spec);
    }

    static int caseIndex;
    static bool? forceExtendedLast;

    static void emit(string name, Stacking.Case c, params string[] specs)
    {
        var difficulty = new BeatmapDifficulty
        {
            DrainRate = float.Parse(c.Hp, inv),
            CircleSize = float.Parse(c.Cs, inv),
            OverallDifficulty = float.Parse(c.Od, inv),
            ApproachRate = float.Parse(c.Ar, inv),
            SliderMultiplier = double.Parse(c.Sm, inv),
            SliderTickRate = double.Parse(c.Tr, inv),
        };
        var cpi = new ControlPointInfo();
        cpi.Timing.BeatLength = double.Parse(c.Beat, inv);

        // The order of the settings of Difficulty Adjust alternates per case; it does not change the results.
        bool extendedLast = forceExtendedLast ?? (caseIndex++ & 1) == 1;
        var mods = specs.Select(sp => createMod(sp, extendedLast)).ToList();

        foreach (var mod in mods.OfType<IApplicableToDifficulty>())
            mod.ApplyToDifficulty(difficulty);

        // As LegacyBeatmapDecoder.applyDefaults: the velocity of a slider comes from the last inherited timing
        // point at or before its start (the later line wins at equal times), whichever slider added the line.
        var points = c.TimingLines.Select(l => l.Split(',')).Select(f => (time: double.Parse(f[0], inv), bl: double.Parse(f[1], inv))).ToList();
        foreach (var h in c.Objects)
        {
            if (h is Slider sl)
            {
                double velocity = 1;
                foreach (var (time, bl) in points)
                {
                    if (time <= sl.StartTime)
                        velocity = bl < 0 ? 100.0 / -bl : 1;
                }

                sl.SliderVelocityMultiplier = velocity;
            }
        }

        foreach (var h in c.Objects)
        {
            // As OsuBeatmapConverter: formats before 8 scale the tick distance by the slider velocity.
            if (h is Slider s && c.Version < 8)
                s.TickDistanceMultiplier = 1f / s.SliderVelocityMultiplier;
            h.ApplyDefaults(cpi, difficulty);
        }

        foreach (var mod in mods.OfType<IApplicableToHitObject>())
        {
            foreach (var h in c.Objects)
                mod.ApplyToHitObject(h);
        }

        var beatmap = new StackingBeatmap
        {
            Objects = c.Objects,
            BeatmapVersion = c.Version,
            StackLeniency = float.Parse(c.Leniency, inv),
        };
        OsuBeatmapProcessor.ApplyStacking(beatmap);

        sb.Append("case ").Append(name).Append('\n');
        sb.Append("map ").Append(I(c.Version)).Append(' ').Append(c.Leniency).Append(' ').Append(c.Hp).Append(' ').Append(c.Cs)
          .Append(' ').Append(c.Od).Append(' ').Append(c.Ar).Append(' ').Append(c.Sm).Append(' ').Append(c.Tr).Append(' ').Append(c.Beat).Append('\n');
        sb.Append("mods");
        foreach (string spec in specs)
            sb.Append(' ').Append(spec);
        sb.Append('\n');
        foreach (string line in c.TimingLines)
            sb.Append("timing ").Append(line).Append('\n');
        sb.Append("difficulty ").Append(F(difficulty.DrainRate)).Append(' ').Append(F(difficulty.CircleSize))
          .Append(' ').Append(F(difficulty.OverallDifficulty)).Append(' ').Append(F(difficulty.ApproachRate)).Append('\n');
        for (int i = 0; i < c.Lines.Count; i++)
        {
            var h = c.Objects[i];
            sb.Append("object ").Append(c.Lines[i]).Append(' ').Append(I(h.StackHeight))
              .Append(' ').Append(F(h.Position.X)).Append(' ').Append(F(h.Position.Y))
              .Append(' ').Append(F(h.StackedPosition.X)).Append(' ').Append(F(h.StackedPosition.Y))
              .Append(' ').Append(F(h.StackedEndPosition.X)).Append(' ').Append(F(h.StackedEndPosition.Y))
              .Append(' ').Append(D(h.GetEndTime())).Append(' ').Append(h is Slider sd ? D(sd.Path.Distance) : "-").Append('\n');

            foreach (var n in h.NestedHitObjects)
            {
                var o = (OsuHitObject)n;
                string kind = n switch
                {
                    SliderHeadCircle => "head",
                    SliderTick => "tick",
                    SliderRepeat => "repeat",
                    SliderTailCircle => "tail",
                    _ => throw new InvalidOperationException(),
                };
                sb.Append("nested ").Append(kind).Append(' ').Append(D(o.StartTime))
                  .Append(' ').Append(F(o.Position.X)).Append(' ').Append(F(o.Position.Y))
                  .Append(' ').Append(F(o.StackedPosition.X)).Append(' ').Append(F(o.StackedPosition.Y)).Append('\n');
            }
        }
    }

    // A slider with the velocity of the timing point in effect at its start, as LegacyBeatmapDecoder.applyDefaults.
    class Builder
    {
        public readonly Stacking.Case Case;
        double velocity = 1;

        public Builder(Stacking.Case c) { Case = c; }

        public Slider Slider(Vector2 pos, double time, (char type, Vector2[] points)[] segments, int slides, double? length = null, string beatLength = null)
        {
            if (length == null)
            {
                var start = new Vector2(Case.Coordinate(Case.Coordinate(pos.X)), Case.Coordinate(Case.Coordinate(pos.Y)));
                length = Math.Round(new SliderPath(Stacking.Case.ControlPoints(start, segments), null, true).Distance);
            }

            if (beatLength != null)
            {
                double bl = double.Parse(beatLength, inv);
                velocity = bl < 0 ? 100.0 / -bl : 1;
            }

            var slider = Case.SliderSegments(pos, time, segments, slides, length.Value, beatLength);
            slider.SliderVelocityMultiplier = velocity;
            return slider;
        }
    }

    static Builder newCase(int version, string leniency = "0.7", string hp = "5", string cs = "4", string od = "7", string ar = "9", string sm = "1.4", string tr = "1", string beat = "500")
        => new Builder(new Stacking.Case { Version = version, Leniency = leniency, Hp = hp, Cs = cs, Od = od, Ar = ar, Sm = sm, Tr = tr, Beat = beat, OwnSliderPath = true });

    static Vector2[] V(params int[] xy)
    {
        var result = new Vector2[xy.Length / 2];
        for (int i = 0; i < result.Length; i++)
            result[i] = new Vector2(xy[2 * i], xy[2 * i + 1]);
        return result;
    }

    static (char, Vector2[]) seg(char type, params int[] xy) => (type, V(xy));

    // Positions are integers before format version 128 and get a fractional part from it on.
    static Vector2 P(int version, float x, float y) => version >= 128 ? new Vector2(x + 0.25f, y - 0.125f) : new Vector2((int)x, (int)y);

    static Builder circlesMap(int v)
    {
        var b = newCase(v);
        var c = b.Case;
        for (int i = 0; i < 5; i++)
            c.Circle(P(v, 200, 200), 1000 + i * 100);
        c.Circle(P(v, 30, 350), 2000);
        c.Circle(P(v, 500, 20), 2200);
        c.Spinner(2600, 3400);
        c.Circle(P(v, 256, 192), 3600);
        c.Circle(P(v, 258, 190), 3700);
        c.Circle(P(v, 300, 100), 3900);
        return b;
    }

    static Builder slidersMap(int v)
    {
        var b = newCase(v, "0.7", "6", "3.5", "8", "8", "1.4", "2");
        b.Slider(P(v, 100, 100), 1000, new[] { seg('L', 200, 100, 200, 180) }, 1);
        b.Slider(P(v, 300, 250), 2500, new[] { seg('P', 360, 200, 420, 260) }, 2);
        b.Slider(P(v, 50, 300), 4500, new[] { seg('B', 100, 250, 150, 350, 200, 300) }, 1);
        b.Slider(P(v, 400, 50), 6000, new[] { seg('C', 450, 100, 400, 150, 350, 100) }, 3);
        b.Slider(P(v, 100, 200), 8500, new[] { seg('L', 160, 200, 160, 260), seg('B', 200, 300, 240, 260, 280, 300) }, 2);
        b.Slider(P(v, 440, 330), 10500, new[] { seg('B', 400, 300, 360, 340), seg('C', 330, 300, 300, 250, 250, 280) }, 1, null, "-50");
        b.Slider(P(v, 20, 20), 12500, new[] { seg('L', 120, 60) }, 4, 90, "-200");
        return b;
    }

    static Builder edgesMap(int v)
    {
        var b = newCase(v, "0.5", "4", "5", "6", "7", "1.2");
        var c = b.Case;
        c.Circle(P(v, 0, 0), 1000);
        c.Circle(P(v, 512, 384), 1200);
        c.Circle(P(v, 511, 383), 1250);
        b.Slider(P(v, 4, 380), 1500, new[] { seg('L', 60, 380) }, 1);
        b.Slider(P(v, 508, 4), 2500, new[] { seg('P', 470, 40, 450, 90) }, 2);
        b.Slider(P(v, 500, 200), 3500, new[] { seg('L', 560, 200, 560, 240) }, 1);
        b.Slider(P(v, 12, 190), 4500, new[] { seg('B', -40, 150, -40, 230) }, 1);
        c.Spinner(5500, 6200);
        b.Slider(P(v, 256, 380), 6500, new[] { seg('C', 300, 420, 340, 380) }, 2);
        c.Circle(P(v, 340, 380), 7200);
        return b;
    }

    static Builder stackMap(int v)
    {
        var b = newCase(v, "0.7", "5", "4", "5", "9", "1.4", "1");
        var c = b.Case;
        // Circles under the end of a slider, the head of another one and the end of a repeating slider.
        b.Slider(P(v, 100, 100), 1000, new[] { seg('L', 200, 100) }, 1, 100);
        for (int i = 0; i < 3; i++)
            c.Circle(P(v, 200, 100), 1400 + i * 100);
        b.Slider(P(v, 300, 200), 2200, new[] { seg('L', 380, 200) }, 1, 80);
        b.Slider(P(v, 300, 200), 2500, new[] { seg('L', 380, 260) }, 2, 80);
        c.Circle(P(v, 300, 200), 2900);
        c.Circle(P(v, 380, 260), 3200);
        c.Circle(P(v, 300, 200), 3300);
        b.Slider(P(v, 150, 300), 4500, new[] { seg('P', 200, 250, 250, 300) }, 2);
        c.Circle(P(v, 150, 300), 5600);
        c.Circle(P(v, 250, 300), 5700);
        c.Spinner(6000, 6500);
        c.Circle(P(v, 250, 300), 6700);
        return b;
    }

    static Builder velocityMap(int v)
    {
        var b = newCase(v, "0.6", "7", "6", "9", "10", "1.8", "3", "400");
        b.Slider(P(v, 100, 300), 1000, new[] { seg('B', 160, 240, 220, 300) }, 1);
        b.Slider(P(v, 220, 300), 3000, new[] { seg('L', 300, 300) }, 3, null, "-50");
        b.Slider(P(v, 300, 300), 5000, new[] { seg('C', 340, 250, 380, 300, 420, 250) }, 1, null, "-200");
        b.Slider(P(v, 420, 250), 7500, new[] { seg('P', 440, 200, 470, 150) }, 2);
        b.Case.Circle(P(v, 470, 150), 9500);
        return b;
    }

    static readonly string[][] mod_sets =
    {
        new string[0],
        new[] { "HR" },
        new[] { "EZ" },
        new[] { "MR:0" },
        new[] { "MR:1" },
        new[] { "MR:2" },
        new[] { "MR:3" },
        new[] { "DA:null,null,null,null,0" },
        new[] { "DA:11,-5,null,null,1" },
        new[] { "DA:7.5,10.5,3,6,0" },
        new[] { "DA:null,-10.5,null,null,0" },
        new[] { "EZ", "MR:1" },
        new[] { "DA:null,5,null,null,0", "MR:1" },
        new[] { "MR:2", "DA:4,8,null,null,0" },
        new[] { "HR", "EZ" },
        new[] { "HR", "DA:5,null,6,null,0" },
        new[] { "HR", "MR:0" },
        new[] { "HR", "MR:2" },
        new[] { "MR:1", "HR" },
        new[] { "EZ", "HR" },
    };

    static string modTag(string[] specs) => specs.Length == 0 ? "none" : string.Join("+", specs).Replace(":", "").Replace(",", "_").Replace(".", "p").Replace("-", "m");

    static void handpicked()
    {
        // The indices are those of mod_sets: 1 HR, 2 EZ, 4 MR:1, 5 MR:2, 8 and 9 Difficulty Adjust, 11 EZ+MR:1,
        // 13 MR:2+DA, 14 HR+EZ, 15 HR+DA, 16 HR+MR:0, 18 MR:1+HR; the slider map runs all of them.
        var maps = new List<(string name, Func<Builder> make, int[] sets)>
        {
            ("circles-v14", () => circlesMap(14), new[] { 0, 1, 4 }),
            ("sliders-v14", () => slidersMap(14), Enumerable.Range(0, mod_sets.Length).ToArray()),
            ("edges-v14", () => edgesMap(14), new[] { 1, 5, 11 }),
            ("stack-v14", () => stackMap(14), new[] { 1, 4, 8 }),
            ("velocity-v14", () => velocityMap(14), new[] { 2, 13, 15 }),
            ("sliders-v5", () => slidersMap(5), new[] { 1, 4, 2 }),
            ("stack-v5", () => stackMap(5), new[] { 5, 16, 9 }),
            ("sliders-v128", () => slidersMap(128), new[] { 1, 5, 18 }),
            ("edges-v128", () => edgesMap(128), new[] { 4, 2, 14 }),
        };

        foreach (var (name, make, sets) in maps)
        {
            foreach (int index in sets)
                emit(name + "-" + modTag(mod_sets[index]), make().Case, mod_sets[index]);
        }

        // Extended values with the extended limits applied last (and, for comparison, first).
        forceExtendedLast = true;
        emit("sliders-v14-extlast-DA11_m5", slidersMap(14).Case, "DA:11,-5,null,null,1");
        emit("sliders-v14-extlast-DA-AR-5-ext0", slidersMap(14).Case, "DA:null,-5,null,null,0");
        emit("circles-v14-extlast-DA11_m5-MR1", circlesMap(14).Case, "DA:11,-5,11,11,1", "MR:1");
        forceExtendedLast = false;
        emit("sliders-v14-extfirst-DA11_m5", slidersMap(14).Case, "DA:11,-5,null,null,1");
        forceExtendedLast = null;
    }

    static string[] randomMods(Random rng)
    {
        int count = rng.NextDouble() switch { < 0.15 => 0, < 0.5 => 1, < 0.85 => 2, _ => 3 };
        var kinds = new List<string> { "HR", "EZ", "MR", "DA" };
        var result = new List<string>();
        for (int i = 0; i < count; i++)
        {
            string kind = kinds[rng.Next(kinds.Count)];
            kinds.Remove(kind);
            result.Add(kind switch
            {
                "MR" => "MR:" + I(rng.Next(0, 4)),
                "DA" => "DA:" + string.Join(",", Enumerable.Range(0, 4).Select(_ => randomSetting(rng))) + "," + (rng.Next(2) == 0 ? "0" : "1"),
                _ => kind,
            });
        }

        return result.ToArray();
    }

    static string randomSetting(Random rng)
    {
        if (rng.Next(2) == 0)
            return "null";
        return rng.Next(5) switch
        {
            0 or 1 or 2 => S(Math.Round(-10 + rng.NextDouble() * 21, 1) + 0.0),
            3 => I(rng.Next(-11, 13)),
            _ => S(Math.Round(-11 + rng.NextDouble() * 23, 3) + 0.0),
        };
    }

    static Vector2 randomPoint(Random rng) => new Vector2(rng.Next(0, 513), rng.Next(0, 385));

    static (char, Vector2[])[] randomSegments(Random rng, Vector2 start)
    {
        // Points are written as integers, all distinct from each other and from the start, so that the decoder
        // keeps one explicit segment per listed type.
        var used = new HashSet<(int, int)> { ((int)start.X, (int)start.Y) };
        Vector2 last = new Vector2((int)start.X, (int)start.Y);

        Vector2[] points(int count)
        {
            var result = new Vector2[count];
            for (int i = 0; i < count; i++)
            {
                while (true)
                {
                    var p = new Vector2((int)last.X + rng.Next(-140, 141), (int)last.Y + rng.Next(-140, 141));
                    if (used.Add(((int)p.X, (int)p.Y)))
                    {
                        result[i] = p;
                        last = p;
                        break;
                    }
                }
            }

            return result;
        }

        double kind = rng.NextDouble();
        if (kind < 0.3)
            return new[] { ('L', points(rng.Next(1, 4))) };
        if (kind < 0.5)
        {
            while (true)
            {
                used.Clear();
                used.Add(((int)start.X, (int)start.Y));
                last = new Vector2((int)start.X, (int)start.Y);
                var p = points(2);
                var a = new Vector2((int)start.X, (int)start.Y);
                // Non-collinear, so that every format version keeps the perfect curve.
                if (Math.Abs((p[0].Y - a.Y) * (p[1].X - a.X) - (p[0].X - a.X) * (p[1].Y - a.Y)) >= 1)
                    return new[] { ('P', p) };
            }
        }
        if (kind < 0.65)
            return new[] { ('B', points(rng.Next(2, 5))) };
        if (kind < 0.8)
            return new[] { ('C', points(rng.Next(2, 5))) };

        char[] types = { 'L', 'B', 'C' };
        int segments = rng.Next(2, 4);
        var list = new List<(char, Vector2[])>();
        for (int i = 0; i < segments; i++)
            list.Add((types[rng.Next(types.Length)], points(i == 0 ? rng.Next(1, 3) : rng.Next(2, 4))));
        return list.ToArray();
    }

    static void random(int count)
    {
        var rng = new Random(20261064);
        int[] versions = { 14, 14, 6, 5, 4, 128 };
        string[] leniencies = { "0.7", "0.2", "0.5", "1", "0.35", "0" };
        string[] multipliers = { "1.4", "1", "2", "0.6", "3.2" };
        string[] tickRates = { "1", "2", "4", "0.5", "3" };
        string[] beatLengths = { "-100", "-50", "-200", "-75", "-133.33", "-40", "-1000", "-10" };

        for (int k = 0; k < count; k++)
        {
            int version = versions[rng.Next(versions.Length)];
            string Dec() => S(Math.Round(rng.NextDouble() * 10, 1));
            var b = newCase(version, leniencies[rng.Next(leniencies.Length)], Dec(), Dec(), Dec(), Dec(), multipliers[rng.Next(multipliers.Length)],
                tickRates[rng.Next(tickRates.Length)], S(Math.Round(250 + rng.NextDouble() * 500, 2)));
            var c = b.Case;

            double ar = float.Parse(c.Ar, inv);
            double preempt = ar <= 5 ? 1800 - 120 * ar : 1200 - 150 * (ar - 5);
            double threshold = preempt * float.Parse(c.Leniency, inv);

            var anchors = new Vector2[rng.Next(1, 4)];
            for (int i = 0; i < anchors.Length; i++)
                anchors[i] = rng.Next(4) == 0 ? new Vector2(rng.Next(0, 3) * 256, rng.Next(0, 3) * 192) : randomPoint(rng);

            int objects = rng.Next(3, 16);
            double time = 1000;
            Slider lastSlider = null;
            double lastTimingTime = double.NaN;

            Vector2 jitter() => c.FloatPositions ? new Vector2((float)Math.Round(rng.NextDouble() * 2 - 1, 3), (float)Math.Round(rng.NextDouble() * 2 - 1, 3)) : Vector2.Zero;

            for (int i = 0; i < objects; i++)
            {
                if (i > 0)
                {
                    double r = rng.NextDouble();
                    double gap = r < 0.1 ? 0
                        : r < 0.75 ? rng.NextDouble() * threshold * 0.6
                        : r < 0.9 ? threshold + rng.Next(-2, 3)
                        : rng.NextDouble() * threshold * 2;
                    time = Math.Max(time, Math.Round(time + gap, rng.Next(2) == 0 ? 0 : 2));
                }

                Vector2 pos;
                double p = rng.NextDouble();
                if (p < 0.5)
                    pos = anchors[rng.Next(anchors.Length)] + new Vector2(rng.Next(-3, 4), rng.Next(-3, 4));
                else if (p < 0.7 && lastSlider != null)
                    pos = new Vector2((int)lastSlider.EndPosition.X + rng.Next(-2, 3), (int)lastSlider.EndPosition.Y + rng.Next(-2, 3));
                else
                    pos = randomPoint(rng);

                double kind = rng.NextDouble();
                if (kind < 0.5)
                    c.Circle(pos + jitter(), time);
                else if (kind < 0.9)
                {
                    pos = new Vector2((int)pos.X, (int)pos.Y);
                    if (rng.Next(2) == 0)
                        pos += jitter();
                    var start = new Vector2(c.Coordinate(c.Coordinate(pos.X)), c.Coordinate(c.Coordinate(pos.Y)));

                    string beatLength = null;
                    if (time != lastTimingTime && rng.Next(2) == 0)
                    {
                        beatLength = beatLengths[rng.Next(beatLengths.Length)];
                        lastTimingTime = time;
                    }

                    var segments = randomSegments(rng, start);
                    double natural = new SliderPath(Stacking.Case.ControlPoints(start, segments), null, true).Distance;
                    double length = Math.Round(Math.Max(1, natural * (0.6 + rng.NextDouble() * 0.6)), rng.Next(2) == 0 ? 0 : 2);
                    lastSlider = b.Slider(pos, time, segments, rng.Next(1, 4), length, beatLength);
                }
                else
                {
                    double end = time + Math.Round(100 + rng.NextDouble() * 1500);
                    c.Spinner(time, end);
                    if (rng.Next(2) == 0)
                        time = end;
                }
            }

            emit("random-" + I(k), c, randomMods(rng));
        }
    }

    public static string Generate()
    {
        sb.Append("# Generated from osu!lazer 2026.1005.0-lazer ModHardRock.cs, ModEasy.cs, ModDifficultyAdjust.cs, OsuModHardRock.cs, OsuModEasy.cs, OsuModDifficultyAdjust.cs, OsuModMirror.cs, OsuHitObjectGenerationUtils.cs, OsuBeatmapProcessor.cs (ApplyStacking), Slider.cs, the object and slider path sources, osu-framework 2026.921.1, on osuTK 1.0.211, .NET ").Append(Environment.Version).Append('\n');
        sb.Append("# case <name>\n");
        sb.Append("# map <format version> <StackLeniency> <HP> <CS> <OD> <AR> <SliderMultiplier> <SliderTickRate> <beat length>\n");
        sb.Append("# mods <mod specs in order>: HR | EZ | MR:<MirrorType int> | DA:<cs>,<ar>,<hp>,<od>,<extended limits 0/1> (each value JSON text or null; settings applied through the Bindable conversion; extended limits are applied first in even-numbered cases and last in odd ones, which does not change the results); none for no mods\n");
        sb.Append("# timing <inherited timing point line>\n");
        sb.Append("# difficulty <HP> <CS> <OD> <AR> as f32 bits after the difficulty mods\n");
        sb.Append("# object <.osu hit object line> <stack height> <position x, y bits> <stacked position x, y bits> <stacked end position x, y bits> <end time f64 bits> <path distance f64 bits or ->, after the mods and the stacking (slider end time and distance are of the reflected path)\n");
        sb.Append("# nested <head|tick|repeat|tail> <start time f64 bits> <position x, y bits> <stacked position x, y bits>: follows the object line of its slider, in NestedHitObjects order\n");
        sb.Append("# Applied as WorkingBeatmap.GetPlayableBeatmap: ApplyToDifficulty of each mod in order, ApplyDefaults of every object (TickDistanceMultiplier = 1 / velocity before format version 8), ApplyToHitObject with the mods in the outer loop, ApplyStacking.\n");
        handpicked();
        random(150);
        return sb.ToString();
    }
}
