//! Reusable settings widgets: [`Toggle`], [`Slider`], [`Dropdown`] and [`KeyBind`].
//!
//! Like the menu they are input-agnostic: the application maps keys and mouse events to
//! each widget's input enum and places the widget in a [`Rect`] of its choosing. Each widget
//! has update-side state, a `*Layout` shared by hit testing and drawing, and a draw-side
//! `*View`. Texts are given already localised and laid out only when they or the scale
//! change, never per frame.

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::shape::{Gradient, Shape, ShapeBatch, ShapeKind};
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

/// A button-like element (ADR-0116, ADR-0117, ADR-0136): rounded, skewed by the theme,
/// layered over `fill` in its own colour `tint` (see [`push_button`]); highlighted, it also
/// gets an outline of `tint`. `seed` offsets the pattern so neighbours differ; `opacity`
/// fades the whole.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tinted_button(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    rect: &Rect,
    theme: &Theme,
    scale: f32,
    fill: Color,
    highlighted: bool,
    seed: f32,
    opacity: f32,
    tint: Color,
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
        tint,
    );
}

/// A byte from a float seed, for the shader's pattern: neighbouring seeds differ.
pub(crate) fn pattern_seed(seed: f32) -> u8 {
    let bits = seed.to_bits().wrapping_mul(0x9e37_79b1);
    (bits >> 24) as u8
}

/// The soft shadow under `shape` (ADR-0136): a black glow of the same outline, dropped
/// down, as strong as the theme's shadow setting; none at strength 0. `fade` is the layer's
/// opacity.
pub(crate) fn push_shadow(
    shapes: &mut ShapeBatch,
    shape: &Shape,
    theme: &Theme,
    scale: f32,
    fade: f32,
) {
    let strength = theme.shadow;
    if strength <= 0.0 || fade <= 0.0 {
        return;
    }
    let drop = theme.shadow_offset * scale;
    shapes.push_shape(&Shape {
        position: [shape.position[0], shape.position[1] + drop],
        glow: theme.shadow_radius * scale * (0.5 + 0.5 * strength),
        sheen: 0.0,
        pattern: 0.0,
        gradient: None,
        blend: slam_render::Blend::Alpha,
        colour: Color {
            a: (theme.shadow_alpha * strength).min(0.95) * fade,
            ..Color::rgb(0.0, 0.0, 0.0)
        },
        ..*shape
    });
}

/// `colour` with its red, green and blue multiplied by `factor`.
fn shaded(colour: Color, factor: f32) -> Color {
    Color {
        r: colour.r * factor,
        g: colour.g * factor,
        b: colour.b * factor,
        a: colour.a,
    }
}

/// [`button`] with `fill` faded already and the highlight faded by `fade`, layered like
/// lazer's beatmap panels (ADR-0136): a shadow; `fill` darkening to the right; a wash of
/// `accent` fading out to the right; with the effects on (ADR-0123), outlined triangles
/// floating up in `accent`, fading out downwards, faster and more of them when the button is
/// lit by hover (a fill other than the idle one) or selected (ADR-0131). Selected, the button
/// also brightens towards the right in `accent`, gets an outline of it and, with the effects,
/// glows, pulsing, and a sheen sweeps it.
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
        ..Shape::new(
            white.texture,
            white.uv,
            [rect.x, rect.y],
            [rect.width, rect.height],
            fill,
        )
    };
    push_shadow(shapes, &shape, theme, scale, fade);
    let effects = highlighted && theme.effects;
    if effects {
        shapes.push_shape(&Shape {
            glow: theme.glow_width * scale,
            sheen: theme.glow_pulse,
            colour: faded(accent, theme.glow_alpha * fade),
            ..shape
        });
    }
    shapes.push_shape(&Shape {
        sheen: if effects { theme.button_sheen } else { 0.0 },
        gradient: Some(Gradient::Horizontal(shaded(fill, theme.button_shade))),
        ..shape
    });
    shapes.push_shape(&Shape {
        colour: faded(accent, theme.button_tint_alpha * fade),
        gradient: Some(Gradient::Horizontal(faded(accent, 0.0))),
        ..shape
    });
    let strength = theme.pattern(level);
    if strength > 0.0 {
        shapes.push_shape(&Shape {
            kind: ShapeKind::PatternLayer,
            pattern: strength,
            pattern_seed: pattern_seed(seed),
            colour: faded(accent, theme.pattern_alpha * fade),
            gradient: Some(Gradient::Vertical(faded(accent, 0.0))),
            ..shape
        });
    }
    if highlighted {
        shapes.push_shape(&Shape {
            colour: faded(accent, 0.0),
            gradient: Some(Gradient::Horizontal(faded(
                accent,
                theme.selection_alpha * fade,
            ))),
            blend: slam_render::Blend::Additive,
            ..shape
        });
        shapes.push_shape(&Shape {
            outline: theme.outline_width * scale,
            colour: faded(accent, fade),
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
    let shape = Shape {
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
    };
    push_shadow(shapes, &shape, theme, scale, opacity);
    shapes.push_shape(&shape);
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
    /// A panel after lazer's wedges (ADR-0136): skewed by `lean` (`tan` of the angle),
    /// rounded, its fill fading towards the right, with a shadow and an outline of
    /// `outline` (none when transparent).
    Wedge { lean: f32, outline: Color },
    /// A five-pointed star filling the rectangle's smaller side (ADR-0136).
    Star,
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
        Look::LeaningPanel { lean } => {
            let shape = Shape {
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
            };
            push_shadow(shapes, &shape, theme, scale, colour.a.min(1.0));
            shapes.push_shape(&shape);
        }
        Look::Wedge { lean, outline } => {
            let shape = Shape {
                radius: theme.wedge_radius * scale,
                skew: lean.atan(),
                gradient: Some(Gradient::Horizontal(Color {
                    a: colour.a * theme.wedge_fade,
                    ..colour
                })),
                ..Shape::new(
                    white.texture,
                    white.uv,
                    [rect.x, rect.y],
                    [rect.width, rect.height],
                    colour,
                )
            };
            push_shadow(shapes, &shape, theme, scale, colour.a.min(1.0));
            shapes.push_shape(&shape);
            if outline.a > 0.0 {
                shapes.push_shape(&Shape {
                    outline: theme.outline_width * scale,
                    colour: outline,
                    gradient: None,
                    ..shape
                });
            }
        }
        Look::Star => {
            let side = rect.width.min(rect.height);
            shapes.push_shape(&Shape {
                kind: ShapeKind::Star,
                ..Shape::new(
                    white.texture,
                    white.uv,
                    [
                        rect.x + (rect.width - side) / 2.0,
                        rect.y + (rect.height - side) / 2.0,
                    ],
                    [side, side],
                    colour,
                )
            });
        }
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
        let theme = Theme::default().with_shadow(0.0);
        let look = Look::Button {
            highlighted: true,
            seed: 0.0,
            fade: 0.25,
        };
        let quads = quads(&theme, Color::WHITE, look);
        // The glow, the fill, the wash, the triangles, the brightening, the outline.
        assert_eq!(quads.len(), 6, "{quads:?}");
        let glow = quads[0];
        assert!(glow.edge < 0.0, "a glow");
        assert!(
            (glow.colour[3] - theme.accent.a * theme.glow_alpha * 0.25).abs() < 1e-6,
            "{glow:?}"
        );
        let outline = quads[5];
        assert!(outline.edge > 0.0);
        assert!((outline.colour[3] - theme.accent.a * 0.25).abs() < 1e-6);
        // Without the effects: no glow, no sheen and no triangles.
        let flat = theme.with_effects(false);
        let quads = super::look_tests::quads(&flat, Color::WHITE, look);
        assert_eq!(quads.len(), 4, "{quads:?}");
        assert!(quads.iter().all(|q| q.sheen == 0.0 && q.pattern == 0.0));
    }

    #[test]
    fn a_shadow_drops_under_a_button_as_strong_as_the_setting() {
        let look = Look::Button {
            highlighted: false,
            seed: 0.0,
            fade: 1.0,
        };
        let theme = Theme::default();
        let shadowed = quads(&theme, theme.button_fill, look);
        let shadow = shadowed[0];
        assert!(shadow.edge < 0.0, "a glow");
        assert_eq!(shadow.colour[..3], [0.0, 0.0, 0.0]);
        assert!((shadow.colour[3] - theme.shadow_alpha).abs() < 1e-6);
        // Dropped below the fill.
        assert!(shadow.middle > shadowed[1].middle);
        let strong = quads(&theme.with_shadow(2.0), theme.button_fill, look)[0];
        assert!(strong.colour[3] > shadow.colour[3]);
        assert!(strong.edge < shadow.edge, "wider");
        let none = quads(&theme.with_shadow(0.0), theme.button_fill, look);
        assert_eq!(none.len(), shadowed.len() - 1);
        // Panels cast one too.
        let panel = quads(&theme, theme.settings_panel_fill, Look::Panel);
        assert_eq!(panel.len(), 2);
        assert_eq!(panel[0].colour[..3], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_button_washes_in_its_tint_fading_to_the_right_and_its_fill_darkens() {
        let theme = Theme::default().with_shadow(0.0);
        let tint = Color::rgb(0.9, 0.2, 0.3);
        let look = Look::TintedButton {
            highlighted: false,
            seed: 0.0,
            fade: 1.0,
            tint,
        };
        let quads = quads(&theme, theme.button_fill, look);
        let (fill, wash, triangles) = (quads[0], quads[1], quads[2]);
        // Corners: top left, top right, bottom right, bottom left.
        assert!(fill.right_colour[0] < fill.colour[0]);
        assert_eq!(wash.colour[..3], [tint.r, tint.g, tint.b]);
        assert!(wash.colour[3] > wash.right_colour[3]);
        assert_eq!(triangles.kind, ShapeKind::PatternLayer);
        assert_eq!(triangles.colour[..3], [tint.r, tint.g, tint.b]);
        assert!(triangles.colour[3] > triangles.bottom_colour[3]);
    }

    /// One quad as drawn: its kind, edge (negative: a glow), top-left colour, the colours of
    /// its top-right and bottom-left corners, effects and vertical middle.
    #[derive(Clone, Copy, Debug)]
    pub(super) struct Quad {
        pub kind: ShapeKind,
        pub edge: f32,
        pub colour: [f32; 4],
        pub right_colour: [f32; 4],
        pub bottom_colour: [f32; 4],
        pub sheen: f32,
        pub pattern: f32,
        pub middle: f32,
    }

    pub(super) fn quads(theme: &Theme, colour: Color, look: Look) -> Vec<Quad> {
        let mut shapes = ShapeBatch::new();
        draw(&mut shapes, white(), &RECT, colour, look, theme, 1.0);
        shapes
            .vertices()
            .chunks(4)
            .map(|v| {
                let (sheen, pattern, _, kind) = slam_render::shape::unpack_effects(v[0].params[2]);
                Quad {
                    kind,
                    edge: v[0].params[1],
                    colour: v[0].colour,
                    right_colour: v[1].colour,
                    bottom_colour: v[3].colour,
                    sheen,
                    pattern,
                    middle: (v[0].position[1] + v[3].position[1]) / 2.0,
                }
            })
            .collect()
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

    /// The strongest pattern among the quads drawn.
    fn pattern_of(theme: &Theme, colour: Color, look: Look) -> f32 {
        quads(theme, colour, look)
            .iter()
            .map(|q| q.pattern)
            .fold(0.0, f32::max)
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
        let theme = Theme::default().with_shadow(0.0);
        let tint = Color::rgb(0.9, 0.2, 0.3);
        let look = Look::TintedButton {
            highlighted: true,
            seed: 0.0,
            fade: 1.0,
            tint,
        };
        let quads = quads(&theme, Color::WHITE, look);
        let (glow, outline) = (quads[0], quads[quads.len() - 1]);
        assert_eq!(glow.colour[..3], [tint.r, tint.g, tint.b]);
        assert_eq!(outline.colour[..3], [tint.r, tint.g, tint.b]);
        // The glow and the outline carry no pattern.
        assert_eq!(glow.pattern, 0.0);
        assert_eq!(outline.pattern, 0.0);
    }
}
