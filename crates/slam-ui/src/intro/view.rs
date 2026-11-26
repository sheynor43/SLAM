//! Draw side of the intro. The title is laid out as the menu lays out its own, so the
//! letters land exactly where the menu draws them; laid out again only when the window
//! size changes. Once warm, `push` does not allocate.

use std::sync::Arc;

use slam_render::atlas::AtlasRegion;
use slam_render::shape::ShapeBatch;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_text::{EM_PX_HIRES, FontRegistry, Glyph, GlyphCache, GlyphKey};

use super::{BIG_SCALE, IntroSnapshot, LetterPose, backdrop, letter_pose};

/// Most of the window's width the big letters' row may take.
const MAX_ROW_WIDTH: f32 = 0.85;
use crate::menu::MenuLayout;
use crate::menu::view::{Frame, Line, Typesetter};
use crate::theme::{Theme, faded};

/// The draw side of [`super::Intro`].
pub struct IntroView {
    theme: Theme,
    type_setter: Typesetter,
    frame: Frame,
    layout: MenuLayout,
    title: Line,
}

impl std::fmt::Debug for IntroView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntroView")
            .field("title", &self.title.text)
            .finish_non_exhaustive()
    }
}

impl IntroView {
    /// A view of `title` (the menu's) with the fonts of `registry`. Lays the title out
    /// once, so that later layouts do not allocate: call it off the hot paths.
    pub fn new(registry: Arc<FontRegistry>, theme: Theme, title: &str) -> IntroView {
        let (width, height) = (1280.0, theme.reference_height);
        let mut view = IntroView {
            type_setter: Typesetter::new(registry, theme.font_weight),
            frame: Frame { width, height },
            layout: MenuLayout::new(width, height, &theme),
            title: Line::new(title),
            theme,
        };
        view.lay_out();
        view
    }

    /// Asks `glyphs` for the letters at the resolution the intro draws them at; how many
    /// are not ready yet.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        self.title
            .layout
            .glyphs()
            .iter()
            .filter(|g| matches!(glyphs.glyph(GlyphKey::hires_of(g)), Glyph::Pending))
            .count()
    }

    /// How many letters the intro animates: the title's glyphs, as [`super::cues`] takes.
    pub fn letters(&self) -> usize {
        self.title.layout.glyphs().len()
    }

    fn lay_out(&mut self) {
        let Frame { width, height } = self.frame;
        self.layout = MenuLayout::new(width, height, &self.theme);
        self.type_setter
            .lay_out(&mut self.title, self.theme.title_size * self.layout.scale);
    }

    /// Writes the intro at `snapshot` over a `width` × `height` window: the black into
    /// `shapes` (tinting `white`, an opaque white image), the filled letters into `text`
    /// (drawn with the MSDF shader) and the outlined ones into `outlines` (drawn with the
    /// MSDF outline shader, after `text`). Allocation-free once warm and once the batches
    /// have room.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        outlines: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        width: f32,
        height: f32,
        snapshot: &IntroSnapshot,
        white: AtlasRegion,
    ) {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
        let t = (snapshot.elapsed_ns as f64 / 1e9) as f32;
        let fade = 1.0 - snapshot.exit;
        let black = backdrop(t) * fade;
        if black > 0.0 {
            let mut sprite = Sprite::new(white.texture, white.uv, [0.0, 0.0], [width, height]);
            sprite.colour = faded(self.theme.intro_backdrop, black);
            shapes.push(&sprite);
        }
        let layout = &self.title.layout;
        // As `push_centred` places the menu's title.
        let title_centre = self.layout.title.centre();
        let origin = [
            title_centre[0] - layout.width() / 2.0,
            title_centre[1] - layout.height() / 2.0,
        ];
        let screen_centre = [width / 2.0, height / 2.0];
        // The letters grow to several times the menu's title, so they are made at
        // `EM_PX_HIRES` atlas pixels per em, not the 32 the menu's text is: scaled up from
        // those they show the field's wobble as jagged edges (ADR-0178).
        let scale = layout.font_size() / EM_PX_HIRES;
        let count = layout.glyphs().len();
        // The big row within most of the window's width.
        let widest = layout.width() * BIG_SCALE;
        let fit = if widest > 0.0 {
            (MAX_ROW_WIDTH * width / widest).min(1.0)
        } else {
            1.0
        };
        for (index, g) in layout.glyphs().iter().enumerate() {
            let Glyph::Ready { region, offset } = glyphs.glyph(GlyphKey::hires_of(g)) else {
                continue;
            };
            // The glyph's sprite in the menu's title, as `push_layout` writes it.
            let top_left = [
                origin[0] + g.x + offset[0] * scale,
                origin[1] + g.y + offset[1] * scale,
            ];
            let size = [
                region.size.width as f32 * scale,
                region.size.height as f32 * scale,
            ];
            let pose = letter_pose(index, count, t);
            let (position, size) = place(
                Placement {
                    top_left,
                    size,
                    title_centre,
                    screen_centre,
                    fit,
                },
                &pose,
            );
            let mut sprite = Sprite::new(region.texture, region.uv, position, size);
            let outline = pose.outline * fade;
            if outline > 0.0 {
                sprite.colour = faded(self.theme.text, outline);
                outlines.push(&sprite);
            }
            let fill = pose.fill * fade;
            if fill > 0.0 {
                sprite.colour = faded(self.theme.text, fill);
                text.push(&sprite);
            }
        }
    }
}

/// Where a glyph's sprite is in the menu's title, and around what it flies.
#[derive(Clone, Copy, Debug)]
struct Placement {
    top_left: [f32; 2],
    size: [f32; 2],
    title_centre: [f32; 2],
    screen_centre: [f32; 2],
    /// Scales the big letters down to fit the window, 0 ..= 1.
    fit: f32,
}

/// The glyph's sprite (top left, size) at `pose`. Flying in scales the letter about its
/// centre and moves it, both down to nothing at `land == 1`, where the sprite is exactly
/// the menu's: the terms vanish, they are not interpolated to it.
fn place(at: Placement, pose: &LetterPose) -> ([f32; 2], [f32; 2]) {
    let away = 1.0 - pose.land;
    let scale = pose.scale * at.fit;
    let grow = (scale - 1.0) * away;
    let position = |axis: usize| {
        let top_left = at.top_left[axis];
        let centre = top_left + at.size[axis] / 2.0;
        let big = at.screen_centre[axis] + (centre - at.title_centre[axis]) * scale;
        top_left + ((top_left - centre) * grow + (big - centre) * away)
    };
    (
        [position(0), position(1)],
        [at.size[0] * (1.0 + grow), at.size[1] * (1.0 + grow)],
    )
}

/// The largest size, in pixels per em, the intro draws a letter at in a `height` pixels
/// high window: the big row before the flight.
#[cfg(test)]
fn largest_em_px(theme: &Theme, height: f32) -> f32 {
    theme.title_size * theme.scale(height) * BIG_SCALE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intro::{END_S, IMPACT_S};

    #[test]
    fn the_letters_are_made_at_least_as_big_as_they_are_ever_drawn_up_to_4k() {
        let theme = Theme::default();
        for height in [720.0, 1080.0, 1440.0, 2160.0] {
            let drawn = largest_em_px(&theme, height);
            // Never scaled up more than 3 times, where 32 pixels per em showed 10-fold
            // enlargement as a pixelated edge.
            assert!(
                drawn / EM_PX_HIRES <= 3.0,
                "{height}: drawn {drawn} from {EM_PX_HIRES}"
            );
            assert!(
                drawn / slam_text::EM_PX > 3.0,
                "{height}: low res was enough"
            );
        }
    }

    #[test]
    fn the_intro_asks_for_the_hires_glyphs_not_the_menus() {
        use slam_text::{FontRegistry, GlyphKey};
        use std::sync::Arc;
        let registry = Arc::new(FontRegistry::with_builtin());
        let view = IntroView::new(registry, Theme::default(), "SLAM");
        assert_eq!(view.letters(), 4);
        for g in view.title.layout.glyphs() {
            assert!(GlyphKey::hires_of(g).hires);
            assert_ne!(GlyphKey::hires_of(g), GlyphKey::of(g));
        }
    }

    const AT: Placement = Placement {
        top_left: [612.25, 301.5],
        size: [41.75, 57.125],
        title_centre: [640.0, 330.0],
        screen_centre: [640.0, 360.0],
        fit: 1.0,
    };

    #[test]
    fn landed_letter_is_exactly_the_menus_glyph() {
        for index in 0..4 {
            let pose = letter_pose(index, 4, END_S);
            assert_eq!(place(AT, &pose), (AT.top_left, AT.size));
        }
    }

    #[test]
    fn letters_fly_in_from_big_around_the_screen_centre() {
        // At the impact: big, in a row about the screen centre.
        let pose = letter_pose(0, 4, IMPACT_S);
        let (position, size) = place(AT, &pose);
        assert!((size[0] - AT.size[0] * pose.scale).abs() < 1e-3);
        let centre = [position[0] + size[0] / 2.0, position[1] + size[1] / 2.0];
        let own = [
            AT.top_left[0] + AT.size[0] / 2.0,
            AT.top_left[1] + AT.size[1] / 2.0,
        ];
        for axis in 0..2 {
            let expected =
                AT.screen_centre[axis] + (own[axis] - AT.title_centre[axis]) * pose.scale;
            assert!((centre[axis] - expected).abs() < 1e-3, "{axis}");
        }
        // Fitted to a narrow window: smaller, landing exactly all the same.
        let narrow = Placement { fit: 0.5, ..AT };
        let small = place(narrow, &pose).1;
        assert!((small[0] - AT.size[0] * pose.scale * 0.5).abs() < 1e-3);
        assert_eq!(
            place(narrow, &letter_pose(0, 4, END_S)),
            (AT.top_left, AT.size)
        );
        // Half way in, between the two sizes.
        let half = place(AT, &letter_pose(0, 4, (IMPACT_S + END_S) / 2.0)).1;
        assert!(half[0] > AT.size[0] && half[0] < AT.size[0] * BIG_SCALE);
    }
}
