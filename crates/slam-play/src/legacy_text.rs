//! Numbers in a legacy skin font (`LegacySpriteText`): one texture per digit, `{prefix}-0` to
//! `{prefix}-9`, laid out side by side with the font's overlap.
//!
//! Only digits are drawn; the font's other glyphs (`comma`, `dot`, `percent`, `x`) and fixed
//! width layout are not needed yet.

use slam_osu::Vec2;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};

use crate::circles::SkinSprite;

/// The digits of a number, most significant first, without allocating.
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
            digits[len] = (n % 10) as u8;
            len += 1;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        digits[..len].reverse();
        Digits { digits, len }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.digits[..self.len]
    }
}

/// A legacy font's digit textures and overlap; missing digits are left out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegacyFont<'a> {
    pub digits: &'a [Option<SkinSprite>; 10],
    /// Positive values bring digits closer together: the spacing is its negation.
    pub overlap: f32,
}

impl LegacyFont<'_> {
    /// The size of `digits` at a scale of 1: glyphs are top-aligned (`UseFullGlyphHeight =
    /// false`), so the height is the tallest glyph's; the spacing is between glyphs only.
    pub fn measure(&self, digits: &Digits) -> Vec2 {
        let mut width = 0.0;
        let mut height: f32 = 0.0;
        let mut first = true;
        for glyph in self.glyphs(digits) {
            if !first {
                width -= self.overlap;
            }
            first = false;
            width += glyph.size.x;
            height = height.max(glyph.size.y);
        }
        Vec2::new(width, height)
    }

    /// Writes `digits` with the point `origin` of their box (fractions of its size: 0.5, 0.5
    /// is the centre) at `at`, each display unit `scale` window units.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySpriteText.cs
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        digits: &Digits,
        at: Vec2,
        origin: Vec2,
        scale: f32,
        colour: Color,
    ) {
        if colour.a <= 0.0 {
            return;
        }
        let size = self.measure(digits);
        let mut x = at.x - size.x * scale * origin.x;
        let top = at.y - size.y * scale * origin.y;
        let mut first = true;
        for glyph in self.glyphs(digits) {
            if !first {
                x -= self.overlap * scale;
            }
            first = false;
            let size = Vec2::new(glyph.size.x * scale, glyph.size.y * scale);
            batch.push(&Sprite {
                position: [x, top],
                size: [size.x, size.y],
                origin: [0.0, 0.0],
                rotation: 0.0,
                colour,
                texture: glyph.texture,
                uv: glyph.uv,
                blend: Blend::Alpha,
            });
            x += size.x;
        }
    }

    fn glyphs<'a>(&'a self, digits: &'a Digits) -> impl Iterator<Item = &'a SkinSprite> + 'a {
        digits
            .as_slice()
            .iter()
            .filter_map(|&d| self.digits[usize::from(d)].as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_most_significant_first() {
        assert_eq!(Digits::new(0).as_slice(), &[0]);
        assert_eq!(Digits::new(1050).as_slice(), &[1, 0, 5, 0]);
        assert_eq!(Digits::new(u64::MAX).as_slice().len(), 20);
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
            overlap: 2.0,
        };
        let number = Digits::new(1203);
        // 6 + 10 + 10 wide less two overlaps of 2, as tall as the "1".
        assert_eq!(font.measure(&number), Vec2::new(22.0, 24.0));

        let mut batch = SpriteBatch::new();
        let centre = Vec2::new(100.0, 50.0);
        font.push(
            &mut batch,
            &number,
            centre,
            Vec2::new(0.5, 0.5),
            2.0,
            Color::WHITE,
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
            digits: &digits,
            overlap: -1.0,
        };
        batch.clear();
        spaced.push(
            &mut batch,
            &Digits::new(30),
            centre,
            Vec2::new(1.0, 0.0),
            1.0,
            Color::WHITE,
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
        font.push(&mut batch, &number, centre, Vec2::new(0.5, 0.5), 1.0, clear);
        assert_eq!(batch.len(), 0);
    }
}
