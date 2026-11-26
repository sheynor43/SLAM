//! The mod settings ("customise") panel of the mod select (ADR-0057, ADR-0177): a floating
//! panel the player opens with a right click on a mod tile (or Tab) and drags by its header.
//! The panel knows nothing of mods: the application gives it rows (a section header per mod,
//! then its settings) and writes back what the player changes, resolving a row index to a
//! mod and a setting itself.
//!
//! The panel opens where the pointer is and always opens there again: its position is not
//! kept. Esc, a press outside it and another right click close it. While open it takes
//! every key.

use super::{ModSelectAction, SongSelectInput};
use crate::menu::Rect;
use crate::theme::Theme;
use crate::widgets::{
    Slider, SliderAction, SliderInput, SliderLayout, Toggle, ToggleAction, ToggleInput,
    ToggleLayout,
};

/// Most rows the panel holds (section headers included); the rest are dropped.
pub const MAX_SETTING_ROWS: usize = 32;
/// Most named values of a choice row.
pub const MAX_CHOICES: usize = 3;

/// The share of a row's height the label takes; the control gets the rest.
const LABEL_SHARE: f32 = 0.45;

/// Whether the panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CustomisationState {
    #[default]
    Collapsed,
    Expanded,
}

/// What a row holds. Labels are Fluent keys (`SongSelectLabels::setting_texts` has the
/// texts).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SettingRowKind {
    /// A mod's name, above its settings.
    #[default]
    Header,
    Number {
        /// What the slider shows; for an unset value the beatmap's.
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        decimals: u8,
        /// The value is not set (shown dimmed): it follows the beatmap.
        unset: bool,
    },
    Toggle {
        value: bool,
    },
    Choice {
        /// `None`: a value without a name.
        index: Option<usize>,
        count: usize,
        /// Keys of the value names; the first `count` count.
        values: [&'static str; MAX_CHOICES],
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SettingRow {
    /// Fluent key of the label (of the mod's name for a header).
    pub label: &'static str,
    /// Text after a number ("x" for speeds).
    pub suffix: &'static str,
    pub kind: SettingRowKind,
}

impl SettingRow {
    pub const EMPTY: SettingRow = SettingRow {
        label: "",
        suffix: "",
        kind: SettingRowKind::Header,
    };

    pub fn is_header(&self) -> bool {
        matches!(self.kind, SettingRowKind::Header)
    }
}

/// A value the player gave a row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingValue {
    Number(f64),
    Toggle(bool),
    Choice(usize),
}

/// Where the panel is: the header (its drag handle), the rows below it. A function of the
/// window size, theme and the panel's origin ([`CustomisationLayout::at`]), shared by hit
/// testing and drawing. [`CustomisationLayout::new`] places it at the top right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CustomisationLayout {
    pub header: Rect,
    content_top: f32,
    pub row_height: f32,
    /// Rows that fit in the window below a header at its top.
    max_rows: usize,
    pub pad: f32,
    pub scale: f32,
    /// The window's size and the margin the panel keeps from its edges.
    window: [f32; 2],
    margin: f32,
    value_width: f32,
    value_gap: f32,
    track_height: f32,
}

/// The panel's width in tile widths: the tiles' former width, so enlarging the tiles does not
/// widen the settings panel (ADR-0167).
const SETTINGS_TILE_BASIS: f32 = 210.0;

impl CustomisationLayout {
    pub fn new(width: f32, height: f32, theme: &Theme) -> CustomisationLayout {
        let scale = theme.scale(height);
        let margin = theme.song_margin * scale;
        let panel_width = (2.0 * SETTINGS_TILE_BASIS + theme.mod_column_gap) * scale;
        let header = Rect {
            x: width - margin - panel_width,
            y: (theme.toolbar_height + theme.song_top_gap) * scale,
            width: panel_width,
            height: theme.widget_height * scale,
        };
        let row_height = theme.widget_height * scale * 1.5;
        let content_top = header.y + header.height;
        // The panel stays above the footer bar (ADR-0175).
        let floor = height - theme.footer_height * scale;
        let room = (floor - 2.0 * margin - header.height).max(0.0);
        CustomisationLayout {
            header,
            content_top,
            row_height,
            max_rows: ((room / row_height).floor() as usize).max(1),
            pad: theme.widget_pad * scale,
            scale,
            window: [width, floor],
            margin,
            value_width: theme.widget_value_width * scale,
            value_gap: theme.widget_value_gap * scale,
            track_height: theme.widget_track_height * scale,
        }
    }

    /// The layout with the header's top left corner at `origin`, moved as little as keeps
    /// the whole panel of `len` rows inside the window (less its margin).
    pub fn at(&self, origin: (f32, f32), len: usize) -> CustomisationLayout {
        let panel = self.panel(len);
        let [width, height] = self.window;
        let x = origin
            .0
            .min(width - self.margin - panel.width)
            .max(self.margin.min(width - panel.width).max(0.0));
        let y = origin
            .1
            .min(height - self.margin - panel.height)
            .max(self.margin.min(height - panel.height).max(0.0));
        let mut placed = *self;
        placed.header.x = x;
        placed.header.y = y;
        placed.content_top = y + self.header.height;
        placed
    }

    /// Where the top left corner of the header is.
    pub fn origin(&self) -> (f32, f32) {
        (self.header.x, self.header.y)
    }

    /// How many of `len` rows show at once.
    pub fn visible_rows(&self, len: usize) -> usize {
        len.min(self.max_rows)
    }

    /// The rows' area when the panel holds `len` rows.
    pub fn content(&self, len: usize) -> Rect {
        Rect {
            x: self.header.x,
            y: self.content_top,
            width: self.header.width,
            height: self.visible_rows(len) as f32 * self.row_height,
        }
    }

    /// The header and the rows.
    pub fn panel(&self, len: usize) -> Rect {
        let content = self.content(len);
        Rect {
            height: self.header.height + content.height,
            ..self.header
        }
    }

    /// The area of the row at `slot` (0 is the first row shown).
    pub fn row_rect(&self, slot: usize) -> Rect {
        Rect {
            x: self.header.x,
            y: self.content_top + slot as f32 * self.row_height,
            width: self.header.width,
            height: self.row_height,
        }
    }

    /// Where a row's label goes.
    pub fn label_rect(&self, row: &Rect) -> Rect {
        Rect {
            x: row.x + self.pad,
            y: row.y,
            width: row.width - 2.0 * self.pad,
            height: row.height * LABEL_SHARE,
        }
    }

    /// Where a row's control (slider, switch, value) goes.
    pub fn control_rect(&self, row: &Rect) -> Rect {
        let top = row.height * LABEL_SHARE;
        Rect {
            x: row.x + self.pad,
            y: row.y + top,
            width: row.width - 2.0 * self.pad,
            height: (row.height - top) * 0.9,
        }
    }

    /// The slider of a number row at `row`.
    pub fn slider(&self, row: &Rect) -> SliderLayout {
        let bounds = self.control_rect(row);
        let track_width = (bounds.width - self.value_width - self.value_gap).max(0.0);
        SliderLayout {
            bounds,
            track: Rect {
                x: bounds.x,
                y: bounds.y + (bounds.height - self.track_height) / 2.0,
                width: track_width,
                height: self.track_height,
            },
            hit: Rect {
                x: bounds.x,
                y: bounds.y,
                width: track_width,
                height: bounds.height,
            },
            value: Rect {
                x: bounds.x + bounds.width - self.value_width,
                y: bounds.y,
                width: self.value_width,
                height: bounds.height,
            },
            // `control_rect` is already inset by the row's padding.
            value_pad: 0.0,
            scale: self.scale,
        }
    }

    /// A toggle over the whole row: a press anywhere on it flips it.
    fn toggle(&self, row: &Rect) -> ToggleLayout {
        ToggleLayout {
            bounds: *row,
            track: self.control_rect(row),
            scale: self.scale,
        }
    }
}

/// What the draw side needs; `Copy`, so a snapshot allocates nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CustomisationSnapshot {
    pub state: CustomisationState,
    pub len: usize,
    pub rows: [SettingRow; MAX_SETTING_ROWS],
    /// The row the keyboard acts on.
    pub selected: usize,
    pub hover: Option<usize>,
    /// The top left corner of the panel as the player placed it; the layout moves it into
    /// the window ([`CustomisationLayout::at`]).
    pub origin: (f32, f32),
    /// The header is being dragged.
    pub moving: bool,
    /// The first row shown.
    pub first: usize,
}

impl Default for CustomisationSnapshot {
    fn default() -> CustomisationSnapshot {
        CustomisationSnapshot {
            state: CustomisationState::Collapsed,
            len: 0,
            rows: [SettingRow::EMPTY; MAX_SETTING_ROWS],
            selected: 0,
            hover: None,
            origin: (0.0, 0.0),
            moving: false,
            first: 0,
        }
    }
}

impl CustomisationSnapshot {
    /// Whether some row has a setting.
    pub fn is_enabled(&self) -> bool {
        self.rows[..self.len].iter().any(|r| !r.is_header())
    }
}

/// The panel's state on the update thread.
#[derive(Clone, Debug)]
pub struct Customisation {
    state: CustomisationState,
    rows: [SettingRow; MAX_SETTING_ROWS],
    len: usize,
    selected: usize,
    hover: Option<usize>,
    /// Where the panel was put (the header's top left corner) and, while its header is
    /// dragged, where the pointer holds it.
    origin: (f32, f32),
    moving: Option<(f32, f32)>,
    scroll: f32,
    /// How many rows showed at the last input; keeps the selection in view.
    visible: usize,
    /// The slider being dragged, with its row.
    drag: Option<(usize, Slider)>,
}

impl Default for Customisation {
    fn default() -> Customisation {
        Customisation {
            state: CustomisationState::Collapsed,
            rows: [SettingRow::EMPTY; MAX_SETTING_ROWS],
            len: 0,
            selected: 0,
            hover: None,
            origin: (0.0, 0.0),
            moving: None,
            scroll: 0.0,
            visible: MAX_SETTING_ROWS,
            drag: None,
        }
    }
}

impl Customisation {
    pub fn state(&self) -> CustomisationState {
        self.state
    }

    pub fn is_expanded(&self) -> bool {
        self.state != CustomisationState::Collapsed
    }

    /// Whether some row has a setting; the panel cannot expand otherwise.
    pub fn is_enabled(&self) -> bool {
        self.rows[..self.len].iter().any(|r| !r.is_header())
    }

    pub fn rows(&self) -> &[SettingRow] {
        &self.rows[..self.len]
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    fn first(&self) -> usize {
        (self.scroll.max(0.0).floor() as usize).min(self.len.saturating_sub(1))
    }

    pub fn snapshot(&self) -> CustomisationSnapshot {
        CustomisationSnapshot {
            state: self.state,
            len: self.len,
            rows: self.rows,
            selected: self.selected,
            hover: self.hover,
            origin: self.origin,
            moving: self.moving.is_some(),
            first: self.first(),
        }
    }

    /// Replaces the rows (keeping the selection by position and a drag in progress when the
    /// row is still a number). Rows past [`MAX_SETTING_ROWS`] are dropped. Without a
    /// setting the panel collapses.
    pub fn set_rows(&mut self, rows: &[SettingRow]) {
        self.len = rows.len().min(MAX_SETTING_ROWS);
        self.rows[..self.len].copy_from_slice(&rows[..self.len]);
        self.rows[self.len..].fill(SettingRow::EMPTY);
        if self.selected >= self.len || self.rows[self.selected].is_header() {
            self.selected = self.rows[..self.len]
                .iter()
                .position(|r| !r.is_header())
                .unwrap_or(0);
        }
        if let Some((row, _)) = self.drag
            && !matches!(
                self.rows.get(row).map(|r| r.kind),
                Some(SettingRowKind::Number { .. })
            )
        {
            self.drag = None;
        }
        self.hover = self.hover.filter(|&r| r < self.len);
        self.scroll = self
            .scroll
            .clamp(0.0, self.len.saturating_sub(self.visible) as f32);
        if !self.is_enabled() {
            self.collapse();
        }
    }

    pub fn collapse(&mut self) {
        self.state = CustomisationState::Collapsed;
        self.drag = None;
        self.moving = None;
        self.hover = None;
    }

    /// Opens the panel with its header's top left corner at `origin` (kept inside the window
    /// when drawn); where it was before is forgotten. Does nothing without settings.
    pub fn open_at(&mut self, origin: (f32, f32)) {
        if !self.is_enabled() {
            return;
        }
        self.state = CustomisationState::Expanded;
        self.origin = origin;
        self.moving = None;
        self.drag = None;
        self.scroll = 0.0;
    }

    /// The layout of the panel where it is now.
    fn placed(&self, layout: &CustomisationLayout) -> CustomisationLayout {
        layout.at(self.origin, self.len)
    }

    fn row_at(&self, x: f32, y: f32, layout: &CustomisationLayout) -> Option<usize> {
        let content = layout.content(self.len);
        if !content.contains(x, y) {
            return None;
        }
        let slot = ((y - content.y) / layout.row_height).floor() as usize;
        let row = self.first() + slot;
        (row < self.len).then_some(row)
    }

    fn changed(&mut self, row: usize, value: SettingValue) -> Option<ModSelectAction> {
        match (&mut self.rows[row].kind, value) {
            (
                SettingRowKind::Number {
                    value: v, unset, ..
                },
                SettingValue::Number(n),
            ) => {
                *v = n;
                *unset = false;
            }
            (SettingRowKind::Toggle { value: v }, SettingValue::Toggle(b)) => *v = b,
            (SettingRowKind::Choice { index, .. }, SettingValue::Choice(i)) => *index = Some(i),
            _ => return None,
        }
        Some(ModSelectAction::SetSetting { row, value })
    }

    /// Steps the number at `row` by `input`, or flips a toggle, or cycles a choice by
    /// `direction`.
    fn step(
        &mut self,
        row: usize,
        direction: i32,
        layout: &CustomisationLayout,
    ) -> Option<ModSelectAction> {
        match self.rows[row].kind {
            SettingRowKind::Number {
                value,
                min,
                max,
                step,
                ..
            } => {
                let mut slider = Slider::new(min, max, step, value);
                let input = if direction < 0 {
                    SliderInput::Decrease
                } else {
                    SliderInput::Increase
                };
                let rect = layout.row_rect(0);
                match slider.input(input, &layout.slider(&rect))? {
                    SliderAction::Changed(v) => self.changed(row, SettingValue::Number(v)),
                }
            }
            SettingRowKind::Toggle { value } => self.changed(row, SettingValue::Toggle(!value)),
            SettingRowKind::Choice { index, count, .. } => {
                if count == 0 {
                    return None;
                }
                let next = match index {
                    None => 0,
                    Some(i) if direction < 0 => (i + count - 1) % count,
                    Some(i) => (i + 1) % count,
                };
                self.changed(row, SettingValue::Choice(next))
            }
            SettingRowKind::Header => None,
        }
    }

    /// Enter and Space: flips a toggle, advances a choice.
    fn activate(&mut self, row: usize) -> Option<ModSelectAction> {
        match self.rows[row].kind {
            SettingRowKind::Toggle { value } => self.changed(row, SettingValue::Toggle(!value)),
            SettingRowKind::Choice { index, count, .. } if count > 0 => self.changed(
                row,
                SettingValue::Choice(index.map_or(0, |i| (i + 1) % count)),
            ),
            _ => None,
        }
    }

    /// Scrolls the least that shows `row` (and the header above it, when it is the first of
    /// its section).
    fn ensure_visible(&mut self, row: usize) {
        let top = if row > 0 && self.rows[row - 1].is_header() {
            row - 1
        } else {
            row
        };
        let visible = self.visible.max(1);
        let first = self.first();
        if top < first {
            self.scroll = top as f32;
        } else if row >= first + visible {
            self.scroll = (row + 1 - visible) as f32;
        }
        self.scroll = self
            .scroll
            .clamp(0.0, self.len.saturating_sub(visible) as f32);
    }

    fn select(&mut self, from: usize, forward: bool) {
        let found = if forward {
            (from + 1..self.len).find(|&r| !self.rows[r].is_header())
        } else {
            (0..from).rev().find(|&r| !self.rows[r].is_header())
        };
        if let Some(row) = found {
            self.selected = row;
            self.ensure_visible(row);
        }
    }

    /// Applies `input` to the expanded panel: it takes every input. Does not allocate.
    pub fn input_expanded(
        &mut self,
        input: SongSelectInput,
        layout: &CustomisationLayout,
    ) -> Option<ModSelectAction> {
        use SongSelectInput as I;
        let layout = &self.placed(layout);
        self.visible = layout.visible_rows(self.len);
        let selected = (self.selected < self.len && !self.rows[self.selected].is_header())
            .then_some(self.selected);
        match input {
            I::Back | I::ToggleCustomisation | I::SecondaryPress { .. } => {
                self.collapse();
                None
            }
            I::Up => {
                self.select(self.selected, false);
                None
            }
            I::Down => {
                self.select(self.selected, true);
                None
            }
            I::Left => self.step(selected?, -1, layout),
            I::Right => self.step(selected?, 1, layout),
            I::Activate | I::Space => self.activate(selected?),
            I::ResetSetting => selected.map(|row| ModSelectAction::ResetSetting { row }),
            I::Scroll { lines } => {
                let max = self.len.saturating_sub(self.visible) as f32;
                self.scroll = (self.scroll + lines).clamp(0.0, max);
                None
            }
            I::PointerMove { x, y } => {
                if let Some((dx, dy)) = self.moving {
                    // The header follows the pointer, kept in the window.
                    self.origin = layout.at((x - dx, y - dy), self.len).origin();
                    return None;
                }
                if let Some((row, mut slider)) = self.drag {
                    let rect = layout.row_rect(row.saturating_sub(self.first()));
                    let action =
                        slider.input(SliderInput::PointerMove { x, y }, &layout.slider(&rect));
                    self.drag = Some((row, slider));
                    return match action {
                        Some(SliderAction::Changed(v)) => {
                            self.changed(row, SettingValue::Number(v))
                        }
                        None => None,
                    };
                }
                self.hover = self.row_at(x, y, layout);
                None
            }
            I::PointerPress { x, y } => {
                if !layout.panel(self.len).contains(x, y) {
                    self.collapse();
                    return None;
                }
                if layout.header.contains(x, y) {
                    self.moving = Some((x - layout.header.x, y - layout.header.y));
                    return None;
                }
                let row = self.row_at(x, y, layout)?;
                if self.rows[row].is_header() {
                    return None;
                }
                self.selected = row;
                let rect = layout.row_rect(row - self.first());
                match self.rows[row].kind {
                    SettingRowKind::Number {
                        value,
                        min,
                        max,
                        step,
                        ..
                    } => {
                        let mut slider = Slider::new(min, max, step, value);
                        let action =
                            slider.input(SliderInput::PointerPress { x, y }, &layout.slider(&rect));
                        if slider.is_dragging() {
                            self.drag = Some((row, slider));
                        }
                        match action {
                            Some(SliderAction::Changed(v)) => {
                                self.changed(row, SettingValue::Number(v))
                            }
                            None => None,
                        }
                    }
                    SettingRowKind::Toggle { value } => {
                        let mut toggle = Toggle::new(value);
                        match toggle
                            .input(ToggleInput::PointerPress { x, y }, &layout.toggle(&rect))
                        {
                            Some(ToggleAction::Changed(v)) => {
                                self.changed(row, SettingValue::Toggle(v))
                            }
                            None => None,
                        }
                    }
                    SettingRowKind::Choice { .. } => self.activate(row),
                    SettingRowKind::Header => None,
                }
            }
            I::PointerRelease => {
                self.drag = None;
                self.moving = None;
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
