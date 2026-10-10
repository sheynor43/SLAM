// Extracted verbatim from osu-framework 2026.921.1: osu.Framework/Graphics/Vector2Extensions.cs (lines 1-2, 9-12, 32-54).
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
using System; using osuTK;
namespace osu.Framework.Graphics
{
    public static class Vector2Extensions
    {

        /// <summary>
        /// Compute the euclidean distance between two vectors.
        /// </summary>
        /// <param name="vec1">The first vector</param>
        /// <param name="vec2">The second vector</param>
        /// <returns>The distance</returns>
        public static float Distance(Vector2 vec1, Vector2 vec2)
        {
            Distance(ref vec1, ref vec2, out float result);
            return result;
        }

        /// <summary>
        /// Compute the euclidean distance between two vectors.
        /// </summary>
        /// <param name="vec1">The first vector</param>
        /// <param name="vec2">The second vector</param>
        /// <param name="result">The distance</param>
        public static void Distance(ref Vector2 vec1, ref Vector2 vec2, out float result)
        {
            result = MathF.Sqrt((vec2.X - vec1.X) * (vec2.X - vec1.X) + (vec2.Y - vec1.Y) * (vec2.Y - vec1.Y));
        }
    }
}
