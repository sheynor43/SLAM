// Method bodies extracted verbatim from osu!lazer 2026.1005.0-lazer: osu/osu.Game.Rulesets.Osu/Objects/OsuHitObject.cs (lines 19-52, 64-74, 76, 80, 92, 170-183), Slider.cs (lines 28, 31-35, 61-69, 88-107, 156-225, 227-254), Spinner.cs (lines 31-37, 59), SliderTick.cs (lines 18-32), SliderEndCircle.cs (lines 28-45),
// osu/osu.Game/Rulesets/Objects/HitObject.cs (ApplyDefaults, lines 105-114, 130-134). The class shells around them are minimal stand-ins without Bindable, samples, judgements and hit windows; the shells of OsuHitObject.StackHeight (propagation to nested objects, OsuHitObject.cs lines 160-167) and of Slider.Path (the owned path, Slider.cs lines 45-57) are written out by hand; see README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
using System; using System.Collections.Generic; using System.Linq; using System.Threading; using osu.Framework.Caching; using osu.Game.Beatmaps; using osu.Game.Beatmaps.ControlPoints; using osu.Game.Rulesets.Objects; using osu.Game.Rulesets.Objects.Legacy; using osu.Game.Rulesets.Objects.Types; using osuTK;
namespace osu.Game.Rulesets.Objects
{
    public abstract class HitObject
    {
        public double StartTime { get; set; }
        private readonly List<HitObject> nestedHitObjects = new List<HitObject>();
        public IReadOnlyList<HitObject> NestedHitObjects => nestedHitObjects;
        protected void AddNested(HitObject hitObject) => nestedHitObjects.Add(hitObject);
        protected virtual void CreateNestedHitObjects(CancellationToken cancellationToken) { }
        protected virtual void ApplyDefaultsToSelf(ControlPointInfo controlPointInfo, IBeatmapDifficultyInfo difficulty) { }

        public void ApplyDefaults(ControlPointInfo controlPointInfo, IBeatmapDifficultyInfo difficulty, CancellationToken cancellationToken = default)
        {
            cancellationToken.ThrowIfCancellationRequested();

            ApplyDefaultsToSelf(controlPointInfo, difficulty);

            nestedHitObjects.Clear();

            CreateNestedHitObjects(cancellationToken);

            nestedHitObjects.Sort((h1, h2) => h1.StartTime.CompareTo(h2.StartTime));

            foreach (var h in nestedHitObjects)
                h.ApplyDefaults(controlPointInfo, difficulty, cancellationToken);

        }
    }
}
namespace osu.Game.Rulesets.Osu.Objects
{
    public class OsuHitObject : HitObject
    {
        /// <summary>
        /// The radius of hit objects (ie. the radius of a <see cref="HitCircle"/>).
        /// </summary>
        public const float OBJECT_RADIUS = 64;

        /// <summary>
        /// The width and height any element participating in display of a hitcircle (or similarly sized object) should be.
        /// </summary>
        public static readonly Vector2 OBJECT_DIMENSIONS = new Vector2(OBJECT_RADIUS * 2);

        /// <summary>
        /// Scoring distance with a speed-adjusted beat length of 1 second (ie. the speed slider balls move through their track).
        /// </summary>
        internal const float BASE_SCORING_DISTANCE = 100;

        /// <summary>
        /// Minimum preempt time at AR=10.
        /// </summary>
        public const double PREEMPT_MIN = 450;

        /// <summary>
        /// Median preempt time at AR=5.
        /// </summary>
        public const double PREEMPT_MID = 1200;

        /// <summary>
        /// Maximum preempt time at AR=0.
        /// </summary>
        public const double PREEMPT_MAX = 1800;

        public static readonly DifficultyRange PREEMPT_RANGE = new DifficultyRange(PREEMPT_MAX, PREEMPT_MID, PREEMPT_MIN);

        public double TimePreempt { get; set; } = 600;
        public double TimeFadeIn = 400;
        public virtual Vector2 Position { get; set; }
        public virtual Vector2 EndPosition => Position;
        public float X
        {
            get => Position.X;
            set => Position = new Vector2(value, Position.Y);
        }

        public float Y
        {
            get => Position.Y;
            set => Position = new Vector2(Position.X, value);
        }

        // Shell of the bindable: a changed height is copied to the nested objects (OsuHitObject constructor).
        private int stackHeight;
        public int StackHeight
        {
            get => stackHeight;
            set
            {
                if (stackHeight == value)
                    return;

                stackHeight = value;

                foreach (var nested in NestedHitObjects)
                {
                    if (nested is OsuHitObject osuHitObject)
                        osuHitObject.StackHeight = value;
                }
            }
        }
        public float Scale { get; set; } = 1;
        public Vector2 StackedPosition => Position + StackOffset;
        public Vector2 StackedEndPosition => EndPosition + StackOffset;
        public virtual Vector2 StackOffset => new Vector2(StackHeight * Scale * -6.4f);

        protected override void ApplyDefaultsToSelf(ControlPointInfo controlPointInfo, IBeatmapDifficultyInfo difficulty)
        {
            base.ApplyDefaultsToSelf(controlPointInfo, difficulty);

            TimePreempt = IBeatmapDifficultyInfo.DifficultyRangeInt(difficulty.ApproachRate, PREEMPT_RANGE);

            // Preempt time can go below 450ms. Normally, this is achieved via the DT mod which uniformly speeds up all animations game wide regardless of AR.
            // This uniform speedup is hard to match 1:1, however we can at least make AR>10 (via mods) feel good by extending the upper linear function above.
            // Note that this doesn't exactly match the AR>10 visuals as they're classically known, but it feels good.
            // This adjustment is necessary for AR>10, otherwise TimePreempt can become smaller leading to hitcircles not fully fading in.
            TimeFadeIn = 400 * Math.Min(1, TimePreempt / PREEMPT_MIN);

            Scale = LegacyRulesetExtensions.CalculateScaleFromCircleSize(difficulty.CircleSize, true);
        }
    }

    public class HitCircle : OsuHitObject { }
    public class Spinner : OsuHitObject, IHasDuration
    {
        public double EndTime
        {
            get => StartTime + Duration;
            set => Duration = value - StartTime;
        }

        public double Duration { get; set; }

        public override Vector2 StackOffset => Vector2.Zero;
    }
    public class SliderHeadCircle : HitCircle { public bool ClassicSliderBehaviour; }

    public abstract class SliderEndCircle : HitCircle
    {
        protected readonly Slider Slider;
        protected SliderEndCircle(Slider slider) { Slider = slider; }
        public int RepeatIndex { get; set; }
        public double SpanDuration => Slider.SpanDuration;

        protected override void ApplyDefaultsToSelf(ControlPointInfo controlPointInfo, IBeatmapDifficultyInfo difficulty)
        {
            base.ApplyDefaultsToSelf(controlPointInfo, difficulty);

            if (RepeatIndex > 0)
            {
                // Repeat points after the first span should appear behind the still-visible one.
                TimeFadeIn = 0;

                // The next end circle should appear exactly after the previous circle (on the same end) is hit.
                TimePreempt = SpanDuration * 2;
            }
            else
            {
                // The first end circle should fade in with the slider.
                TimePreempt += StartTime - Slider.StartTime;
            }
        }
    }

    public class SliderTailCircle : SliderEndCircle { public bool ClassicSliderBehaviour; public SliderTailCircle(Slider slider) : base(slider) { } }
    public class SliderRepeat : SliderEndCircle { public double PathProgress { get; set; } public SliderRepeat(Slider slider) : base(slider) { } }

    public class SliderTick : OsuHitObject
    {
        public int SpanIndex { get; set; }
        public double SpanStartTime { get; set; }
        public double PathProgress { get; set; }

        protected override void ApplyDefaultsToSelf(ControlPointInfo controlPointInfo, IBeatmapDifficultyInfo difficulty)
        {
            base.ApplyDefaultsToSelf(controlPointInfo, difficulty);

            double offset;

            if (SpanIndex > 0)
                // Adding 200 to include the offset stable used.
                // This is so on repeats ticks don't appear too late to be visually processed by the player.
                offset = 200;
            else
                offset = TimePreempt * 0.66f;

            TimePreempt = (StartTime - SpanStartTime) / 2 + offset;
        }
    }

    public class Slider : OsuHitObject, IHasPathWithRepeats, IHasSliderVelocity, IHasDuration
    {
        public double EndTime => StartTime + this.SpanCount() * Path.Distance / Velocity;
        public double Duration
        {
            get => EndTime - StartTime;
            set => throw new System.NotSupportedException($"Adjust via {nameof(RepeatCount)} instead"); // can be implemented if/when needed.
        }
        public override Vector2 EndPosition => Position + this.CurvePositionAt(1);
        private readonly Cached endPositionCache = new Cached();

        // Shell of the owned path of lazer's Slider (Slider.cs lines 45-57). With OwnsPath the setter copies
        // the control points and the expected distance into a path with OptimiseCatmull = true, as lazer does,
        // and then recomputes the nested positions (lazer's Path.Version.ValueChanged handler). Without it the
        // assigned path is used as it is, which is how the older generators build their sliders.
        public bool OwnsPath;
        private SliderPath path = new SliderPath(Array.Empty<PathControlPoint>(), null, true);

        public SliderPath Path
        {
            get => path;
            set
            {
                if (!OwnsPath)
                {
                    path = value;
                    return;
                }

                path = new SliderPath(value.ControlPoints.Select(c => new PathControlPoint(c.Position, c.Type)).ToArray(), value.ExpectedDistance.Value, true);
                updateNestedPositions();
            }
        }

        public override Vector2 Position
        {
            get => base.Position;
            set
            {
                base.Position = value;
                updateNestedPositions();
            }
        }

        private void updateNestedPositions()
        {
            endPositionCache.Invalidate();

            foreach (var nested in NestedHitObjects)
            {
                switch (nested)
                {
                    case SliderHeadCircle headCircle:
                        headCircle.Position = Position;
                        break;

                    case SliderTailCircle tailCircle:
                        tailCircle.Position = EndPosition;
                        break;

                    case SliderRepeat repeat:
                        repeat.Position = Position + Path.PositionAt(repeat.PathProgress);
                        break;

                    case SliderTick tick:
                        tick.Position = Position + Path.PositionAt(tick.PathProgress);
                        break;
                }
            }
        }

        public int RepeatCount { get; set; }
        public double SliderVelocityMultiplier { get; set; } = 1;
        public bool GenerateTicks { get; set; } = true;
        public bool ClassicSliderBehaviour;
        public SliderHeadCircle HeadCircle { get; protected set; }
        public SliderTailCircle TailCircle { get; protected set; }
        public SliderRepeat LastRepeat { get; protected set; }
        /// <summary>
        /// The length of one span of this <see cref="Slider"/>.
        /// </summary>
        public double SpanDuration => Duration / this.SpanCount();

        /// <summary>
        /// The computed velocity of this <see cref="Slider"/>. This is the amount of path distance travelled in 1 ms.
        /// </summary>
        public double Velocity { get; private set; }

        /// <summary>
        /// Spacing between <see cref="SliderTick"/>s of this <see cref="Slider"/>.
        /// </summary>
        public double TickDistance { get; private set; }

        /// <summary>
        /// An extra multiplier that affects the number of <see cref="SliderTick"/>s generated by this <see cref="Slider"/>.
        /// An increase in this value increases <see cref="TickDistance"/>, which reduces the number of ticks generated.
        /// </summary>
        public double TickDistanceMultiplier = 1;

        protected override void ApplyDefaultsToSelf(ControlPointInfo controlPointInfo, IBeatmapDifficultyInfo difficulty)
        {
            base.ApplyDefaultsToSelf(controlPointInfo, difficulty);

            TimingControlPoint timingPoint = controlPointInfo.TimingPointAt(StartTime);

            Velocity = BASE_SCORING_DISTANCE * difficulty.SliderMultiplier / LegacyRulesetExtensions.GetPrecisionAdjustedBeatLength(this, timingPoint, OsuRuleset.SHORT_NAME);
            // WARNING: this is intentionally not computed as `BASE_SCORING_DISTANCE * difficulty.SliderMultiplier`
            // for backwards compatibility reasons (intentionally introducing floating point errors to match stable).
            double scoringDistance = Velocity * timingPoint.BeatLength;

            TickDistance = GenerateTicks ? (scoringDistance / difficulty.SliderTickRate * TickDistanceMultiplier) : double.PositiveInfinity;
        }

        protected override void CreateNestedHitObjects(CancellationToken cancellationToken)
        {
            base.CreateNestedHitObjects(cancellationToken);

            var sliderEvents = SliderEventGenerator.Generate(StartTime, SpanDuration, Velocity, TickDistance, Path.Distance, this.SpanCount(), cancellationToken);

            foreach (var e in sliderEvents)
            {
                switch (e.Type)
                {
                    case SliderEventType.Tick:
                        AddNested(new SliderTick
                        {
                            SpanIndex = e.SpanIndex,
                            SpanStartTime = e.SpanStartTime,
                            StartTime = e.Time,
                            Position = Position + Path.PositionAt(e.PathProgress),
                            PathProgress = e.PathProgress,
                            StackHeight = StackHeight,
                        });
                        break;

                    case SliderEventType.Head:
                        AddNested(HeadCircle = new SliderHeadCircle
                        {
                            StartTime = e.Time,
                            Position = Position,
                            StackHeight = StackHeight,
                            ClassicSliderBehaviour = ClassicSliderBehaviour,
                        });
                        break;

                    case SliderEventType.Tail:
                        AddNested(TailCircle = new SliderTailCircle(this)
                        {
                            RepeatIndex = e.SpanIndex,
                            StartTime = e.Time,
                            Position = EndPosition,
                            StackHeight = StackHeight,
                            ClassicSliderBehaviour = ClassicSliderBehaviour,
                        });
                        break;

                    case SliderEventType.Repeat:
                        AddNested(LastRepeat = new SliderRepeat(this)
                        {
                            RepeatIndex = e.SpanIndex,
                            StartTime = StartTime + (e.SpanIndex + 1) * SpanDuration,
                            Position = Position + Path.PositionAt(e.PathProgress),
                            StackHeight = StackHeight,
                            PathProgress = e.PathProgress,
                        });
                        break;
                }
            }

        }
    }
}
