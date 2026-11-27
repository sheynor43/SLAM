//! Draw side of the intro (ADR-0186): the logo's masks in the theme's colours, placed as
//! the menu places its logo, so that the logo settles exactly where the menu draws it;
//! around them the shaking, the coloured glitch copies, the beams of the charge and the
//! explosion. Does not allocate once the batch has room.

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::hal::Blend;
use slam_render::shape::{Shape, ShapeBatch};
use slam_render::sprite::Sprite;

use super::{
    BURST_TRIANGLES, IntroSnapshot, PIECES, backdrop, blast, burst_triangle, charge, colour,
    logo_pose, piece_pose, shake, split,
};
use crate::menu::MenuLayout;
use crate::menu::view::{LogoArt, mark, push_mask};
use crate::theme::{Theme, faded};

/// Beams closing in during the charge.
const BEAMS: usize = 10;

/// `colour`'s grey of the same lightness, a little brighter: the black-and-white intro.
fn grey(colour: Color) -> Color {
    let v = ((0.2126 * colour.r + 0.7152 * colour.g + 0.0722 * colour.b) * 1.3).min(1.0);
    Color {
        r: v,
        g: v,
        b: v,
        a: colour.a,
    }
}

/// `pair` in grey, `k` of the way to its colour.
fn tone(pair: [Color; 2], k: f32) -> [Color; 2] {
    pair.map(|c| {
        let g = grey(c);
        Color {
            r: g.r + (c.r - g.r) * k,
            g: g.g + (c.g - g.g) * k,
            b: g.b + (c.b - g.b) * k,
            a: c.a,
        }
    })
}

/// White and grey: the intro's glows, beams, copies, waves and burst.
const GREYS: [Color; 2] = [Color::WHITE, Color::rgb(0.55, 0.55, 0.55)];

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
    /// backdrop (tinting `white`, an opaque white image), its glow, beams and explosion, and
    /// the logo's masks from `art`, settling on `target` (the menu's logo: centre and size;
    /// lazer's logo alone when `None`).
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
        let theme = &self.theme;
        let real = &theme.logo;
        let t = (snapshot.elapsed_ns as f64 / 1e9) as f32;
        let fade = 1.0 - snapshot.exit;
        // Black and white until the colour floods in after the impact.
        let k = colour(t);
        let colours = &crate::theme::LogoColours {
            disc: tone(real.disc, k),
            shine: tone([real.shine; 2], k)[0],
            ring: tone(real.ring, k),
            frame: tone(real.frame, k),
            play: tone(real.play, k),
            facet: tone(real.facet, k),
            accent: tone(real.accent, k),
        };
        let black = backdrop(t) * fade;
        if black > 0.0 {
            let mut sprite = Sprite::new(white.texture, white.uv, [0.0, 0.0], [width, height]);
            sprite.colour = faded(theme.intro_backdrop, black);
            shapes.push(&sprite);
        }
        let Some(art) = art else {
            return;
        };
        let (home, menu_size) =
            target.unwrap_or_else(|| MenuLayout::new(width, height, &self.theme).logo_alone());
        let logo = logo_pose(t);
        let size = menu_size * logo.scale;
        let [sx, sy] = shake(t);
        let centre = [home[0] + sx * size, home[1] + sy * size];
        let layers = art.layers(size);
        let charge = charge(t);
        let boom = blast(t);
        // A coloured glow behind everything, swelling with the charge.
        let glow = black * (0.35 + 0.65 * charge).max(boom.wave_alpha * 0.8);
        if glow > 0.0 {
            let halo = GREYS;
            push_mask(
                shapes,
                layers.shine,
                centre,
                size * 3.2,
                0.0,
                halo,
                glow * 0.6,
                Blend::Additive,
            );
        }
        // Beams from beyond the window closing in on the centre.
        if charge > 0.0 {
            let reach = width.hypot(height);
            let thickness = (size * 0.022).max(2.0);
            for beam in 0..BEAMS {
                let angle = beam as f32 / BEAMS as f32 * std::f32::consts::TAU + t * 1.7;
                let (sin, cos) = angle.sin_cos();
                let near = size * 0.5 + reach * (1.0 - charge);
                let length = reach;
                let mut sprite = Sprite::new(
                    white.texture,
                    white.uv,
                    [centre[0] + cos * near, centre[1] + sin * near],
                    [thickness * (1.0 + charge), length],
                );
                sprite.origin = [thickness / 2.0, 0.0];
                sprite.rotation = angle - std::f32::consts::FRAC_PI_2;
                let colour = GREYS[beam % 2];
                sprite.colour = faded(colour, charge * fade);
                sprite.blend = Blend::Additive;
                shapes.push(&sprite);
            }
        }
        if logo.disc > 0.0 {
            let a = logo.disc * fade;
            push_mask(
                shapes,
                layers.disc,
                centre,
                size,
                0.0,
                colours.disc,
                a,
                Blend::Alpha,
            );
            let shine = [colours.shine, faded(colours.shine, 0.0)];
            push_mask(
                shapes,
                layers.shine,
                centre,
                size,
                0.0,
                shine,
                a,
                Blend::Alpha,
            );
            push_mask(
                shapes,
                layers.ring,
                centre,
                size,
                0.0,
                colours.ring,
                a,
                Blend::Alpha,
            );
        }
        let glitch = split(t) * fade;
        for (index, (image, piece)) in mark(layers, colours).into_iter().enumerate() {
            let pose = piece_pose(index, t);
            let alpha = pose.alpha * fade;
            if alpha <= 0.0 {
                continue;
            }
            let at = [
                centre[0] + pose.offset[0] * size,
                centre[1] + pose.offset[1] * size,
            ];
            // The glitch: copies to either side in the ring's two colours.
            if glitch > 0.01 {
                let away = glitch * size * 0.035;
                for (side, colour) in [(-1.0, GREYS[0]), (1.0, GREYS[1])] {
                    let copy = [at[0] + side * away, at[1] - side * away * 0.3];
                    push_mask(
                        shapes,
                        image,
                        copy,
                        size,
                        pose.rotation,
                        [colour; 2],
                        alpha * glitch * 0.7,
                        Blend::Additive,
                    );
                }
            }
            push_mask(
                shapes,
                image,
                at,
                size,
                pose.rotation,
                piece,
                alpha,
                Blend::Alpha,
            );
        }
        // The core glowing up in the middle of the mark.
        if charge > 0.0 {
            let core = size * (0.1 + 0.35 * charge * charge);
            let white_hot = GREYS;
            push_mask(
                shapes,
                layers.shine,
                centre,
                core,
                0.0,
                white_hot,
                charge * fade,
                Blend::Additive,
            );
        }
        // The explosion: shock waves, a burst of triangles and the window's flash.
        if boom.wave_alpha > 0.0 && boom.burst > 0.0 {
            for (delay, colours) in [(0.0, GREYS), (0.25, [GREYS[1]; 2])] {
                let wave = ((boom.wave - delay) / (1.0 - delay)).max(0.0);
                let ring = size * (1.0 + 2.6 * wave);
                push_mask(
                    shapes,
                    layers.ring,
                    centre,
                    ring,
                    0.0,
                    colours,
                    boom.wave_alpha * fade,
                    Blend::Additive,
                );
            }
        }
        if boom.burst > 0.0 && boom.burst < 1.0 {
            let fly = crate::anim::ease_out_expo(boom.burst);
            for index in 0..BURST_TRIANGLES {
                let (angle, far, side) = burst_triangle(index);
                let (sin, cos) = angle.sin_cos();
                let distance = size * (0.3 + far * fly);
                let side = side * size;
                let mut sprite = Sprite::new(
                    art.triangle.texture,
                    art.triangle.uv,
                    [centre[0] + cos * distance, centre[1] + sin * distance],
                    [side, side],
                );
                sprite.origin = [side / 2.0, side / 2.0];
                sprite.rotation = angle + boom.burst * 4.0;
                let colour = GREYS[index % 2];
                sprite.colour = faded(colour, (1.0 - boom.burst) * fade);
                sprite.blend = Blend::Additive;
                shapes.push(&sprite);
            }
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
        if boom.flash > 0.01 {
            let mut sprite = Sprite::new(white.texture, white.uv, [0.0, 0.0], [width, height]);
            sprite.colour = faded(Color::WHITE, boom.flash * fade);
            sprite.blend = Blend::Additive;
            shapes.push(&sprite);
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
        let art = LogoArt::all(region(1));
        view.push(
            &mut shapes,
            1280.0,
            720.0,
            &snapshot(END_S - 1e-4),
            region(0),
            Some(&art),
            None,
        );
        // The black is gone and nothing shakes; disc, shine, ring and four pieces are whole,
        // at the menu's place.
        let (centre, size) = MenuLayout::new(1280.0, 720.0, &Theme::default()).logo_alone();
        let quads = shapes.vertices().as_chunks::<4>().0;
        let side = size / crate::menu::view::DISC_SHARE;
        let layers: Vec<_> = quads
            .iter()
            .filter(|q| (q[1].position[0] - q[0].position[0] - side).abs() < 3.0)
            .collect();
        assert!(layers.len() >= 7, "{}", layers.len());
        for quad in layers {
            let mid_x = (quad[0].position[0] + quad[2].position[0]) / 2.0;
            let mid_y = (quad[0].position[1] + quad[2].position[1]) / 2.0;
            assert!((mid_x - centre[0]).abs() < 2.0 && (mid_y - centre[1]).abs() < 2.0);
        }
    }

    #[test]
    fn the_impact_explodes() {
        let view = IntroView::new(Theme::default());
        let art = LogoArt::all(region(1));
        let mut before = ShapeBatch::new();
        view.push(
            &mut before,
            1280.0,
            720.0,
            &snapshot(0.0),
            region(0),
            Some(&art),
            None,
        );
        assert!(before.len() <= 2, "black and its glow");
        let mut boom = ShapeBatch::new();
        view.push(
            &mut boom,
            1280.0,
            720.0,
            &snapshot(IMPACT_S + 0.05),
            region(0),
            Some(&art),
            None,
        );
        assert!(boom.len() > BURST_TRIANGLES + PIECES, "{}", boom.len());
    }
}
