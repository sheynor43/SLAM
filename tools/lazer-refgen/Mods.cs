// Writes reference mod setting values and score multipliers, computed by the verbatim
// osu-framework bindables and lazer multiplier functions on values deserialised by Newtonsoft
// exactly as lazer deserialises the settings of an `APIMod`.
//
// Every setting case is one JSON value; the Rust test wraps it into a score block the same
// way, so both sides read the same text.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;
using Newtonsoft.Json;
using osu.Framework.Bindables;
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
    static void parse<T>(Bindable<T> bindable, string json)
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

    public static string Generate()
    {
        sb.Clear();
        sb.Append("# Generated from osu-framework 2026.921.1 Bindable.cs, BindableBool.cs, BindableNumber.cs and osu!lazer 2026.1005.0-lazer mod settings and score multipliers, Newtonsoft.Json ")
          .Append(typeof(JsonConvert).Assembly.GetName().Version).Append(", .NET ").Append(Environment.Version).Append('\n');

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
        return sb.ToString();
    }
}
