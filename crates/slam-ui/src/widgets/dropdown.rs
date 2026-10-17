//! A dropdown: a box showing the selected option and a list below it while open.
//!
//! Keyboard: while closed, Up/Down change the selection directly; while open they move the
//! highlight and Activate confirms it. Back closes without changing anything. Neither
//! wraps around.

use std::sync::Arc;

use slam_render::atlas::AtlasRegion;
use slam_render::sprite::SpriteBatch;
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey};

use super::{fill, push_left};
use crate::menu::Rect;
use crate::menu::view::{Line, Typesetter};
use crate::theme::{Theme, faded};

/// What a dropdown understands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DropdownInput {
    Up,
    Down,
    /// Opens the list; while open, confirms the highlighted option.
    Activate,
    /// Closes the list without a change.
    Back,
    /// Wheel: positive `lines` scroll the open list towards its end. The highlight stays.
    Scroll {
        lines: f32,
    },
    /// Pointer moved: while open, highlights the option under it.
    PointerMove {
        x: f32,
        y: f32,
    },
    /// Primary button pressed: opens or closes the list, or picks an option.
    PointerPress {
        x: f32,
        y: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropdownAction {
    /// The selection changed to this option.
    Selected(usize),
}

/// The most options an open list shows at once; longer lists scroll.
pub const MAX_VISIBLE_OPTIONS: usize = 10;

/// Where a dropdown and its open list are. The list's options are stacked right below the
/// box, each as high as the box, at most [`MAX_VISIBLE_OPTIONS`] of them. The list never
/// leaves the window: if it does not fit below the box it opens above it when there is more
/// room there, and shows fewer options when neither side has room for all of them. The
/// first option shown is the scroll offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropdownLayout {
    pub bounds: Rect,
    pub count: usize,
    pub scale: f32,
    /// The window's height: the list stays above it. Unbounded by default.
    window_height: f32,
    scroll: usize,
}

impl DropdownLayout {
    pub fn new(bounds: Rect, count: usize, scale: f32) -> DropdownLayout {
        DropdownLayout {
            bounds,
            count,
            scale,
            window_height: f32::INFINITY,
            scroll: 0,
        }
    }

    /// The layout keeping the open list within a window `height` high.
    pub fn within(mut self, height: f32) -> DropdownLayout {
        self.window_height = height;
        self
    }

    /// The layout with the list scrolled to start at option `scroll`.
    pub fn scrolled(mut self, scroll: usize) -> DropdownLayout {
        self.scroll = scroll;
        self
    }

    /// How many options the open list shows, and whether it opens above the box.
    fn fit(&self) -> (usize, bool) {
        let wanted = self.count.min(MAX_VISIBLE_OPTIONS);
        let height = self.bounds.height;
        if wanted == 0 || height <= 0.0 {
            return (wanted, false);
        }
        let rows = |space: f32| (space.max(0.0) / height).floor() as usize;
        let below = rows(self.window_height - (self.bounds.y + height));
        if below >= wanted {
            return (wanted, false);
        }
        let above = rows(self.bounds.y);
        if above > below {
            (above.max(1).min(wanted), true)
        } else {
            (below.max(1).min(wanted), false)
        }
    }

    /// The number of options the open list shows.
    pub fn visible(&self) -> usize {
        self.fit().0
    }

    /// The index of the first option shown: the scroll offset kept within the list.
    pub fn first(&self) -> usize {
        self.scroll.min(self.count - self.visible())
    }

    /// The rectangle of option `index` in the open list; meaningful for the shown ones.
    pub fn option(&self, index: usize) -> Rect {
        Rect {
            y: self.list().y + self.bounds.height * (index as f32 - self.first() as f32),
            height: self.bounds.height,
            ..self.list()
        }
    }

    /// The whole open list.
    pub fn list(&self) -> Rect {
        let (visible, above) = self.fit();
        let height = self.bounds.height * visible as f32;
        Rect {
            x: self.bounds.x,
            y: if above {
                self.bounds.y - height
            } else {
                self.bounds.y + self.bounds.height
            },
            width: self.bounds.width,
            height,
        }
    }

    /// The option of the open list under the point.
    pub fn option_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.list().contains(x, y) || self.bounds.height <= 0.0 {
            return None;
        }
        let slot = ((y - self.list().y) / self.bounds.height) as usize;
        let index = self.first() + slot;
        (slot < self.visible() && index < self.count).then_some(index)
    }
}

/// A dropdown's state on the update thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dropdown {
    count: usize,
    selected: usize,
    highlight: usize,
    /// The first option an open list shows.
    scroll: usize,
    open: bool,
}

impl Dropdown {
    /// A dropdown over `count` options with `selected` chosen (clamped).
    pub fn new(count: usize, selected: usize) -> Dropdown {
        let selected = selected.min(count.saturating_sub(1));
        Dropdown {
            count,
            selected,
            highlight: selected,
            scroll: 0,
            open: false,
        }
    }

    /// The number of options.
    pub fn count(&self) -> usize {
        self.count
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The option an open list highlights.
    pub fn highlight(&self) -> usize {
        self.highlight
    }

    /// The first option an open list shows (before clamping to the layout).
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Selects `index` (clamped) without reporting an action.
    pub fn set(&mut self, index: usize) {
        self.selected = index.min(self.count.saturating_sub(1));
        self.highlight = self.selected;
    }

    fn close(&mut self) {
        self.open = false;
        self.highlight = self.selected;
    }

    fn open(&mut self, layout: &DropdownLayout) {
        if self.count > 0 {
            self.open = true;
            self.highlight = self.selected;
            self.reveal(self.selected, layout);
        }
    }

    /// Scrolls the least that shows option `index`.
    fn reveal(&mut self, index: usize, layout: &DropdownLayout) {
        let visible = layout.visible().max(1);
        if index < self.scroll {
            self.scroll = index;
        } else if index >= self.scroll + visible {
            self.scroll = index + 1 - visible;
        }
        self.scroll = self.scroll.min(self.count.saturating_sub(visible));
    }

    fn scroll_by(&mut self, lines: f32, layout: &DropdownLayout) {
        if !self.open || lines == 0.0 || lines.is_nan() {
            return;
        }
        // At least one option per wheel event, however small the delta.
        let steps = (lines.round() as isize).abs().max(1) * lines.signum() as isize;
        let last = self.count.saturating_sub(layout.visible());
        self.scroll = self.scroll.saturating_add_signed(steps).min(last);
    }

    fn choose(&mut self, index: usize) -> Option<DropdownAction> {
        let changed = index != self.selected && index < self.count;
        if changed {
            self.selected = index;
        }
        self.close();
        changed.then_some(DropdownAction::Selected(index))
    }

    fn step(&mut self, forward: bool, layout: &DropdownLayout) -> Option<DropdownAction> {
        let from = if self.open {
            self.highlight
        } else {
            self.selected
        };
        let to = if forward {
            (from + 1).min(self.count.saturating_sub(1))
        } else {
            from.saturating_sub(1)
        };
        if self.open {
            self.highlight = to;
            self.reveal(to, layout);
            None
        } else if to != self.selected && self.count > 0 {
            self.selected = to;
            self.highlight = to;
            Some(DropdownAction::Selected(to))
        } else {
            None
        }
    }

    /// Applies `input`; `layout` places the box and list for pointer input. Does not
    /// allocate.
    pub fn input(
        &mut self,
        input: DropdownInput,
        layout: &DropdownLayout,
    ) -> Option<DropdownAction> {
        let layout = &layout.scrolled(self.scroll);
        match input {
            DropdownInput::Up => self.step(false, layout),
            DropdownInput::Down => self.step(true, layout),
            DropdownInput::Scroll { lines } => {
                self.scroll_by(lines, layout);
                None
            }
            DropdownInput::Activate => {
                if self.open {
                    self.choose(self.highlight)
                } else {
                    self.open(layout);
                    None
                }
            }
            DropdownInput::Back => {
                self.close();
                None
            }
            DropdownInput::PointerMove { x, y } => {
                if self.open
                    && let Some(index) = layout.option_at(x, y)
                {
                    self.highlight = index;
                }
                None
            }
            DropdownInput::PointerPress { x, y } => {
                if self.open {
                    match layout.option_at(x, y) {
                        Some(index) => self.choose(index),
                        None => {
                            self.close();
                            None
                        }
                    }
                } else {
                    if layout.bounds.contains(x, y) {
                        self.open(layout);
                    }
                    None
                }
            }
        }
    }
}

/// The draw side of [`Dropdown`]. The option texts are laid out when the view is made and
/// when the scale changes.
pub struct DropdownView {
    theme: Theme,
    type_setter: Typesetter,
    scale: f32,
    options: Vec<Line>,
}

impl std::fmt::Debug for DropdownView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DropdownView")
            .field("options", &self.options.len())
            .finish_non_exhaustive()
    }
}

impl DropdownView {
    /// `options` are the localised option labels. Lays them out once: call it off the hot
    /// paths.
    pub fn new<S: AsRef<str>>(
        registry: Arc<FontRegistry>,
        theme: Theme,
        options: &[S],
    ) -> DropdownView {
        let mut view = DropdownView {
            type_setter: Typesetter::new(registry, theme.font_weight),
            scale: 1.0,
            options: options.iter().map(|o| Line::new(o.as_ref())).collect(),
            theme,
        };
        view.lay_out();
        view
    }

    fn lay_out(&mut self) {
        let size = self.theme.widget_text_size * self.scale;
        for option in &mut self.options {
            self.type_setter.lay_out(option, size);
        }
    }

    /// Asks `glyphs` for every glyph of every option. Returns how many are not ready yet.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        self.options
            .iter()
            .flat_map(|line| line.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count()
    }

    /// Writes the dropdown into `shapes` and its texts into `text`; the list only while
    /// open. Allocation-free once warm and once the batches have room.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut SpriteBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        state: &Dropdown,
        layout: &DropdownLayout,
        opacity: f32,
        white: AtlasRegion,
    ) {
        if layout.scale != self.scale {
            self.scale = layout.scale;
            self.lay_out();
        }
        let layout = &layout.scrolled(state.scroll);
        let theme = &self.theme;
        let pad = theme.widget_pad * layout.scale;
        let box_fill = if state.open {
            theme.button_hover_fill
        } else {
            theme.button_fill
        };
        fill(shapes, white, &layout.bounds, faded(box_fill, opacity));
        let colour = faded(theme.text, opacity);
        if let Some(line) = self.options.get(state.selected) {
            push_left(text, line, &layout.bounds, pad, colour, glyphs);
        }
        if !state.open {
            return;
        }
        fill(
            shapes,
            white,
            &layout.list(),
            faded(theme.widget_popup_fill, opacity),
        );
        let first = layout.first();
        let shown = self.options.iter().enumerate().skip(first);
        for (i, line) in shown.take(layout.visible().min(layout.count)) {
            let rect = layout.option(i);
            if i == state.highlight {
                fill(
                    shapes,
                    white,
                    &rect,
                    faded(theme.button_hover_fill, opacity),
                );
            }
            push_left(text, line, &rect, pad, colour, glyphs);
        }
    }
}
