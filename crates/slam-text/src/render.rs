//! Layout to sprites.

use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};

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

/// A horizontal window text is cut to (target pixels). Within `fade_left` of `left` and
/// `fade_right` of `right` the text fades out linearly towards the edge; a zero fade cuts
/// hard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clip {
    pub left: f32,
    pub right: f32,
    pub fade_left: f32,
    pub fade_right: f32,
}

impl Clip {
    /// How opaque the text is at `x`: 0 outside the window, rising to 1 across the fades.
    pub fn alpha(&self, x: f32) -> f32 {
        let mut alpha = 1.0f32;
        if self.fade_left > 0.0 {
            alpha = alpha.min((x - self.left) / self.fade_left);
        }
        if self.fade_right > 0.0 {
            alpha = alpha.min((self.right - x) / self.fade_right);
        }
        alpha.clamp(0.0, 1.0)
    }
}

/// Records `layout` like [`push_layout`], cut to `clip`: the part of a glyph outside the
/// window is left out (its texture coordinates follow the cut), and a glyph crossing a
/// fade is split at the fade's ends so its opacity, interpolated across each piece, is
/// exact. Glyph quads are unrotated, so a cut keeps the texture coordinates affine in
/// the position. Does not allocate once the batch has room.
pub fn push_layout_clipped(
    batch: &mut SpriteBatch,
    layout: &Layout,
    position: [f32; 2],
    colour: Color,
    clip: &Clip,
    glyphs: &mut GlyphCache,
) -> TextStats {
    let scale = layout.font_size() / EM_PX;
    let mut stats = TextStats::default();
    if clip.right <= clip.left {
        return stats;
    }
    // Fades that would overlap share the window in proportion, so opacity has one peak
    // and every piece between split points stays linear.
    let (mut fade_left, mut fade_right) = (clip.fade_left.max(0.0), clip.fade_right.max(0.0));
    let width = clip.right - clip.left;
    if fade_left + fade_right > width {
        let k = width / (fade_left + fade_right);
        fade_left *= k;
        fade_right *= k;
    }
    let clip = &Clip {
        fade_left,
        fade_right,
        ..*clip
    };
    // Where opacity stops changing, inside the window.
    let inner = [clip.left + clip.fade_left, clip.right - clip.fade_right];
    for g in layout.glyphs() {
        match glyphs.glyph(GlyphKey::of(g)) {
            Glyph::Ready { region, offset } => {
                let x0 = position[0] + g.x + offset[0] * scale;
                let y0 = position[1] + g.y + offset[1] * scale;
                let x1 = x0 + region.size.width as f32 * scale;
                let y1 = y0 + region.size.height as f32 * scale;
                let (a, b) = (x0.max(clip.left), x1.min(clip.right));
                if a >= b {
                    continue;
                }
                let uv = region.uv;
                let u_at = |x: f32| uv.u0 + (x - x0) / (x1 - x0) * (uv.u1 - uv.u0);
                let tinted = |x: f32| Color {
                    a: colour.a * clip.alpha(x),
                    ..colour
                };
                // Split points: the cut ends and the fade ends between them.
                let mut cuts = [a, b, b, b];
                let mut n = 1;
                for edge in inner {
                    if edge > a && edge < b && edge > cuts[n - 1] {
                        cuts[n] = edge;
                        n += 1;
                    }
                }
                cuts[n] = b;
                let mut pushed = false;
                for piece in cuts[..=n].windows(2) {
                    let (l, r) = (piece[0], piece[1]);
                    if r <= l {
                        continue;
                    }
                    let (cl, cr) = (tinted(l), tinted(r));
                    if cl.a <= 0.0 && cr.a <= 0.0 {
                        continue;
                    }
                    let (ul, ur) = (u_at(l), u_at(r));
                    batch.push_quad_colours(
                        region.texture,
                        Blend::Alpha,
                        [[l, y0], [r, y0], [r, y1], [l, y1]],
                        [[ul, uv.v0], [ur, uv.v0], [ur, uv.v1], [ul, uv.v1]],
                        [cl, cr, cr, cl],
                    );
                    pushed = true;
                }
                if pushed {
                    stats.drawn += 1;
                }
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

    fn clipped(rig: &mut Rig, layout: &Layout, clip: &Clip) -> SpriteBatch {
        let mut batch = SpriteBatch::with_capacity(64);
        push_layout_clipped(
            &mut batch,
            layout,
            [0.0; 2],
            Color::WHITE,
            clip,
            &mut rig.cache,
        );
        batch
    }

    #[test]
    fn a_wide_clip_draws_what_push_layout_draws() {
        let mut rig = Rig::new(64, 16);
        let layout = layout(&rig, "Hello", 32.0);
        settle_all(&mut rig, &layout);
        let mut plain = SpriteBatch::with_capacity(16);
        push_layout(&mut plain, &layout, [0.0; 2], Color::WHITE, &mut rig.cache);
        let clip = Clip {
            left: -1000.0,
            right: 1000.0,
            fade_left: 0.0,
            fade_right: 0.0,
        };
        let cut = clipped(&mut rig, &layout, &clip);
        assert_eq!(cut.len(), plain.len());
        for (a, b) in cut.vertices().iter().zip(plain.vertices()) {
            assert!((a.position[0] - b.position[0]).abs() < 1e-3);
            assert!((a.uv[0] - b.uv[0]).abs() < 1e-5);
            assert_eq!(a.colour, b.colour);
        }
    }

    #[test]
    fn a_hard_clip_cuts_glyphs_and_their_texture_coordinates() {
        let mut rig = Rig::new(64, 16);
        let layout = layout(&rig, "Hello", 32.0);
        settle_all(&mut rig, &layout);
        let width = layout.width();
        let clip = Clip {
            left: width * 0.3,
            right: width * 0.6,
            fade_left: 0.0,
            fade_right: 0.0,
        };
        let cut = clipped(&mut rig, &layout, &clip);
        assert!(!cut.is_empty());
        assert!(cut.len() < 5);
        for v in cut.vertices() {
            assert!(v.position[0] >= clip.left - 1e-3 && v.position[0] <= clip.right + 1e-3);
            assert_eq!(v.colour[3], 1.0);
        }
        // The texture coordinate stays affine in x within every quad.
        for q in cut.vertices().chunks(4) {
            let (l, r) = (q[0], q[1]);
            assert!(r.position[0] > l.position[0]);
            assert!(r.uv[0] >= l.uv[0]);
        }
        let none = Clip {
            left: width + 10.0,
            right: width + 20.0,
            ..clip
        };
        assert!(clipped(&mut rig, &layout, &none).is_empty());
    }

    #[test]
    fn a_fade_splits_quads_and_ramps_opacity_linearly() {
        let mut rig = Rig::new(64, 16);
        let layout = layout(&rig, "HHHHHH", 32.0);
        settle_all(&mut rig, &layout);
        let width = layout.width();
        let clip = Clip {
            left: 0.0,
            right: width,
            fade_left: width * 0.25,
            fade_right: width * 0.25,
        };
        let cut = clipped(&mut rig, &layout, &clip);
        // Every vertex carries exactly the opacity of its x.
        for v in cut.vertices() {
            let expected = clip.alpha(v.position[0]);
            assert!((v.colour[3] - expected).abs() < 1e-4, "{v:?}");
        }
        // No quad spans a fade end: opacity is linear across each piece.
        for q in cut.vertices().chunks(4) {
            for edge in [clip.left + clip.fade_left, clip.right - clip.fade_right] {
                assert!(!(q[0].position[0] < edge - 1e-3 && q[1].position[0] > edge + 1e-3));
            }
        }
        let mut plain = SpriteBatch::with_capacity(16);
        push_layout(&mut plain, &layout, [0.0; 2], Color::WHITE, &mut rig.cache);
        assert!(cut.len() > plain.len());
    }

    #[test]
    fn overlapping_fades_share_the_window_with_one_peak() {
        let mut rig = Rig::new(64, 16);
        let layout = layout(&rig, "HHHHHH", 32.0);
        settle_all(&mut rig, &layout);
        let width = layout.width();
        let clip = Clip {
            left: 0.0,
            right: width,
            fade_left: width * 0.75,
            fade_right: width * 0.25,
        };
        // As if the fades were 3:1 of the window: the peak sits at three quarters.
        let shared = Clip {
            fade_left: width * 0.75,
            fade_right: width * 0.25,
            ..clip
        };
        let wide = Clip {
            fade_left: width * 1.5,
            fade_right: width * 0.5,
            ..clip
        };
        let cut = clipped(&mut rig, &layout, &wide);
        for v in cut.vertices() {
            let expected = shared.alpha(v.position[0]);
            assert!((v.colour[3] - expected).abs() < 1e-4, "{v:?}");
        }
        let peak = width * 0.75;
        assert!(
            cut.vertices()
                .iter()
                .any(|v| (v.position[0] - peak).abs() < 1e-3 && v.colour[3] > 0.999)
        );
    }

    #[test]
    fn clipped_recording_does_not_allocate() {
        let mut rig = Rig::new(64, 16);
        let layout = layout(&rig, "Hello", 24.0);
        settle_all(&mut rig, &layout);
        let mut batch = SpriteBatch::with_capacity(32);
        let clip = Clip {
            left: 5.0,
            right: 40.0,
            fade_left: 8.0,
            fade_right: 8.0,
        };
        let (_, allocs) = count_allocs(|| {
            push_layout_clipped(
                &mut batch,
                &layout,
                [0.0; 2],
                Color::WHITE,
                &clip,
                &mut rig.cache,
            )
        });
        assert_eq!(allocs.total(), 0, "{allocs}");
    }
}
