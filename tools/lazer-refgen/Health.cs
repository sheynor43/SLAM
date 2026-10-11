// Writes reference health drain rates, computed by the verbatim lazer OsuHealthProcessor and OsuLegacyHealthProcessor
// on objects built from small `.osu` maps with lazer's defaults applied (ApplyDefaults creates the nested objects).
//
// Every case is a small `.osu` map, as in Stacking.cs; the Rust test decodes the same text. Each case is written twice:
// with the classic slider behaviour off and on (Slider.ClassicSliderBehaviour, which the head and tail circles inherit
// when the nested objects are created and which changes the judgements of the slider and its head and tail).
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text;
using osu.Game.Beatmaps;
using osu.Game.Beatmaps.ControlPoints;
using osu.Game.Beatmaps.Timing;
using osu.Game.Rulesets.Objects;
using osu.Game.Rulesets.Osu.Objects;
using osu.Game.Rulesets.Osu.Scoring;
using osuTK;

class HealthBeatmap : IBeatmap
{
    public List<OsuHitObject> Objects = new List<OsuHitObject>();
    public IReadOnlyList<HitObject> HitObjects => Objects;
    public int BeatmapVersion { get; set; }
    public float StackLeniency { get; set; }
    public BeatmapDifficulty Difficulty { get; set; } = new BeatmapDifficulty();
    public osu.Framework.Lists.SortedList<BreakPeriod> Breaks { get; set; } = new osu.Framework.Lists.SortedList<BreakPeriod>();
}

// OsuLegacyHealthProcessor with the protected multiplier exposed and the failed iterations counted.
class ProbedLegacyHealthProcessor : OsuLegacyHealthProcessor
{
    public readonly Dictionary<string, int> Iterations = new Dictionary<string, int>();

    public ProbedLegacyHealthProcessor(double drainStartTime)
        : base(drainStartTime)
    {
        OnIterationFail = message =>
        {
            // "FAILED drop <d>: <reason> (...)"
            string reason = message.Substring(message.IndexOf(": ", StringComparison.Ordinal) + 2);
            reason = reason.Substring(0, reason.IndexOf(" (", StringComparison.Ordinal));
            Iterations[reason] = Iterations.GetValueOrDefault(reason) + 1;
        };
    }

    public double HpMultiplier => HpMultiplierNormal;
}

static class Health
{
    static readonly StringBuilder sb = new StringBuilder();
    static readonly CultureInfo inv = CultureInfo.InvariantCulture;
    static readonly Dictionary<string, int> totals = new Dictionary<string, int>();

    static string D(double d) => BitConverter.DoubleToUInt64Bits(d).ToString("x16");
    static string S(double d) => d.ToString("R", inv);
    static string I(int i) => i.ToString(inv);

    // `drainStart` is null for the start time of the first object, as Player.cs passes it.
    static void emit(string name, double? drainStart, Func<Stacking.Case> build, params string[] events)
    {
        foreach (bool classic in new[] { false, true })
        {
            var c = build();
            c.OwnSliderPath = true;
            double drain = drainStart ?? c.Objects[0].StartTime;

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

            // As LegacyBeatmapDecoder.applyDefaults: the velocity of a slider comes from the last inherited timing
            // point at or before its start (the later line wins at equal times).
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
                    sl.ClassicSliderBehaviour = classic;
                }
            }

            foreach (var h in c.Objects)
            {
                // As OsuBeatmapConverter: formats before 8 scale the tick distance by the slider velocity.
                if (h is Slider s && c.Version < 8)
                    s.TickDistanceMultiplier = 1f / s.SliderVelocityMultiplier;
                h.ApplyDefaults(cpi, difficulty);
            }

            var beatmap = new HealthBeatmap
            {
                Objects = c.Objects,
                BeatmapVersion = c.Version,
                StackLeniency = float.Parse(c.Leniency, inv),
                Difficulty = difficulty,
            };

            // As LegacyBeatmapDecoder.handleEvents (format versions from 5 apply no offset).
            foreach (string line in events)
            {
                string[] split = line.Split(',');
                double start = double.Parse(split[1], inv);
                double end = Math.Max(start, double.Parse(split[2], inv));
                beatmap.Breaks.Add(new BreakPeriod(start, end));
            }

            var osu = new OsuHealthProcessor(drain);
            osu.ApplyBeatmap(beatmap);
            var legacy = new ProbedLegacyHealthProcessor(drain);
            legacy.ApplyBeatmap(beatmap);

            if (Environment.GetEnvironmentVariable("REFGEN_VERBOSE") != null)
                Console.Error.WriteLine($"health case {name} classic={classic}: " + string.Join(", ", legacy.Iterations.OrderBy(kv => kv.Key, StringComparer.Ordinal).Select(kv => $"{kv.Value} x {kv.Key}")));
            foreach (var (reason, count) in legacy.Iterations)
                totals[reason] = totals.GetValueOrDefault(reason) + count;
            if (legacy.HpMultiplier != 1)
                totals["(cases with a multiplier != 1)"] = totals.GetValueOrDefault("(cases with a multiplier != 1)") + 1;

            sb.Append("case ").Append(name).Append(' ').Append(D(drain)).Append(' ').Append(classic ? 1 : 0).Append('\n');
            sb.Append("map ").Append(I(c.Version)).Append(' ').Append(c.Leniency).Append(' ').Append(c.Hp).Append(' ').Append(c.Cs)
              .Append(' ').Append(c.Od).Append(' ').Append(c.Ar).Append(' ').Append(c.Sm).Append(' ').Append(c.Tr).Append(' ').Append(c.Beat).Append('\n');
            foreach (string line in c.TimingLines)
                sb.Append("timing ").Append(line).Append('\n');
            foreach (string line in events)
                sb.Append("event ").Append(line).Append('\n');
            foreach (string line in c.Lines)
                sb.Append("object ").Append(line).Append('\n');
            sb.Append("osu_drain_rate ").Append(D(osu.DrainRate)).Append('\n');
            sb.Append("legacy_drain_rate ").Append(D(legacy.DrainRate)).Append('\n');
            sb.Append("legacy_hp_multiplier ").Append(D(legacy.HpMultiplier)).Append('\n');
        }
    }

    internal static Stacking.Case newCase(string hp = "5", string od = "5", string cs = "4", string ar = "9", string sm = "1.4", string tr = "1", string beat = "500")
        => new Stacking.Case { Version = 14, Leniency = "0.7", Hp = hp, Od = od, Cs = cs, Ar = ar, Sm = sm, Tr = tr, Beat = beat };

    static Vector2 pos(int i) => new Vector2(60 + i * 37 % 380, 50 + i * 53 % 280);

    // New combos only change the type bits of the line; lazer's decoder reads them, the health processors do not.
    internal static void circle(Stacking.Case c, int i, double time, bool newCombo = false)
    {
        var p = pos(i);
        c.Lines.Add($"{I((int)p.X)},{I((int)p.Y)},{S(time)},{(newCombo ? 5 : 1)},0");
        c.Objects.Add(new HitCircle { StartTime = time, Position = new Vector2((int)p.X, (int)p.Y) });
    }

    static double[] gaps = { 250, 180, 320, 400, 150 };

    internal static Stacking.Case circles(string hp, int count, double start, Func<int, double> gap)
    {
        var c = newCase(hp);
        double time = start;
        for (int i = 0; i < count; i++)
        {
            circle(c, i, time, i % 5 == 0);
            time += gap(i);
        }

        return c;
    }

    // Eight circles, a gap of 6 seconds and eight more circles.
    internal static Stacking.Case gapped()
    {
        var c = newCase();
        double time = 1000;
        for (int i = 0; i < 16; i++)
        {
            circle(c, i, time);
            time += i == 7 ? 6000 : 250;
        }

        return c;
    }

    internal static Stacking.Case sliders(string sm, string tr, string beat, string hp, bool withVelocity)
    {
        var c = newCase(hp, sm: sm, tr: tr, beat: beat);
        circle(c, 0, 1000);
        c.Slider(new Vector2(100, 100), 1400, false, new[] { new Vector2(300, 100) }, 1, 200);
        circle(c, 1, 2600, true);
        c.Slider(new Vector2(300, 200), 3000, true, new[] { new Vector2(380, 260), new Vector2(300, 320) }, 3, 160, withVelocity ? "-50" : null);
        circle(c, 2, 5200);
        c.Slider(new Vector2(60, 300), 5600, false, new[] { new Vector2(200, 300), new Vector2(200, 160) }, 2, 330, withVelocity ? "-200" : null);
        c.Slider(new Vector2(400, 60), 8200, false, new[] { new Vector2(400, 330) }, 4, 270);
        circle(c, 3, 11000, true);
        c.Slider(new Vector2(120, 120), 11500, false, new[] { new Vector2(220, 120), new Vector2(220, 220) }, 1, 640, withVelocity ? "-75" : null);
        circle(c, 4, 14000);
        return c;
    }

    static void emitAll()
    {
        // Circles only, over the range of HP.
        foreach (string hp in new[] { "0", "2.5", "5", "7.3", "10" })
            emit("circles-hp" + hp, null, () => circles(hp, 40, 1000, i => gaps[i % gaps.Length]));

        // Dense circles at low HP (recovery) and sparse ones at high HP (end HP).
        emit("circles-dense-hp0", null, () => circles("0", 60, 1000, i => 60 + i % 3 * 20));
        emit("circles-dense-hp2.5", null, () => circles("2.5", 60, 1000, i => 90 + i % 4 * 10));
        emit("circles-sparse-hp10", null, () => circles("10", 12, 1000, i => 1500 + i % 3 * 300));
        emit("circles-sparse-hp8", null, () => circles("8", 12, 1000, i => 2000));
        emit("circles-sparse-hp5", null, () => circles("5", 12, 1000, i => 2500));

        // Breaks.
        emit("break-single", null, gapped, "2,2500,7000");
        emit("break-overlapping", null, gapped, "2,2400,5000", "2,4000,7100");
        emit("break-sequential", null, gapped, "2,2500,5000", "2,5000,7000");
        emit("break-nested", null, gapped, "2,2300,7200", "2,3000,4000");
        emit("break-short", null, gapped, "2,3000,3300");
        emit("break-reversed", null, gapped, "2,5000,3000");
        emit("break-two-gaps", null, () =>
        {
            var c = newCase("6");
            double time = 1000;
            for (int i = 0; i < 30; i++)
            {
                circle(c, i, time);
                time += i == 9 || i == 19 ? 5000 : 300;
            }

            return c;
        }, "2,3600,5700", "2,8700,10800");
        emit("break-before-first", null, () => circles("5", 10, 5000, i => 300), "2,0,4000");
        emit("break-before-first-early-drain", 1000, () => circles("5", 10, 5000, i => 300), "2,1500,4500");
        emit("break-after-last", null, () => circles("5", 10, 1000, i => 300), "2,4500,8000");
        emit("break-at-object-times", null, gapped, "2,2750,7000");
        emit("break-dense", null, () => circles("5", 12, 1000, i => 250), "2,1600,2100");

        // Sliders with ticks and repeats at different slider velocities and tick rates.
        emit("sliders-sv1.4-tr1", null, () => sliders("1.4", "1", "500", "5", false));
        emit("sliders-sv2-tr2-inherited", null, () => sliders("2", "2", "450", "6", true));
        emit("sliders-sv0.8-tr3-inherited", null, () => sliders("0.8", "3", "600", "4", true));
        emit("sliders-hp9", null, () => sliders("1.4", "2", "500", "9", true));
        emit("sliders-hp0", null, () => sliders("2.4", "1", "400", "0", false));
        emit("sliders-with-break", null, () => sliders("1.4", "1", "500", "5", true), "2,6000,8000");

        // A long slider with many ticks at high HP: the legacy algorithm overkills.
        emit("long-slider-hp10", null, () =>
        {
            var c = newCase("10", sm: "1", tr: "2", beat: "600");
            circle(c, 0, 1000);
            circle(c, 1, 1400);
            c.Slider(new Vector2(60, 200), 2000, false, new[] { new Vector2(460, 200) }, 4, 400, "-200");
            circle(c, 2, 12000);
            circle(c, 3, 12400);
            return c;
        });
        emit("long-slider-hp8", null, () =>
        {
            var c = newCase("8", sm: "1", tr: "1", beat: "500");
            circle(c, 0, 1000);
            c.Slider(new Vector2(60, 200), 1500, true, new[] { new Vector2(260, 100), new Vector2(460, 200) }, 2, 600, "-300");
            circle(c, 1, 11000);
            circle(c, 2, 11300);
            return c;
        });

        // Spinners.
        foreach (string od in new[] { "5", "10" })
        {
            emit("spinner-short-od" + od, null, () =>
            {
                var c = newCase(od: od);
                circle(c, 0, 1000);
                circle(c, 1, 1300);
                c.Spinner(2000, 2400);
                circle(c, 2, 3000);
                circle(c, 3, 3300);
                return c;
            });
            emit("spinner-long-od" + od, null, () =>
            {
                var c = newCase(od: od);
                circle(c, 0, 1000);
                circle(c, 1, 1300);
                c.Spinner(2000, 8000);
                circle(c, 2, 9000);
                circle(c, 3, 9300);
                return c;
            });
        }

        emit("spinner-only-hp7", null, () =>
        {
            var c = newCase("7");
            c.Spinner(1000, 5000);
            c.Spinner(6000, 6700);
            return c;
        });

        // Everything together with new combos.
        emit("mixed", null, () =>
        {
            var c = newCase("6", sm: "1.6", tr: "2", beat: "480");
            circle(c, 0, 1200, true);
            circle(c, 1, 1500);
            c.Slider(new Vector2(100, 100), 1900, false, new[] { new Vector2(260, 100) }, 2, 300);
            circle(c, 2, 3300, true);
            c.Slider(new Vector2(300, 250), 3700, true, new[] { new Vector2(360, 300), new Vector2(300, 340) }, 1, 150, "-60");
            c.Spinner(5000, 7500);
            circle(c, 3, 8200, true);
            circle(c, 4, 8500);
            c.Slider(new Vector2(60, 60), 9000, false, new[] { new Vector2(260, 60), new Vector2(260, 200) }, 3, 520, "-120");
            circle(c, 5, 13000, true);
            circle(c, 6, 13300);
            circle(c, 7, 13600);
            return c;
        }, "2,6000,9000");

        // Few objects.
        emit("single-circle", null, () => circles("5", 1, 1000, i => 0));
        emit("single-slider", null, () =>
        {
            var c = newCase();
            c.Slider(new Vector2(100, 100), 1000, false, new[] { new Vector2(300, 100) }, 2, 200);
            return c;
        });
        emit("single-spinner", null, () =>
        {
            var c = newCase();
            c.Spinner(1000, 4000);
            return c;
        });
        emit("two-circles", null, () => circles("5", 2, 1000, i => 1000));
        emit("two-circles-hp10", null, () => circles("10", 2, 1000, i => 3000));
        emit("circle-and-slider", null, () =>
        {
            var c = newCase("7");
            circle(c, 0, 1000);
            c.Slider(new Vector2(100, 100), 2000, false, new[] { new Vector2(300, 100) }, 3, 200);
            return c;
        });

        // The unit tests of lazer (TestSceneOsuLegacyHealthProcessor): default difficulty (the objects get no defaults
        // there, which changes nothing for circles) and the drain starting at -1000.
        Func<Stacking.Case> unit = () =>
        {
            var c = newCase(ar: "5", cs: "5");
            circle(c, 0, 0);
            circle(c, 1, 2000);
            return c;
        };
        emit("lazer-unit-no-break", -1000, unit);
        emit("lazer-unit-single-break", -1000, unit, "2,500,1500");
        emit("lazer-unit-overlapping-break", -1000, unit, "2,500,1400", "2,750,1500");
        emit("lazer-unit-sequential-break", -1000, unit, "2,500,1000", "2,1000,1500");
    }

    public static string Generate()
    {
        sb.Append("# Generated from osu!lazer 2026.1005.0-lazer OsuHealthProcessor.cs, OsuLegacyHealthProcessor.cs, HealthProcessor.cs, DrainingHealthProcessor.cs, LegacyDrainingHealthProcessor.cs, JudgementProcessor.cs, Judgement.cs, JudgementResult.cs, HitResult.cs, the osu judgement sources, PeriodTracker.cs, BreakPeriod.cs, Slider.cs, Spinner.cs and the object and slider path sources, osu-framework 2026.921.1 SortedList.cs, on osuTK 1.0.211, .NET ").Append(Environment.Version).Append('\n');
        sb.Append("# case <name> <drain start time f64 bits> <classic slider behaviour 0/1>: every map is written twice, with the classic behaviour off and on\n");
        sb.Append("# map <format version> <StackLeniency> <HP> <CS> <OD> <AR> <SliderMultiplier> <SliderTickRate> <beat length>\n");
        sb.Append("# timing <inherited timing point line>\n");
        sb.Append("# event <break line `2,<start>,<end>` of [Events], in file order>\n");
        sb.Append("# object <.osu hit object line>\n");
        sb.Append("# osu_drain_rate <f64 bits>: OsuHealthProcessor.DrainRate after ApplyBeatmap\n");
        sb.Append("# legacy_drain_rate <f64 bits>: OsuLegacyHealthProcessor.DrainRate after ApplyBeatmap\n");
        sb.Append("# legacy_hp_multiplier <f64 bits>: OsuLegacyHealthProcessor.HpMultiplierNormal after ApplyBeatmap\n");
        sb.Append("# Applied as the decoder and the player do: format version 14, ApplyDefaults of every object (which creates the nested objects, with Slider.ClassicSliderBehaviour set beforehand), a break is BreakPeriod(start, max(start, end)) in a sorted list, then ApplyBeatmap of each processor with the given drain start time (the start time of the first object unless a unit test of lazer sets it).\n");
        emitAll();
        foreach (var (reason, count) in totals.OrderBy(kv => kv.Key, StringComparer.Ordinal))
            Console.Error.WriteLine($"health: {count} x {reason}");
        return sb.ToString();
    }
}
