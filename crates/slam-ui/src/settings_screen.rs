//! The settings screen: its update-side state and layout, input-agnostic (the application
//! maps its events to [`SettingsInput`]). The draw side is [`view::SettingsView`].
//!
//! The screen knows nothing of the settings themselves: the application describes the rows
//! ([`RowSpec`]) in sections, and gets a [`SettingsAction::Changed`] with the row's index
//! and new value for every change. Section tabs are on the left, the rows of the chosen
//! section on the right.

use crate::menu::Rect;
use crate::theme::Theme;
use crate::widgets::{
    Dropdown, DropdownAction, DropdownInput, DropdownLayout, Slider, SliderAction, SliderInput,
    SliderLayout, Toggle, ToggleAction, ToggleInput, ToggleLayout,
};

pub mod view;

pub use view::{RowLabels, SettingsLabels, SettingsView};

/// Most rows; more are ignored.
pub const MAX_ROWS: usize = 24;
/// Most sections; rows of later ones go into the last.
pub const MAX_SECTIONS: usize = 6;

/// What a row edits, with its starting value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowKind {
    Toggle {
        value: bool,
    },
    Slider {
        min: f64,
        max: f64,
        step: f64,
        value: f64,
    },
    Dropdown {
        options: usize,
        selected: usize,
    },
}

/// One row: where it is and what it edits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowSpec {
    /// Index of the section it is shown in.
    pub section: usize,
    pub kind: RowKind,
    /// A change only takes effect after the application restarts: the row is marked.
    pub restart_required: bool,
}

/// The state of a row's widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowWidget {
    Toggle(Toggle),
    Slider(Slider),
    Dropdown(Dropdown),
}

/// A row's new value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowValue {
    Toggle(bool),
    Slider(f64),
    /// The index of the chosen dropdown option.
    Choice(usize),
}

/// What the screen understands; the application turns keys and mouse events into these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingsInput {
    /// Focus the row above, or the option above in an open dropdown.
    Up,
    Down,
    /// Decrease a slider, or step a closed dropdown back.
    Left,
    Right,
    /// Flip a toggle, open a dropdown or choose its highlighted option.
    Activate,
    /// Close an open dropdown, else close the screen.
    Back,
    NextSection,
    PrevSection,
    /// Pointer moved to `x`, `y` in layout units.
    PointerMove {
        x: f32,
        y: f32,
    },
    PointerPress {
        x: f32,
        y: f32,
    },
    PointerRelease,
}

/// What the screen asks the application to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingsAction {
    Changed { row: usize, value: RowValue },
    Close,
}

/// Where the screen's parts are: a pure function of the window size and theme, shared by
/// hit testing (update) and drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingsLayout {
    pub title: Rect,
    pub tabs: [Rect; MAX_SECTIONS],
    content: Rect,
    row_height: f32,
    pitch: f32,
    label_width: f32,
    theme: Theme,
    /// Pixels per reference unit.
    pub scale: f32,
}

impl SettingsLayout {
    pub fn new(width: f32, height: f32, theme: &Theme) -> SettingsLayout {
        let scale = theme.scale(height);
        let margin = theme.settings_margin * scale;
        let title_height = theme.settings_title_size * scale;
        let top = margin + title_height + margin;
        let (tab_width, pitch) = (
            theme.settings_tab_width * scale,
            theme.settings_row_pitch * scale,
        );
        let row_height = theme.widget_height * scale;
        let mut tabs = [Rect::default(); MAX_SECTIONS];
        for (i, tab) in tabs.iter_mut().enumerate() {
            *tab = Rect {
                x: margin,
                y: top + i as f32 * pitch,
                width: tab_width,
                height: row_height,
            };
        }
        let left = margin + tab_width + margin;
        let content = Rect {
            x: left,
            y: top,
            width: (width - left - margin).max(0.0),
            height: (height - top - margin).max(0.0),
        };
        SettingsLayout {
            title: Rect {
                x: margin,
                y: margin,
                width: (width - 2.0 * margin).max(0.0),
                height: title_height,
            },
            tabs,
            content,
            row_height,
            pitch,
            label_width: content.width * theme.settings_label_ratio,
            theme: *theme,
            scale,
        }
    }

    /// The layout moved `dx` to the right.
    pub fn shifted(&self, dx: f32) -> SettingsLayout {
        let mut layout = *self;
        layout.title.x += dx;
        layout.content.x += dx;
        for tab in &mut layout.tabs {
            tab.x += dx;
        }
        layout
    }

    /// The whole row at position `slot` in its section.
    pub fn row(&self, slot: usize) -> Rect {
        Rect {
            x: self.content.x,
            y: self.content.y + slot as f32 * self.pitch,
            width: self.content.width,
            height: self.row_height,
        }
    }

    /// Where the label of the row at `slot` is.
    pub fn label(&self, slot: usize) -> Rect {
        Rect {
            width: self.label_width,
            ..self.row(slot)
        }
    }

    /// Where the widget of the row at `slot` is: right of the label.
    pub fn widget(&self, slot: usize) -> Rect {
        let row = self.row(slot);
        let gap = self.theme.widget_pad * self.scale;
        Rect {
            x: row.x + self.label_width + gap,
            width: (row.width - self.label_width - gap).max(0.0),
            ..row
        }
    }

    /// The tab among the first `count` that contains the point.
    pub fn tab_at(&self, count: usize, x: f32, y: f32) -> Option<usize> {
        self.tabs
            .iter()
            .take(count)
            .position(|tab| tab.contains(x, y))
    }

    fn toggle(&self, slot: usize) -> ToggleLayout {
        // The whole row switches, not only the track.
        ToggleLayout::new(self.row(slot), &self.theme, self.scale)
    }

    fn slider(&self, slot: usize) -> SliderLayout {
        SliderLayout::new(self.widget(slot), &self.theme, self.scale)
    }

    fn dropdown(&self, slot: usize, count: usize) -> DropdownLayout {
        DropdownLayout::new(self.widget(slot), count, self.scale)
    }
}

/// What the draw side needs; `Copy` data of fixed size, no allocation.
#[derive(Clone, Debug)]
pub struct SettingsSnapshot {
    pub widgets: [RowWidget; MAX_ROWS],
    pub len: usize,
    /// Index of the focused row.
    pub focus: usize,
    pub section: usize,
}

/// The screen's state on the update thread.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsScreen {
    widgets: [RowWidget; MAX_ROWS],
    meta: [(u8, u8); MAX_ROWS],
    len: usize,
    sections: usize,
    focus: usize,
    section: usize,
}

impl SettingsScreen {
    /// A screen for `rows` in `sections` sections, on the first section's first row. Call it
    /// off the hot paths.
    pub fn new(rows: &[RowSpec], sections: usize) -> SettingsScreen {
        let sections = sections.clamp(1, MAX_SECTIONS);
        let mut widgets = [RowWidget::Toggle(Toggle::new(false)); MAX_ROWS];
        let mut meta = [(0, 0); MAX_ROWS];
        let mut counts = [0_u8; MAX_SECTIONS];
        let len = rows.len().min(MAX_ROWS);
        for (i, spec) in rows.iter().take(len).enumerate() {
            let section = spec.section.min(sections - 1);
            meta[i] = (section as u8, counts[section]);
            counts[section] = counts[section].saturating_add(1);
            widgets[i] = match spec.kind {
                RowKind::Toggle { value } => RowWidget::Toggle(Toggle::new(value)),
                RowKind::Slider {
                    min,
                    max,
                    step,
                    value,
                } => RowWidget::Slider(Slider::new(min, max, step, value)),
                RowKind::Dropdown { options, selected } => {
                    RowWidget::Dropdown(Dropdown::new(options, selected))
                }
            };
        }
        let mut screen = SettingsScreen {
            widgets,
            meta,
            len,
            sections,
            focus: 0,
            section: 0,
        };
        screen.focus = screen.first_row(0).unwrap_or(0);
        screen
    }

    pub fn sections(&self) -> usize {
        self.sections
    }

    pub fn section(&self) -> usize {
        self.section
    }

    /// Index of the focused row.
    pub fn focus(&self) -> usize {
        self.focus
    }

    /// The widget of row `row`.
    pub fn widget(&self, row: usize) -> Option<&RowWidget> {
        self.widgets[..self.len].get(row)
    }

    pub fn snapshot(&self) -> SettingsSnapshot {
        SettingsSnapshot {
            widgets: self.widgets,
            len: self.len,
            focus: self.focus,
            section: self.section,
        }
    }

    /// The position of `row` within its section.
    pub fn slot(&self, row: usize) -> usize {
        usize::from(self.meta[row].1)
    }

    fn section_of(&self, row: usize) -> usize {
        usize::from(self.meta[row].0)
    }

    fn rows_of(&self, section: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.len).filter(move |row| self.section_of(*row) == section)
    }

    fn first_row(&self, section: usize) -> Option<usize> {
        self.rows_of(section).next()
    }

    /// The row of the current section with an open dropdown.
    fn open_dropdown(&self) -> Option<usize> {
        self.rows_of(self.section)
            .find(|row| matches!(&self.widgets[*row], RowWidget::Dropdown(d) if d.is_open()))
    }

    fn close_dropdowns(&mut self) {
        for widget in &mut self.widgets[..self.len] {
            if let RowWidget::Dropdown(d) = widget {
                // Closing never changes the selection; the layout is not looked at.
                d.input(
                    DropdownInput::Back,
                    &DropdownLayout::new(Rect::default(), 0, 1.0),
                );
            }
        }
    }

    fn move_focus(&mut self, forward: bool) {
        let next = if forward {
            self.rows_of(self.section).find(|row| *row > self.focus)
        } else {
            self.rows_of(self.section)
                .filter(|row| *row < self.focus)
                .last()
        };
        if let Some(row) = next {
            self.focus = row;
        }
    }

    fn switch_section(&mut self, section: usize) {
        self.close_dropdowns();
        self.section = section;
        if let Some(row) = self.first_row(section) {
            self.focus = row;
        }
    }

    fn step_section(&mut self, forward: bool) {
        let n = self.sections;
        let section = if forward {
            (self.section + 1) % n
        } else {
            (self.section + n - 1) % n
        };
        self.switch_section(section);
    }

    /// Gives `input` to row `row`'s widget, in the form that widget understands.
    fn route(
        &mut self,
        row: usize,
        input: SettingsInput,
        layout: &SettingsLayout,
    ) -> Option<SettingsAction> {
        let slot = self.slot(row);
        let value = match &mut self.widgets[row] {
            RowWidget::Toggle(toggle) => {
                let input = match input {
                    SettingsInput::Activate => ToggleInput::Activate,
                    SettingsInput::PointerPress { x, y } => ToggleInput::PointerPress { x, y },
                    _ => return None,
                };
                let ToggleAction::Changed(v) = toggle.input(input, &layout.toggle(slot))?;
                RowValue::Toggle(v)
            }
            RowWidget::Slider(slider) => {
                let input = match input {
                    SettingsInput::Left => SliderInput::Decrease,
                    SettingsInput::Right => SliderInput::Increase,
                    SettingsInput::PointerPress { x, y } => SliderInput::PointerPress { x, y },
                    SettingsInput::PointerMove { x, y } => SliderInput::PointerMove { x, y },
                    SettingsInput::PointerRelease => SliderInput::PointerRelease,
                    _ => return None,
                };
                let SliderAction::Changed(v) = slider.input(input, &layout.slider(slot))?;
                RowValue::Slider(v)
            }
            RowWidget::Dropdown(dropdown) => {
                let open = dropdown.is_open();
                let input = match input {
                    SettingsInput::Up => DropdownInput::Up,
                    SettingsInput::Down => DropdownInput::Down,
                    // Closed, the arrows step the selection; open they do nothing.
                    SettingsInput::Left if !open => DropdownInput::Up,
                    SettingsInput::Right if !open => DropdownInput::Down,
                    SettingsInput::Activate => DropdownInput::Activate,
                    SettingsInput::Back => DropdownInput::Back,
                    SettingsInput::PointerMove { x, y } => DropdownInput::PointerMove { x, y },
                    SettingsInput::PointerPress { x, y } => DropdownInput::PointerPress { x, y },
                    _ => return None,
                };
                let layout = layout.dropdown(slot, dropdown.count());
                let DropdownAction::Selected(i) = dropdown.input(input, &layout)?;
                RowValue::Choice(i)
            }
        };
        Some(SettingsAction::Changed { row, value })
    }

    /// Applies `input`; `layout` places the rows for pointer input. Does not allocate.
    pub fn input(
        &mut self,
        input: SettingsInput,
        layout: &SettingsLayout,
    ) -> Option<SettingsAction> {
        if self.len == 0 {
            return matches!(input, SettingsInput::Back).then_some(SettingsAction::Close);
        }
        match input {
            SettingsInput::Up | SettingsInput::Down => {
                if self.open_dropdown().is_some() {
                    return self.route(self.focus, input, layout);
                }
                self.move_focus(matches!(input, SettingsInput::Down));
                None
            }
            SettingsInput::Left | SettingsInput::Right | SettingsInput::Activate => {
                self.route(self.focus, input, layout)
            }
            SettingsInput::Back => {
                if let Some(row) = self.open_dropdown() {
                    return self.route(row, input, layout);
                }
                Some(SettingsAction::Close)
            }
            SettingsInput::NextSection => {
                self.step_section(true);
                None
            }
            SettingsInput::PrevSection => {
                self.step_section(false);
                None
            }
            SettingsInput::PointerMove { x, y } => {
                if let Some(row) = self.open_dropdown() {
                    return self.route(row, input, layout);
                }
                let dragging = self.rows_of(self.section).find(
                    |row| matches!(&self.widgets[*row], RowWidget::Slider(s) if s.is_dragging()),
                );
                if let Some(row) = dragging {
                    return self.route(row, input, layout);
                }
                if let Some(row) = self.row_at(layout, x, y) {
                    self.focus = row;
                }
                None
            }
            SettingsInput::PointerPress { x, y } => {
                // An open list takes the press, inside it or not.
                if let Some(row) = self.open_dropdown() {
                    return self.route(row, input, layout);
                }
                if let Some(section) = layout.tab_at(self.sections, x, y) {
                    self.switch_section(section);
                    return None;
                }
                let row = self.row_at(layout, x, y)?;
                self.focus = row;
                self.route(row, input, layout)
            }
            SettingsInput::PointerRelease => {
                let mut action = None;
                for row in 0..self.len {
                    if matches!(self.widgets[row], RowWidget::Slider(_)) {
                        action = self.route(row, input, layout).or(action);
                    }
                }
                action
            }
        }
    }

    fn row_at(&self, layout: &SettingsLayout, x: f32, y: f32) -> Option<usize> {
        self.rows_of(self.section)
            .find(|row| layout.row(self.slot(*row)).contains(x, y))
    }
}

#[cfg(test)]
mod tests;
