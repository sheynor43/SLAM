//! The settings screen: its update-side state and layout, input-agnostic (the application
//! maps its events to [`SettingsInput`]). The draw side is [`view::SettingsView`].
//!
//! The screen knows nothing of the settings themselves: the application describes the rows
//! ([`RowSpec`]) in sections, and gets a [`SettingsAction::Changed`] with the row's index
//! and new value for every change. Section tabs are on the left, the rows of the chosen
//! section on the right.

use std::sync::Arc;

use crate::menu::Rect;
use crate::song_select::filter::with_term_pairs;
use crate::song_select::{SearchQuery, ease_scroll};
use crate::theme::Theme;
use crate::widgets::{
    Dropdown, DropdownAction, DropdownInput, DropdownLayout, KeyBind, KeyBindAction, KeyBindInput,
    KeyBindLayout, Slider, SliderAction, SliderInput, SliderLayout, Toggle, ToggleAction,
    ToggleInput, ToggleLayout,
};

pub mod panel;
pub mod view;

pub use view::{KeyLabels, RowLabels, SettingsLabels, SettingsView};

/// Most rows; more are ignored.
pub const MAX_ROWS: usize = 96;
/// Most sections; rows of later ones go into the last.
pub const MAX_SECTIONS: usize = 9;

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

/// What a key row shows: nothing, a keyboard key by scancode (USB HID usage id), a mouse
/// button, or a keyboard key with modifiers (a hotkey, ADR-0095). The screen never turns it
/// into text; the draw side looks the label up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyValue {
    Empty,
    Key(u16),
    Mouse(MouseKey),
    /// `key` with the modifiers in `mods`: [`KeyValue::CTRL`], [`KeyValue::SHIFT`],
    /// [`KeyValue::ALT`]; never empty.
    Chord {
        mods: u8,
        key: u16,
    },
}

impl KeyValue {
    pub const CTRL: u8 = 1;
    pub const SHIFT: u8 = 2;
    pub const ALT: u8 = 4;
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
    /// A line of text (the row's [`RowLabels::label`]) across the whole row: never focused,
    /// never pointed at, never gives an action.
    Note,
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
    Note,
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
    /// Close an open dropdown, else clear the search, else close the screen.
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
    /// Wheel: positive `lines` scroll an open dropdown list towards its end, else the rows.
    Scroll {
        lines: f32,
    },
    /// A typed character: appended to the search (ADR-0095). Control characters are
    /// ignored, and so is a space on an empty search.
    Type(char),
    /// Backspace: erases the last character of the search.
    Erase,
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
    /// The width laid out for: the window's, or the panel's.
    width: f32,
    /// The window's height, which an open dropdown list stays within.
    window_height: f32,
    row_height: f32,
    pitch: f32,
    label_width: f32,
    theme: Theme,
    /// Pixels per reference unit.
    pub scale: f32,
    /// Rows scrolled off the top of the content area.
    scroll: f32,
}

/// A layout for inputs that do not look at it.
fn layout_none() -> SliderLayout {
    SliderLayout::new(Rect::default(), &Theme::default(), 1.0)
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
            width,
            window_height: height,
            row_height,
            pitch,
            label_width: content.width * theme.settings_label_ratio,
            theme: *theme,
            scale,
            scroll: 0.0,
        }
    }

    /// The width laid out for.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// The layout with its rows scrolled by `rows` (the snapshot's
    /// [`SettingsSnapshot::scroll`]).
    pub fn scrolled(&self, rows: f32) -> SettingsLayout {
        SettingsLayout {
            scroll: rows,
            ..*self
        }
    }

    /// Where the search box is: at the top, in the place of a title.
    pub fn search(&self) -> Rect {
        self.title
    }

    /// How many rows fit the content area (fractional).
    pub fn visible_rows(&self) -> f32 {
        if self.pitch <= 0.0 {
            return 0.0;
        }
        // The last row needs its own height, not a whole pitch.
        (self.content.height - self.row_height + self.pitch).max(0.0) / self.pitch
    }

    /// Whether the row at `slot` lies wholly in the content area: a row partly scrolled out
    /// is neither drawn nor pointed at.
    pub fn shows(&self, slot: usize) -> bool {
        let row = self.row(slot);
        row.y >= self.content.y - 0.5
            && row.y + row.height <= self.content.y + self.content.height + 0.5
    }

    /// The slots of the lines that touch the content area at the scroll laid out: the only
    /// ones to lay out and draw (ADR-0133). A superset by a line at the most.
    pub fn visible_slots(&self) -> std::ops::Range<usize> {
        let first = self.scroll.floor().max(0.0) as usize;
        let last = (self.scroll + self.visible_rows()).ceil().max(0.0) as usize + 1;
        first..last
    }

    /// The part (0 to 1) of the line at `slot` inside the content area: a line partly
    /// scrolled out is drawn faded by it, as there is no clipping.
    pub fn fraction(&self, slot: usize) -> f32 {
        let row = self.row(slot);
        let top = row.y.max(self.content.y);
        let bottom = (row.y + row.height).min(self.content.y + self.content.height);
        if row.height <= 0.0 {
            return 0.0;
        }
        ((bottom - top) / row.height).clamp(0.0, 1.0)
    }

    /// Whether the point is in the content area, where the lines are.
    pub fn in_content(&self, x: f32, y: f32) -> bool {
        self.content.contains(x, y)
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
            y: self.content.y + (slot as f32 - self.scroll) * self.pitch,
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
            ..self.dialog_row(0)
        }
    }

    /// Row `i` (0 to 2) of the conflict box, at the top whatever the scroll.
    pub fn dialog_row(&self, i: usize) -> Rect {
        self.scrolled(0.0).row(i)
    }

    /// The "keep" and "apply" buttons, left and right.
    pub fn conflict_buttons(&self) -> [Rect; 2] {
        let row = self.dialog_row(3);
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

    /// Whether the open list of the dropdown at `open` with `count` options covers the row
    /// at `slot`. The list opens below its row, or above it when there is more room there.
    pub fn covered_by_list(&self, open: usize, count: usize, slot: usize) -> bool {
        let list = self.dropdown(open, count).list();
        let row = self.row(slot);
        slot != open && row.y < list.y + list.height && list.y < row.y + row.height
    }
}

/// What the draw side needs; cloning copies fixed-size data and an `Arc`, no allocation.
#[derive(Clone, Debug)]
pub struct SettingsSnapshot {
    pub widgets: [RowWidget; MAX_ROWS],
    pub len: usize,
    /// Index of the focused row.
    pub focus: usize,
    /// The section at the top of the list, which the sidebar highlights.
    pub section: usize,
    pub conflict: Option<Conflict>,
    /// The text of the button row that shows a value (see [`RowLabels::shows_value`]).
    pub value_text: Arc<str>,
    /// Per row: it cannot be changed; the draw side dims it.
    pub disabled: [bool; MAX_ROWS],
    /// Per row: its hint text ([`RowLabels::hint`]) is shown.
    pub hinted: [bool; MAX_ROWS],
    /// Per row: it is not shown and takes no room; the draw side skips it.
    pub hidden: [bool; MAX_ROWS],
    /// Per row: its line in the one continuous list (section headers count as lines), as
    /// the screen lays out and hit-tests it. Meaningless for a hidden row.
    pub slots: [u8; MAX_ROWS],
    /// Per section: the line of its header, `None` when the section has no row shown.
    pub headers: [Option<u8>; MAX_SECTIONS],
    /// What the player typed in the search box.
    pub search: SearchQuery,
    /// The list shows the search's results only; a row that does not match is `hidden`.
    pub searching: bool,
    /// Lines scrolled off the top as shown, eased ([`SettingsLayout::scrolled`]).
    pub scroll: f32,
}

/// The screen's state on the update thread.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsScreen {
    widgets: [RowWidget; MAX_ROWS],
    meta: [(u8, u8); MAX_ROWS],
    len: usize,
    sections: usize,
    focus: usize,
    conflict: Option<Conflict>,
    value_text: Arc<str>,
    disabled: [bool; MAX_ROWS],
    hinted: [bool; MAX_ROWS],
    hidden: [bool; MAX_ROWS],
    search: SearchQuery,
    /// Per section: the line of its header, `None` without rows shown.
    headers: [Option<u8>; MAX_SECTIONS],
    /// The number of lines in the list: headers and rows shown.
    lines: usize,
    /// A section picked in the sidebar, highlighted until the list is scrolled by hand (the
    /// last sections cannot scroll to the top).
    pinned: Option<usize>,
    /// Where the list was before a search began, to which clearing it returns.
    before_search: f32,
    /// How many lines fit the content area, as of the last layout seen.
    visible_lines: f32,
    /// Per row, the lowercased text the search looks in ([`SettingsScreen::set_search_texts`]).
    texts: Arc<[Box<str>]>,
    /// Per row: it matches the search.
    matched: [bool; MAX_ROWS],
    /// The scroll the list eases towards, in lines.
    scroll: f32,
    /// The scroll as shown (ADR-0121's easing).
    shown: f32,
    shown_ns: u64,
}

impl SettingsScreen {
    /// A screen for `rows` in `sections` sections, on the first row of the list. Call it
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
                RowKind::Note => RowWidget::Note,
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
            conflict: None,
            value_text: Arc::from(""),
            disabled: [false; MAX_ROWS],
            hinted: [false; MAX_ROWS],
            hidden: [false; MAX_ROWS],
            search: SearchQuery::default(),
            headers: [None; MAX_SECTIONS],
            lines: 0,
            pinned: None,
            before_search: 0.0,
            visible_lines: 0.0,
            texts: Arc::from([]),
            matched: [false; MAX_ROWS],
            scroll: 0.0,
            shown: 0.0,
            shown_ns: 0,
        };
        screen.recompute_slots();
        screen.focus = screen.first_shown().unwrap_or(0);
        screen
    }

    pub fn sections(&self) -> usize {
        self.sections
    }

    /// The section at the top of the list (or the one just picked in the sidebar).
    pub fn section(&self) -> usize {
        self.pinned.unwrap_or_else(|| self.section_at(self.shown))
    }

    /// The section at the top of the list scrolled to `scroll`: the last whose header is
    /// at or above it (give or take half a line), the last section once at the end.
    fn section_at(&self, scroll: f32) -> usize {
        let at_end = scroll > 0.0 && scroll >= self.max_scroll() - 0.01;
        let mut top = 0;
        for (section, header) in self.headers[..self.sections].iter().enumerate() {
            if let Some(line) = header
                && (at_end || f32::from(*line) <= scroll + 0.5)
            {
                top = section;
            }
        }
        top
    }

    /// The line of `section`'s header, if the section has rows shown.
    pub fn header(&self, section: usize) -> Option<usize> {
        self.headers
            .get(section)
            .copied()
            .flatten()
            .map(usize::from)
    }

    /// Moves the scroll as shown `now_ns` on towards its target, which is clamped to the
    /// content `layout` has. Call it every tick the screen shows. Does not allocate.
    pub fn tick(&mut self, now_ns: u64, layout: &SettingsLayout) {
        self.visible_lines = layout.visible_rows();
        let elapsed = now_ns.saturating_sub(self.shown_ns);
        self.shown_ns = now_ns;
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
        if self.shown != self.scroll {
            self.shown = ease_scroll(self.shown, self.scroll, elapsed);
        }
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
            section: self.section(),
            conflict: self.conflict,
            value_text: Arc::clone(&self.value_text),
            disabled: self.disabled,
            hinted: self.hinted,
            hidden: std::array::from_fn(|row| !self.visible(row)),
            slots: self.meta.map(|(_, slot)| slot),
            headers: self.headers,
            search: self.search,
            searching: self.searching(),
            scroll: self.shown,
        }
    }

    /// Gives the search the text it looks in for each row, lowercased (a row without one
    /// never matches). The application makes it off the hot paths, in the language shown.
    pub fn set_search_texts(&mut self, texts: Arc<[Box<str>]>) {
        self.texts = texts;
        self.refilter();
    }

    /// What the player typed.
    pub fn search(&self) -> &SearchQuery {
        &self.search
    }

    /// Whether the rows shown are the search's results.
    pub fn searching(&self) -> bool {
        !self.search.is_empty()
    }

    /// The scroll the list eases towards, in lines.
    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    /// The scroll as shown, in lines.
    pub fn shown_scroll(&self) -> f32 {
        self.shown
    }

    /// The place in the list to come back to: the scroll target (where the list was before
    /// a search, while one runs) and the sidebar's pick.
    pub fn position(&self) -> (f32, Option<usize>) {
        let scroll = if self.searching() {
            self.before_search
        } else {
            self.scroll
        };
        (scroll, self.pinned)
    }

    /// The section picked in the sidebar and highlighted until the list is scrolled by hand.
    pub fn pinned_section(&self) -> Option<usize> {
        self.pinned
    }

    /// Puts the list back where it was (ADR-0136): the scroll `scroll`, held to the lines
    /// there are now, shown at once, and the sidebar's pick `pinned` if its section still
    /// has a header. Does not allocate.
    pub fn restore_position(&mut self, scroll: f32, pinned: Option<usize>) {
        let last = self.lines.saturating_sub(1) as f32;
        let scroll = if scroll.is_finite() {
            scroll.clamp(0.0, last)
        } else {
            0.0
        };
        self.scroll = scroll;
        self.shown = scroll;
        self.before_search = scroll;
        self.pinned = pinned.filter(|section| self.header(*section).is_some());
    }

    /// The number of lines in the list: the section headers and the rows shown.
    pub fn lines(&self) -> usize {
        self.lines
    }

    /// Whether row `row` takes room: not hidden, and matching the search if there is one.
    fn visible(&self, row: usize) -> bool {
        row < self.len && !self.hidden[row] && (!self.searching() || self.matched[row])
    }

    /// Whether row `row` is among the rows shown: all sections are one list.
    fn shown(&self, row: usize) -> bool {
        self.visible(row)
    }

    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Containers/SearchContainer.cs (match)
    /// Matches the rows against the search: every word a substring of the row's text. Notes
    /// never match. Renumbers the rows, and puts the focus on the first result and the
    /// scroll target on it (the search cleared, back where the list was before the search,
    /// with the focus on the first row there). Does not allocate.
    fn refilter(&mut self) {
        let texts = Arc::clone(&self.texts);
        // Every word, as typed or as read on the other keyboard layout (ADR-0137).
        with_term_pairs(&self.search, |terms| {
            for row in 0..self.len {
                self.matched[row] = !matches!(self.widgets[row], RowWidget::Note)
                    && texts.get(row).is_some_and(|text| {
                        terms
                            .iter()
                            .all(|&(term, alt)| text.contains(term) || text.contains(alt))
                    });
            }
        });
        self.close_dropdowns();
        self.recompute_slots();
        self.pinned = None;
        // The results are the whole list: its first line is a header over the first match.
        self.scroll = if self.searching() {
            0.0
        } else {
            self.before_search
        };
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
        let first = if self.searching() {
            self.first_shown()
        } else {
            self.first_at(self.scroll)
        };
        if let Some(row) = first {
            self.focus = row;
        }
    }

    fn type_char(&mut self, c: char) {
        if c.is_control() || (c.is_whitespace() && self.search.is_empty()) {
            return;
        }
        let began = self.search.is_empty();
        if self.search.push(c) {
            if began {
                self.before_search = self.scroll;
            }
            self.refilter();
        }
    }

    fn erase(&mut self) {
        if self.search.pop() {
            self.refilter();
        }
    }

    /// Clears the search; returns whether there was one.
    fn clear_search(&mut self) -> bool {
        if self.search.is_empty() {
            return false;
        }
        self.search.clear();
        self.refilter();
        true
    }

    /// The rows shown that can take the focus: not notes.
    fn shown_rows(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.len)
            .filter(|row| self.shown(*row) && !matches!(self.widgets[*row], RowWidget::Note))
    }

    /// The first row of the list that can take the focus.
    fn first_shown(&self) -> Option<usize> {
        self.shown_rows().min_by_key(|row| self.slot(*row))
    }

    /// The first row at or below line `scroll` that can take the focus.
    fn first_at(&self, scroll: f32) -> Option<usize> {
        self.shown_rows()
            .filter(|row| self.slot(*row) as f32 >= scroll.ceil())
            .min_by_key(|row| self.slot(*row))
            .or_else(|| self.first_shown())
    }

    /// The farthest the list scrolls: the last line at the bottom.
    fn max_scroll(&self) -> f32 {
        (self.lines as f32 - self.visible_lines).max(0.0)
    }

    /// Scrolls the target the least that shows the focused row whole, and its section's
    /// header with it when it is the section's first row.
    fn scroll_to_focus(&mut self) {
        if !self.shown(self.focus) {
            return;
        }
        let slot = self.slot(self.focus);
        let header = self.header(self.section_of(self.focus));
        let top = if header.is_some_and(|h| h + 1 == slot) {
            slot - 1
        } else {
            slot
        };
        let top = top as f32;
        let visible = self.visible_lines.floor().max(1.0);
        let slot = slot as f32;
        if top < self.scroll {
            self.scroll = top;
        } else if slot + 1.0 > self.scroll + visible {
            self.scroll = slot + 1.0 - visible;
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    /// Makes toggle or slider row `row` unchangeable (or changeable again): input to it is
    /// ignored and it is drawn dimmed. Rows of other kinds and unknown rows are left alone.
    pub fn set_disabled(&mut self, row: usize, disabled: bool) {
        if row < self.len {
            self.disabled[row] = disabled;
        }
    }

    /// Whether row `row` is unchangeable.
    pub fn is_disabled(&self, row: usize) -> bool {
        self.disabled.get(row).copied().unwrap_or(false) && row < self.len
    }

    /// Shows or hides the hint text of row `row` ([`RowLabels::hint`]).
    pub fn set_hint(&mut self, row: usize, shown: bool) {
        if row < self.len {
            self.hinted[row] = shown;
        }
    }

    /// Whether the hint of row `row` is shown.
    pub fn is_hinted(&self, row: usize) -> bool {
        self.hinted.get(row).copied().unwrap_or(false) && row < self.len
    }

    /// Hides row `row` or shows it again. A hidden row is not drawn, takes no room (the rows
    /// below move up), cannot be pointed at and is skipped by the focus; its value, disabled
    /// and hint state are kept. If it holds the focus, the focus moves to the next visible
    /// row of the list, else the previous; an open dropdown closes and a listening key row
    /// stops, without an action; a slider being dragged is released (as by
    /// [`SettingsInput::PointerRelease`]: dragging ends, no `Changed`). Unknown rows are left
    /// alone. Does not allocate.
    pub fn set_hidden(&mut self, row: usize, hidden: bool) {
        if row >= self.len || self.hidden[row] == hidden {
            return;
        }
        let at = self.slot(row);
        self.hidden[row] = hidden;
        self.recompute_slots();
        if !hidden {
            return;
        }
        match &mut self.widgets[row] {
            RowWidget::Dropdown(d) => {
                // Closing never changes the selection; the layout is not looked at.
                d.input(
                    DropdownInput::Back,
                    &DropdownLayout::new(Rect::default(), 0, 1.0),
                );
            }
            RowWidget::Key(k) => {
                k.bind.input::<KeyValue>(
                    KeyBindInput::Back,
                    &KeyBindLayout::new(Rect::default(), 1.0),
                );
            }
            RowWidget::Slider(s) => {
                // Release emits nothing, so neither does hiding.
                s.input(SliderInput::PointerRelease, &layout_none());
            }
            _ => {}
        }
        if self.focus == row {
            // Notes take no focus either.
            let next = self
                .shown_rows()
                .filter(|r| self.slot(*r) >= at)
                .min_by_key(|r| self.slot(*r))
                .or_else(|| {
                    self.shown_rows()
                        .filter(|r| self.slot(*r) < at)
                        .max_by_key(|r| self.slot(*r))
                });
            if let Some(next) = next {
                self.focus = next;
            }
        }
    }

    /// Whether row `row` is hidden.
    pub fn is_hidden(&self, row: usize) -> bool {
        self.hidden.get(row).copied().unwrap_or(false) && row < self.len
    }

    /// Numbers the lines of the list: each section with a visible row has a header, then
    /// its visible rows in the order they were given; a row's slot is its line.
    fn recompute_slots(&mut self) {
        self.headers = [None; MAX_SECTIONS];
        let mut next = 0_usize;
        for section in 0..self.sections {
            let header = next;
            next += 1;
            let mut any = false;
            for row in 0..self.len {
                if usize::from(self.meta[row].0) == section && self.visible(row) {
                    self.meta[row].1 = next.min(usize::from(u8::MAX)) as u8;
                    next += 1;
                    any = true;
                }
            }
            if any {
                self.headers[section] = Some(header as u8);
            } else {
                next = header;
            }
        }
        self.lines = next;
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

    /// The key row that waits for a key, if any.
    pub fn listening_row(&self) -> Option<usize> {
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

    /// The line of `row` in the list: section headers count as lines.
    pub fn slot(&self, row: usize) -> usize {
        usize::from(self.meta[row].1)
    }

    fn section_of(&self, row: usize) -> usize {
        usize::from(self.meta[row].0)
    }

    /// The rows of `section` that can take the focus: shown and not notes.
    fn rows_of(&self, section: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.len).filter(move |row| {
            self.section_of(*row) == section
                && self.shown(*row)
                && !matches!(self.widgets[*row], RowWidget::Note)
        })
    }

    fn first_row(&self, section: usize) -> Option<usize> {
        self.rows_of(section).min_by_key(|row| self.slot(*row))
    }

    /// The row with an open dropdown.
    fn open_dropdown(&self) -> Option<usize> {
        self.shown_rows()
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
        // Down and up follow the list, whatever the order the rows were given in.
        let at = self.slot(self.focus);
        let next = if forward {
            self.shown_rows()
                .filter(|row| self.slot(*row) > at)
                .min_by_key(|row| self.slot(*row))
        } else {
            self.shown_rows()
                .filter(|row| self.slot(*row) < at)
                .max_by_key(|row| self.slot(*row))
        };
        if let Some(row) = next {
            self.focus = row;
            self.pinned = None;
        }
    }

    /// Scrolls to `section`'s header, leaving a search; the sidebar then shows it as the
    /// section at the top even where the list cannot scroll far enough.
    fn switch_section(&mut self, section: usize) {
        self.close_dropdowns();
        self.clear_search();
        if let Some(header) = self.header(section) {
            self.scroll = (header as f32).clamp(0.0, self.max_scroll());
            self.pinned = Some(section);
        }
        if let Some(row) = self.first_row(section) {
            self.focus = row;
        }
    }

    /// Goes to the next (or previous) section that has rows shown.
    fn step_section(&mut self, forward: bool) {
        let n = self.sections;
        // From where the list is going, so that quick presses go on.
        let mut section = self.pinned.unwrap_or_else(|| self.section_at(self.scroll));
        for _ in 0..n {
            section = if forward {
                (section + 1) % n
            } else {
                (section + n - 1) % n
            };
            if self.header(section).is_some() {
                self.switch_section(section);
                return;
            }
        }
    }

    /// Gives `input` to row `row`'s widget, if the row may take input: visible and not a
    /// note.
    fn route(
        &mut self,
        row: usize,
        input: SettingsInput,
        layout: &SettingsLayout,
    ) -> Option<SettingsAction> {
        if !self.shown(row) || matches!(self.widgets[row], RowWidget::Note) {
            return None;
        }
        self.route_widget(row, input, layout)
    }

    /// Gives `input` to row `row`'s widget, in the form that widget understands.
    fn route_widget(
        &mut self,
        row: usize,
        input: SettingsInput,
        layout: &SettingsLayout,
    ) -> Option<SettingsAction> {
        let slot = self.slot(row);
        let disabled = self.disabled[row];
        let value = match &mut self.widgets[row] {
            RowWidget::Toggle(_) | RowWidget::Slider(_) if disabled => return None,
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
            RowWidget::Note => return None,
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

    /// Applies `input`; `layout` places the rows for pointer input, unscrolled (the screen
    /// scrolls it). Does not allocate.
    pub fn input(
        &mut self,
        input: SettingsInput,
        layout: &SettingsLayout,
    ) -> Option<SettingsAction> {
        self.visible_lines = layout.visible_rows();
        let action = self.input_scrolled(input, &layout.scrolled(self.shown));
        if matches!(
            input,
            SettingsInput::Up
                | SettingsInput::Down
                | SettingsInput::Type(_)
                | SettingsInput::Erase
                | SettingsInput::Back
                | SettingsInput::NextSection
                | SettingsInput::PrevSection
        ) {
            self.scroll_to_focus();
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
        action
    }

    fn input_scrolled(
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
                // As the search on the song select: Escape clears it before it leaves.
                (!self.clear_search()).then_some(SettingsAction::Close)
            }
            SettingsInput::Type(c) => {
                self.type_char(c);
                None
            }
            SettingsInput::Erase => {
                self.erase();
                None
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
                let dragging = self.shown_rows().find(
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
            SettingsInput::Scroll { lines } => {
                if let Some(row) = self.open_dropdown() {
                    return self.route(row, input, layout);
                }
                self.scroll += lines;
                self.pinned = None;
                None
            }
            // Only meaningful while a key row listens.
            SettingsInput::Captured(_) | SettingsInput::Clear => None,
            SettingsInput::PointerRelease => {
                let mut action = None;
                for row in 0..self.len {
                    if matches!(self.widgets[row], RowWidget::Slider(_)) {
                        // Not `route`: a drag must end wherever the slider is.
                        action = self.route_widget(row, input, layout).or(action);
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
        if !layout.in_content(x, y) {
            return None;
        }
        self.shown_rows()
            .find(|row| layout.row(self.slot(*row)).contains(x, y))
    }
}

#[cfg(test)]
mod tests;
