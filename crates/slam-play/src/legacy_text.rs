//! Numbers in a legacy skin font (`LegacySpriteText`): one texture per character,
//! `{prefix}-0` to `{prefix}-9` and `{prefix}-comma`, `-dot`, `-percent`, `-x`, laid out side
//! by side with the font's overlap. The score and accuracy counters lay their digits out at a
//! fixed width (`FixedWidth`), the width of `5`.

use slam_osu::Vec2;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};

use crate::circles::SkinSprite;

/// The characters of a legacy font beside the digits, in [`FontSprites::symbols`] order.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySpriteText.cs (getLookupName)
pub const SYMBOLS: [(u8, &str); 4] = [
    (b',', "comma"),
    (b'.', "dot"),
    (b'%', "percent"),
    (b'x', "x"),
];

/// No symbols: for fonts that draw digits only.
pub const NO_SYMBOLS: [Option<SkinSprite>; 4] = [None; 4];

/// The texture names of a font with `prefix`: the digits, then [`SYMBOLS`].
pub fn glyph_names(prefix: &str) -> impl Iterator<Item = String> + '_ {
    (0..10)
        .map(|d| d.to_string())
        .chain(SYMBOLS.iter().map(|(_, name)| (*name).to_owned()))
        .map(move |glyph| format!("{prefix}-{glyph}"))
}

/// The digits of a number as text, most significant first, without allocating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Digits {
    digits: [u8; 20],
    len: usize,
}

impl Digits {
    /// The decimal digits of `number`: one `0` for zero.
    pub fn new(number: u64) -> Self {
        let mut digits = [0u8; 20];
        let mut len = 0;
        let mut n = number;
        loop {
            digits[len] = b'0' + (n % 10) as u8;
            len += 1;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        digits[..len].reverse();
        Digits { digits, len }
    }

    /// The digits as ASCII text.
    pub fn as_bytes(&self) -> &[u8] {
        &self.digits[..self.len]
    }
}

/// A legacy font's sprites and overlap, as a frame's textures hold them; missing characters
/// are left out.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FontSprites {
    pub digits: [Option<SkinSprite>; 10],
    /// The characters of [`SYMBOLS`].
    pub symbols: [Option<SkinSprite>; 4],
    /// The font's overlap (`ScoreOverlap`, `ComboOverlap`).
    pub overlap: f32,
}

impl FontSprites {
    /// Whether every digit is there: a counter draws in the font only then.
    pub fn has_digits(&self) -> bool {
        self.digits.iter().all(Option::is_some)
    }

    /// The font, with digits at a fixed width or not.
    pub fn font(&self, fixed_width: bool) -> LegacyFont<'_> {
        LegacyFont {
            digits: &self.digits,
            symbols: &self.symbols,
            overlap: self.overlap,
            fixed_width,
        }
    }
}

/// A legacy font's textures and overlap; missing characters are left out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegacyFont<'a> {
    pub digits: &'a [Option<SkinSprite>; 10],
    /// The characters of [`SYMBOLS`]; [`NO_SYMBOLS`] for fonts of digits only.
    pub symbols: &'a [Option<SkinSprite>; 4],
    /// Positive values bring characters closer together: the spacing is its negation.
    pub overlap: f32,
    /// Whether digits take the width of `5`, centred in it; the symbols keep their own.
    pub fixed_width: bool,
}

/// A glyph placed on a line: its left edge, its sprite.
struct Placed<'a> {
    x: f32,
    sprite: &'a SkinSprite,
}

impl<'a> LegacyFont<'a> {
    /// The sprite of the ASCII character `c` and whether it is a digit.
    fn glyph(&self, c: u8) -> Option<(&'a SkinSprite, bool)> {
        if c.is_ascii_digit() {
            return self.digits[usize::from(c - b'0')]
                .as_ref()
                .map(|s| (s, true));
        }
        SYMBOLS
            .iter()
            .position(|(symbol, _)| *symbol == c)
            .and_then(|i| self.symbols[i].as_ref())
            .map(|s| (s, false))
    }

    // Ported from osu-framework 2026.921.1: osu.Framework/Text/TextBuilder.cs (AddCharacter, getConstantWidth)
    // Ported from osu-framework 2026.921.1: osu.Framework/Text/TextBuilderGlyph.cs (XOffset, XAdvance)
    /// The glyphs of `text` at a scale of 1, left to right, and the line's width.
    fn layout(&self, text: &'a [u8]) -> (impl Iterator<Item = Placed<'a>> + 'a, f32) {
        let constant = self
            .fixed_width
            .then(|| self.digits[5].map_or(0.0, |s| s.size.x));
        let font = *self;
        let mut pen = 0.0;
        let mut first = true;
        let mut width = 0.0;
        // Spacing goes between glyphs only: from the second one on.
        for (sprite, digit) in text.iter().filter_map(|&c| font.glyph(c)) {
            if !first {
                pen -= font.overlap;
            }
            first = false;
            let advance = match constant {
                Some(w) if digit => w,
                _ => sprite.size.x,
            };
            pen += advance;
            width = pen;
        }
        let mut pen = 0.0;
        let mut first = true;
        let glyphs = text.iter().filter_map(move |&c| {
            let (sprite, digit) = font.glyph(c)?;
            if !first {
                pen -= font.overlap;
            }
            first = false;
            let (offset, advance) = match constant {
                Some(w) if digit => ((w - sprite.size.x) / 2.0, w),
                _ => (0.0, sprite.size.x),
            };
            let x = pen + offset;
            pen += advance;
            Some(Placed { x, sprite })
        });
        (glyphs, width)
    }

    /// The size of `text` (ASCII) at a scale of 1: glyphs are top-aligned
    /// (`UseFullGlyphHeight = false`), so the height is the tallest glyph's; the spacing is
    /// between glyphs only.
    pub fn measure(&self, text: &[u8]) -> Vec2 {
        let height = text
            .iter()
            .filter_map(|&c| self.glyph(c))
            .fold(0.0f32, |h, (s, _)| h.max(s.size.y));
        let (_, width) = self.layout(text);
        Vec2::new(width, height)
    }

    /// Writes `text` (ASCII) with the point `origin` of its box (fractions of its size: 0.5,
    /// 0.5 is the centre) at `at`, each display unit `scale` window units, blended with
    /// `blend`.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySpriteText.cs
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        text: &[u8],
        at: Vec2,
        origin: Vec2,
        scale: f32,
        colour: Color,
        blend: Blend,
    ) {
        if colour.a <= 0.0 {
            return;
        }
        let size = self.measure(text);
        let left = at.x - size.x * scale * origin.x;
        let top = at.y - size.y * scale * origin.y;
        let (glyphs, _) = self.layout(text);
        for Placed { x, sprite } in glyphs {
            batch.push(&Sprite {
                position: [left + x * scale, top],
                size: [sprite.size.x * scale, sprite.size.y * scale],
                origin: [0.0, 0.0],
                rotation: 0.0,
                colour,
                texture: sprite.texture,
                uv: sprite.uv,
                blend,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_most_significant_first() {
        assert_eq!(Digits::new(0).as_bytes(), b"0");
        assert_eq!(Digits::new(1050).as_bytes(), b"1050");
        assert_eq!(Digits::new(u64::MAX).as_bytes().len(), 20);
    }

    #[test]
    fn glyph_names_follow_the_prefix() {
        let names: Vec<String> = glyph_names("score").collect();
        assert_eq!(names[0], "score-0");
        assert_eq!(names[9], "score-9");
        assert_eq!(
            names[10..],
            ["score-comma", "score-dot", "score-percent", "score-x"]
        );
    }

    fn glyph(id: u32, width: f32, height: f32) -> Option<SkinSprite> {
        Some(SkinSprite {
            texture: slam_render::textures::TextureId(id),
            uv: slam_render::atlas::UvRect::FULL,
            size: Vec2::new(width, height),
        })
    }

    /// The top left and bottom right of each sprite of `batch`.
    fn boxes(batch: &SpriteBatch) -> Vec<([f32; 2], [f32; 2])> {
        batch
            .vertices()
            .chunks(4)
            .map(|v| (v[0].position, v[2].position))
            .collect()
    }

    #[test]
    fn digits_are_laid_out_with_the_overlap_around_the_origin() {
        // "1" is narrower and taller; "2" has no glyph and is left out.
        let mut digits: [Option<SkinSprite>; 10] =
            std::array::from_fn(|d| glyph(d as u32, 10.0, 20.0));
        digits[1] = glyph(1, 6.0, 24.0);
        digits[2] = None;
        let font = LegacyFont {
            digits: &digits,
            symbols: &NO_SYMBOLS,
            overlap: 2.0,
            fixed_width: false,
        };
        let number = Digits::new(1203);
        // 6 + 10 + 10 wide less two overlaps of 2, as tall as the "1".
        assert_eq!(font.measure(number.as_bytes()), Vec2::new(22.0, 24.0));

        let mut batch = SpriteBatch::new();
        let centre = Vec2::new(100.0, 50.0);
        font.push(
            &mut batch,
            number.as_bytes(),
            centre,
            Vec2::new(0.5, 0.5),
            2.0,
            Color::WHITE,
            Blend::Alpha,
        );
        // 44 × 48 centred, glyphs top-aligned.
        assert_eq!(
            boxes(&batch),
            [
                ([78.0, 26.0], [90.0, 74.0]),
                ([86.0, 26.0], [106.0, 66.0]),
                ([102.0, 26.0], [122.0, 66.0]),
            ]
        );

        // A top right origin; a negative overlap spaces the glyphs out.
        let spaced = LegacyFont {
            overlap: -1.0,
            ..font
        };
        batch.clear();
        spaced.push(
            &mut batch,
            b"30",
            centre,
            Vec2::new(1.0, 0.0),
            1.0,
            Color::WHITE,
            Blend::Alpha,
        );
        assert_eq!(
            boxes(&batch),
            [([79.0, 50.0], [89.0, 70.0]), ([90.0, 50.0], [100.0, 70.0])]
        );

        // Transparent text is not drawn.
        batch.clear();
        let clear = Color {
            a: 0.0,
            ..Color::WHITE
        };
        font.push(
            &mut batch,
            number.as_bytes(),
            centre,
            Vec2::new(0.5, 0.5),
            1.0,
            clear,
            Blend::Alpha,
        );
        assert_eq!(batch.len(), 0);
    }

    #[test]
    fn fixed_width_centres_digits_in_the_width_of_five_and_keeps_symbols() {
        // "5" is 10 wide, "1" 4, the dot 3 and the percent 12.
        let mut digits: [Option<SkinSprite>; 10] =
            std::array::from_fn(|d| glyph(d as u32, 10.0, 20.0));
        digits[1] = glyph(1, 4.0, 20.0);
        let mut symbols = NO_SYMBOLS;
        symbols[1] = glyph(11, 3.0, 22.0);
        symbols[2] = glyph(12, 12.0, 20.0);
        let sprites = FontSprites {
            digits,
            symbols,
            overlap: 1.0,
        };
        let fixed = sprites.font(true);
        // 10 + 10 + 3 + 10 + 12 less four overlaps of 1; "," has no glyph.
        assert_eq!(fixed.measure(b"1,1.5%"), Vec2::new(41.0, 22.0));
        let free = sprites.font(false);
        assert_eq!(free.measure(b"1,1.5%"), Vec2::new(29.0, 22.0));

        let mut batch = SpriteBatch::new();
        fixed.push(
            &mut batch,
            b"11.5%",
            Vec2::ZERO,
            Vec2::ZERO,
            1.0,
            Color::WHITE,
            Blend::Additive,
        );
        // The ones centred in 10, then the dot, the five and the percent at their widths.
        assert_eq!(
            boxes(&batch),
            [
                ([3.0, 0.0], [7.0, 20.0]),
                ([12.0, 0.0], [16.0, 20.0]),
                ([18.0, 0.0], [21.0, 22.0]),
                ([20.0, 0.0], [30.0, 20.0]),
                ([29.0, 0.0], [41.0, 20.0]),
            ]
        );
        assert!(sprites.has_digits());
        assert!(!FontSprites::default().has_digits());
    }
}
