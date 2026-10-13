//! Layout to sprites.

use slam_render::Color;
use slam_render::sprite::{Sprite, SpriteBatch};

use crate::{EM_PX, Glyph, GlyphCache, GlyphKey, Layout};

/// What [`push_layout`] recorded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextStats {
    /// Glyph sprites recorded.
    pub drawn: u32,
    /// Glyphs skipped because they are not ready yet; draw the text again next frame.
    pub pending: u32,
}

/// Records `layout` with its top-left corner at `position` (target pixels). Glyphs
/// that are not ready yet are skipped and counted; blank glyphs are skipped silently.
///
/// The sprites sample MSDF atlas pages: draw the batch with a
/// [`SpriteRenderer`](slam_render::sprite::SpriteRenderer) made by `with_shader`
/// and [`shaders::MSDF`](slam_render::shaders::MSDF). Does not allocate once the
/// batch has room.
pub fn push_layout(
    batch: &mut SpriteBatch,
    layout: &Layout,
    position: [f32; 2],
    colour: Color,
    glyphs: &mut GlyphCache,
) -> TextStats {
    let scale = layout.font_size() / EM_PX;
    let mut stats = TextStats::default();
    for g in layout.glyphs() {
        match glyphs.glyph(GlyphKey::of(g)) {
            Glyph::Ready { region, offset } => {
                let mut sprite = Sprite::new(
                    region.texture,
                    region.uv,
                    [
                        position[0] + g.x + offset[0] * scale,
                        position[1] + g.y + offset[1] * scale,
                    ],
                    [
                        region.size.width as f32 * scale,
                        region.size.height as f32 * scale,
                    ],
                );
                sprite.colour = colour;
                batch.push(&sprite);
                stats.drawn += 1;
            }
            Glyph::Pending => stats.pending += 1,
            Glyph::Empty => {}
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Rig;
    use crate::{LayoutContext, TextStyle, UI_FAMILY, generate_glyph};
    use slam_testkit::count_allocs;

    fn layout(rig: &Rig, text: &str, size: f32) -> Layout {
        let family = rig.registry.family(UI_FAMILY).unwrap();
        LayoutContext::new().layout(&rig.registry, text, &TextStyle::new(family, 400, size))
    }

    fn settle_all(rig: &mut Rig, layout: &Layout) {
        for g in layout.glyphs() {
            rig.settle(GlyphKey::of(g));
        }
    }

    const TINT: Color = Color {
        r: 1.0,
        g: 0.5,
        b: 0.25,
        a: 0.75,
    };

    #[test]
    fn glyph_sprites_follow_bitmap_offset_and_size() {
        let mut rig = Rig::new(64, 16);
        let layout = layout(&rig, "Hi!", 64.0);
        settle_all(&mut rig, &layout);
        let mut batch = SpriteBatch::with_capacity(8);
        let stats = push_layout(&mut batch, &layout, [10.0, 20.0], TINT, &mut rig.cache);
        assert_eq!(
            stats,
            TextStats {
                drawn: 3,
                pending: 0
            }
        );
        assert_eq!(batch.len(), 3);
        let scale = 64.0 / EM_PX;
        for (i, g) in layout.glyphs().iter().enumerate() {
            let bitmap = generate_glyph(&rig.registry, GlyphKey::of(g))
                .unwrap()
                .unwrap();
            let v = &batch.vertices()[i * 4..i * 4 + 4];
            let left = 10.0 + g.x + bitmap.offset[0] * scale;
            let top = 20.0 + g.y + bitmap.offset[1] * scale;
            let right = left + bitmap.image.size.width as f32 * scale;
            let bottom = top + bitmap.image.size.height as f32 * scale;
            let near =
                |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3;
            assert!(near(v[0].position, [left, top]), "{i}: {:?}", v[0].position);
            assert!(
                near(v[2].position, [right, bottom]),
                "{i}: {:?}",
                v[2].position
            );
            assert_eq!(v[0].colour, [1.0, 0.5, 0.25, 0.75]);
        }
        // One atlas page: one batch.
        assert_eq!(batch.batches().len(), 1);
    }

    #[test]
    fn pending_glyphs_are_skipped_and_counted() {
        let mut rig = Rig::new(64, 16);
        let layout = layout(&rig, "H i", 32.0);
        let mut batch = SpriteBatch::with_capacity(8);
        let stats = push_layout(&mut batch, &layout, [0.0; 2], Color::WHITE, &mut rig.cache);
        assert_eq!(stats.drawn, 0);
        assert_eq!(stats.pending, 3);
        assert!(batch.is_empty());
        settle_all(&mut rig, &layout);
        batch.clear();
        let stats = push_layout(&mut batch, &layout, [0.0; 2], Color::WHITE, &mut rig.cache);
        assert_eq!(
            stats,
            TextStats {
                drawn: 2,
                pending: 0
            }
        );
    }

    #[test]
    fn recording_does_not_allocate() {
        let mut rig = Rig::new(64, 16);
        let layout = layout(&rig, "Hello", 24.0);
        settle_all(&mut rig, &layout);
        let mut batch = SpriteBatch::with_capacity(16);
        let (stats, allocs) = count_allocs(|| {
            push_layout(
                &mut batch,
                &layout,
                [1.0, 2.0],
                Color::WHITE,
                &mut rig.cache,
            )
        });
        assert_eq!(stats.drawn, 5);
        assert_eq!(allocs.total(), 0, "{allocs}");
    }
}
