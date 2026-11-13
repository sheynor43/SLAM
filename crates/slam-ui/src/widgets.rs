//! Reusable settings widgets: [`Toggle`], [`Slider`], [`Dropdown`] and [`KeyBind`].
//!
//! Like the menu they are input-agnostic: the application maps keys and mouse events to
//! each widget's input enum and places the widget in a [`Rect`] of its choosing. Each widget
//! has update-side state, a `*Layout` shared by hit testing and drawing, and a draw-side
//! `*View`. Texts are given already localised and laid out only when they or the scale
//! change, never per frame.

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::shape::{Shape, ShapeBatch};
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_text::{GlyphCache, push_layout};

use crate::menu::Rect;
use crate::menu::view::Line;
use crate::theme::{Theme, faded};

pub mod dropdown;
pub mod keybind;
pub mod slider;
pub mod toggle;

pub use dropdown::{
    Dropdown, DropdownAction, DropdownInput, DropdownLayout, DropdownView, MAX_VISIBLE_OPTIONS,
};
pub use keybind::{KeyBind, KeyBindAction, KeyBindInput, KeyBindLayout, KeyBindView};
pub use slider::{Slider, SliderAction, SliderInput, SliderLayout, SliderView};
pub use toggle::{Toggle, ToggleAction, ToggleInput, ToggleLayout, ToggleView};

/// Bytes reserved up front in every dynamic text buffer.
pub(crate) const TEXT_CAPACITY: usize = 64;

/// A button-like element (ADR-0116, ADR-0117): rounded, skewed by the theme, `fill` lit
/// by the button pattern; highlighted, it also gets an outline of the accent. `seed`
/// offsets the pattern so neighbours differ; `opacity` fades the whole.
#[allow(clippy::too_many_arguments)]
pub(crate) fn button(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    rect: &Rect,
    theme: &Theme,
    scale: f32,
    fill: Color,
    highlighted: bool,
    seed: f32,
    opacity: f32,
) {
    let shape = Shape {
        radius: theme.button_radius * scale,
        skew: theme.button_skew(),
        pattern: theme.button_pattern,
        pattern_phase: seed,
        ..Shape::new(
            white.texture,
            white.uv,
            [rect.x, rect.y],
            [rect.width, rect.height],
            faded(fill, opacity),
        )
    };
    shapes.push_shape(&shape);
    if highlighted {
        shapes.push_shape(&Shape {
            outline: theme.outline_width * scale,
            pattern: 0.0,
            colour: faded(theme.accent, opacity),
            ..shape
        });
    }
}

/// A panel (ADR-0116, ADR-0117): rounded, unskewed, `fill` lit faintly by the panel
/// pattern.
pub(crate) fn panel(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    rect: &Rect,
    theme: &Theme,
    scale: f32,
    fill: Color,
    opacity: f32,
) {
    shapes.push_shape(&Shape {
        radius: theme.panel_radius * scale,
        pattern: theme.panel_pattern,
        pattern_phase: rect.x * 0.013,
        ..Shape::new(
            white.texture,
            white.uv,
            [rect.x, rect.y],
            [rect.width, rect.height],
            faded(fill, opacity),
        )
    });
}

/// A rounded rectangle of `colour` without skew or pattern: tracks, knobs, boxes.
pub(crate) fn rounded(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    rect: &Rect,
    radius: f32,
    colour: Color,
) {
    shapes.push_shape(&Shape {
        radius,
        ..Shape::new(
            white.texture,
            white.uv,
            [rect.x, rect.y],
            [rect.width, rect.height],
            colour,
        )
    });
}

/// How [`draw`] shapes a rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// Square corners, no pattern.
    Plain,
    /// [`button`]: highlighted or not, with a pattern seed; `fade` is the layer's opacity,
    /// which the highlight's outline takes too.
    Button {
        highlighted: bool,
        seed: f32,
        fade: f32,
    },
    /// [`panel`].
    Panel,
    /// [`panel`] skewed by `lean` (`tan` of the angle): behind a leaning list (ADR-0121).
    LeaningPanel { lean: f32 },
    /// [`rounded`] with the button radius.
    Round,
    /// Ends fully rounded: tracks, knobs, switches.
    Pill,
}

/// Draws `rect` of `colour` (already faded) the way `look` says, at `scale` pixels per
/// reference unit.
pub fn draw(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    rect: &Rect,
    colour: Color,
    look: Look,
    theme: &Theme,
    scale: f32,
) {
    match look {
        Look::Plain => fill(shapes, white, rect, colour),
        Look::Button {
            highlighted,
            seed,
            fade,
        } => {
            // `colour` is faded already; only the outline takes `fade` here.
            let shape = Shape {
                radius: theme.button_radius * scale,
                skew: theme.button_skew(),
                pattern: theme.button_pattern,
                pattern_phase: seed,
                ..Shape::new(
                    white.texture,
                    white.uv,
                    [rect.x, rect.y],
                    [rect.width, rect.height],
                    colour,
                )
            };
            shapes.push_shape(&shape);
            if highlighted {
                shapes.push_shape(&Shape {
                    outline: theme.outline_width * scale,
                    pattern: 0.0,
                    colour: faded(theme.accent, fade),
                    ..shape
                });
            }
        }
        Look::Panel => panel(shapes, white, rect, theme, scale, colour, 1.0),
        Look::LeaningPanel { lean } => shapes.push_shape(&Shape {
            radius: theme.panel_radius * scale,
            skew: lean.atan(),
            pattern: theme.panel_pattern,
            pattern_phase: rect.x * 0.013,
            ..Shape::new(
                white.texture,
                white.uv,
                [rect.x, rect.y],
                [rect.width, rect.height],
                colour,
            )
        }),
        Look::Round => rounded(shapes, white, rect, theme.button_radius * scale, colour),
        Look::Pill => rounded(
            shapes,
            white,
            rect,
            rect.width.min(rect.height) / 2.0,
            colour,
        ),
    }
}

/// A rectangle of `colour` (tinting the opaque white image `white`).
pub(crate) fn fill(shapes: &mut ShapeBatch, white: AtlasRegion, rect: &Rect, colour: Color) {
    let mut sprite = Sprite::new(
        white.texture,
        white.uv,
        [rect.x, rect.y],
        [rect.width, rect.height],
    );
    sprite.colour = colour;
    shapes.push(&sprite);
}

/// Draws `line` inside `rect`, vertically centred, `pad` from the left edge.
pub(crate) fn push_left(
    batch: &mut SpriteBatch,
    line: &Line,
    rect: &Rect,
    pad: f32,
    colour: Color,
    glyphs: &mut GlyphCache,
) {
    let position = [
        rect.x + pad,
        rect.y + (rect.height - line.layout.height()) / 2.0,
    ];
    push_layout(batch, &line.layout, position, colour, glyphs);
}

/// Draws `line` inside `rect`, vertically centred, `pad` from the right edge.
pub(crate) fn push_right(
    batch: &mut SpriteBatch,
    line: &Line,
    rect: &Rect,
    pad: f32,
    colour: Color,
    glyphs: &mut GlyphCache,
) {
    let position = [
        rect.x + rect.width - pad - line.layout.width(),
        rect.y + (rect.height - line.layout.height()) / 2.0,
    ];
    push_layout(batch, &line.layout, position, colour, glyphs);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod look_tests {
    use slam_render::atlas::UvRect;
    use slam_render::hal::Origin;
    use slam_render::textures::TextureId;

    use super::*;

    #[test]
    fn a_highlight_fades_with_its_layer() {
        let white = AtlasRegion {
            texture: TextureId(0),
            origin: Origin::new(0, 0),
            size: slam_render::Extent::new(1, 1),
            uv: UvRect::FULL,
        };
        let theme = Theme::default();
        let mut shapes = ShapeBatch::new();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let look = Look::Button {
            highlighted: true,
            seed: 0.0,
            fade: 0.25,
        };
        draw(&mut shapes, white, &rect, Color::WHITE, look, &theme, 1.0);
        assert_eq!(shapes.len(), 2);
        let outline = shapes.vertices()[4].colour[3];
        assert!((outline - theme.accent.a * 0.25).abs() < 1e-6, "{outline}");
    }
}
