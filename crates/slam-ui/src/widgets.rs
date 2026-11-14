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
use crate::theme::{PatternLevel, Theme, faded};

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
    push_button(
        shapes,
        white,
        rect,
        theme,
        scale,
        faded(fill, opacity),
        highlighted,
        seed,
        opacity,
        theme.accent,
    );
}

/// A byte from a float seed, for the shader's pattern: neighbouring seeds differ.
pub(crate) fn pattern_seed(seed: f32) -> u8 {
    let bits = seed.to_bits().wrapping_mul(0x9e37_79b1);
    (bits >> 24) as u8
}

/// [`button`] with `fill` faded already and the highlight faded by `fade`: with the
/// effects on (ADR-0123), a highlighted button glows, pulsing, and a sheen sweeps it;
/// triangles and circles float up every button, faster and brighter when it is lit by hover
/// (a fill other than the idle one) or selected (ADR-0131). `accent` colours the glow and
/// the outline.
#[allow(clippy::too_many_arguments)]
fn push_button(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    rect: &Rect,
    theme: &Theme,
    scale: f32,
    fill: Color,
    highlighted: bool,
    seed: f32,
    fade: f32,
    accent: Color,
) {
    let level = if highlighted {
        PatternLevel::Selected
    } else if fill.r != theme.button_fill.r || fill.g != theme.button_fill.g {
        PatternLevel::Hover
    } else {
        PatternLevel::Button
    };
    let shape = Shape {
        radius: theme.button_radius * scale,
        skew: theme.button_skew(),
        phase: seed,
        pattern: theme.pattern(level),
        pattern_seed: pattern_seed(seed),
        ..Shape::new(
            white.texture,
            white.uv,
            [rect.x, rect.y],
            [rect.width, rect.height],
            fill,
        )
    };
    let effects = highlighted && theme.effects;
    if effects {
        shapes.push_shape(&Shape {
            glow: theme.glow_width * scale,
            sheen: theme.glow_pulse,
            colour: faded(accent, theme.glow_alpha * fade),
            pattern: 0.0,
            ..shape
        });
    }
    shapes.push_shape(&Shape {
        sheen: if effects { theme.button_sheen } else { 0.0 },
        ..shape
    });
    if highlighted {
        shapes.push_shape(&Shape {
            outline: theme.outline_width * scale,
            colour: faded(accent, fade),
            pattern: 0.0,
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
        pattern: theme.pattern(PatternLevel::Panel),
        pattern_seed: pattern_seed(rect.width),
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
    /// [`Look::Button`] with the glow and outline in `tint` instead of the accent: a
    /// difficulty row in its star colour (ADR-0131).
    TintedButton {
        highlighted: bool,
        seed: f32,
        fade: f32,
        tint: Color,
    },
    /// A thin strip leaning like the buttons, small corners, no pattern: the colour mark on
    /// a difficulty row's left edge (ADR-0131).
    Strip,
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
        } => push_button(
            shapes,
            white,
            rect,
            theme,
            scale,
            colour,
            highlighted,
            seed,
            fade,
            theme.accent,
        ),
        Look::TintedButton {
            highlighted,
            seed,
            fade,
            tint,
        } => push_button(
            shapes,
            white,
            rect,
            theme,
            scale,
            colour,
            highlighted,
            seed,
            fade,
            tint,
        ),
        Look::Strip => shapes.push_shape(&Shape {
            radius: (theme.button_radius * scale * 0.4).min(rect.width * 0.5),
            skew: theme.button_skew(),
            ..Shape::new(
                white.texture,
                white.uv,
                [rect.x, rect.y],
                [rect.width, rect.height],
                colour,
            )
        }),
        Look::Panel => panel(shapes, white, rect, theme, scale, colour, 1.0),
        Look::LeaningPanel { lean } => shapes.push_shape(&Shape {
            radius: theme.panel_radius * scale,
            skew: lean.atan(),
            pattern: theme.pattern(PatternLevel::Panel),
            pattern_seed: pattern_seed(rect.width),
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
        // The glow under it, the button, the outline.
        assert_eq!(shapes.len(), 3);
        let glow = shapes.vertices()[0].colour[3];
        assert!(
            (glow - theme.accent.a * theme.glow_alpha * 0.25).abs() < 1e-6,
            "{glow}"
        );
        assert!(shapes.vertices()[0].params[1] < 0.0, "a glow");
        let outline = shapes.vertices()[8].colour[3];
        assert!((outline - theme.accent.a * 0.25).abs() < 1e-6, "{outline}");
        // Without the effects: no glow and no sheen.
        shapes.clear();
        let flat = theme.with_effects(false);
        draw(&mut shapes, white, &rect, Color::WHITE, look, &flat, 1.0);
        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes.vertices()[0].params[2], 0.0);
    }

    fn white() -> AtlasRegion {
        AtlasRegion {
            texture: TextureId(0),
            origin: Origin::new(0, 0),
            size: slam_render::Extent::new(1, 1),
            uv: UvRect::FULL,
        }
    }

    const RECT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 40.0,
    };

    /// The pattern strength of the first quad drawn.
    fn pattern_of(theme: &Theme, colour: Color, look: Look) -> f32 {
        let mut shapes = ShapeBatch::new();
        draw(&mut shapes, white(), &RECT, colour, look, theme, 1.0);
        let quad = shapes.len()
            - if matches!(
                look,
                Look::Button {
                    highlighted: true,
                    ..
                }
            ) {
                2
            } else {
                1
            };
        slam_render::shape::unpack_effects(shapes.vertices()[quad * 4].params[2]).1
    }

    #[test]
    fn buttons_carry_a_pattern_that_grows_with_hover_and_selection() {
        let theme = Theme::default();
        let button = |highlighted| Look::Button {
            highlighted,
            seed: 0.4,
            fade: 1.0,
        };
        let idle = pattern_of(&theme, theme.button_fill, button(false));
        let hover = pattern_of(&theme, theme.button_hover_fill, button(false));
        let selected = pattern_of(&theme, theme.button_hover_fill, button(true));
        let panel = pattern_of(&theme, theme.settings_panel_fill, Look::Panel);
        assert!(idle > 0.0 && panel > 0.0);
        assert!(panel < idle && idle < hover && hover < selected);
        assert!((idle - theme.pattern_button).abs() < 0.005);
        assert!((selected - theme.pattern_selected).abs() < 0.005);
        // Plain shapes and the strip are left bare.
        assert_eq!(pattern_of(&theme, Color::WHITE, Look::Strip), 0.0);
        assert_eq!(pattern_of(&theme, Color::WHITE, Look::Round), 0.0);
    }

    #[test]
    fn without_effects_there_is_no_pattern() {
        let flat = Theme::default().with_effects(false);
        assert_eq!(flat.pattern(PatternLevel::Selected), 0.0);
        for look in [
            Look::Button {
                highlighted: true,
                seed: 0.4,
                fade: 1.0,
            },
            Look::Panel,
            Look::LeaningPanel { lean: 0.1 },
            Look::TintedButton {
                highlighted: true,
                seed: 0.1,
                fade: 1.0,
                tint: Color::WHITE,
            },
        ] {
            assert_eq!(pattern_of(&flat, flat.button_hover_fill, look), 0.0);
        }
    }

    #[test]
    fn neighbouring_buttons_get_different_pattern_seeds() {
        let seeds: Vec<u8> = (0..6).map(|i| pattern_seed(i as f32 * 0.37)).collect();
        for (i, a) in seeds.iter().enumerate() {
            assert!(!seeds[i + 1..].contains(a), "{seeds:?}");
        }
    }

    #[test]
    fn a_tinted_button_glows_and_is_outlined_in_its_tint() {
        let theme = Theme::default();
        let tint = Color::rgb(0.9, 0.2, 0.3);
        let mut shapes = ShapeBatch::new();
        let look = Look::TintedButton {
            highlighted: true,
            seed: 0.0,
            fade: 1.0,
            tint,
        };
        draw(&mut shapes, white(), &RECT, Color::WHITE, look, &theme, 1.0);
        assert_eq!(shapes.len(), 3);
        let v = shapes.vertices();
        assert_eq!(v[0].colour[..3], [tint.r, tint.g, tint.b]);
        assert_eq!(v[8].colour[..3], [tint.r, tint.g, tint.b]);
        // The glow and the outline carry no pattern.
        assert_eq!(slam_render::shape::unpack_effects(v[0].params[2]).1, 0.0);
        assert_eq!(slam_render::shape::unpack_effects(v[8].params[2]).1, 0.0);
    }
}
