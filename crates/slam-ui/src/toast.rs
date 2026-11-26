//! A notification over every screen (ADR-0154): the replay was saved; a click shows the file.
//!
//! The update side ([`Toast`]) keeps when it was shown and whether the pointer is over it, and
//! puts a [`ToastFrame`] into the snapshot; the draw side ([`ToastView`]) slides it in and out
//! by the frame's time. Neither allocates after it is made.

use std::sync::Arc;

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey};

use crate::anim::{ease_out_cubic, ease_out_quint, progress};
use crate::menu::Rect;
use crate::menu::view::{Line, Typesetter};
use crate::theme::{Theme, faded};
use crate::widgets::{Look, push_left};

/// What the draw side needs; `Copy`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToastFrame {
    /// Shown or on its way out.
    pub visible: bool,
    /// When it last appeared or started to leave, by `visible`.
    pub since_ns: u64,
    /// The pointer is over it.
    pub hover: bool,
    /// Which entry of [`ToastLabels`] it shows: [`REPLAY_SAVED`] or an import notice.
    pub variant: u8,
}

/// The variant of the replay toast; the others are notices of the app (ADR-0169).
pub const REPLAY_SAVED: u8 = 0;

/// Where the toast is: the window's lower right corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToastLayout {
    pub rect: Rect,
    pub scale: f32,
}

impl ToastLayout {
    pub fn new(width: f32, height: f32, theme: &Theme) -> ToastLayout {
        let scale = theme.scale(height);
        let margin = theme.song_margin * scale;
        let (w, h) = (theme.toast_width * scale, theme.toast_height * scale);
        // Above the screens' hint line at the bottom.
        let hint_line = theme.song_detail_size * scale * 1.5;
        ToastLayout {
            rect: Rect {
                x: width - margin - w,
                y: height - 2.0 * margin - hint_line - h,
                width: w,
                height: h,
            },
            scale,
        }
    }
}

/// The toast on the update thread.
#[derive(Clone, Copy, Debug, Default)]
pub struct Toast {
    frame: ToastFrame,
    /// When it hides unless the pointer is over it.
    hide_at_ns: u64,
    show_ns: u64,
}

impl Toast {
    /// A toast that stays `show_ms` once shown.
    pub fn new(theme: &Theme) -> Toast {
        Toast {
            show_ns: (theme.toast_show_ms.max(0.0) as f64 * 1e6) as u64,
            ..Toast::default()
        }
    }

    /// Shows it at `now_ns`, again if it is shown.
    pub fn show(&mut self, now_ns: u64) {
        self.show_variant(REPLAY_SAVED, now_ns);
    }

    /// Shows the entry `variant` of the labels at `now_ns`, again if it is shown.
    pub fn show_variant(&mut self, variant: u8, now_ns: u64) {
        self.frame.variant = variant;
        self.frame.visible = true;
        self.frame.since_ns = now_ns;
        self.hide_at_ns = now_ns.saturating_add(self.show_ns);
    }

    pub fn is_shown(&self) -> bool {
        self.frame.visible
    }

    /// Hides it once its time is up and the pointer is not over it.
    pub fn tick(&mut self, now_ns: u64) {
        if self.frame.visible && !self.frame.hover && now_ns >= self.hide_at_ns {
            self.hide(now_ns);
        }
    }

    /// Follows the pointer at `x`, `y`.
    pub fn pointer(&mut self, x: f32, y: f32, layout: &ToastLayout, now_ns: u64) {
        let over = self.frame.visible && layout.rect.contains(x, y);
        if self.frame.hover && !over {
            // Out again: a little more time to read it.
            self.hide_at_ns = self.hide_at_ns.max(now_ns.saturating_add(self.show_ns / 2));
        }
        self.frame.hover = over;
    }

    /// A press at `x`, `y`: true when it lands on the shown toast, which then hides. The
    /// press is the toast's.
    pub fn press(&mut self, x: f32, y: f32, layout: &ToastLayout, now_ns: u64) -> bool {
        if !self.frame.visible || !layout.rect.contains(x, y) {
            return false;
        }
        self.hide(now_ns);
        true
    }

    fn hide(&mut self, now_ns: u64) {
        self.frame.visible = false;
        self.frame.hover = false;
        self.frame.since_ns = now_ns;
    }

    pub fn frame(&self) -> ToastFrame {
        self.frame
    }
}

/// One text pair of the toast, already localised.
#[derive(Clone, Debug)]
pub struct ToastEntry {
    pub title: String,
    pub hint: String,
}

/// The toast's texts, already localised: entry `i` is shown for the frame's variant `i`.
#[derive(Clone, Debug)]
pub struct ToastLabels {
    pub entries: Vec<ToastEntry>,
}

/// The toast on the draw thread.
pub struct ToastView {
    theme: Theme,
    type_setter: Typesetter,
    scale: f32,
    title: Vec<Line>,
    hint: Vec<Line>,
}

impl std::fmt::Debug for ToastView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToastView").finish_non_exhaustive()
    }
}

impl ToastView {
    pub fn new(registry: Arc<FontRegistry>, theme: Theme, labels: &ToastLabels) -> ToastView {
        let mut view = ToastView {
            type_setter: Typesetter::new(registry, theme.font_weight),
            scale: 0.0,
            title: labels.entries.iter().map(|e| Line::new(&e.title)).collect(),
            hint: labels.entries.iter().map(|e| Line::new(&e.hint)).collect(),
            theme,
        };
        view.lay_out(1.0);
        view
    }

    /// Lays the texts out for a `width` × `height` window ahead of the first showing there.
    /// Allocates: call it off the hot paths.
    pub fn lay_out_for(&mut self, width: f32, height: f32) {
        let scale = ToastLayout::new(width, height, &self.theme).scale;
        if scale != self.scale {
            self.lay_out(scale);
        }
    }

    /// How many glyphs of its texts are not ready yet.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        self.title
            .iter()
            .chain(&self.hint)
            .flat_map(|l| l.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count()
    }

    fn lay_out(&mut self, scale: f32) {
        self.scale = scale;
        for title in &mut self.title {
            self.type_setter
                .lay_out_face(title, self.theme.caption_face(scale));
        }
        for hint in &mut self.hint {
            self.type_setter
                .lay_out_face(hint, self.theme.note_face(scale));
        }
    }

    /// Draws the toast of `frame` at `now_ns` on a `width` x `height` window. Does not
    /// allocate once laid out at the window's scale.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        frame: &ToastFrame,
        width: f32,
        height: f32,
        now_ns: u64,
        white: AtlasRegion,
    ) {
        let ms = |ms: f32| (ms.max(0.0) as f64 * 1e6) as u64;
        // In it decelerates onto its place; out it eases away.
        let shown = if frame.visible {
            ease_out_quint(progress(frame.since_ns, now_ns, ms(self.theme.toast_in_ms)))
        } else {
            let t = progress(frame.since_ns, now_ns, ms(self.theme.toast_out_ms));
            if frame.since_ns == 0 || t >= 1.0 {
                return;
            }
            1.0 - ease_out_cubic(t)
        };
        let layout = ToastLayout::new(width, height, &self.theme);
        if layout.scale != self.scale {
            self.lay_out(layout.scale);
        }
        let variant = usize::from(frame.variant);
        let (Some(title), Some(hint_line)) = (self.title.get(variant), self.hint.get(variant))
        else {
            return;
        };
        let theme = &self.theme;
        let scale = layout.scale;
        // From the right edge onto its place.
        let slide = (1.0 - shown) * (width - layout.rect.x);
        let rect = Rect {
            x: layout.rect.x + slide,
            ..layout.rect
        };
        let fill = if frame.hover {
            theme.button_hover_fill
        } else {
            theme.button_fill
        };
        crate::widgets::draw(
            shapes,
            white,
            &rect,
            faded(fill, shown),
            Look::Button {
                highlighted: frame.hover,
                seed: 0.37,
                fade: shown,
            },
            theme,
            scale,
        );
        // The accent strip on the left, as lazer's notifications.
        let strip = Rect {
            width: theme.toast_strip * scale,
            ..rect
        };
        crate::widgets::draw(
            shapes,
            white,
            &strip,
            faded(theme.accent, shown),
            Look::Strip,
            theme,
            scale,
        );
        let pad = theme.widget_pad * scale + strip.width;
        let half = Rect {
            height: rect.height / 2.0,
            y: rect.y + rect.height * 0.08,
            ..rect
        };
        push_left(text, title, &half, pad, faded(theme.text, shown), glyphs);
        let lower = Rect {
            y: rect.y + rect.height * 0.46,
            ..half
        };
        let hint: Color = if frame.hover {
            theme.text
        } else {
            theme.disabled_text
        };
        push_left(text, hint_line, &lower, pad, faded(hint, shown), glyphs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn it_shows_hides_after_its_time_and_waits_while_hovered() {
        let theme = Theme::default();
        let layout = ToastLayout::new(1280.0, 720.0, &theme);
        let mut toast = Toast::new(&theme);
        assert!(!toast.is_shown());
        toast.show(100 * MS);
        assert!(toast.is_shown());
        let show = theme.toast_show_ms as u64;
        toast.tick((100 + show - 1) * MS);
        assert!(toast.is_shown());
        // Hovered, it stays past its time.
        let [x, y] = layout.rect.centre();
        toast.pointer(x, y, &layout, 200 * MS);
        assert!(toast.frame().hover);
        toast.tick((100 + show + 1000) * MS);
        assert!(toast.is_shown());
        // Left, it waits a little more, then goes.
        toast.pointer(0.0, 0.0, &layout, (100 + show + 1000) * MS);
        toast.tick((100 + show + 1001) * MS);
        assert!(toast.is_shown());
        toast.tick((100 + 3 * show) * MS);
        assert!(!toast.is_shown());
    }

    #[test]
    fn a_press_on_it_is_taken_and_hides_it_elsewhere_not() {
        let theme = Theme::default();
        let layout = ToastLayout::new(1280.0, 720.0, &theme);
        let mut toast = Toast::new(&theme);
        let [x, y] = layout.rect.centre();
        // Hidden, it takes nothing.
        assert!(!toast.press(x, y, &layout, 0));
        toast.show(MS);
        assert!(!toast.press(10.0, 10.0, &layout, 2 * MS));
        assert!(toast.is_shown());
        assert!(toast.press(x, y, &layout, 3 * MS));
        assert!(!toast.is_shown());
        assert_eq!(toast.frame().since_ns, 3 * MS);
    }

    #[test]
    fn it_sits_in_the_lower_right_corner() {
        let theme = Theme::default();
        let layout = ToastLayout::new(1280.0, 720.0, &theme);
        assert!(layout.rect.x + layout.rect.width < 1280.0);
        assert!(layout.rect.y + layout.rect.height < 720.0);
        assert!(layout.rect.x > 640.0 && layout.rect.y > 360.0);
    }
}
