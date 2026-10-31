//! Slider body presentation (ADR-0041): the classic skin's colours and gradient, the
//! snaking range and the mesh of a slider's path. Drawing is `slam_render::slider`.

use slam_osu::SliderPath;
use slam_osu::objects::Slider;
use slam_render::slider::{SliderGradient, SliderMesh};
use slam_skin::{Rgba, Skin, SkinChain};

/// A straight-alpha colour as lazer's `Color4`: `[r, g, b, a]` in `0..=1`.
pub type Colour4 = [f32; 4];

const WHITE: Colour4 = [1.0; 4];

fn colour4(c: Rgba) -> Colour4 {
    [c.r, c.g, c.b, c.a].map(|v| f32::from(v) / 255.0)
}

/// Colours of a classic-skin slider body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegacyBodyColours {
    /// `SliderTrackOverride` or the combo colour, at alpha 0.7.
    pub accent: Colour4,
    /// `SliderBorder`, white by default.
    pub border: Colour4,
}

impl LegacyBodyColours {
    /// The colours lazer's `LegacySliderBody` reads through the skin source: the first
    /// skin of the chain that sets the key. `combo` is the slider's combo colour (its
    /// `AccentColour`).
    ///
    /// These are `OsuSkinColour` lookups, which lazer gates by `BeatmapSkins` rather than
    /// `BeatmapColours`: the beatmap's `SliderBorder` and `SliderTrackOverride` apply only
    /// when the chain has its beatmap layer.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBody.cs (GetBorderColour, GetBodyAccentColour)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/SkinProvidingContainer.cs (DisableableSkinSource.GetConfig)
    pub fn new(combo: Colour4, skins: &SkinChain) -> Self {
        let lookup = |get: fn(&Skin) -> Option<Rgba>| skins.config(get).map(|found| found.value);
        let [r, g, b, _] = lookup(Skin::slider_track_override).map_or(combo, colour4);
        Self {
            // Legacy skins use a constant slider track alpha whatever the source colour.
            accent: [r, g, b, 0.7],
            border: lookup(Skin::slider_border).map_or(WHITE, colour4),
        }
    }

    /// The colours Traceable gives a body of `combo` colour: a transparent track and a border
    /// of the combo colour, whatever the skin.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModTraceable.cs (applySliderState)
    pub fn traceable(combo: Colour4) -> Self {
        let [r, g, b, _] = combo;
        Self {
            accent: [r, g, b, 0.0],
            border: combo,
        }
    }

    /// The body's colour at `position` across the path: 0 at the edge, 1 at the
    /// centre (osu!stable's `MmSliderRendererGL`: shadow, border, then the track).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBody.cs (LegacyDrawableSliderPath.ColourAt)
    pub fn colour_at(&self, position: f32) -> Colour4 {
        const LEGACY_CIRCLE_RADIUS: f32 = slam_osu::objects::OBJECT_RADIUS - 5.0;
        const SHADOW_PORTION: f32 = 1.0 - (LEGACY_CIRCLE_RADIUS / slam_osu::objects::OBJECT_RADIUS);
        const BORDER_PORTION: f32 = 0.1875;

        let shadow = [0.0, 0.0, 0.0, 0.25];
        let outer = darken(self.accent, 0.1);
        let inner = lighten(self.accent, 0.5);

        if position <= SHADOW_PORTION {
            return interpolate_non_linear(position, [0.0; 4], shadow, 0.0, SHADOW_PORTION);
        }
        if position <= BORDER_PORTION {
            return self.border;
        }
        interpolate_non_linear(position, outer, inner, BORDER_PORTION, 1.0)
    }

    /// The gradient of a body of `radius` (osu! pixels).
    pub fn gradient(&self, radius: f32) -> SliderGradient {
        SliderGradient::new(radius, |p| self.colour_at(p))
    }
}

// Ported from osu!lazer 2026.1005.0-lazer (osu-framework 2026.921.1): osu.Framework/Extensions/Color4Extensions/Color4Extensions.cs (Darken, Multiply)
fn darken([r, g, b, a]: Colour4, amount: f32) -> Colour4 {
    let scalar = 1.0 / (1.0 + amount);
    [
        (r * scalar).min(1.0),
        (g * scalar).min(1.0),
        (b * scalar).min(1.0),
        a,
    ]
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBody.cs (lighten)
fn lighten([r, g, b, a]: Colour4, amount: f32) -> Colour4 {
    let amount = amount * 0.5;
    let l = |c: f32| (c * (1.0 + 0.5 * amount) + amount).min(1.0);
    [l(r), l(g), l(b), a]
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/LegacyUtils.cs (InterpolateNonLinear, Easing.None)
fn interpolate_non_linear(
    time: f32,
    start: Colour4,
    end: Colour4,
    start_time: f32,
    end_time: f32,
) -> Colour4 {
    if start == end {
        return start;
    }
    let current = f64::from(time) - f64::from(start_time);
    let duration = f64::from(end_time) - f64::from(start_time);
    if duration == 0.0 || current == 0.0 {
        return start;
    }
    let t = ((current / duration) as f32).clamp(0.0, 1.0);
    std::array::from_fn(|i| start[i] + t * (end[i] - start[i]))
}

/// Snaking settings (`OsuRulesetSetting.SnakingInSliders` / `SnakingOutSliders`).
/// Both off by default, unlike lazer: bodies show whole from the start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Snaking {
    pub snaking_in: bool,
    pub snaking_out: bool,
}

impl Snaking {
    /// Lazer's defaults: bodies snake in and out.
    pub const LAZER: Snaking = Snaking {
        snaking_in: true,
        snaking_out: true,
    };
}

/// What snaking needs of a slider: its timing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnakingTiming {
    pub start_time: f64,
    /// `Slider.Duration`: from the start to the end time.
    pub duration: f64,
    /// `Slider.SpanCount`: repeats plus one.
    pub span_count: i32,
    pub time_preempt: f64,
}

impl SnakingTiming {
    /// The timing of `slider` starting at `start_time` with `time_preempt`.
    pub fn new(slider: &Slider, start_time: f64, time_preempt: f64) -> Self {
        Self {
            start_time,
            duration: slider.duration(start_time),
            span_count: slider.span_count(),
            time_preempt,
        }
    }
}

/// The visible part of a slider body at `time`, as progress along the path
/// `(start, end)` with `start <= end`. `head_hit`: the head circle has been hit
/// (lazer passes progress 0 until then, so snaking out waits for it).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SnakingSliderBody.cs (UpdateProgress, setRange)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (Update)
pub fn snaking_range(
    slider: &SnakingTiming,
    time: f64,
    head_hit: bool,
    snaking: Snaking,
) -> (f64, f64) {
    let SnakingTiming {
        start_time,
        duration,
        span_count,
        time_preempt,
    } = *slider;
    let completion = if head_hit {
        ((time - start_time) / duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // IHasPathWithRepeats.SpanAt and ProgressAt; C#'s (int) cast saturates like `as`.
    let span = (completion * f64::from(span_count)) as i32;
    let mut span_progress = completion * f64::from(span_count) % 1.0;
    if span % 2 == 1 {
        span_progress = 1.0 - span_progress;
    }

    let mut start = 0.0;
    let mut end = if snaking.snaking_in {
        ((time - (start_time - time_preempt)) / (time_preempt / 3.0)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    if span >= span_count - 1 {
        if span.min(span_count - 1) % 2 == 1 {
            start = 0.0;
            end = if snaking.snaking_out {
                span_progress
            } else {
                1.0
            };
        } else {
            start = if snaking.snaking_out {
                span_progress
            } else {
                0.0
            };
        }
    }
    if start > end {
        (start, end) = (end, start);
    }
    (start, end)
}

/// A snaking range in path length for the mesh of a path `distance` long, as
/// `SliderPath.progressToDistance`.
pub fn range_distances(distance: f64, (start, end): (f64, f64)) -> (f32, f32) {
    let d = |p: f64| (p.clamp(0.0, 1.0) * distance) as f32;
    (d(start), d(end))
}

/// The body mesh of a path (at load).
pub fn body_mesh(path: &SliderPath) -> SliderMesh {
    let points: Vec<[f32; 2]> = path.calculated_path().iter().map(|p| [p.x, p.y]).collect();
    // The lengths can have one extra entry (extension skipped when the last two points
    // coincide); `GetPathToProgress` reads only one per point.
    let cumulative = path.cumulative_length();
    SliderMesh::new(&points, &cumulative[..points.len().min(cumulative.len())])
}
