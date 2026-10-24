//! The settings screen: its update-side state and layout, input-agnostic (the application
//! maps its events to [`SettingsInput`]). The draw side is [`view::SettingsView`].
//!
//! The screen knows nothing of the settings themselves: the application describes the rows
//! ([`RowSpec`]) in sections, and gets a [`SettingsAction::Changed`] with the row's index
//! and new value for every change. Section tabs are on the left, the rows of the chosen
//! section on the right.

use std::sync::Arc;

use crate::menu::Rect;
use crate::theme::Theme;
use crate::widgets::{
    Dropdown, DropdownAction, DropdownInput, DropdownLayout, KeyBind, KeyBindAction, KeyBindInput,
    KeyBindLayout, Slider, SliderAction, SliderInput, SliderLayout, Toggle, ToggleAction,
    ToggleInput, ToggleLayout,
};

pub mod view;

pub use view::{KeyLabels, RowLabels, SettingsLabels, SettingsView};

/// Most rows; more are ignored.
pub const MAX_ROWS: usize = 32;
/// Most sections; rows of later ones go into the last.
pub const MAX_SECTIONS: usize = 6;

/// A mouse button, as the screen names it; the application converts to and from its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKey {
    Left,
    Middle,
    Right,
    X1,
    X2,
}

impl MouseKey {
    /// Every button, in the order of [`MouseKey::index`].
    pub const ALL: [MouseKey; 5] = [
        MouseKey::Left,
        MouseKey::Middle,
        MouseKey::Right,
        MouseKey::X1,
        MouseKey::X2,
    ];

    /// The position in [`MouseKey::ALL`].
    pub fn index(self) -> usize {
        match self {
            MouseKey::Left => 0,
            MouseKey::Middle => 1,
            MouseKey::Right => 2,
            MouseKey::X1 => 3,
            MouseKey::X2 => 4,
        }
    }
}

/// What a key row shows: nothing, a keyboard key by scancode (USB HID usage id) or a mouse
/// button. The screen never turns it into text; the draw side looks the label up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyValue {
    Empty,
    Key(u16),
    Mouse(MouseKey),
}

/// What a row edits, with its starting value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowKind {
    /// A key binding slot; the application owns the binding, the screen only listens.
    Key {
        value: KeyValue,
    },
    /// A button that asks the application to do something.
    Button,
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
    Key(KeyRow),
    Button,
    Toggle(Toggle),
    Slider(Slider),
    Dropdown(Dropdown),
}

/// The state of a key row: the binding shown and whether it waits for a key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyRow {
    bind: KeyBind,
    value: KeyValue,
}

impl KeyRow {
    pub fn value(&self) -> KeyValue {
        self.value
    }

    pub fn is_listening(&self) -> bool {
        self.bind.is_listening()
    }
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
    /// Wheel: positive `lines` scroll an open dropdown list towards its end.
    Scroll {
        lines: f32,
    },
    PointerRelease,
    /// An input the application captured while a key row listens. `Empty` is ignored.
    Captured(KeyValue),
    /// Delete while a key row listens: empties the slot.
    Clear,
}

/// What the screen asks the application to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingsAction {
    Changed {
        row: usize,
        value: RowValue,
    },
    /// The listening key row `row` got `key`. The application binds it, or asks
    /// [`SettingsScreen::ask_conflict`] when it is taken, and refreshes the key rows with
    /// [`SettingsScreen::set_key`].
    KeyCaptured {
        row: usize,
        key: KeyValue,
    },
    /// The listening key row `row` is to be emptied.
    KeyCleared {
        row: usize,
    },
    /// The button row `row` was pressed.
    Pressed {
        row: usize,
    },
    /// The player answered the conflict of `key` and key row `row`: `apply` takes the key
    /// over from its other place, else the old binding stays.
    ConflictResolved {
        row: usize,
        key: KeyValue,
        apply: bool,
    },
    Close,
}

/// The question shown when a captured key is already bound elsewhere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conflict {
    /// The key row being bound.
    pub row: usize,
    pub key: KeyValue,
    /// The key row that holds `key` now, if it is on the screen.
    pub other: Option<usize>,
    /// The highlighted answer: apply the new binding (`true`) or keep the old.
    pub apply: bool,
}

/// Where the screen's parts are: a pure function of the window size and theme, shared by
/// hit testing (update) and drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingsLayout {
    pub title: Rect,
    pub tabs: [Rect; MAX_SECTIONS],
    content: Rect,
    /// The window's height, which an open dropdown list stays within.
    window_height: f32,
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
            window_height: height,
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

    /// The box of the conflict question: it takes the place of the rows. Its parts are the
    /// rows 0 to 2 of [`SettingsLayout::dialog_row`] (title, key, other binding) and the two
    /// buttons of [`SettingsLayout::conflict_buttons`] below them.
    pub fn dialog(&self) -> Rect {
        Rect {
            height: 3.0 * self.pitch + self.row_height,
            ..self.row(0)
        }
    }

    /// Row `i` (0 to 2) of the conflict box.
    pub fn dialog_row(&self, i: usize) -> Rect {
        self.row(i)
    }

    /// The "keep" and "apply" buttons, left and right.
    pub fn conflict_buttons(&self) -> [Rect; 2] {
        let row = self.row(3);
        let gap = self.theme.widget_pad * self.scale;
        let width = ((row.width - gap) / 2.0).max(0.0);
        [
            Rect { width, ..row },
            Rect {
                x: row.x + width + gap,
                width,
                ..row
            },
        ]
    }

    fn toggle(&self, slot: usize) -> ToggleLayout {
        // The whole row switches, not only the track.
        ToggleLayout::new(self.row(slot), &self.theme, self.scale)
    }

    fn slider(&self, slot: usize) -> SliderLayout {
        SliderLayout::new(self.widget(slot), &self.theme, self.scale)
    }

    fn dropdown(&self, slot: usize, count: usize) -> DropdownLayout {
        DropdownLayout::new(self.widget(slot), count, self.scale).within(self.window_height)
    }

    /// Where the dropdown of the row at `slot` and its open list are.
    pub fn dropdown_at(&self, slot: usize, count: usize) -> DropdownLayout {
        self.dropdown(slot, count)
    }
}

/// What the draw side needs; cloning copies fixed-size data and an `Arc`, no allocation.
#[derive(Clone, Debug)]
pub struct SettingsSnapshot {
    pub widgets: [RowWidget; MAX_ROWS],
    pub len: usize,
    /// Index of the focused row.
    pub focus: usize,
    pub section: usize,
    pub conflict: Option<Conflict>,
    /// The text of the button row that shows a value (see [`RowLabels::shows_value`]).
    pub value_text: Arc<str>,
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
    conflict: Option<Conflict>,
    value_text: Arc<str>,
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
                RowKind::Key { value } => RowWidget::Key(KeyRow {
                    bind: KeyBind::new(),
                    value,
                }),
                RowKind::Button => RowWidget::Button,
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
            conflict: None,
            value_text: Arc::from(""),
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
            conflict: self.conflict,
            value_text: Arc::clone(&self.value_text),
        }
    }

    /// Sets the text of the button row that shows a value (an empty text shows the row's
    /// own button label as a placeholder). The application builds the `Arc` off the update
    /// thread; this only stores it.
    pub fn set_value_text(&mut self, text: Arc<str>) {
        self.value_text = text;
    }

    /// Gives the dropdown row `row` `options` options with `selected` chosen (clamped),
    /// closed: its options changed while the screen was open. Other rows are left alone.
    pub fn set_dropdown(&mut self, row: usize, options: usize, selected: usize) {
        if let Some(RowWidget::Dropdown(dropdown)) = self.widgets[..self.len].get_mut(row) {
            *dropdown = Dropdown::new(options, selected);
        }
    }

    /// The text set by [`Self::set_value_text`].
    pub fn value_text(&self) -> &str {
        &self.value_text
    }

    /// Whether a key row waits for a key: the application then gives it every key and mouse
    /// button as [`SettingsInput::Captured`].
    pub fn is_listening(&self) -> bool {
        self.listening_row().is_some()
    }

    fn listening_row(&self) -> Option<usize> {
        self.widgets[..self.len]
            .iter()
            .position(|w| matches!(w, RowWidget::Key(k) if k.is_listening()))
    }

    /// Shows `value` in key row `row`. Anything else is left alone.
    pub fn set_key(&mut self, row: usize, value: KeyValue) {
        if let Some(RowWidget::Key(k)) = self.widgets[..self.len].get_mut(row) {
            k.value = value;
        }
    }

    /// Shows `value` in toggle row `row`, changed from outside the screen (a hotkey).
    /// Anything else is left alone.
    pub fn set_toggle(&mut self, row: usize, value: bool) {
        if let Some(RowWidget::Toggle(t)) = self.widgets[..self.len].get_mut(row) {
            t.set(value);
        }
    }

    /// Asks whether to take `key` for key row `row` although the key row `other` (if on the
    /// screen) has it. The screen answers with [`SettingsAction::ConflictResolved`].
    pub fn ask_conflict(&mut self, row: usize, key: KeyValue, other: Option<usize>) {
        self.conflict = Some(Conflict {
            row,
            key,
            other,
            apply: false,
        });
    }

    /// The question being asked.
    pub fn conflict(&self) -> Option<&Conflict> {
        self.conflict.as_ref()
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
            RowWidget::Key(key) => {
                let input = match input {
                    SettingsInput::Activate => KeyBindInput::Activate,
                    SettingsInput::PointerPress { x, y } => KeyBindInput::PointerPress { x, y },
                    _ => return None,
                };
                // Starting to listen is not for the application to know; it asks the screen.
                key.bind.input::<KeyValue>(
                    input,
                    &KeyBindLayout::new(layout.widget(slot), layout.scale),
                );
                return None;
            }
            RowWidget::Button => {
                return matches!(
                    input,
                    SettingsInput::Activate | SettingsInput::PointerPress { .. }
                )
                .then_some(SettingsAction::Pressed { row });
            }
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
                    SettingsInput::Scroll { lines } => DropdownInput::Scroll { lines },
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
        if let Some(conflict) = self.conflict {
            return self.answer(conflict, input, layout);
        }
        if let Some(row) = self.listening_row() {
            return self.listen(row, input);
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
            SettingsInput::Scroll { .. } => {
                let row = self.open_dropdown()?;
                self.route(row, input, layout)
            }
            // Only meaningful while a key row listens.
            SettingsInput::Captured(_) | SettingsInput::Clear => None,
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

    /// Input while key row `row` listens: it ends with a key, Escape or Delete.
    fn listen(&mut self, row: usize, input: SettingsInput) -> Option<SettingsAction> {
        let RowWidget::Key(key) = &mut self.widgets[row] else {
            return None;
        };
        let none = KeyBindLayout::new(Rect::default(), 1.0);
        match input {
            SettingsInput::Captured(value) if value != KeyValue::Empty => {
                let KeyBindAction::Bound(key) =
                    key.bind.input(KeyBindInput::Captured(value), &none)?
                else {
                    return None;
                };
                Some(SettingsAction::KeyCaptured { row, key })
            }
            SettingsInput::Back => {
                key.bind.input::<KeyValue>(KeyBindInput::Back, &none);
                None
            }
            SettingsInput::Clear => {
                key.bind.input::<KeyValue>(KeyBindInput::Back, &none);
                Some(SettingsAction::KeyCleared { row })
            }
            _ => None,
        }
    }

    /// Input while the conflict question is open: the arrows or the pointer choose,
    /// Activate or a press answers, Back keeps the old binding.
    fn answer(
        &mut self,
        conflict: Conflict,
        input: SettingsInput,
        layout: &SettingsLayout,
    ) -> Option<SettingsAction> {
        let [keep, apply] = layout.conflict_buttons();
        let resolve = |screen: &mut SettingsScreen, apply: bool| {
            screen.conflict = None;
            Some(SettingsAction::ConflictResolved {
                row: conflict.row,
                key: conflict.key,
                apply,
            })
        };
        let choose = |screen: &mut SettingsScreen, apply: bool| {
            if let Some(c) = &mut screen.conflict {
                c.apply = apply;
            }
            None
        };
        match input {
            SettingsInput::Left => choose(self, false),
            SettingsInput::Right => choose(self, true),
            SettingsInput::Activate => resolve(self, conflict.apply),
            SettingsInput::Back => resolve(self, false),
            SettingsInput::PointerMove { x, y } if keep.contains(x, y) => choose(self, false),
            SettingsInput::PointerMove { x, y } if apply.contains(x, y) => choose(self, true),
            SettingsInput::PointerPress { x, y } if keep.contains(x, y) => resolve(self, false),
            SettingsInput::PointerPress { x, y } if apply.contains(x, y) => resolve(self, true),
            _ => None,
        }
    }

    fn row_at(&self, layout: &SettingsLayout, x: f32, y: f32) -> Option<usize> {
        self.rows_of(self.section)
            .find(|row| layout.row(self.slot(*row)).contains(x, y))
    }
}

#[cfg(test)]
mod tests;
