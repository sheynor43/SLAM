// Writes reference mod setting values and score multipliers, computed by the verbatim
// osu-framework bindables and lazer multiplier functions on values deserialised by Newtonsoft
// exactly as lazer deserialises the settings of an `APIMod`.
//
// Setting lines (`setting<TAB>acronym<TAB>key<TAB>json<TAB>expected`) hold one JSON value each:
//   DT/HT/NC/DC speed_change: f64 bits (16 hex digits); other numbers likewise; bools 0/1; ints in decimal.
//   DA circle_size, approach_rate, drain_rate, overall_difficulty: `null` or the f32 bits (8 hex digits) of the
//     final DifficultyBindable value (a failed conversion keeps the default null). DA extended_limits: 0/1.
//   MR reflection: the MirrorType as an int (a failed conversion keeps 0 = Horizontal).
//   `setting2<TAB>DA<TAB>key1<TAB>json1<TAB>key2<TAB>json2<TAB>expected`: both settings go through the same
//     conversion, in this order, into one fresh DA mod; expected is the final value of the float setting of the two.
// Multiplier lines (`mul<TAB>kind<TAB>input<TAB>expected f64 bits`):
//   dt2, ht2, v1: input is the f64 bits of the speed; ez2: the number of retries; hd2: 0/1.
//   da1: input `-`, the V1 multiplier of Difficulty Adjust.
//   da2: input `cs,ar,hp,od/bcs,bar,bhp,bod`: the four DA settings (`null` or f32 bits, assigned to the setting
//     before its clamping) and the beatmap difficulty without mods (f32 bits).
//
// Every setting case is one JSON value; the Rust test wraps it into a score block the same
// way, so both sides read the same text.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;
using Newtonsoft.Json;
using osu.Framework.Bindables;
using osu.Game.Beatmaps;
using osu.Game.Rulesets.Mods;
using osu.Game.Rulesets.Osu.Mods;
using osu.Game.Rulesets.Osu.Scoring;

static class Mods
{
    static readonly StringBuilder sb = new StringBuilder();
    static readonly CultureInfo inv = CultureInfo.InvariantCulture;

    static string D(double d) => BitConverter.DoubleToUInt64Bits(d).ToString("x16");

    // `APIMod.Settings` is a `Dictionary<string, object>` filled by Newtonsoft's defaults.
    static object deserialise(string json) =>
        JsonConvert.DeserializeObject<Dictionary<string, object>>("{\"v\":" + json + "}")["v"];

    // APIMod.ToMod: a failed conversion is logged and the setting keeps its value.
    internal static void parse<T>(Bindable<T> bindable, string json)
    {
        object value = deserialise(json);

        try
        {
            bindable.Parse(value, inv);
        }
        catch (Exception)
        {
        }
    }

    static void setting<T>(string acronym, string key, string json, Func<Bindable<T>> create, Func<T, string> format)
    {
        var bindable = create();
        parse(bindable, json);
        sb.Append("setting\t").Append(acronym).Append('\t').Append(key).Append('\t').Append(json).Append('\t').Append(format(bindable.Value)).Append('\n');
    }

    static string R(double d) => d.ToString("R", inv);

    static IEnumerable<string> speedLiterals(Random rng)
    {
        // Every hundredth and thousandth with its neighbouring doubles: ties of the decimal rounding.
        for (int k = 400; k <= 2200; k++)
        {
            double v = k / 1000.0;
            yield return R(v);
            yield return R(Math.BitIncrement(v));
            yield return R(Math.BitDecrement(v));
        }

        for (int i = 0; i < 1000; i++)
            yield return R(0.3 + rng.NextDouble() * 2.0);

        foreach (string s in new[]
                 {
                     "1", "2", "0", "-1", "3", "1.0", "1.5e0", "15e-1", "true", "false", "null", "[]", "{}",
                     "\"1.25\"", "\" 1.3 \"", "\"1,234.5\"", "\"Infinity\"", "\"-Infinity\"", "\"NaN\"", "\"abc\"", "\"\"",
                     "\"1e400\"", "\"0.995\"", "9223372036854775807", "9223372036854775808", "100000000000000000000",
                     "1.7976931348623157e308", "5e-324", "-0.0",
                 })
            yield return s;

        foreach (string s in odd_number_strings)
            yield return JsonConvert.ToString(s);
    }

    // Strings that .NET's number parsing treats in unusual ways, written as JSON strings.
    static readonly string[] odd_number_strings =
    {
        "--1", "-+1.5", "+-1.5", "+NaN", "-NaN", "nan", "infinity", "+Infinity", "1.", ".5", "1,e5", "1,,000", "1,5",
        "1.5\0", "1.5 \0", "1.5\0 ", "\t1.5\n", "1.5e", "e5", "1e-1", "1E+0", "0x10",
    };

    static readonly string[] bool_literals =
    {
        "true", "false", "1", "0", "2", "-1", "0.0", "0.5", "\"1\"", "\"0\"", "\"true\"", "\" True \"", "\"FALSE\"",
        "\"yes\"", "\"\"", "null", "[]", "{}", "\"NaN\"", "\"true\\u0000\"", "\" true \\u0000\"", "\"true\\u0000 \"",
        "\"\\ttrue\\n\"", "\" 1\"", "1e3",
    };

    static readonly string[] int_literals =
    {
        "0", "1", "2", "5", "10", "11", "12", "-1", "2.5", "3.5", "4.4999", "-0.5", "\"4\"", "\" 5 \"", "\"5.0\"",
        "\"+6\"", "1e10", "2147483647", "2147483648", "-2147483649", "2147483647.4", "true", "false", "null", "[]",
        "\"x\"", "\"5\\u0000\"", "\"5 \\u0000\"", "\"5\\u0000 \"", "\"--5\"", "\"- 5\"", "\"0x5\"", "\"1,000\"",
    };

    static void multipliers(Random rng)
    {
        var speeds = new List<double>();
        for (int k = 50; k <= 200; k++)
            speeds.Add(k / 100.0);
        for (int i = 0; i < 500; i++)
            speeds.Add(0.5 + rng.NextDouble() * 1.5);

        foreach (double s in speeds)
        {
            sb.Append("mul\tdt2\t").Append(D(s)).Append('\t').Append(D(OsuScoreMultiplierCalculatorV2.doubleTimeMultiplier(s))).Append('\n');
            sb.Append("mul\tht2\t").Append(D(s)).Append('\t').Append(D(OsuScoreMultiplierCalculatorV2.halfTimeMultiplier(s))).Append('\n');
            sb.Append("mul\tv1\t").Append(D(s)).Append('\t').Append(D(OsuScoreMultiplierCalculatorV1.rateAdjustMultiplier(s))).Append('\n');
        }

        for (int r = 0; r <= 10; r++)
        {
            var easy = new OsuModEasy();
            easy.Retries.Value = r;
            sb.Append("mul\tez2\t").Append(r.ToString(inv)).Append('\t').Append(D(OsuScoreMultiplierCalculatorV2.easyMultiplier(easy))).Append('\n');
        }

        foreach (bool only in new[] { false, true })
        {
            var hidden = new OsuModHidden();
            hidden.OnlyFadeApproachCircles.Value = only;
            sb.Append("mul\thd2\t").Append(only ? '1' : '0').Append('\t').Append(D(OsuScoreMultiplierCalculatorV2.hiddenMultiplier(hidden, false))).Append('\n');
        }
    }


    static string F(float f) => BitConverter.SingleToUInt32Bits(f).ToString("x8");
    static string FN(float? f) => f == null ? "null" : F(f.Value);

    // JSON text of a random float, rounded to the given number of decimals.
    static string number(Random rng, double lo, double hi, int decimals)
    {
        double v = Math.Round(lo + rng.NextDouble() * (hi - lo), decimals);
        return R(v == 0 ? 0 : v);
    }

    static readonly string[] float_literals =
    {
        "null", "true", "false", "0", "5", "11", "12", "-5", "-11", "10", "-10", "9223372036854775807", "-9223372036854775808",
        "9223372036854775808", "100000000000000000000", "0.05", "4.2", "8.35", "10.04", "10.5", "11.0000001", "10.95", "-10.5", "-10.04",
        "1e39", "-1e39", "3.4028235677973366e38", "3.4028234663852886e38", "-3.4028235677973366e38", "1e-50", "-1e-50", "5e-324",
        "1.00000005960464477539062500001", "\"1.00000005960464477539062500001\"", "1.0000000596046448", "0.1", "0.3", "0.7", "7.3", "7.35",
        "\"5.5\"", "\" 5.5 \"", "\"1,000\"", "\"NaN\"", "\"-Infinity\"", "\"Infinity\"", "\"\"", "\"abc\"", "\"5.5e1\"", "\"5.5e\"", "\"-5\"", "\"1e39\"",
        "[]", "{}", "[5]", "-0.0", "1e2", "\"+3\"", "\"0x5\"", "\"5\\u0000\"",
    };

    // Hand-picked literals, the 0.1 grid of the setting precision and random numbers with up to nine decimals.
    static IEnumerable<string> floatSettingLiterals(Random rng)
    {
        foreach (string s in float_literals)
            yield return s;

        for (int k = -120; k <= 120; k++)
            yield return R(k / 10.0);

        for (int i = 0; i < 1000; i++)
            yield return number(rng, -12, 12, rng.Next(0, 10));
    }

    static readonly string[] mirror_literals =
    {
        "0", "1", "2", "3", "-1", "2147483648", "1.0", "1.5", "2e0", "-0.0", "true", "\"Horizontal\"", "\"Vertical\"", "\"Both\"",
        "\"vertical\"", "\" Vertical \"", "\"Horizontal, Vertical\"", "\"Vertical,Both\"", "\"Vertical,\"", "\",Vertical\"", "\"1\"", "\" 1\"",
        "\"1 \"", "\"+1\"", "\"-1\"", "\"01\"", "\"1\\u0000\"", "\"0x1\"", "\"1e0\"", "\"\"", "\" \"", "\"abc\"", "\"Vertical, 2\"", "null",
        "\"BOTH\"", "\"2,1\"", "\"Both, Horizontal\"", "\"3\"", "4", "\"Horizontal | Vertical\"", "[]", "{}", "0.0", "2.0", "1e1",
    };

    static void differenceAdjustSettings(Random rng)
    {
        foreach (string json in floatSettingLiterals(rng))
        {
            setting("DA", "circle_size", json, () => new OsuModDifficultyAdjust().CircleSize, FN);
            setting("DA", "approach_rate", json, () => new OsuModDifficultyAdjust().ApproachRate, FN);
            setting("DA", "drain_rate", json, () => new OsuModDifficultyAdjust().DrainRate, FN);
            setting("DA", "overall_difficulty", json, () => new OsuModDifficultyAdjust().OverallDifficulty, FN);
        }

        foreach (string json in bool_literals)
            setting("DA", "extended_limits", json, () => new OsuModDifficultyAdjust().ExtendedLimits, v => v ? "1" : "0");

        foreach (string json in mirror_literals)
            setting("MR", "reflection", json, () => new OsuModMirror().Reflection, v => ((int)v).ToString(inv));
    }

    static void setting2(string key1, string json1, string key2, string json2)
    {
        var mod = new OsuModDifficultyAdjust();
        foreach (var (key, json) in new[] { (key1, json1), (key2, json2) })
        {
            switch (key)
            {
                case "extended_limits": parse(mod.ExtendedLimits, json); break;
                case "circle_size": parse(mod.CircleSize, json); break;
                case "approach_rate": parse(mod.ApproachRate, json); break;
                case "drain_rate": parse(mod.DrainRate, json); break;
                case "overall_difficulty": parse(mod.OverallDifficulty, json); break;
                default: throw new InvalidOperationException(key);
            }
        }

        string floatKey = key1 == "extended_limits" ? key2 : key1;
        float? result = floatKey switch
        {
            "circle_size" => mod.CircleSize.Value,
            "approach_rate" => mod.ApproachRate.Value,
            "drain_rate" => mod.DrainRate.Value,
            "overall_difficulty" => mod.OverallDifficulty.Value,
            _ => throw new InvalidOperationException(floatKey),
        };
        sb.Append("setting2\tDA\t").Append(key1).Append('\t').Append(json1).Append('\t').Append(key2).Append('\t').Append(json2).Append('\t').Append(FN(result)).Append('\n');
    }

    static void orderCases()
    {
        foreach (string key in new[] { "approach_rate", "circle_size", "overall_difficulty", "drain_rate" })
        {
            foreach (string value in new[] { "-10.5", "-5", "-0.5", "10.7", "11.5", "12.5", "7.3", "\"NaN\"" })
            {
                foreach (string ext in new[] { "true", "false" })
                {
                    setting2("extended_limits", ext, key, value);
                    setting2(key, value, "extended_limits", ext);
                }
            }
        }
    }

    static void difficultyMultipliers(Random rng)
    {
        sb.Append("mul\tda1\t-\t").Append(D(OsuScoreMultiplierCalculatorV1.difficultyAdjustMultiplier)).Append('\n');

        var cases = new List<(float?[] da, float[] map)>();
        float[] defaults = { 4, 9, 5, 8 };
        cases.Add((new float?[] { null, null, null, null }, defaults));
        cases.Add((new float?[] { 4, 9, 5, 8 }, defaults));
        cases.Add((new float?[] { 5, 9, 5, 8 }, defaults));
        cases.Add((new float?[] { 4, 10, 5, 8 }, defaults));
        cases.Add((new float?[] { 4, 9, 6, 8 }, defaults));
        cases.Add((new float?[] { 4, 9, 5, 7 }, defaults));
        cases.Add((new float?[] { 11, -10, 10, 0 }, defaults));
        cases.Add((new float?[] { 0, 11, 0, 11 }, new float[] { 10, 0, 10, 0 }));
        cases.Add((new float?[] { 11, 11, 11, 11 }, new float[] { 0, 0, 0, 0 }));
        cases.Add((new float?[] { -10, -10, -10, -10 }, new float[] { 10, 10, 10, 10 }));
        cases.Add((new float?[] { 4.1f, null, null, null }, defaults));
        cases.Add((new float?[] { null, 9.1f, null, null }, defaults));
        cases.Add((new float?[] { null, null, 5.1f, null }, defaults));
        cases.Add((new float?[] { null, null, null, 8.1f }, defaults));
        cases.Add((new float?[] { null, -5, null, null }, new float[] { 5, 0, 5, 5 }));
        cases.Add((new float?[] { 4.05f, 9.05f, 5.05f, 8.05f }, defaults));
        cases.Add((new float?[] { float.NaN, null, null, null }, defaults));
        cases.Add((new float?[] { float.PositiveInfinity, float.NegativeInfinity, null, null }, defaults));

        // Settings near the beatmap values give multipliers between the 0.1 floor and 1; the wide ones hit the floors.
        float? pick(bool grid, bool near, float baseValue)
        {
            if (rng.Next(10) < 3)
                return null;
            if (near)
                return grid ? (float)(Math.Round(baseValue * 10) / 10 + rng.Next(-8, 9) / 10.0) : baseValue + (float)(rng.NextDouble() * 1.6 - 0.8);
            if (grid)
                return (float)(rng.Next(-100, 111) / 10.0);
            return (float)(-12 + rng.NextDouble() * 24);
        }

        for (int i = 0; i < 2000; i++)
        {
            bool grid = rng.Next(2) == 0;
            bool near = rng.Next(10) < 7;
            var map = new float[4];
            for (int j = 0; j < 4; j++)
                map[j] = grid ? (float)(rng.Next(0, 101) / 10.0) : (float)(rng.NextDouble() * 10);
            var da = new float?[4];
            for (int j = 0; j < 4; j++)
                da[j] = pick(grid, near, map[j]);
            cases.Add((da, map));
        }

        foreach (var (da, map) in cases)
        {
            var mod = new OsuModDifficultyAdjust();
            mod.CircleSize.Value = da[0];
            mod.ApproachRate.Value = da[1];
            mod.DrainRate.Value = da[2];
            mod.OverallDifficulty.Value = da[3];
            var difficulty = new BeatmapDifficulty { CircleSize = map[0], ApproachRate = map[1], DrainRate = map[2], OverallDifficulty = map[3] };

            sb.Append("mul\tda2\t").Append(FN(da[0])).Append(',').Append(FN(da[1])).Append(',').Append(FN(da[2])).Append(',').Append(FN(da[3]))
              .Append('/').Append(F(map[0])).Append(',').Append(F(map[1])).Append(',').Append(F(map[2])).Append(',').Append(F(map[3]))
              .Append('\t').Append(D(OsuScoreMultiplierCalculatorV2.difficultyAdjustMultiplier(mod, difficulty))).Append('\n');
        }
    }

    public static string Generate()
    {
        sb.Clear();
        sb.Append("# Generated from osu-framework 2026.921.1 Bindable.cs, BindableBool.cs, BindableNumber.cs and osu!lazer 2026.1005.0-lazer mod settings and score multipliers, Newtonsoft.Json ")
          .Append(typeof(JsonConvert).Assembly.GetName().Version).Append(", .NET ").Append(Environment.Version).Append('\n');
        sb.Append("# setting <acronym> <key> <json> <expected>: tab-separated; expected is f64 bits (DT, HT, NC, DC speed_change), 0/1 (bools), a decimal int (EZ retries, MR reflection as the MirrorType int) or, for the DA float settings, `null` or the f32 bits of the final value\n");
        sb.Append("# setting2 DA <key1> <json1> <key2> <json2> <expected>: both settings applied in this order to one fresh mod; expected is the final value of the float setting (`null` or f32 bits)\n");
        sb.Append("# mul <kind> <input> <expected f64 bits>: dt2/ht2/v1 input is the f64 bits of the speed, ez2 the retries, hd2 0/1, da1 `-`, da2 `cs,ar,hp,od/bcs,bar,bhp,bod` (DA settings as `null` or f32 bits assigned before clamping, then the beatmap difficulty without mods as f32 bits)\n");

        var rng = new Random(46);

        foreach (string json in speedLiterals(rng))
        {
            setting("DT", "speed_change", json, () => new OsuModDoubleTime().SpeedChange, v => D(v));
            setting("HT", "speed_change", json, () => new OsuModHalfTime().SpeedChange, v => D(v));
        }

        // Nightcore and Daycore share the ranges of Double Time and Half Time.
        for (int k = 40; k <= 220; k++)
        {
            string json = R(k / 100.0);
            setting("NC", "speed_change", json, () => new OsuModNightcore().SpeedChange, v => D(v));
            setting("DC", "speed_change", json, () => new OsuModDaycore().SpeedChange, v => D(v));
        }

        foreach (string json in bool_literals)
        {
            setting("DT", "adjust_pitch", json, () => new OsuModDoubleTime().AdjustPitch, v => v ? "1" : "0");
            setting("HD", "only_fade_approach_circles", json, () => new OsuModHidden().OnlyFadeApproachCircles, v => v ? "1" : "0");
            setting("HT", "adjust_pitch", json, () => new OsuModHalfTime().AdjustPitch, v => v ? "1" : "0");
            setting("CL", "no_slider_head_accuracy", json, () => new OsuModClassic().NoSliderHeadAccuracy, v => v ? "1" : "0");
            setting("CL", "classic_note_lock", json, () => new OsuModClassic().ClassicNoteLock, v => v ? "1" : "0");
            setting("CL", "always_play_tail_sample", json, () => new OsuModClassic().AlwaysPlayTailSample, v => v ? "1" : "0");
            setting("CL", "fade_hit_circle_early", json, () => new OsuModClassic().FadeHitCircleEarly, v => v ? "1" : "0");
            setting("CL", "classic_health", json, () => new OsuModClassic().ClassicHealth, v => v ? "1" : "0");
        }

        foreach (string json in int_literals)
            setting("EZ", "retries", json, () => new OsuModEasy().Retries, v => v.ToString(inv));

        multipliers(rng);

        // Everything below was added after the first fixture; a fresh generator keeps the older lines unchanged.
        var rng2 = new Random(64);
        differenceAdjustSettings(rng2);
        orderCases();
        difficultyMultipliers(rng2);
        return sb.ToString();
    }
}
