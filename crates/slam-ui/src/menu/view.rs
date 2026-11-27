//! Draw side of the menu (ADR-0186): the logo in layers with its visualiser, triangles and
//! side flashes, and the buttons of either style. Text is laid out again only when the
//! window size changes; once warm, `push` does not allocate.

use std::sync::Arc;

use slam_render::atlas::AtlasRegion;
use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_text::{
    FamilyId, FontRegistry, Glyph, GlyphCache, GlyphKey, Layout, LayoutContext, TextStyle,
    UI_FAMILY, push_layout,
};

use slam_render::Color;
use slam_render::hal::Blend;
use slam_render::shape::{Gradient, Shape};
use slam_render::sprite::{Affine, Sprite};
use slam_text::Icon;

use super::logo::{BAR_ALPHA, BAR_LENGTH, BARS, DEAD_ZONE, LogoSnapshot, ROUNDS};
use super::{MenuItem, MenuSnapshot, MenuStyle};
use crate::screen::Pose;
use crate::theme::{LogoColours, Theme, faded};

/// The menu's localised texts, already formatted.
#[derive(Clone, Debug)]
pub struct MenuLabels {
    pub title: String,
    /// Button labels in [`MenuItem::ALL`] order.
    pub buttons: [String; MenuItem::ALL.len()],
}

/// One text and its layout at the current window height.
pub(crate) struct Line {
    pub(crate) text: String,
    pub(crate) layout: Layout,
}

impl Line {
    pub(crate) fn new(text: &str) -> Line {
        Line {
            text: text.to_owned(),
            layout: Layout::new(),
        }
    }
}

/// Size, weight and width of a text (ADR-0146): the caption and note styles are small and
/// condensed so long labels fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Face {
    pub(crate) size: f32,
    pub(crate) weight: u16,
    pub(crate) width: u16,
}

/// Fonts and the layout context shared by the views.
pub(crate) struct Typesetter {
    registry: Arc<FontRegistry>,
    family: FamilyId,
    context: LayoutContext,
    weight: u16,
}

impl Typesetter {
    pub(crate) fn new(registry: Arc<FontRegistry>, weight: u16) -> Typesetter {
        // An unknown family lays out nothing; the built-in registry has the UI family.
        let family = registry.family(UI_FAMILY).unwrap_or(FamilyId(u32::MAX));
        Typesetter {
            registry,
            family,
            context: LayoutContext::new(),
            weight,
        }
    }

    pub(crate) fn lay_out(&mut self, line: &mut Line, size: f32) {
        let face = Face {
            size,
            weight: self.weight,
            width: slam_text::NORMAL_WIDTH,
        };
        self.lay_out_face(line, face);
    }

    /// The default font weight.
    pub(crate) fn weight(&self) -> u16 {
        self.weight
    }

    /// Lays `line` out in `face`: its own weight and width instead of the default ones.
    pub(crate) fn lay_out_face(&mut self, line: &mut Line, face: Face) {
        let mut style = TextStyle::new(self.family, face.weight, face.size);
        style.width = face.width;
        self.context
            .layout_into(&self.registry, &line.text, &style, &mut line.layout);
    }
}

/// Draws `line` centred on `centre`.
pub(crate) fn push_centred(
    batch: &mut SpriteBatch,
    line: &Line,
    centre: [f32; 2],
    colour: slam_render::Color,
    glyphs: &mut GlyphCache,
) {
    let position = [
        centre[0] - line.layout.width() / 2.0,
        centre[1] - line.layout.height() / 2.0,
    ];
    push_layout(batch, &line.layout, position, colour, glyphs);
}

/// The window size the lines are laid out for.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Frame {
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// One set of the logo's masks (ADR-0186): white images whose alpha is the layer's
/// shape, coloured by the theme's [`LogoColours`] when drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogoLayers {
    pub disc: AtlasRegion,
    pub shine: AtlasRegion,
    pub ring: AtlasRegion,
    pub frame: AtlasRegion,
    pub play: AtlasRegion,
    pub facet: AtlasRegion,
    pub accent: AtlasRegion,
}

impl LogoLayers {
    /// Every layer at `region` (tests and tools).
    pub const fn all(region: AtlasRegion) -> LogoLayers {
        LogoLayers {
            disc: region,
            shine: region,
            ring: region,
            frame: region,
            play: region,
            facet: region,
            accent: region,
        }
    }
}

/// The logo's images in the atlas: its masks large and small (drawn below
/// [`SMALL_BELOW`] pixels, as there are no mipmaps) and the triangle it throws out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogoArt {
    pub big: LogoLayers,
    pub small: LogoLayers,
    pub triangle: AtlasRegion,
}

impl LogoArt {
    /// Every image at `region` (tests and tools).
    pub const fn all(region: AtlasRegion) -> LogoArt {
        LogoArt {
            big: LogoLayers::all(region),
            small: LogoLayers::all(region),
            triangle: region,
        }
    }

    /// The masks for a logo `size` pixels across.
    pub fn layers(&self, size: f32) -> &LogoLayers {
        if size < SMALL_BELOW {
            &self.small
        } else {
            &self.big
        }
    }
}

/// The mark's pieces, back to front, with their colours.
pub fn mark(layers: &LogoLayers, colours: &LogoColours) -> [(AtlasRegion, [Color; 2]); 4] {
    [
        (layers.frame, colours.frame),
        (layers.play, colours.play),
        (layers.facet, colours.facet),
        (layers.accent, colours.accent),
    ]
}

/// The disc's diameter as a part of the logo images' side (`assets/logo`: 940 of 1000).
pub const DISC_SHARE: f32 = 0.94;
/// Below this many pixels the logo is drawn from its small masks.
const SMALL_BELOW: f32 = 300.0;

/// Draws the mask `image` centred on `centre`, its disc `size` across, turned `rotation`
/// radians, running from `colours[0]` at the top to `colours[1]` at the bottom, faded by
/// `alpha`.
#[allow(clippy::too_many_arguments)]
pub fn push_mask(
    shapes: &mut ShapeBatch,
    image: AtlasRegion,
    centre: [f32; 2],
    size: f32,
    rotation: f32,
    colours: [Color; 2],
    alpha: f32,
    blend: Blend,
) {
    let side = size / DISC_SHARE;
    let first = shapes.len();
    let mut shape = Shape::new(
        image.texture,
        image.uv,
        [centre[0] - side / 2.0, centre[1] - side / 2.0],
        [side, side],
        faded(colours[0], alpha),
    );
    shape.gradient = Some(Gradient::Vertical(faded(colours[1], alpha)));
    shape.blend = blend;
    shapes.push_shape(&shape);
    if rotation != 0.0 {
        let turn = Affine::about(centre, 1.0, rotation, [0.0, 0.0]);
        shapes.transform_from(first, &turn, [1.0; 4]);
    }
}

/// Draws the whole logo from `layers` in `colours`: disc, shine, ring and mark.
pub fn push_whole(
    shapes: &mut ShapeBatch,
    layers: &LogoLayers,
    colours: &LogoColours,
    centre: [f32; 2],
    size: f32,
    alpha: f32,
) {
    let a = Blend::Alpha;
    push_mask(
        shapes,
        layers.disc,
        centre,
        size,
        0.0,
        colours.disc,
        alpha,
        a,
    );
    let shine = [colours.shine, faded(colours.shine, 0.0)];
    push_mask(shapes, layers.shine, centre, size, 0.0, shine, alpha, a);
    push_mask(
        shapes,
        layers.ring,
        centre,
        size,
        0.0,
        colours.ring,
        alpha,
        a,
    );
    for (image, piece) in mark(layers, colours) {
        push_mask(shapes, image, centre, size, 0.0, piece, alpha, a);
    }
}

/// Draws the beating logo `size` across at `centre`, faded by `opacity`: the side flashes
/// across the `width` × `height` window (when `sides`), the visualiser and the triangles
/// behind it, the logo, its ripple and flash. `white` is an opaque white image. Does not
/// allocate once the batch has room.
#[allow(clippy::too_many_arguments)]
pub fn push_logo(
    shapes: &mut ShapeBatch,
    art: &LogoArt,
    white: AtlasRegion,
    theme: &Theme,
    logo: &LogoSnapshot,
    centre: [f32; 2],
    size: f32,
    opacity: f32,
    sides: Option<[f32; 2]>,
) {
    if opacity <= 0.0 || size <= 0.0 {
        return;
    }
    let scale = theme.scale(sides.map_or(768.0, |[_, h]| h));
    if let Some([width, height]) = sides {
        // lazer's MenuSideFlashes: 400 wide, from the edge's colour to nothing.
        let flash_width = 400.0 * scale;
        for (side, alpha) in logo.sides.iter().enumerate() {
            let alpha = alpha * opacity;
            if alpha <= 0.0 {
                continue;
            }
            let edge = faded(theme.menu_side_flash, alpha);
            let clear = faded(theme.menu_side_flash, 0.0);
            let (x, from, to) = if side == 0 {
                (0.0, edge, clear)
            } else {
                (width - flash_width, clear, edge)
            };
            let mut shape = Shape::new(
                white.texture,
                white.uv,
                [x, 0.0],
                [flash_width, height],
                from,
            );
            shape.gradient = Some(Gradient::Horizontal(to));
            shape.blend = Blend::Additive;
            shapes.push_shape(&shape);
        }
    }
    let drawn = size * logo.scale;
    let radius = drawn / 2.0;
    // lazer's LogoVisualisation: rounds of bars around the edge, outward.
    let bar_width =
        drawn * (2.0 * (1.0 - (360.0f32 / BARS as f32).to_radians().cos())).sqrt() / 2.0;
    let bar_colour = faded(Color::WHITE, BAR_ALPHA * logo.visualiser_alpha * opacity);
    if bar_colour.a > 0.0 {
        for round in 0..ROUNDS {
            for (i, &amplitude) in logo.bars.iter().enumerate() {
                if amplitude < DEAD_ZONE {
                    continue;
                }
                let angle = (i as f32 / BARS as f32 * 360.0 + round as f32 * 360.0 / ROUNDS as f32)
                    .to_radians();
                let (sin, cos) = angle.sin_cos();
                let length = BAR_LENGTH * drawn * amplitude;
                let mut bar = Sprite::new(
                    white.texture,
                    white.uv,
                    [centre[0] + cos * radius, centre[1] + sin * radius],
                    [bar_width, length],
                );
                bar.origin = [bar_width / 2.0, 0.0];
                // The bar's length runs along +y; turn it to point outward.
                bar.rotation = angle - std::f32::consts::FRAC_PI_2;
                bar.colour = bar_colour;
                bar.blend = Blend::Additive;
                shapes.push(&bar);
            }
        }
    }
    for particle in logo.particles.iter().filter(|p| p.alpha > 0.0) {
        let colour = theme.menu_particle_colours[usize::from(particle.colour) % 2];
        let side = particle.size * size;
        let mut sprite = Sprite::new(
            art.triangle.texture,
            art.triangle.uv,
            [
                centre[0] + particle.offset[0] * size,
                centre[1] + particle.offset[1] * size,
            ],
            [side, side],
        );
        sprite.origin = [side / 2.0, side / 2.0];
        sprite.rotation = particle.rotation;
        sprite.colour = faded(colour, particle.alpha * opacity);
        shapes.push(&sprite);
    }
    let layers = art.layers(drawn);
    push_whole(shapes, layers, &theme.logo, centre, drawn, opacity);
    if logo.ripple_alpha > 0.0 {
        let ripple = [Color::WHITE; 2];
        push_mask(
            shapes,
            layers.disc,
            centre,
            drawn * logo.ripple_scale,
            0.0,
            ripple,
            logo.ripple_alpha * opacity,
            Blend::Additive,
        );
    }
    if logo.flash > 0.0 {
        let mut flash = Shape::new(
            white.texture,
            white.uv,
            [centre[0] - radius, centre[1] - radius],
            [drawn, drawn],
            faded(Color::WHITE, logo.flash * opacity),
        );
        flash.radius = radius;
        flash.blend = Blend::Additive;
        shapes.push_shape(&flash);
    }
}

/// The icon of each item, in [`MenuItem::ALL`] order.
const ICONS: [Icon; 3] = [Icon::Play, Icon::Gear, Icon::Power];

/// The draw side of [`super::MainMenu`].
pub struct MenuView {
    theme: Theme,
    type_setter: Typesetter,
    frame: Frame,
    scale: f32,
    buttons: [Line; MenuItem::ALL.len()],
    /// The same labels small and regular, for lazer's bar.
    small: [Line; MenuItem::ALL.len()],
    /// The startup intro draws the logo meanwhile (ADR-0186).
    logo_hidden: bool,
}

impl std::fmt::Debug for MenuView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuView").finish_non_exhaustive()
    }
}

impl MenuView {
    /// A view with the fonts of `registry`. Lays the texts out once, so that later layouts
    /// do not allocate: call it off the hot paths.
    pub fn new(registry: Arc<FontRegistry>, theme: Theme, labels: &MenuLabels) -> MenuView {
        let (width, height) = (1280.0, theme.reference_height);
        let mut view = MenuView {
            type_setter: Typesetter::new(registry, theme.font_weight),
            frame: Frame { width, height },
            scale: theme.scale(height),
            buttons: labels.buttons.each_ref().map(|text| Line::new(text)),
            small: labels.buttons.each_ref().map(|text| Line::new(text)),
            logo_hidden: false,
            theme,
        };
        view.lay_out();
        view
    }

    /// Lays the texts out for a `width` × `height` window ahead of the first `push`, so that
    /// a view made off the draw thread does not lay them out there (ADR-0091). Allocates:
    /// call it off the hot paths.
    pub fn lay_out_for(&mut self, width: f32, height: f32) {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
    }

    fn lay_out(&mut self) {
        self.scale = self.theme.scale(self.frame.height);
        for button in &mut self.buttons {
            // Button labels are Bold (ADR-0174).
            self.type_setter.lay_out_face(
                button,
                self.theme
                    .heading_face(self.theme.button_text_size * self.scale),
            );
        }
        let small = self
            .theme
            .regular_face(self.theme.menu_lazer_label_size * self.scale);
        for line in &mut self.small {
            self.type_setter.lay_out_face(line, small);
        }
    }

    /// Leaves the logo out of [`MenuView::push`] while the startup intro draws it.
    pub fn set_logo_hidden(&mut self, hidden: bool) {
        self.logo_hidden = hidden;
    }

    /// Asks `glyphs` for every glyph the menu shows. Returns how many are not ready yet;
    /// repeat with [`GlyphCache::process`] in between until none are.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        let icons = ICONS
            .iter()
            .filter(|&&icon| matches!(glyphs.glyph(GlyphKey::icon(icon, false)), Glyph::Pending))
            .count();
        icons
            + self
                .buttons
                .iter()
                .chain(&self.small)
                .flat_map(|line| line.layout.glyphs())
                .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
                .count()
    }

    /// Writes the menu at `pose` on a `width` × `height` window: shapes and the logo into
    /// `shapes` (`white` is an opaque white image, `art` the logo's), texts and icons into
    /// `text` (drawn with the MSDF shader). Allocation-free once warm and once the batches
    /// have room.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        width: f32,
        height: f32,
        snapshot: &MenuSnapshot,
        pose: Pose,
        white: AtlasRegion,
        art: Option<&LogoArt>,
    ) {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
        let shift = pose.offset * width;
        let theme = &self.theme;
        let scale = self.scale;
        let opacity = pose.opacity;
        if snapshot.bar_alpha > 0.0 {
            let bar = &snapshot.buttons[0].rect;
            let fill = faded(theme.menu_bar_fill, snapshot.bar_alpha * opacity);
            shapes.push_shape(&Shape::new(
                white.texture,
                white.uv,
                [0.0, bar.y],
                [width, bar.height],
                fill,
            ));
        }
        for (i, button) in snapshot.buttons.iter().enumerate() {
            if button.rect.width <= 0.5 {
                continue;
            }
            let mut rect = button.rect;
            rect.x += shift;
            match snapshot.style {
                MenuStyle::Lazer => {
                    // lazer's MainMenuButton: a flat wedge in its colour over a soft shadow,
                    // lit white by the pointer.
                    let skew = theme.button_skew();
                    // Settings and play run on under the logo to its centre, so no gap shows
                    // behind it; as much as the button is out.
                    let out = (rect.width / (super::BUTTON_WIDTH * scale)).min(1.0);
                    let middle = snapshot.logo_centre[0] + shift;
                    let mut fill = rect;
                    if i == 1 && middle > rect.x + rect.width {
                        fill.width += (middle - rect.x - rect.width) * out;
                    } else if i == 0 && middle < rect.x {
                        let reach = (rect.x - middle) * out;
                        fill.x -= reach;
                        fill.width += reach;
                    }
                    let plate = |colour: Color, glow: f32| {
                        let mut shape = Shape::new(
                            white.texture,
                            white.uv,
                            [fill.x, fill.y],
                            [fill.width, fill.height],
                            colour,
                        );
                        shape.skew = skew;
                        shape.glow = glow;
                        shape
                    };
                    shapes.push_shape(&plate(faded(Color::BLACK, 0.2 * opacity), 8.0 * scale));
                    shapes.push_shape(&plate(faded(theme.menu_lazer_tints[i], opacity), 0.0));
                    if button.lit > 0.0 {
                        shapes.push_shape(&plate(
                            faded(Color::WHITE, 0.1 * button.lit * opacity),
                            0.0,
                        ));
                    }
                }
                MenuStyle::Stable => {
                    // Stable's column: purple, the hovered button pink.
                    let tint = if button.hovered {
                        theme.menu_lazer_tints[2]
                    } else {
                        theme.menu_lazer_tints[0]
                    };
                    crate::widgets::tinted_button(
                        shapes,
                        white,
                        &rect,
                        theme,
                        scale,
                        tint,
                        button.hovered,
                        i as f32 * 0.37,
                        opacity * button.alpha.clamp(0.0, 1.0),
                        tint,
                    );
                }
            }
            let colour = faded(theme.text, opacity * button.alpha);
            if colour.a <= 0.0 {
                continue;
            }
            match snapshot.style {
                MenuStyle::Lazer => {
                    // Icon a little above the middle, the label small below, as lazer's.
                    let side = theme.menu_lazer_icon_size * scale;
                    let [cx, cy] = rect.centre();
                    slam_text::push_icon(
                        text,
                        glyphs,
                        ICONS[i],
                        [cx - side / 2.0, cy - side * 0.75],
                        side,
                        colour,
                    );
                    push_centred(text, &self.small[i], [cx, cy + side * 0.8], colour, glyphs);
                }
                MenuStyle::Stable => {
                    // The label right of the logo, the icon at the far end.
                    let line = &self.buttons[i];
                    let left = snapshot.logo_centre[0]
                        + shift
                        + snapshot.logo_size / 2.0
                        + rect.height * 0.3;
                    let [_, cy] = rect.centre();
                    push_centred(
                        text,
                        line,
                        [left + line.layout.width() / 2.0, cy],
                        colour,
                        glyphs,
                    );
                    let side = rect.height * 0.5;
                    let right = rect.x + rect.width - rect.height * 0.4;
                    if right - side > left + line.layout.width() {
                        slam_text::push_icon(
                            text,
                            glyphs,
                            ICONS[i],
                            [right - side, cy - side / 2.0],
                            side,
                            colour,
                        );
                    }
                }
            }
        }
        if !self.logo_hidden
            && let Some(art) = art
        {
            let [cx, cy] = snapshot.logo_centre;
            push_logo(
                shapes,
                art,
                white,
                theme,
                &snapshot.logo,
                [cx + shift, cy],
                snapshot.logo_size,
                opacity,
                Some([width, height]),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_face_reaches_the_layout() {
        let registry = Arc::new(FontRegistry::with_builtin());
        let mut type_setter = Typesetter::new(Arc::clone(&registry), 700);
        let mut line = Line::new("Hit error meter colour scheme");
        type_setter.lay_out(&mut line, 16.0);
        let bold = line.layout.width();
        assert_eq!(line.layout.glyphs()[0].weight, 700);
        let face = Face {
            size: 16.0,
            weight: 600,
            width: slam_text::NORMAL_WIDTH,
        };
        type_setter.lay_out_face(&mut line, face);
        assert_eq!(line.layout.glyphs()[0].weight, 600);
        assert!(line.layout.width() < bold);
        // A face with a `wdth` axis narrows by width.
        let mut narrow = Typesetter::new(registry, 600);
        narrow.family = narrow
            .registry
            .family(slam_text::FALLBACK_FAMILY)
            .expect("built-in");
        narrow.lay_out_face(&mut line, face);
        let normal = line.layout.width();
        narrow.lay_out_face(&mut line, Face { width: 80, ..face });
        assert_eq!(line.layout.glyphs()[0].width, 80);
        assert!(line.layout.width() < normal);
    }
}
