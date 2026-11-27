//! Draw side of the intro (ADR-0186): the logo's layers, placed as the menu places its
//! logo alone, so that the logo settles exactly where the menu draws it. Does not allocate
//! once the batch has room.

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::hal::Blend;
use slam_render::shape::{Shape, ShapeBatch};
use slam_render::sprite::Sprite;

use super::{IntroSnapshot, PIECES, backdrop, logo_pose, piece_pose};
use crate::menu::MenuLayout;
use crate::menu::view::{LogoArt, push_layer};
use crate::theme::{Theme, faded};

/// The draw side of [`super::Intro`].
#[derive(Debug)]
pub struct IntroView {
    theme: Theme,
}

impl IntroView {
    pub fn new(theme: Theme) -> IntroView {
        IntroView { theme }
    }

    /// How many pieces the intro animates, as [`super::cues`] takes.
    pub fn letters(&self) -> usize {
        PIECES
    }

    /// Writes the intro at `snapshot` over a `width` × `height` window into `shapes`: the
    /// black (tinting `white`, an opaque white image) and the logo's layers from `art`,
    /// settling on `target` (the menu's logo: centre and size; lazer's logo alone when
    /// `None`).
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &self,
        shapes: &mut ShapeBatch,
        width: f32,
        height: f32,
        snapshot: &IntroSnapshot,
        white: AtlasRegion,
        art: Option<&LogoArt>,
        target: Option<([f32; 2], f32)>,
    ) {
        let t = (snapshot.elapsed_ns as f64 / 1e9) as f32;
        let fade = 1.0 - snapshot.exit;
        let black = backdrop(t) * fade;
        if black > 0.0 {
            let mut sprite = Sprite::new(white.texture, white.uv, [0.0, 0.0], [width, height]);
            sprite.colour = faded(self.theme.intro_backdrop, black);
            shapes.push(&sprite);
        }
        let Some(art) = art else {
            return;
        };
        let (centre, menu_size) =
            target.unwrap_or_else(|| MenuLayout::new(width, height, &self.theme).logo_alone());
        let logo = logo_pose(t);
        let size = menu_size * logo.scale;
        if logo.disc > 0.0 {
            let tint = faded(Color::WHITE, logo.disc * fade);
            push_layer(shapes, art.disc, centre, size, 0.0, tint, Blend::Alpha);
        }
        for (index, layer) in art.mark().into_iter().enumerate() {
            let pose = piece_pose(index, t);
            let alpha = pose.alpha * fade;
            if alpha <= 0.0 {
                continue;
            }
            let at = [
                centre[0] + pose.offset[0] * size,
                centre[1] + pose.offset[1] * size,
            ];
            let tint = faded(Color::WHITE, alpha);
            push_layer(shapes, layer, at, size, pose.rotation, tint, Blend::Alpha);
        }
        if logo.flash > 0.0 {
            let radius = size / 2.0;
            let mut flash = Shape::new(
                white.texture,
                white.uv,
                [centre[0] - radius, centre[1] - radius],
                [size, size],
                faded(Color::WHITE, logo.flash * fade),
            );
            flash.radius = radius;
            flash.blend = Blend::Additive;
            shapes.push_shape(&flash);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intro::{END_S, IMPACT_S};
    use slam_render::atlas::UvRect;
    use slam_render::hal::{Extent, Origin};
    use slam_render::textures::TextureId;

    fn region(id: u32) -> AtlasRegion {
        AtlasRegion {
            texture: TextureId(id),
            origin: Origin { x: 0, y: 0 },
            size: Extent::new(8, 8),
            uv: UvRect {
                u0: 0.0,
                v0: 0.0,
                u1: 1.0,
                v1: 1.0,
            },
        }
    }

    fn art() -> LogoArt {
        LogoArt {
            disc: region(1),
            frame: region(2),
            play: region(3),
            facet: region(4),
            accent: region(5),
            small: region(6),
            triangle: region(7),
        }
    }

    fn snapshot(t: f32) -> IntroSnapshot {
        IntroSnapshot {
            elapsed_ns: (f64::from(t) * 1e9) as u64,
            exit: 0.0,
            skipped: false,
        }
    }

    #[test]
    fn the_logo_ends_where_the_menu_draws_it_alone() {
        let view = IntroView::new(Theme::default());
        let mut shapes = ShapeBatch::new();
        view.push(
            &mut shapes,
            1280.0,
            720.0,
            &snapshot(END_S - 1e-4),
            region(0),
            Some(&art()),
            None,
        );
        // The black is gone; the disc and the four pieces are whole, at the menu's place.
        let (centre, size) = MenuLayout::new(1280.0, 720.0, &Theme::default()).logo_alone();
        let quads = shapes.vertices().as_chunks::<4>().0;
        assert!(quads.len() >= 5);
        let side = size / crate::menu::view::DISC_SHARE;
        for quad in &quads[quads.len() - 5..] {
            let mid_x = (quad[0].position[0] + quad[2].position[0]) / 2.0;
            let mid_y = (quad[0].position[1] + quad[2].position[1]) / 2.0;
            assert!((mid_x - centre[0]).abs() < 0.5 && (mid_y - centre[1]).abs() < 0.5);
            assert!((quad[1].position[0] - quad[0].position[0] - side).abs() < 0.5);
        }
    }

    #[test]
    fn before_the_impact_only_lit_pieces_show_on_black() {
        let view = IntroView::new(Theme::default());
        let mut shapes = ShapeBatch::new();
        view.push(
            &mut shapes,
            1280.0,
            720.0,
            &snapshot(0.0),
            region(0),
            Some(&art()),
            None,
        );
        assert_eq!(shapes.len(), 1, "the black alone");
        let mut shapes = ShapeBatch::new();
        view.push(
            &mut shapes,
            1280.0,
            720.0,
            &snapshot(IMPACT_S - 0.2),
            region(0),
            Some(&art()),
            None,
        );
        assert_eq!(shapes.len(), 1 + PIECES, "every piece, no disc yet");
    }
}
