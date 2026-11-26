//! A slider over a numeric range with a fixed step.

use std::fmt::Write as _;
use std::sync::Arc;

use slam_render::atlas::AtlasRegion;
use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey};

use super::value_edit::ValueEdit;
use super::{Look, TEXT_CAPACITY, draw, push_right};
use crate::menu::Rect;
use crate::menu::view::{Line, Typesetter};
use crate::theme::{Theme, faded};

/// How many steps a coarse change moves.
pub const COARSE_STEPS: f64 = 10.0;

/// What a slider understands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SliderInput {
    /// One step down (Left arrow).
    Decrease,
    /// One step up (Right arrow).
    Increase,
    /// [`COARSE_STEPS`] steps down (for example Shift + Left).
    DecreaseCoarse,
    /// [`COARSE_STEPS`] steps up.
    IncreaseCoarse,
    /// Primary button pressed at `x`, `y`: on the track it starts dragging and jumps there.
    PointerPress { x: f32, y: f32 },
    /// Pointer moved; only matters while dragging.
    PointerMove { x: f32, y: f32 },
    /// Primary button released: stops dragging.
    PointerRelease,
    /// Enter: starts typing the value, or commits what is typed.
    Activate,
    /// A typed character while typing the value (see [`ValueEdit::type_char`]).
    Type(char),
    /// Backspace while typing.
    Erase,
    /// Ctrl+Backspace while typing.
    EraseWord,
    /// Escape while typing: drops the typed text.
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SliderAction {
    /// The snapped value changed to this.
    Changed(f64),
}

/// Where a slider is: the track on the left, the value text on the right of `bounds`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderLayout {
    pub bounds: Rect,
    /// The visible track.
    pub track: Rect,
    /// The area that starts a drag: the track's width, the whole height of `bounds`.
    pub hit: Rect,
    /// Where the value text goes (right aligned).
    pub value: Rect,
    /// How far the value text stays from the right end of `value`, so it does not touch
    /// the edge of the row's plate.
    pub value_pad: f32,
    pub scale: f32,
}

impl SliderLayout {
    /// `scale` is pixels per reference unit (see [`Theme::scale`]).
    pub fn new(bounds: Rect, theme: &Theme, scale: f32) -> SliderLayout {
        let value_width = theme.widget_value_width * scale;
        let gap = theme.widget_value_gap * scale;
        let track_width = (bounds.width - value_width - gap).max(0.0);
        let thickness = theme.widget_track_height * scale;
        SliderLayout {
            bounds,
            track: Rect {
                x: bounds.x,
                y: bounds.y + (bounds.height - thickness) / 2.0,
                width: track_width,
                height: thickness,
            },
            hit: Rect {
                x: bounds.x,
                y: bounds.y,
                width: track_width,
                height: bounds.height,
            },
            value: Rect {
                x: bounds.x + bounds.width - value_width,
                y: bounds.y,
                width: value_width,
                height: bounds.height,
            },
            value_pad: theme.widget_pad * scale,
            scale,
        }
    }
}

/// A slider's state on the update thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slider {
    min: f64,
    max: f64,
    step: f64,
    value: f64,
    dragging: bool,
    /// The value being typed (ADR-0172).
    edit: Option<ValueEdit>,
}

impl Slider {
    /// A slider over `min..=max` in steps of `step` (counted from `min`), at `value`
    /// clamped and snapped. A reversed range collapses to `min`; a step that is not a
    /// positive finite number becomes the whole range (or 1 for an empty one).
    pub fn new(min: f64, max: f64, step: f64, value: f64) -> Slider {
        let max = if max >= min { max } else { min };
        let step = if step.is_finite() && step > 0.0 {
            step
        } else if max > min {
            max - min
        } else {
            1.0
        };
        let mut slider = Slider {
            min,
            max,
            step,
            value: min,
            dragging: false,
            edit: None,
        };
        slider.value = slider.snap(value);
        slider
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn min(&self) -> f64 {
        self.min
    }

    pub fn max(&self) -> f64 {
        self.max
    }

    pub fn step(&self) -> f64 {
        self.step
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// The value as a fraction of the range, `0.0..=1.0`.
    pub fn fraction(&self) -> f32 {
        if self.max > self.min {
            ((self.value - self.min) / (self.max - self.min)) as f32
        } else {
            0.0
        }
    }

    /// Sets the value (clamped and snapped) without reporting an action.
    pub fn set(&mut self, value: f64) {
        self.value = self.snap(value);
    }

    /// `value` clamped to the range and rounded to the nearest step. The grid point is
    /// computed from a whole step count, so no error builds up over repeated steps.
    fn snap(&self, value: f64) -> f64 {
        if value.is_nan() {
            return self.value;
        }
        let clamped = value.clamp(self.min, self.max);
        let steps = ((clamped - self.min) / self.step).round();
        (self.min + steps * self.step).clamp(self.min, self.max)
    }

    fn change(&mut self, value: f64) -> Option<SliderAction> {
        let snapped = self.snap(value);
        if snapped == self.value {
            return None;
        }
        self.value = snapped;
        Some(SliderAction::Changed(snapped))
    }

    fn at_pointer(&mut self, x: f32, layout: &SliderLayout) -> Option<SliderAction> {
        let width = f64::from(layout.track.width);
        let t = if width > 0.0 {
            ((f64::from(x) - f64::from(layout.track.x)) / width).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.change(self.min + t * (self.max - self.min))
    }

    /// Digits after the point that tell the steps apart.
    fn decimals(&self) -> usize {
        let mut decimals = 0;
        let mut scaled = self.step;
        while decimals < 6 && (scaled - scaled.round()).abs() > 1e-9 {
            scaled *= 10.0;
            decimals += 1;
        }
        decimals
    }

    /// Whether the value is being typed.
    pub fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// The text being typed, if any.
    pub fn edit_text(&self) -> Option<&str> {
        self.edit.as_ref().map(ValueEdit::as_str)
    }

    /// Starts typing the value, showing the current one (the first key replaces it).
    pub fn begin_edit(&mut self) {
        self.dragging = false;
        self.edit = Some(ValueEdit::of_value(self.value, self.decimals()));
    }

    /// Ends the typing, dropping the text.
    pub fn cancel_edit(&mut self) {
        self.edit = None;
    }

    /// Ends the typing and takes the number typed, clamped to the range and snapped to the
    /// step; text that is no number leaves the value alone.
    pub fn commit_edit(&mut self) -> Option<SliderAction> {
        let typed = self.edit.take()?.parse()?;
        self.change(typed)
    }

    /// Applies `input`; `layout` places the track for pointer input. Does not allocate.
    pub fn input(&mut self, input: SliderInput, layout: &SliderLayout) -> Option<SliderAction> {
        if let Some(edit) = &mut self.edit {
            match input {
                SliderInput::Type(c) => {
                    edit.type_char(c);
                    return None;
                }
                SliderInput::Erase => {
                    edit.erase();
                    return None;
                }
                SliderInput::EraseWord => {
                    edit.erase_all();
                    return None;
                }
                SliderInput::Back => {
                    self.edit = None;
                    return None;
                }
                SliderInput::Activate => return self.commit_edit(),
                // A press on the value text keeps typing; elsewhere it ends the typing and
                // goes on as a press.
                SliderInput::PointerPress { x, y } => {
                    if layout.value.contains(x, y) {
                        return None;
                    }
                    let committed = self.commit_edit();
                    return self.input(input, layout).or(committed);
                }
                SliderInput::PointerMove { .. }
                | SliderInput::PointerRelease
                | SliderInput::Decrease
                | SliderInput::Increase
                | SliderInput::DecreaseCoarse
                | SliderInput::IncreaseCoarse => return None,
            }
        }
        match input {
            SliderInput::Activate => {
                self.begin_edit();
                None
            }
            SliderInput::Type(_)
            | SliderInput::Erase
            | SliderInput::EraseWord
            | SliderInput::Back => None,
            SliderInput::Decrease => self.change(self.value - self.step),
            SliderInput::Increase => self.change(self.value + self.step),
            SliderInput::DecreaseCoarse => self.change(self.value - COARSE_STEPS * self.step),
            SliderInput::IncreaseCoarse => self.change(self.value + COARSE_STEPS * self.step),
            SliderInput::PointerPress { x, y } => {
                if layout.value.contains(x, y) {
                    // A press on the value text types the value.
                    self.begin_edit();
                    None
                } else if layout.hit.contains(x, y) {
                    self.dragging = true;
                    self.at_pointer(x, layout)
                } else {
                    None
                }
            }
            SliderInput::PointerMove { x, .. } => {
                if self.dragging {
                    self.at_pointer(x, layout)
                } else {
                    None
                }
            }
            SliderInput::PointerRelease => {
                self.dragging = false;
                None
            }
        }
    }
}

/// Glyphs a value text can contain, requested up front so that a changing value never
/// meets a glyph that was not asked for.
const VALUE_CHARS: &str = "0123456789.-";

/// The draw side of [`Slider`]. The value text is formatted into a reserved buffer with a
/// fixed number of decimals and laid out again only when the value changes.
pub struct SliderView {
    theme: Theme,
    type_setter: Typesetter,
    scale: f32,
    decimals: usize,
    /// Drop trailing zeros and a bare point, as `0.##` does.
    trim_zeros: bool,
    unit: String,
    /// Shown instead of the value and the unit at zero; empty shows the number.
    zero_text: String,
    /// The value `value_line` shows (as bits, so that NaN-free comparison is exact).
    shown: Option<u64>,
    value_line: Line,
    charset: Line,
    /// The text being typed, laid out, and what it did last (the caret's blink).
    edit_line: Line,
    edit_type: crate::typing::TypeFeedback,
    editing: bool,
    edit_scale: f32,
}

impl std::fmt::Debug for SliderView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SliderView")
            .field("value", &self.value_line.text)
            .finish_non_exhaustive()
    }
}

impl SliderView {
    /// `decimals` digits after the point and `unit` (localised, may be empty; shown after
    /// the number as given, so include a leading space if wanted). Lays the texts out once:
    /// call it off the hot paths.
    pub fn new(
        registry: Arc<FontRegistry>,
        theme: Theme,
        decimals: usize,
        unit: &str,
    ) -> SliderView {
        let mut value_line = Line::new("");
        value_line.text.reserve(TEXT_CAPACITY);
        let mut edit_line = Line::new("");
        edit_line.text.reserve(super::value_edit::EDIT_CAPACITY);
        let mut charset = Line::new(VALUE_CHARS);
        charset.text.push_str(unit);
        let mut view = SliderView {
            type_setter: Typesetter::new(registry, theme.font_weight),
            scale: 1.0,
            decimals,
            trim_zeros: false,
            unit: unit.to_owned(),
            zero_text: String::new(),
            shown: None,
            value_line,
            charset,
            edit_line,
            edit_type: crate::typing::TypeFeedback::new(),
            editing: false,
            edit_scale: 0.0,
            theme,
        };
        view.lay_out_charset();
        view
    }

    /// Shows the value without trailing zeros (`1`, `1.5`, `2.25` for two decimals), as
    /// lazer's `0.##` format.
    pub fn trimmed(mut self) -> SliderView {
        self.trim_zeros = true;
        self
    }

    /// Shows `text` (localised) instead of the value and the unit when the value is zero,
    /// as lazer's "Disabled" of the hitsound stereo separation. Empty shows the number.
    pub fn with_zero_text(mut self, text: &str) -> SliderView {
        self.zero_text = text.to_owned();
        self.charset.text.push_str(text);
        self.lay_out_charset();
        // Grow the value line to hold the zero text, so that showing it later in a frame
        // does not allocate.
        self.value_line.text.clear();
        self.value_line.text.reserve(text.len());
        self.value_line.text.push_str(text);
        let face = self.theme.caption_face(self.scale);
        self.type_setter.lay_out_face(&mut self.value_line, face);
        self.shown = None;
        self
    }

    fn lay_out_charset(&mut self) {
        let face = self.theme.caption_face(self.scale);
        self.type_setter.lay_out_face(&mut self.charset, face);
    }

    /// Asks `glyphs` for every glyph a value can show. Returns how many are not ready yet.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        self.charset
            .layout
            .glyphs()
            .iter()
            .chain(self.value_line.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count()
    }

    fn update_text(&mut self, value: f64) {
        let bits = value.to_bits();
        if self.shown == Some(bits) {
            return;
        }
        self.shown = Some(bits);
        self.value_line.text.clear();
        if value == 0.0 && !self.zero_text.is_empty() {
            self.value_line.text.push_str(&self.zero_text);
            let face = self.theme.caption_face(self.scale);
            self.type_setter.lay_out_face(&mut self.value_line, face);
            return;
        }
        let _ = write!(
            self.value_line.text,
            "{value:.decimals$}",
            decimals = self.decimals
        );
        if self.trim_zeros && self.value_line.text.contains('.') {
            let kept = self
                .value_line
                .text
                .trim_end_matches('0')
                .trim_end_matches('.');
            let len = kept.len();
            self.value_line.text.truncate(len);
        }
        self.value_line.text.push_str(&self.unit);
        let face = self.theme.caption_face(self.scale);
        self.type_setter.lay_out_face(&mut self.value_line, face);
    }

    /// Writes the slider into `shapes` and its value text into `text`. Allocation-free once
    /// warm and once the batches have room.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        slider: &Slider,
        layout: &SliderLayout,
        opacity: f32,
        white: AtlasRegion,
        now_ns: u64,
    ) {
        if layout.scale != self.scale {
            self.scale = layout.scale;
            self.shown = None;
            self.lay_out_charset();
        }
        self.update_text(slider.value);
        let theme = &self.theme;
        let s = layout.scale;
        let track = faded(theme.button_fill, opacity);
        draw(shapes, white, &layout.track, track, Look::Pill, theme, s);
        let filled = Rect {
            width: layout.track.width * slider.fraction(),
            ..layout.track
        };
        let accent = faded(theme.widget_accent_fill, opacity);
        draw(shapes, white, &filled, accent, Look::Pill, theme, s);
        let knob = (
            theme.widget_knob_width * layout.scale,
            theme.widget_knob_height * layout.scale,
        );
        let knob = Rect {
            x: filled.x + filled.width - knob.0 / 2.0,
            y: layout.bounds.y + (layout.bounds.height - knob.1) / 2.0,
            width: knob.0,
            height: knob.1,
        };
        draw(
            shapes,
            white,
            &knob,
            faded(theme.text, opacity),
            Look::Pill,
            theme,
            s,
        );
        let Some(typed) = slider.edit_text() else {
            self.editing = false;
            push_right(
                text,
                &self.value_line,
                &layout.value,
                layout.value_pad,
                faded(theme.text, opacity),
                glyphs,
            );
            return;
        };
        // Typing the value: a field over the value text, the typed text and a caret.
        if !self.editing {
            self.editing = true;
            self.edit_type = crate::typing::TypeFeedback::new();
            self.edit_type.reset(now_ns);
            self.edit_line.text.clear();
        }
        self.edit_type.observe(typed, now_ns);
        if self.edit_line.text != typed || layout.scale != self.edit_scale {
            self.edit_line.text.clear();
            self.edit_line.text.push_str(typed);
            let face = theme.caption_face(layout.scale);
            self.type_setter.lay_out_face(&mut self.edit_line, face);
            self.edit_scale = layout.scale;
        }
        let inset = layout.value_pad / 4.0;
        let field = Rect {
            y: layout.value.y + inset,
            height: (layout.value.height - 2.0 * inset).max(0.0),
            ..layout.value
        };
        draw(
            shapes,
            white,
            &field,
            faded(theme.button_hover_fill, opacity),
            Look::Round,
            theme,
            s,
        );
        push_right(
            text,
            &self.edit_line,
            &layout.value,
            layout.value_pad,
            faded(theme.text, opacity),
            glyphs,
        );
        if self.edit_type.caret_on(now_ns, theme) {
            let end = layout.value.x + layout.value.width - layout.value_pad;
            crate::typing::push_caret(
                shapes,
                white,
                end + s,
                layout.value.y + layout.value.height / 2.0,
                self.edit_line.layout.height().max(theme.caption_size * s),
                opacity,
                theme,
                s,
            );
        }
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;

    fn shown(view: &mut SliderView, value: f64) -> String {
        view.update_text(value);
        view.value_line.text.clone()
    }

    #[test]
    fn a_fixed_view_keeps_its_zeros() {
        let registry = Arc::new(FontRegistry::with_builtin());
        let mut view = SliderView::new(registry, Theme::default(), 2, "x");
        assert_eq!(shown(&mut view, 1.0), "1.00x");
        assert_eq!(shown(&mut view, 1.5), "1.50x");
    }

    #[test]
    fn a_trimmed_view_drops_trailing_zeros_like_the_0_dot_hash_format() {
        let registry = Arc::new(FontRegistry::with_builtin());
        let mut view = SliderView::new(registry, Theme::default(), 2, "x").trimmed();
        assert_eq!(shown(&mut view, 1.0), "1x");
        assert_eq!(shown(&mut view, 1.5), "1.5x");
        assert_eq!(shown(&mut view, 2.25), "2.25x");
        assert_eq!(shown(&mut view, 10.0), "10x");
        assert_eq!(shown(&mut view, 0.1), "0.1x");
        assert_eq!(shown(&mut view, 0.0), "0x");
    }

    #[test]
    fn a_zero_text_replaces_the_value_and_unit_at_zero_only() {
        let registry = Arc::new(FontRegistry::with_builtin());
        let mut view = SliderView::new(registry, Theme::default(), 0, "%").with_zero_text("Off");
        assert_eq!(shown(&mut view, 0.0), "Off");
        assert_eq!(shown(&mut view, 20.0), "20%");
        assert_eq!(shown(&mut view, 0.0), "Off");
    }

    #[test]
    fn a_trimmed_view_without_decimals_keeps_whole_numbers() {
        let registry = Arc::new(FontRegistry::with_builtin());
        let mut view = SliderView::new(registry, Theme::default(), 0, "").trimmed();
        assert_eq!(shown(&mut view, 100.0), "100");
        assert_eq!(shown(&mut view, 0.0), "0");
    }
}
