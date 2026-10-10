// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModHardRock.cs (lines 23-28), ModEasy.cs (lines 51-58), ModDifficultyAdjust.cs (lines 116-126), osu.Game.Rulesets.Osu/Mods/OsuModHardRock.cs (lines 78-92), OsuModEasy.cs (lines 108-114), OsuModDifficultyAdjust.cs (lines 78-84), OsuModMirror.cs (lines 138-157), osu.Game.Rulesets.Osu/Utils/OsuHitObjectGenerationUtils.cs (lines 112-142, 163-170), osu.Game.Rulesets.Osu/UI/OsuPlayfield.cs (line 47). The mods are shells holding only the methods applied to the map; the order in which GetPlayableBeatmap applies them is in MapMods.cs; see README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.

using System;
using System.Linq;
using osu.Game.Beatmaps;
using osu.Game.Rulesets.Mods;
using osu.Game.Rulesets.Objects;
using osu.Game.Rulesets.Osu.Objects;
using osu.Game.Rulesets.Osu.UI;
using osu.Game.Rulesets.Osu.Utils;
using osuTK;

namespace osu.Game.Rulesets.Mods
{
    public abstract class ModHardRock : IApplicableToDifficulty
    {
        protected const float ADJUST_RATIO = 1.4f;

        public virtual void ApplyToDifficulty(BeatmapDifficulty difficulty)
        {
            difficulty.DrainRate = Math.Min(difficulty.DrainRate * ADJUST_RATIO, 10.0f);
        }
    }

    public abstract class ModEasy : IApplicableToDifficulty
    {
        protected const float ADJUST_RATIO = 0.5f;

        public virtual void ApplyToDifficulty(BeatmapDifficulty difficulty)
        {
            difficulty.CircleSize *= ADJUST_RATIO;
            difficulty.ApproachRate *= ADJUST_RATIO;
            difficulty.DrainRate *= ADJUST_RATIO;
        }
    }

    public abstract partial class ModDifficultyAdjust
    {
        public void ApplyToDifficulty(BeatmapDifficulty difficulty) => ApplySettings(difficulty);

        /// <summary>
        /// Apply all custom settings to the provided beatmap.
        /// </summary>
        /// <param name="difficulty">The beatmap to have settings applied.</param>
        protected virtual void ApplySettings(BeatmapDifficulty difficulty)
        {
            if (DrainRate.Value != null) difficulty.DrainRate = DrainRate.Value.Value;
            if (OverallDifficulty.Value != null) difficulty.OverallDifficulty = OverallDifficulty.Value.Value;
        }
    }
}

namespace osu.Game.Rulesets.Osu.Mods
{
    public class OsuModHardRock : ModHardRock, IApplicableToHitObject
    {
        public void ApplyToHitObject(HitObject hitObject)
        {
            var osuObject = (OsuHitObject)hitObject;

            OsuHitObjectGenerationUtils.ReflectVerticallyAlongPlayfield(osuObject);
        }

        public override void ApplyToDifficulty(BeatmapDifficulty difficulty)
        {
            base.ApplyToDifficulty(difficulty);

            difficulty.OverallDifficulty = Math.Min(difficulty.OverallDifficulty * ADJUST_RATIO, 10.0f);
            difficulty.CircleSize = Math.Min(difficulty.CircleSize * 1.3f, 10.0f); // CS uses a custom 1.3 ratio.
            difficulty.ApproachRate = Math.Min(difficulty.ApproachRate * ADJUST_RATIO, 10.0f);
        }
    }

    public partial class OsuModEasy
    {
        public override void ApplyToDifficulty(BeatmapDifficulty difficulty)
        {
            base.ApplyToDifficulty(difficulty);

            difficulty.OverallDifficulty *= ADJUST_RATIO;
        }
    }

    public partial class OsuModDifficultyAdjust
    {
        protected override void ApplySettings(BeatmapDifficulty difficulty)
        {
            base.ApplySettings(difficulty);

            if (CircleSize.Value != null) difficulty.CircleSize = CircleSize.Value.Value;
            if (ApproachRate.Value != null) difficulty.ApproachRate = ApproachRate.Value.Value;
        }
    }

    public partial class OsuModMirror
    {
        public void ApplyToHitObject(HitObject hitObject)
        {
            var osuObject = (OsuHitObject)hitObject;

            switch (Reflection.Value)
            {
                case MirrorType.Horizontal:
                    OsuHitObjectGenerationUtils.ReflectHorizontallyAlongPlayfield(osuObject);
                    break;

                case MirrorType.Vertical:
                    OsuHitObjectGenerationUtils.ReflectVerticallyAlongPlayfield(osuObject);
                    break;

                case MirrorType.Both:
                    OsuHitObjectGenerationUtils.ReflectHorizontallyAlongPlayfield(osuObject);
                    OsuHitObjectGenerationUtils.ReflectVerticallyAlongPlayfield(osuObject);
                    break;
            }
        }
    }
}

namespace osu.Game.Rulesets.Osu.UI
{
    public static class OsuPlayfield
    {
        public static readonly Vector2 BASE_SIZE = new Vector2(512, 384);
    }
}

namespace osu.Game.Rulesets.Osu.Utils
{
    public static partial class OsuHitObjectGenerationUtils
    {
        /// <summary>
        /// Reflects the position of the <see cref="OsuHitObject"/> in the playfield horizontally.
        /// </summary>
        /// <param name="osuObject">The object to reflect.</param>
        public static void ReflectHorizontallyAlongPlayfield(OsuHitObject osuObject)
        {
            osuObject.Position = new Vector2(OsuPlayfield.BASE_SIZE.X - osuObject.X, osuObject.Position.Y);

            if (osuObject is not Slider slider)
                return;

            static void reflectControlPoint(PathControlPoint point) => point.Position = new Vector2(-point.Position.X, point.Position.Y);

            modifySlider(slider, reflectControlPoint);
        }

        /// <summary>
        /// Reflects the position of the <see cref="OsuHitObject"/> in the playfield vertically.
        /// </summary>
        /// <param name="osuObject">The object to reflect.</param>
        public static void ReflectVerticallyAlongPlayfield(OsuHitObject osuObject)
        {
            osuObject.Position = new Vector2(osuObject.Position.X, OsuPlayfield.BASE_SIZE.Y - osuObject.Y);

            if (osuObject is not Slider slider)
                return;

            static void reflectControlPoint(PathControlPoint point) => point.Position = new Vector2(point.Position.X, -point.Position.Y);

            modifySlider(slider, reflectControlPoint);
        }

        private static void modifySlider(Slider slider, Action<PathControlPoint> modifyControlPoint)
        {
            var controlPoints = slider.Path.ControlPoints.Select(p => new PathControlPoint(p.Position, p.Type)).ToArray();
            foreach (var point in controlPoints)
                modifyControlPoint(point);

            slider.Path = new SliderPath(controlPoints, slider.Path.ExpectedDistance.Value);
        }
    }
}
