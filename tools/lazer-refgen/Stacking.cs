// Writes reference stack heights and stacked positions, computed by the verbatim lazer
// OsuBeatmapProcessor stacking on objects with lazer's defaults applied.
//
// Every case is a small `.osu` map. Its values are written as text and parsed back here the
// way lazer's decoder parses them, so the Rust test decodes exactly the same input.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text;
using osu.Game.Beatmaps;
using osu.Game.Beatmaps.ControlPoints;
using osu.Game.Rulesets.Objects;
using osu.Game.Rulesets.Objects.Types;
using osu.Game.Rulesets.Osu.Beatmaps;
using osu.Game.Rulesets.Osu.Objects;
using osuTK;

class StackingBeatmap : IBeatmap
{
    public List<OsuHitObject> Objects = new List<OsuHitObject>();
    public IReadOnlyList<HitObject> HitObjects => Objects;
    public int BeatmapVersion { get; set; }
    public float StackLeniency { get; set; }
}

static class Stacking
{
    static readonly StringBuilder sb = new StringBuilder();
    static readonly CultureInfo inv = CultureInfo.InvariantCulture;

    static string F(float f) => BitConverter.SingleToUInt32Bits(f).ToString("x8");
    static string S(double d) => d.ToString("R", inv);
    static string S(float f) => f.ToString("R", inv);
    static string I(int i) => i.ToString(inv);

    // The `.osu` text of one case and the lazer objects built from it.
    internal class Case
    {
        public int Version;
        public string Leniency, Ar, Cs, Sm, Beat;
        // Used by MapMods only; stacking ignores them.
        public string Hp = "5", Od = "5", Tr = "1";
        // Whether the sliders own their path as lazer's Slider does (see Slider.OwnsPath).
        public bool OwnSliderPath;
        public readonly List<string> Lines = new List<string>();
        public readonly List<string> TimingLines = new List<string>();
        public readonly List<OsuHitObject> Objects = new List<OsuHitObject>();

        public bool FloatPositions => Version >= 128;

        // Coordinates as the decoder reads them: truncated to int before format version 128.
        public float Coordinate(string s) => FloatPositions ? float.Parse(s, inv) : (int)float.Parse(s, inv);

        public string Coordinate(float f) => FloatPositions ? S(f) : I((int)f);

        public void Circle(Vector2 pos, double time)
        {
            string x = Coordinate(pos.X), y = Coordinate(pos.Y), t = S(time);
            Lines.Add($"{x},{y},{t},1,0");
            Objects.Add(new HitCircle { StartTime = double.Parse(t, inv), Position = new Vector2(Coordinate(x), Coordinate(y)) });
        }

        public void Spinner(double time, double endTime)
        {
            string t = S(time), e = S(endTime);
            Lines.Add($"256,192,{t},8,0,{e}");
            double start = double.Parse(t, inv);
            // As ConvertHitObjectParser (ConvertSpinner.EndTime is StartTime + Duration) and
            // OsuBeatmapConverter, which sets EndTime after StartTime.
            double duration = Math.Max(0, double.Parse(e, inv) - start);
            Objects.Add(new Spinner { StartTime = start, EndTime = start + duration, Position = new Vector2(256, 192) });
        }

        // `points` are absolute integer positions after the start; perfect curves need exactly two.
        // `beatLength` is a negative beat length for a new inherited timing point at the slider's
        // start time; the caller sets the slider velocity of the point in effect.
        public Slider Slider(Vector2 pos, double time, bool perfect, Vector2[] points, int slides, double length, string beatLength = null)
            => SliderSegments(pos, time, new[] { (perfect ? 'P' : 'L', points) }, slides, length, beatLength);

        // The control points the decoder builds for a path made of explicit segments whose points are all
        // distinct (no implicit segment splits): the first point of the path is the zero vertex, and each
        // later segment starts at its first listed point, which carries the type.
        public static PathControlPoint[] ControlPoints(Vector2 start, (char type, Vector2[] points)[] segments)
        {
            var result = new List<PathControlPoint>();
            for (int k = 0; k < segments.Length; k++)
            {
                var type = segments[k].type switch
                {
                    'P' => PathType.PERFECT_CURVE,
                    'L' => PathType.LINEAR,
                    'B' => PathType.BEZIER,
                    'C' => PathType.CATMULL,
                    _ => throw new InvalidOperationException(),
                };
                if (k == 0)
                    result.Add(new PathControlPoint(Vector2.Zero, type));
                for (int i = 0; i < segments[k].points.Length; i++)
                    result.Add(new PathControlPoint(new Vector2((int)segments[k].points[i].X, (int)segments[k].points[i].Y) - start, k > 0 && i == 0 ? type : null));
            }

            return result.ToArray();
        }

        public Slider SliderSegments(Vector2 pos, double time, (char type, Vector2[] points)[] segments, int slides, double length, string beatLength = null)
        {
            string x = Coordinate(pos.X), y = Coordinate(pos.Y), t = S(time), l = S(length);
            string path = string.Join("|", segments.Select(seg => seg.type + string.Concat(seg.points.Select(p => $"|{I((int)p.X)}:{I((int)p.Y)}"))));
            Lines.Add($"{x},{y},{t},2,0,{path},{I(slides)},{l}");

            var start = new Vector2(Coordinate(x), Coordinate(y));
            var controlPoints = ControlPoints(start, segments);

            var slider = new Slider
            {
                OwnsPath = OwnSliderPath,
                StartTime = double.Parse(t, inv),
                Position = start,
                RepeatCount = slides - 1,
                Path = new SliderPath(controlPoints, double.Parse(l, inv), false),
            };
            if (beatLength != null)
                TimingLines.Add($"{t},{beatLength},4,2,0,100,0,0");
            Objects.Add(slider);
            return slider;
        }
    }

    static void emit(string name, Case c)
    {
        var difficulty = new Difficulty
        {
            ApproachRate = float.Parse(c.Ar, inv),
            CircleSize = float.Parse(c.Cs, inv),
            SliderMultiplier = double.Parse(c.Sm, inv),
            SliderTickRate = 1,
        };
        var cpi = new ControlPointInfo();
        cpi.Timing.BeatLength = double.Parse(c.Beat, inv);

        foreach (var h in c.Objects)
            h.ApplyDefaults(cpi, difficulty);

        var beatmap = new StackingBeatmap
        {
            Objects = c.Objects,
            BeatmapVersion = c.Version,
            StackLeniency = float.Parse(c.Leniency, inv),
        };
        OsuBeatmapProcessor.ApplyStacking(beatmap);

        sb.Append("case ").Append(name).Append('\n');
        sb.Append("map ").Append(I(c.Version)).Append(' ').Append(c.Leniency).Append(' ').Append(c.Ar).Append(' ').Append(c.Cs)
          .Append(' ').Append(c.Sm).Append(' ').Append(c.Beat).Append('\n');
        foreach (string line in c.TimingLines)
            sb.Append("timing ").Append(line).Append('\n');
        for (int i = 0; i < c.Lines.Count; i++)
        {
            var h = c.Objects[i];
            sb.Append("object ").Append(c.Lines[i]).Append(' ').Append(I(h.StackHeight))
              .Append(' ').Append(F(h.StackedPosition.X)).Append(' ').Append(F(h.StackedPosition.Y))
              .Append(' ').Append(F(h.StackedEndPosition.X)).Append(' ').Append(F(h.StackedEndPosition.Y)).Append('\n');
        }
    }

    static Case newCase(int version, string leniency, string ar = "9", string cs = "4", string sm = "1.4", string beat = "500")
        => new Case { Version = version, Leniency = leniency, Ar = ar, Cs = cs, Sm = sm, Beat = beat };

    static void handpicked()
    {
        foreach (int v in new[] { 14, 5 })
        {
            string tag = "-v" + I(v);

            // A plain stack of circles; the last one stays on top.
            var c = newCase(v, "0.7");
            for (int i = 0; i < 6; i++)
                c.Circle(new Vector2(200, 200), 1000 + i * 100);
            emit("circles" + tag, c);

            // Distances just inside and outside STACK_DISTANCE.
            c = newCase(v, "0.7");
            c.Circle(new Vector2(200, 200), 1000);
            c.Circle(new Vector2(202, 202), 1100);
            c.Circle(new Vector2(205, 202), 1200);
            c.Circle(new Vector2(205, 199), 1300);
            c.Circle(new Vector2(208, 199), 1400);
            emit("distance" + tag, c);

            // Circles under the end of a slider stack down and right.
            c = newCase(v, "0.7");
            c.Slider(new Vector2(100, 100), 1000, false, new[] { new Vector2(200, 100) }, 1, 100);
            for (int i = 0; i < 4; i++)
                c.Circle(new Vector2(200, 100), 1400 + i * 100);
            emit("under-slider-end" + tag, c);

            // A repeating slider ends at its start: the end position follows the spans.
            c = newCase(v, "0.7");
            c.Slider(new Vector2(100, 100), 1000, false, new[] { new Vector2(200, 100) }, 2, 100);
            c.Circle(new Vector2(100, 100), 1900);
            c.Circle(new Vector2(200, 100), 2000);
            emit("repeat-slider" + tag, c);

            // Sliders stacked on sliders, with a spinner in between.
            c = newCase(v, "0.7");
            c.Slider(new Vector2(300, 200), 1000, false, new[] { new Vector2(380, 200) }, 1, 80);
            c.Spinner(1100, 1150);
            c.Slider(new Vector2(300, 200), 1200, false, new[] { new Vector2(380, 200) }, 1, 80);
            c.Slider(new Vector2(380, 200), 1400, false, new[] { new Vector2(380, 280) }, 1, 80);
            c.Circle(new Vector2(380, 280), 1600);
            emit("slider-chain" + tag, c);

            // Gaps around the threshold: (int)preempt(AR 9 = 600) * 0.5 = 300.
            c = newCase(v, "0.5");
            c.Circle(new Vector2(50, 50), 1000);
            c.Circle(new Vector2(50, 50), 1300);
            c.Circle(new Vector2(50, 50), 1600.9);
            c.Circle(new Vector2(50, 50), 1901.5);
            c.Circle(new Vector2(50, 50), 2202);
            emit("threshold" + tag, c);

            // Two interwound stacks.
            c = newCase(v, "0.7");
            c.Circle(new Vector2(100, 100), 1000);
            c.Circle(new Vector2(300, 100), 1100);
            c.Circle(new Vector2(100, 100), 1200);
            c.Circle(new Vector2(300, 100), 1300);
            emit("interwound" + tag, c);
        }
    }

    static Vector2 randomPoint(Random rng) => new Vector2(rng.Next(0, 513), rng.Next(0, 385));

    static void random(int count)
    {
        var rng = new Random(20261010);
        int[] versions = { 14, 14, 6, 5, 4, 128 };
        string[] leniencies = { "0.7", "0.2", "0.5", "1", "0.35", "0" };
        string[] beatLengths = { "-100", "-50", "-200", "-75", "-133.33", "-40", "-1000", "-10" };
        string[] multipliers = { "1.4", "1", "2", "0.6", "3.2" };

        for (int k = 0; k < count; k++)
        {
            int version = versions[rng.Next(versions.Length)];
            string leniency = rng.Next(3) == 0 ? S(Math.Round(0.1 + rng.NextDouble() * 0.9, 2)) : leniencies[rng.Next(leniencies.Length)];
            string ar = S(Math.Round(rng.NextDouble() * 10, 1));
            var c = newCase(version, leniency, ar, S(Math.Round(rng.NextDouble() * 10, 1)), multipliers[rng.Next(multipliers.Length)],
                S(Math.Round(250 + rng.NextDouble() * 500, 2)));

            // (int)preempt * leniency, roughly: the gaps are drawn around it.
            double preempt = float.Parse(ar, inv) <= 5 ? 1800 - 120 * float.Parse(ar, inv) : 1200 - 150 * (float.Parse(ar, inv) - 5);
            double threshold = preempt * float.Parse(leniency, inv);

            var anchors = new Vector2[rng.Next(1, 4)];
            for (int i = 0; i < anchors.Length; i++)
                anchors[i] = randomPoint(rng);

            int objects = rng.Next(4, 40);
            double time = 1000 + (rng.Next(2) == 0 ? 0 : Math.Round(rng.NextDouble(), 2));
            Slider lastSlider = null;
            double lastTimingTime = double.NaN;
            double sliderVelocity = 1;

            for (int i = 0; i < objects; i++)
            {
                if (i > 0)
                {
                    double r = rng.NextDouble();
                    double gap = r < 0.1 ? 0
                        : r < 0.75 ? rng.NextDouble() * threshold * 0.6
                        : r < 0.9 ? threshold + rng.Next(-2, 3) + (rng.Next(2) == 0 ? 0 : Math.Round(rng.NextDouble(), 2))
                        : rng.NextDouble() * threshold * 2;
                    time = Math.Max(time, Math.Round(time + gap, rng.Next(2) == 0 ? 0 : 2));
                }

                Vector2 pos;
                double p = rng.NextDouble();
                if (p < 0.55)
                    pos = anchors[rng.Next(anchors.Length)] + new Vector2(rng.Next(-3, 4), rng.Next(-3, 4));
                else if (p < 0.8 && lastSlider != null)
                    pos = new Vector2((int)lastSlider.EndPosition.X + rng.Next(-2, 3), (int)lastSlider.EndPosition.Y + rng.Next(-2, 3));
                else
                    pos = randomPoint(rng);

                double kind = rng.NextDouble();
                if (kind < 0.6)
                {
                    if (c.FloatPositions)
                        pos += new Vector2((float)Math.Round(rng.NextDouble() * 2 - 1, 3), (float)Math.Round(rng.NextDouble() * 2 - 1, 3));
                    c.Circle(pos, time);
                }
                else if (kind < 0.9)
                {
                    pos = new Vector2((int)pos.X, (int)pos.Y);
                    if (c.FloatPositions && rng.Next(2) == 0)
                        pos += new Vector2((float)Math.Round(rng.NextDouble() * 2 - 1, 3), (float)Math.Round(rng.NextDouble() * 2 - 1, 3));
                    bool perfect = rng.Next(3) == 0;
                    Vector2[] points;
                    while (true)
                    {
                        points = new Vector2[perfect ? 2 : rng.Next(1, 3)];
                        for (int j = 0; j < points.Length; j++)
                        {
                            var from = j == 0 ? pos : points[j - 1];
                            // Sometimes end the path on an anchor so that circles stack under it.
                            points[j] = j == points.Length - 1 && rng.Next(3) == 0
                                ? anchors[rng.Next(anchors.Length)]
                                : from + new Vector2(rng.Next(-120, 121), rng.Next(-120, 121));
                            // Path points are written as integers.
                            points[j] = new Vector2((int)points[j].X, (int)points[j].Y);
                        }
                        // Avoid the decoder's special cases: repeated points and straight perfect curves.
                        bool distinct = points[0] != pos && (points.Length < 2 || points[1] != points[0]);
                        bool curved = !perfect || Math.Abs((points[0].Y - pos.Y) * (points[1].X - pos.X) - (points[0].X - pos.X) * (points[1].Y - pos.Y)) >= 1;
                        if (distinct && curved)
                            break;
                    }

                    var probe = new PathControlPoint[points.Length + 1];
                    probe[0] = new PathControlPoint(Vector2.Zero, perfect ? PathType.PERFECT_CURVE : PathType.LINEAR);
                    for (int j = 0; j < points.Length; j++)
                        probe[j + 1] = new PathControlPoint(points[j] - pos, null);
                    double natural = new SliderPath(probe, null, false).Distance;
                    double length = Math.Round(Math.Max(1, natural * (0.6 + rng.NextDouble() * 0.6)), rng.Next(2) == 0 ? 0 : 2);

                    // At most one inherited timing point per time; it applies until the next one.
                    string beatLength = null;
                    if (time != lastTimingTime && rng.Next(2) == 0)
                    {
                        beatLength = beatLengths[rng.Next(beatLengths.Length)];
                        lastTimingTime = time;
                        // As LegacyBeatmapDecoder.handleTimingPoint; beat lengths in -1000..-10
                        // need no clamping.
                        double bl = double.Parse(beatLength, inv);
                        sliderVelocity = bl < 0 ? 100.0 / -bl : 1;
                    }
                    lastSlider = c.Slider(pos, time, perfect, points, rng.Next(1, 4), length, beatLength);
                    // As LegacyBeatmapDecoder.applyDefaults: the point in effect at the start time.
                    lastSlider.SliderVelocityMultiplier = sliderVelocity;
                }
                else
                {
                    double end = time + Math.Round(100 + rng.NextDouble() * 1500);
                    c.Spinner(time, end);
                    if (rng.Next(2) == 0)
                        time = end;
                }
            }

            emit("random-" + I(k), c);
        }
    }

    public static string Generate()
    {
        sb.Append("# Generated from osu!lazer 2026.1005.0-lazer OsuBeatmapProcessor.cs (ApplyStacking), OsuHitObject.cs, Slider.cs, Spinner.cs, HitObject.cs and the slider path sources, osu-framework 2026.921.1 Vector2Extensions.cs, on osuTK 1.0.211, .NET ").Append(Environment.Version).Append('\n');
        sb.Append("# case <name>\n# map <format version> <StackLeniency> <ApproachRate> <CircleSize> <SliderMultiplier> <beat length>\n# timing <inherited timing point line>\n# object <.osu hit object line> <stack height> <stacked position x, y bits> <stacked end position x, y bits>\n");
        handpicked();
        random(400);
        return sb.ToString();
    }
}
