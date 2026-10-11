// Writes reference scores, computed by the verbatim lazer OsuScoreProcessor (standardised scoring) on objects built from
// small `.osu` maps with lazer's defaults applied (ApplyDefaults creates the nested objects).
//
// Every case is a small `.osu` map, as in Health.cs (each map is written with the classic slider behaviour off and on),
// a score multiplier and a flag for the Hidden mod (which only adjusts the rank). A pattern is a sequence of results for
// the judgements in the order of JudgementProcessor.EnumerateHitObjects (nested objects before their parent, recursively),
// applied one by one with ApplyResult; after each one the state of the processor is written.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text;
using osu.Game.Beatmaps;
using osu.Game.Beatmaps.ControlPoints;
using osu.Game.Rulesets.Judgements;
using osu.Game.Rulesets.Mods;
using osu.Game.Rulesets.Objects;
using osu.Game.Rulesets.Osu.Objects;
using osu.Game.Rulesets.Osu.Scoring;
using osu.Game.Rulesets.Scoring;
using osuTK;

// Stand-ins for the infrastructure of the mods: the score processor only asks its mods for a rank adjustment.
namespace osu.Game.Rulesets.Mods
{
    public abstract class Mod
    {
    }

    public interface IApplicableToScoreProcessor
    {
        osu.Game.Scoring.ScoreRank AdjustRank(osu.Game.Scoring.ScoreRank rank, double accuracy);
    }

    public class HiddenForScoring : ModHidden
    {
    }
}

namespace osu.Game.Rulesets.Scoring
{
    // Stand-ins: RevertResultInternal is not ported (rewinding is not needed here), and osu-framework's
    // AddRange for dictionaries.
    public partial class ScoreProcessor
    {
        protected sealed override void RevertResultInternal(JudgementResult result)
            => throw new NotSupportedException();
    }

    public static class DictionaryExtensions
    {
        public static void AddRange<TKey, TValue>(this Dictionary<TKey, TValue> dictionary, IEnumerable<KeyValuePair<TKey, TValue>> pairs)
        {
            foreach (var pair in pairs)
                dictionary.Add(pair.Key, pair.Value);
        }
    }
}

// OsuScoreProcessor with the protected members and the enumeration order exposed.
class ProbedScoreProcessor : OsuScoreProcessor
{
    public IReadOnlyDictionary<HitResult, int> Counts => ScoreResultCounts;
    public IReadOnlyDictionary<HitResult, int> MaximumCounts => MaximumResultCounts;
    public List<HitObject> Order(IBeatmap beatmap) => EnumerateHitObjects(beatmap).ToList();
}

static class Scoring
{
    static readonly StringBuilder sb = new StringBuilder();
    static readonly CultureInfo inv = CultureInfo.InvariantCulture;

    static string D(double d) => BitConverter.DoubleToUInt64Bits(d).ToString("x16");
    static string I(int i) => i.ToString(inv);

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

    static string counts(IReadOnlyDictionary<HitResult, int> dict)
    {
        var parts = dict.Where(kv => kv.Value != 0).OrderBy(kv => (int)kv.Key).Select(kv => $"{(int)kv.Key}:{kv.Value}").ToList();
        return parts.Count == 0 ? "-" : string.Join(",", parts);
    }

    // The values of lazer's unit tests (ScoreProcessorTest.TestSingleOsuHit) are checked on the way.
    static readonly Dictionary<string, long> expectedSingleHit = new Dictionary<string, long>
    {
        ["perfect"] = 1_000_000,
        ["unit-ok"] = 168_724,
        ["unit-meh"] = 83_398,
    };

    // A valid result below the maximum result, as the judgement's own MinResult is the lowest.
    static HitResult lower(HitResult max, Random rng)
    {
        switch (max)
        {
            case HitResult.Great:
                return new[] { HitResult.Ok, HitResult.Meh, HitResult.Miss }[rng.Next(3)];
            case HitResult.LargeTickHit:
                return HitResult.LargeTickMiss;
            case HitResult.SmallTickHit:
                return HitResult.SmallTickMiss;
            default:
                // SliderTailHit, IgnoreHit, SmallBonus, LargeBonus.
                return HitResult.IgnoreMiss;
        }
    }

    class Pattern
    {
        public string Name;
        public HitResult[] Results;
        public int FailedFrom = -1;
    }

    static Pattern[] patterns(string seedKey, HitResult[] max, HitResult[] min, bool full, bool singleHit, bool perfectOnly)
    {
        var list = new List<Pattern>();
        list.Add(new Pattern { Name = "perfect", Results = max.ToArray() });
        if (perfectOnly)
            return list.ToArray();
        if (full)
            list.Add(new Pattern { Name = "allmiss", Results = min.ToArray() });

        double[] ps = full ? new[] { 0.95, 0.7, 0.3 } : new[] { 0.95, 0.7 };
        Pattern random07 = null;
        foreach (double p in ps)
        {
            string name = "random-" + p.ToString(inv);
            var rng = new Random((int)(fnv(seedKey + "/" + name) & 0x7fffffff));
            var results = new HitResult[max.Length];
            for (int i = 0; i < max.Length; i++)
                results[i] = rng.NextDouble() < p ? max[i] : lower(max[i], rng);
            var pattern = new Pattern { Name = name, Results = results };
            list.Add(pattern);
            if (p == 0.7)
                random07 = pattern;
        }

        // random-0.7 with the failed state from about the middle (at least from the second judgement).
        int k = Math.Max(1, max.Length / 2);
        list.Add(new Pattern { Name = "failed-" + I(k), Results = random07.Results.ToArray(), FailedFrom = k });
        if (full)
        {
            // Failed from the very first judgement, and from the last one.
            list.Add(new Pattern { Name = "failed-0", Results = random07.Results.ToArray(), FailedFrom = 0 });
            if (max.Length > 2)
                list.Add(new Pattern { Name = "failed-" + I(max.Length - 1), Results = random07.Results.ToArray(), FailedFrom = max.Length - 1 });
        }

        if (singleHit)
        {
            list.Add(new Pattern { Name = "unit-ok", Results = new[] { HitResult.Ok } });
            list.Add(new Pattern { Name = "unit-meh", Results = new[] { HitResult.Meh } });
        }

        return list.ToArray();
    }

    const int full_listing = 64;
    const int listing_edge = 16;

    // `full` selects all patterns (otherwise a few); `singleHit` adds the unit-test patterns for a single circle;
    // `perfectOnly` keeps only the perfect play (long maps).
    static void emit(string name, Func<Stacking.Case> build, double multiplier = 1, bool hidden = false, bool full = true, bool singleHit = false, bool perfectOnly = false)
    {
        foreach (bool classic in new[] { false, true })
        {
            var c = build();
            c.OwnSliderPath = true;

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

            // As in Health.cs: the velocity of a slider comes from the last inherited timing point at or before its start.
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

            string caseName = name + (multiplier != 1 || hidden ? $"-x{multiplier.ToString("R", inv)}{(hidden ? "-hd" : "")}" : "");

            // The generator sets the mods and the multiplier before ApplyBeatmap, as the player does.
            ProbedScoreProcessor NewProcessor()
            {
                var p = new ProbedScoreProcessor();
                p.scoreMultiplier = multiplier;
                if (hidden)
                    p.Mods.Value = new Mod[] { new HiddenForScoring() };
                return p;
            }

            var reference = NewProcessor();
            reference.ApplyBeatmap(beatmap);
            var order = reference.Order(beatmap);
            var judgements = order.Select(o => o.Judgement).ToArray();
            var max = judgements.Select(j => j.MaxResult).ToArray();
            var min = judgements.Select(j => j.MinResult).ToArray();

            sb.Append("case ").Append(caseName).Append(' ').Append(classic ? 1 : 0).Append(' ').Append(D(multiplier)).Append(' ').Append(hidden ? 1 : 0).Append('\n');
            sb.Append("map ").Append(I(c.Version)).Append(' ').Append(c.Leniency).Append(' ').Append(c.Hp).Append(' ').Append(c.Cs)
              .Append(' ').Append(c.Od).Append(' ').Append(c.Ar).Append(' ').Append(c.Sm).Append(' ').Append(c.Tr).Append(' ').Append(c.Beat).Append('\n');
            foreach (string line in c.TimingLines)
                sb.Append("timing ").Append(line).Append('\n');
            foreach (string line in c.Lines)
                sb.Append("object ").Append(line).Append('\n');
            sb.Append("max_results ").Append(string.Join(" ", max.Select(m => I((int)m)))).Append('\n');
            sb.Append("maxima ").Append(reference.MaximumTotalScore.ToString(inv)).Append(' ').Append(I(reference.MaximumCombo)).Append(' ')
              .Append(D(reference.maximumBaseScore)).Append(' ').Append(D(reference.maximumComboPortion)).Append(' ')
              .Append(I(reference.maximumAccuracyJudgementCount)).Append(' ').Append(counts(reference.MaximumCounts)).Append('\n');

            foreach (var pattern in patterns(caseName + "/" + I(classic ? 1 : 0), max, min, full, singleHit, perfectOnly))
            {
                var proc = NewProcessor();
                proc.ApplyBeatmap(beatmap);

                var states = new List<string>();
                for (int i = 0; i < order.Count; i++)
                {
                    var result = new JudgementResult(order[i], judgements[i]) { Type = pattern.Results[i] };
                    result.FailedAtJudgement = pattern.FailedFrom >= 0 && i >= pattern.FailedFrom;
                    proc.ApplyResult(result);
                    states.Add($"state {I(i)} {state(proc)}");
                }

                sb.Append("pattern ").Append(pattern.Name).Append(' ').Append(I(pattern.FailedFrom)).Append('\n');
                sb.Append("results ").Append(string.Join(" ", pattern.Results.Select(r => I((int)r)))).Append('\n');
                if (states.Count <= full_listing)
                {
                    foreach (string s in states)
                        sb.Append(s).Append('\n');
                }
                else
                {
                    foreach (string s in states.Take(listing_edge))
                        sb.Append(s).Append('\n');
                    foreach (string s in states.Skip(states.Count - listing_edge))
                        sb.Append(s).Append('\n');
                    sb.Append("hash ").Append(fnv(string.Join("\n", states)).ToString("x16")).Append('\n');
                }

                sb.Append("final ").Append(state(proc)).Append(' ').Append(counts(proc.Counts)).Append('\n');

                if (name == "single-circle" && multiplier == 1 && expectedSingleHit.TryGetValue(pattern.Name, out long expected) && proc.TotalScore.Value != expected)
                    throw new InvalidOperationException($"single circle {pattern.Name}: {proc.TotalScore.Value} instead of {expected}");
            }
        }
    }

    static string state(ProbedScoreProcessor p)
        => $"{p.TotalScore.Value.ToString(inv)} {p.TotalScoreWithoutMods.Value.ToString(inv)} {D(p.Accuracy.Value)} {D(p.MinimumAccuracy.Value)} {D(p.MaximumAccuracy.Value)} {I(p.Combo.Value)} {I(p.HighestCombo.Value)} {p.Rank.Value}";

    static void emitAll()
    {
        double hdMult = 1.06;

        // Lazer's unit test shapes: a single circle with Great, Ok and Meh.
        emit("single-circle", () => Health.circles("5", 1, 1000, i => 0), singleHit: true);
        emit("two-circles", () => Health.circles("5", 2, 1000, i => 1000));

        // Circles only.
        emit("circles-10", () => Health.circles("5", 10, 1000, i => 250));
        emit("circles-40", () => Health.circles("5", 40, 1000, i => 250));
        emit("circles-100", () => Health.circles("5", 100, 1000, i => 200));
        emit("circles-250", () => Health.circles("5", 250, 1000, i => 150), full: false);
        // A long perfect play: combos past 2921, the first where the libm `pow(n, 0.5)` and `sqrt(n)` differ. The
        // difference stays below the precision of the combo portion's sum; the Rust unit test of `dotnet::pow` pins it.
        emit("circles-4000", () => Health.circles("5", 4000, 1000, i => 75), perfectOnly: true);

        // Sliders with ticks and repeats at different slider velocities and tick rates.
        emit("sliders-sv1.4-tr1", () => sliders("1.4", "1", "500", "5", false));
        emit("sliders-sv2-tr2-inherited", () => sliders("2", "2", "450", "6", true));
        emit("sliders-sv0.8-tr3-inherited", () => sliders("0.8", "3", "600", "4", true));
        emit("sliders-sv2.4-tr1", () => sliders("2.4", "1", "400", "0", false));
        emit("single-slider", () =>
        {
            var c = Health.newCase();
            c.Slider(new Vector2(100, 100), 1000, false, new[] { new Vector2(300, 100) }, 2, 200);
            return c;
        });
        emit("circle-and-slider", () =>
        {
            var c = Health.newCase("7");
            Health.circle(c, 0, 1000);
            c.Slider(new Vector2(100, 100), 2000, false, new[] { new Vector2(300, 100) }, 3, 200);
            return c;
        });
        emit("long-slider-tr2", () =>
        {
            var c = Health.newCase("10", sm: "1", tr: "2", beat: "600");
            Health.circle(c, 0, 1000);
            Health.circle(c, 1, 1400);
            c.Slider(new Vector2(60, 200), 2000, false, new[] { new Vector2(460, 200) }, 4, 400, "-200");
            Health.circle(c, 2, 12000);
            Health.circle(c, 3, 12400);
            return c;
        });
        emit("long-slider-curved", () =>
        {
            var c = Health.newCase("8", sm: "1", tr: "1", beat: "500");
            Health.circle(c, 0, 1000);
            c.Slider(new Vector2(60, 200), 1500, true, new[] { new Vector2(260, 100), new Vector2(460, 200) }, 2, 600, "-300");
            Health.circle(c, 1, 11000);
            Health.circle(c, 2, 11300);
            return c;
        });

        // Spinners, with and without bonus spins.
        emit("spinner-short", () =>
        {
            var c = Health.newCase();
            Health.circle(c, 0, 1000);
            Health.circle(c, 1, 1300);
            c.Spinner(2000, 2400);
            Health.circle(c, 2, 3000);
            Health.circle(c, 3, 3300);
            return c;
        });
        emit("spinner-long", () =>
        {
            var c = Health.newCase();
            Health.circle(c, 0, 1000);
            Health.circle(c, 1, 1300);
            c.Spinner(2000, 8000);
            Health.circle(c, 2, 9000);
            Health.circle(c, 3, 9300);
            return c;
        });
        emit("spinner-long-od10", () =>
        {
            var c = Health.newCase(od: "10");
            c.Spinner(1000, 7000);
            c.Spinner(8000, 12500);
            return c;
        });
        emit("single-spinner", () =>
        {
            var c = Health.newCase();
            c.Spinner(1000, 4000);
            return c;
        });

        // Everything together with new combos.
        Func<Stacking.Case> mixed = () =>
        {
            var c = Health.newCase("6", sm: "1.6", tr: "2", beat: "480");
            Health.circle(c, 0, 1200, true);
            Health.circle(c, 1, 1500);
            c.Slider(new Vector2(100, 100), 1900, false, new[] { new Vector2(260, 100) }, 2, 300);
            Health.circle(c, 2, 3300, true);
            c.Slider(new Vector2(300, 250), 3700, true, new[] { new Vector2(360, 300), new Vector2(300, 340) }, 1, 150, "-60");
            c.Spinner(5000, 7500);
            Health.circle(c, 3, 8200, true);
            Health.circle(c, 4, 8500);
            c.Slider(new Vector2(60, 60), 9000, false, new[] { new Vector2(260, 60), new Vector2(260, 200) }, 3, 520, "-120");
            Health.circle(c, 5, 13000, true);
            Health.circle(c, 6, 13300);
            Health.circle(c, 7, 13600);
            return c;
        };
        emit("mixed", mixed);
        emit("mixed-long", () =>
        {
            var c = Health.newCase("5", sm: "1.8", tr: "1", beat: "420");
            double time = 1000;
            for (int i = 0; i < 12; i++)
            {
                Health.circle(c, i, time, i % 3 == 0);
                Health.circle(c, i + 1, time + 210);
                c.Slider(new Vector2(80 + i * 20, 100), time + 630, i % 2 == 0, new[] { new Vector2(240 + i * 15, 140) }, 1 + i % 3, 180 + i * 10, i % 4 == 3 ? "-80" : null);
                time += 2400;
            }

            c.Spinner(time, time + 3000);
            return c;
        });

        // The same maps with other multipliers (a double product as the mods compute it) and the Hidden mod.
        var variants = new (string name, Func<Stacking.Case> build)[]
        {
            ("single-circle", () => Health.circles("5", 1, 1000, i => 0)),
            ("circles-40", () => Health.circles("5", 40, 1000, i => 250)),
            ("sliders-sv1.4-tr1", () => sliders("1.4", "1", "500", "5", false)),
            ("spinner-long", () =>
            {
                var c = Health.newCase();
                Health.circle(c, 0, 1000);
                c.Spinner(2000, 8000);
                Health.circle(c, 1, 9000);
                return c;
            }),
            ("mixed", mixed),
        };
        foreach (var (name, build) in variants)
        {
            emit(name, build, 1.09, full: false);
            emit(name, build, 0.5 * 1.23, full: false);
            emit(name, build, 0.96, full: false);
            emit(name, build, 1, hidden: true, full: false);
            emit(name, build, hdMult, hidden: true, full: false);
        }
    }

    static Stacking.Case sliders(string sm, string tr, string beat, string hp, bool withVelocity) => Health.sliders(sm, tr, beat, hp, withVelocity);

    public static string Generate()
    {
        sb.Append("# Generated from osu!lazer 2026.1005.0-lazer ScoreProcessor.cs, OsuScoreProcessor.cs, ScoreRank.cs, ModHidden.cs (AdjustRank), JudgementProcessor.cs, Judgement.cs, JudgementResult.cs, HitResult.cs, the osu judgement sources, Slider.cs, Spinner.cs and the object and slider path sources, osu-framework 2026.921.1 Bindable.cs, BindableNumber.cs, SortedList.cs, on osuTK 1.0.211, .NET ").Append(Environment.Version).Append('\n');
        sb.Append("# case <name> <classic slider behaviour 0/1> <score multiplier f64 bits> <Hidden mod 0/1>: every map is written twice, with the classic behaviour off and on; the multiplier is set directly (ScoreProcessor.scoreMultiplier), the mods are covered by lazer-mods.txt\n");
        sb.Append("# map <format version> <StackLeniency> <HP> <CS> <OD> <AR> <SliderMultiplier> <SliderTickRate> <beat length>\n");
        sb.Append("# timing <inherited timing point line>\n");
        sb.Append("# object <.osu hit object line>\n");
        sb.Append("# max_results <HitResult codes of Judgement.MaxResult of every judgement in JudgementProcessor.EnumerateHitObjects order (nested objects before their parent, recursively)>\n");
        sb.Append("# maxima <MaximumTotalScore> <MaximumCombo> <maximumBaseScore f64 bits> <maximumComboPortion f64 bits> <maximumAccuracyJudgementCount> <MaximumResultCounts as code:count pairs, comma-separated, sorted by code>: after ApplyBeatmap\n");
        sb.Append("# pattern <name> <index of the first judgement applied with FailedAtJudgement set, or -1>: names are perfect (every MaxResult), allmiss (every MinResult), random-<p> (MaxResult with probability p, else a random lower result), failed-<k> (random-0.7 results, failed from judgement k), unit-ok and unit-meh (a single circle, lazer's ScoreProcessorTest.TestSingleOsuHit)\n");
        sb.Append("# results <HitResult codes of the applied results, one per judgement in the order above>\n");
        sb.Append("# state <i> <TotalScore> <TotalScoreWithoutMods> <Accuracy f64 bits> <MinimumAccuracy f64 bits> <MaximumAccuracy f64 bits> <Combo> <HighestCombo> <ScoreRank name>: after the i-th (from 0) ApplyResult; with more than 64 judgements only the first 16 and the last 16 are written\n");
        sb.Append("# hash <FNV-1a 64 of all the state lines, as written above and joined with '\\n' (no trailing one), 16 hex digits>: only with more than 64 judgements\n");
        sb.Append("# final <the state fields after the last judgement, as in state> <ScoreResultCounts as code:count pairs, comma-separated, sorted by code, zero counts omitted, `-` if none>\n");
        sb.Append("# HitResult codes are the integers of the enum (None 0, Miss 1, Meh 2, Ok 3, Good 4, Great 5, Perfect 6, SmallTickMiss 7, SmallTickHit 8, LargeTickMiss 9, LargeTickHit 10, SmallBonus 11, LargeBonus 12, IgnoreMiss 13, IgnoreHit 14, ComboBreak 15, SliderTailHit 16).\n");
        sb.Append("# Applied as the player does: format version 14, ApplyDefaults of every object (which creates the nested objects, with Slider.ClassicSliderBehaviour set beforehand), the multiplier and the mods set, ApplyBeatmap (the autoplay simulation stores the maxima), then ApplyResult of every result with JudgementResult.FailedAtJudgement set from the failed index on (the processor ignores such results but still records the combo at the judgement).\n");
        emitAll();
        return sb.ToString();
    }
}
