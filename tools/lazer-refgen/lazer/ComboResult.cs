// Extracted verbatim from osu!lazer 2026.1005.0-lazer: osu/osu.Game.Rulesets.Osu/Judgements/ComboResult.cs (lines 1-19); Doc comments are removed. See README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
using System.ComponentModel;

namespace osu.Game.Rulesets.Osu.Judgements
{
    public enum ComboResult
    {
        [Description(@"")]
        None,

        [Description(@"Good")]
        Good,

        [Description(@"Amazing")]
        Perfect
    }
}
