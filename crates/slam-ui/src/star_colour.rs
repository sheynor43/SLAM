//! The colour of a star rating (ADR-0131): the spectrum osu!lazer paints difficulty badges,
//! icons and spectrum bars with, from grey-blue through green, yellow and pink to purple and
//! black. Colours are sRGB-encoded, like [`Color`]; the gradient is mixed in linear light as
//! lazer does.

use slam_render::Color;

/// Stars below which lazer's badge text is dark.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuColour.cs
// (`STAR_DIFFICULTY_DEFINED_COLOUR_CUTOFF`)
const DEFINED_COLOUR_CUTOFF: f32 = 6.5;

/// Stars from which lazer's badge text follows a gradient instead of one colour.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuColour.cs
// (`STAR_DIFFICULTY_TEXT_GRADIENT_CUTOFF`)
const TEXT_GRADIENT_CUTOFF: f32 = 9.0;

const fn hex(rgb: u32) -> Color {
    Color::rgb(
        ((rgb >> 16) & 255) as f32 / 255.0,
        ((rgb >> 8) & 255) as f32 / 255.0,
        (rgb & 255) as f32 / 255.0,
    )
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuColour.cs
// (`STAR_DIFFICULTY_SPECTRUM`)
const SPECTRUM: [(f32, Color); 13] = [
    (0.1, hex(0xaaaaaa)),
    (0.1, hex(0x4290fb)),
    (1.25, hex(0x4fc0ff)),
    (2.0, hex(0x4fffd5)),
    (2.5, hex(0x7cff4f)),
    (3.3, hex(0xf6f05c)),
    (4.2, hex(0xff8068)),
    (4.9, hex(0xff4e6f)),
    (5.8, hex(0xc645b8)),
    (6.7, hex(0x6563de)),
    (7.7, hex(0x18158e)),
    (9.0, Color::BLACK),
    (10.0, Color::BLACK),
];

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuColour.cs
// (`STAR_DIFFICULTY_TEXT_SPECTRUM`)
const TEXT_SPECTRUM: [(f32, Color); 5] = [
    (9.0, hex(0xf6f05c)),
    (9.9, hex(0xff8068)),
    (10.6, hex(0xff4e6f)),
    (11.5, hex(0xc645b8)),
    (12.4, hex(0x6563de)),
];

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuColour.cs (`Orange1`)
const ORANGE: Color = hex(0xffd966);

// Ported from osu-framework 2026.921.1:
// osu.Framework/Extensions/Color4Extensions/Color4Extensions.cs (`ToLinear`)
fn to_linear(c: f32) -> f32 {
    if c == 1.0 {
        return 1.0;
    }
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

// Ported from osu-framework 2026.921.1:
// osu.Framework/Extensions/Color4Extensions/Color4Extensions.cs (`ToSRGB`)
fn to_srgb(c: f32) -> f32 {
    if c == 1.0 {
        return 1.0;
    }
    if c < 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/ColourUtils.cs
// (`SampleFromLinearGradient`) and osu-framework 2026.921.1:
// osu.Framework/Utils/Interpolation.cs (`ValueAt` for `Color4`: mixed in linear light)
fn sample(gradient: &[(f32, Color)], point: f32) -> Color {
    if point < gradient[0].0 {
        return gradient[0].1;
    }
    for pair in gradient.windows(2) {
        let ((start, from), (end, to)) = (pair[0], pair[1]);
        if point >= end {
            continue;
        }
        if from == to
            || f64::from(point) - f64::from(start) == 0.0
            || f64::from(end) - f64::from(start) == 0.0
        {
            return from;
        }
        // The fraction is a double in lazer, narrowed to a float after the division.
        let t = (((f64::from(point) - f64::from(start)) / (f64::from(end) - f64::from(start)))
            as f32)
            .clamp(0.0, 1.0);
        let mix = |a: f32, b: f32| {
            let (a, b) = (to_linear(a), to_linear(b));
            to_srgb(a + t * (b - a))
        };
        return Color::rgb(mix(from.r, to.r), mix(from.g, to.g), mix(from.b, to.b));
    }
    gradient[gradient.len() - 1].1
}

/// Rounds half away from zero to two decimals in double precision, as lazer's
/// `Math.Round(double, 2, MidpointRounding.AwayFromZero)` does before sampling.
fn round_hundredths(stars: f32) -> f32 {
    ((f64::from(stars) * 100.0).round() / 100.0) as f32
}

/// The colour of `stars`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuColour.cs
// (`ForStarDifficulty`)
pub fn for_stars(stars: f32) -> Color {
    sample(&SPECTRUM, round_hundredths(stars))
}

/// The colour of the text on a badge in [`for_stars`]'s colour: dark on the light colours.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuColour.cs
// (`ForStarDifficultyText`)
pub fn text_for_stars(stars: f32) -> Color {
    if stars < DEFINED_COLOUR_CUTOFF {
        return Color {
            a: 0.75,
            ..Color::BLACK
        };
    }
    if stars < TEXT_GRADIENT_CUTOFF {
        return ORANGE;
    }
    sample(&TEXT_SPECTRUM, round_hundredths(stars))
}

/// [`for_stars`] for a glow or a strip on a dark interface: lifted towards white until it
/// is as bright as a mid grey, because the spectrum ends in black. `None` (not calculated
/// yet) gives `fallback`.
pub fn glow_for(stars: Option<f32>, fallback: Color) -> Color {
    let Some(stars) = stars else {
        return fallback;
    };
    let c = for_stars(stars);
    let luma = 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
    const FLOOR: f32 = 0.35;
    if luma >= FLOOR {
        return c;
    }
    let lift = (FLOOR - luma) / (1.0 - luma);
    Color::rgb(
        c.r + (1.0 - c.r) * lift,
        c.g + (1.0 - c.g) * lift,
        c.b + (1.0 - c.b) * lift,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb8(c: Color) -> [u8; 3] {
        [c.r, c.g, c.b].map(|x| (x * 255.0).round() as u8)
    }

    #[test]
    fn the_stops_give_lazers_colours_exactly() {
        let expected = [
            (0.1, [0x42, 0x90, 0xfb]),
            (1.25, [0x4f, 0xc0, 0xff]),
            (2.0, [0x4f, 0xff, 0xd5]),
            (2.5, [0x7c, 0xff, 0x4f]),
            (3.3, [0xf6, 0xf0, 0x5c]),
            (4.2, [0xff, 0x80, 0x68]),
            (4.9, [0xff, 0x4e, 0x6f]),
            (5.8, [0xc6, 0x45, 0xb8]),
            (6.7, [0x65, 0x63, 0xde]),
            (7.7, [0x18, 0x15, 0x8e]),
            (9.0, [0, 0, 0]),
            (10.0, [0, 0, 0]),
            (14.0, [0, 0, 0]),
        ];
        for (stars, rgb) in expected {
            assert_eq!(rgb8(for_stars(stars)), rgb, "{stars}");
        }
        // Below the first stop: grey.
        assert_eq!(rgb8(for_stars(0.0)), [0xaa, 0xaa, 0xaa]);
        assert_eq!(rgb8(for_stars(0.05)), [0xaa, 0xaa, 0xaa]);
        assert_eq!(for_stars(3.3).a, 1.0);
    }

    #[test]
    fn between_the_stops_it_mixes_in_linear_light() {
        // Half way between 2.0 (4fffd5) and 2.5 (7cff4f): green stays full, and the mix of
        // the other channels is done on linear values, not on the encoded ones.
        let c = for_stars(2.25);
        let (a, b) = (0x4f as f32 / 255.0, 0x7c as f32 / 255.0);
        let linear_red = to_srgb((to_linear(a) + to_linear(b)) / 2.0);
        assert!((c.r - linear_red).abs() < 1e-5);
        assert!(c.r > (a + b) / 2.0);
        assert!((c.g - 1.0).abs() < 1e-5);
        // Rounded to hundredths first: 2.504 samples as 2.5.
        assert_eq!(for_stars(2.504), for_stars(2.5));
    }

    #[test]
    fn the_conversions_keep_one_and_split_where_lazer_does() {
        assert_eq!(to_linear(1.0), 1.0);
        assert_eq!(to_srgb(1.0), 1.0);
        // `<` below the sRGB threshold, so the threshold itself takes the power branch.
        let edge = 0.003_130_8_f32;
        assert_eq!(to_srgb(edge), 1.055 * edge.powf(1.0 / 2.4) - 0.055);
        assert_eq!(to_srgb(edge * 0.5), 12.92 * edge * 0.5);
        assert_eq!(to_linear(0.04045), 0.04045 / 12.92);
    }

    #[test]
    fn rounding_is_done_in_double_precision() {
        // f32 2.675 is 2.67499995...: below the midpoint in double precision.
        assert_eq!(round_hundredths(2.675), 2.67);
        assert_eq!(round_hundredths(2.504), 2.5);
        assert_eq!(round_hundredths(2.5), 2.5);
        assert_eq!(round_hundredths(0.5), 0.5);
        assert_eq!(round_hundredths(0.0), 0.0);
    }

    #[test]
    fn badge_text_follows_lazer() {
        assert_eq!(text_for_stars(3.0).a, 0.75);
        assert_eq!(rgb8(text_for_stars(3.0)), [0, 0, 0]);
        assert_eq!(rgb8(text_for_stars(6.5)), [0xff, 0xd9, 0x66]);
        assert_eq!(rgb8(text_for_stars(9.0)), [0xf6, 0xf0, 0x5c]);
        assert_eq!(rgb8(text_for_stars(12.4)), [0x65, 0x63, 0xde]);
    }

    #[test]
    fn a_glow_is_never_black_and_unknown_stars_use_the_fallback() {
        let accent = Color::rgb(0.2, 0.4, 0.9);
        assert_eq!(glow_for(None, accent), accent);
        assert_eq!(glow_for(Some(3.3), accent), for_stars(3.3));
        let black = glow_for(Some(9.5), accent);
        let luma = 0.2126 * black.r + 0.7152 * black.g + 0.0722 * black.b;
        assert!((luma - 0.35).abs() < 1e-4);
    }
}
