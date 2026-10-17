//! Reusable settings widgets: [`Toggle`], [`Slider`], [`Dropdown`] and [`KeyBind`].
//!
//! Like the menu they are input-agnostic: the application maps keys and mouse events to
//! each widget's input enum and places the widget in a [`Rect`] of its choosing. Each widget
//! has update-side state, a `*Layout` shared by hit testing and drawing, and a draw-side
//! `*View`. Texts are given already localised and laid out only when they or the scale
//! change, never per frame.

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_text::{GlyphCache, push_layout};

use crate::menu::Rect;
use crate::menu::view::Line;

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

/// A rectangle of `colour` (tinting the opaque white image `white`).
pub(crate) fn fill(shapes: &mut SpriteBatch, white: AtlasRegion, rect: &Rect, colour: Color) {
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
