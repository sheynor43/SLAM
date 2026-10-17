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

/// Where a dropdown and its open list are: the list's options are stacked right below the
/// box, each as high as the box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropdownLayout {
    pub bounds: Rect,
    pub count: usize,
    pub scale: f32,
}

impl DropdownLayout {
    pub fn new(bounds: Rect, count: usize, scale: f32) -> DropdownLayout {
        DropdownLayout {
            bounds,
            count,
            scale,
        }
    }

    /// The rectangle of option `index` in the open list.
    pub fn option(&self, index: usize) -> Rect {
        Rect {
            x: self.bounds.x,
            y: self.bounds.y + self.bounds.height * (index + 1) as f32,
            width: self.bounds.width,
            height: self.bounds.height,
        }
    }

    /// The whole open list.
    pub fn list(&self) -> Rect {
        Rect {
            x: self.bounds.x,
            y: self.bounds.y + self.bounds.height,
            width: self.bounds.width,
            height: self.bounds.height * self.count as f32,
        }
    }

    /// The option of the open list under the point.
    pub fn option_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.list().contains(x, y) || self.bounds.height <= 0.0 {
            return None;
        }
        let index = ((y - self.list().y) / self.bounds.height) as usize;
        (index < self.count).then_some(index)
    }
}

/// A dropdown's state on the update thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dropdown {
    count: usize,
    selected: usize,
    highlight: usize,
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

    fn open(&mut self) {
        if self.count > 0 {
            self.open = true;
            self.highlight = self.selected;
        }
    }

    fn choose(&mut self, index: usize) -> Option<DropdownAction> {
        let changed = index != self.selected && index < self.count;
        if changed {
            self.selected = index;
        }
        self.close();
        changed.then_some(DropdownAction::Selected(index))
    }

    fn step(&mut self, forward: bool) -> Option<DropdownAction> {
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
        match input {
            DropdownInput::Up => self.step(false),
            DropdownInput::Down => self.step(true),
            DropdownInput::Activate => {
                if self.open {
                    self.choose(self.highlight)
                } else {
                    self.open();
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
                        self.open();
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
        for (i, line) in self.options.iter().enumerate().take(layout.count) {
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
