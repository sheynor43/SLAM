//! Draw side of the menu. Text is laid out again only
//! when the window size changes; once warm, `push` does not allocate.

use std::sync::Arc;

use slam_render::atlas::AtlasRegion;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_text::{
    FamilyId, FontRegistry, Glyph, GlyphCache, GlyphKey, Layout, LayoutContext, TextStyle,
    UI_FAMILY, push_layout,
};

use super::{MenuItem, MenuLayout, MenuSnapshot};
use crate::screen::Pose;
use crate::theme::{Theme, faded};

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
        let style = TextStyle::new(self.family, self.weight, size);
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

/// The draw side of [`super::MainMenu`].
pub struct MenuView {
    theme: Theme,
    type_setter: Typesetter,
    frame: Frame,
    layout: MenuLayout,
    title: Line,
    buttons: [Line; MenuItem::ALL.len()],
}

impl std::fmt::Debug for MenuView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuView")
            .field("title", &self.title.text)
            .finish_non_exhaustive()
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
            layout: MenuLayout::new(width, height, &theme),
            title: Line::new(&labels.title),
            buttons: labels.buttons.each_ref().map(|text| Line::new(text)),
            theme,
        };
        view.lay_out();
        view
    }

    fn lay_out(&mut self) {
        let Frame { width, height } = self.frame;
        self.layout = MenuLayout::new(width, height, &self.theme);
        let scale = self.layout.scale;
        self.type_setter
            .lay_out(&mut self.title, self.theme.title_size * scale);
        for button in &mut self.buttons {
            self.type_setter
                .lay_out(button, self.theme.button_text_size * scale);
        }
    }

    /// Asks `glyphs` for every glyph the menu shows. Returns how many are not ready yet;
    /// repeat with [`GlyphCache::process`] in between until none are.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        std::iter::once(&self.title)
            .chain(&self.buttons)
            .flat_map(|line| line.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count()
    }

    /// Writes the menu at `pose` on a `width` × `height` window: button rectangles into
    /// `shapes` (tinting `white`, an opaque white image) and texts into `text` (drawn with
    /// the MSDF shader). Allocation-free once warm and once the batches have room.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut SpriteBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        width: f32,
        height: f32,
        snapshot: &MenuSnapshot,
        pose: Pose,
        white: AtlasRegion,
    ) {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
        let shift = pose.offset * width;
        let theme = &self.theme;
        let mut rect = |rect: &super::Rect, colour| {
            let mut sprite = Sprite::new(
                white.texture,
                white.uv,
                [rect.x + shift, rect.y],
                [rect.width, rect.height],
            );
            sprite.colour = faded(colour, pose.opacity);
            shapes.push(&sprite);
        };
        for (i, item) in MenuItem::ALL.into_iter().enumerate() {
            let selected = item.enabled() && i == snapshot.selected;
            let fill = if selected {
                theme.button_hover_fill
            } else {
                theme.button_fill
            };
            rect(&self.layout.buttons[i], fill);
        }
        let shifted = |centre: [f32; 2]| [centre[0] + shift, centre[1]];
        let title = faded(theme.text, pose.opacity);
        push_centred(
            text,
            &self.title,
            shifted(self.layout.title.centre()),
            title,
            glyphs,
        );
        for (i, item) in MenuItem::ALL.into_iter().enumerate() {
            let colour = if item.enabled() {
                theme.text
            } else {
                theme.disabled_text
            };
            push_centred(
                text,
                &self.buttons[i],
                shifted(self.layout.buttons[i].centre()),
                faded(colour, pose.opacity),
                glyphs,
            );
        }
    }
}
