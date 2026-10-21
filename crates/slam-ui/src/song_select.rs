//! The song select screen: its update-side state and layout, input-agnostic (the
//! application maps its events to [`SongSelectInput`]). The draw side is
//! [`view::SongSelectView`].
//!
//! The list is a flat column of rows: every set is a row, and the selected set is expanded,
//! showing its difficulties as indented rows right beneath it.

use std::ops::Range;
use std::sync::Arc;

use crate::menu::Rect;
use crate::theme::Theme;

pub mod mod_select;
pub mod view;

pub use mod_select::{ModColumns, ModSelect, ModSelectAction, ModSelectLayout, ModSelectSnapshot};
pub use view::{SongSelectLabels, SongSelectView};

/// The sets to show, built by the application from the library. Immutable once built;
/// shared as `Arc<SongList>` and replaced wholesale when the library changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SongList {
    pub sets: Vec<SetRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetRow {
    /// Opaque identity chosen by the application; the selection follows it across
    /// [`SongSelect::set_list`].
    pub key: u64,
    pub artist: String,
    pub title: String,
    pub creator: String,
    pub difficulties: Vec<DifficultyRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DifficultyRow {
    pub version: String,
}

/// What the screen shows besides the list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SongSelectStatus {
    /// No songs folder was given.
    #[default]
    NoFolder,
    /// The folder is being scanned; `sets` found so far.
    Scanning {
        sets: usize,
    },
    Ready,
    Failed,
    /// A difficulty is being loaded to play: only Back is accepted.
    Loading,
}

/// What the screen understands; the application turns keys and mouse events into these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SongSelectInput {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Activate,
    Back,
    /// F1: opens the mod select panel (and closes it).
    ToggleMods,
    /// Backspace: deselects every mod.
    DeselectMods,
    /// A mod hotkey: the tile `index` of `column` of the mod select panel. The application
    /// maps its key rows to the columns that have hotkeys.
    ModHotkey {
        column: usize,
        index: usize,
    },
    /// Wheel: positive `lines` scroll towards the end of the list. Selection stays.
    Scroll {
        lines: f32,
    },
    PointerMove {
        x: f32,
        y: f32,
    },
    PointerPress {
        x: f32,
        y: f32,
    },
}

/// What the screen asks the application to do; indices are into the current list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SongSelectAction {
    Play {
        set: usize,
        difficulty: usize,
    },
    Back,
    /// The mod select panel toggled a tile; the application resolves the new selection and
    /// writes the flags back through [`SongSelect::mods_mut`].
    ToggleMod {
        column: usize,
        index: usize,
    },
    /// Deselect every mod.
    DeselectMods,
}

/// One row of the flat column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Set(usize),
    Difficulty { set: usize, difficulty: usize },
}

impl SongList {
    fn difficulty_count(&self, set: usize) -> usize {
        self.sets.get(set).map_or(0, |s| s.difficulties.len())
    }

    /// Number of rows when `selected_set` is expanded.
    pub fn row_count(&self, selected_set: usize) -> usize {
        self.sets.len() + self.difficulty_count(selected_set)
    }

    /// The row at flat index `index` when `selected_set` is expanded.
    pub fn row(&self, selected_set: usize, index: usize) -> Option<Row> {
        let below = self.difficulty_count(selected_set);
        if index <= selected_set {
            (index < self.sets.len()).then_some(Row::Set(index))
        } else if index <= selected_set + below {
            Some(Row::Difficulty {
                set: selected_set,
                difficulty: index - selected_set - 1,
            })
        } else {
            let set = index - below;
            (set < self.sets.len()).then_some(Row::Set(set))
        }
    }
}

/// Where the list, status line and hint are: a panel on the right of the window. A pure
/// function of the window size and theme, shared by hit testing (update) and drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SongSelectLayout {
    /// Status line above the list.
    pub header: Rect,
    /// The rows' area; row 0 is at its top when not scrolled.
    pub list: Rect,
    /// Key hints below the list.
    pub hint: Rect,
    /// The mod select panel.
    pub mods: ModSelectLayout,
    /// Pitch of the rows.
    pub row_height: f32,
    /// Extra left offset of difficulty rows.
    pub indent: f32,
    /// Pixels per reference unit.
    pub scale: f32,
}

impl SongSelectLayout {
    pub fn new(width: f32, height: f32, theme: &Theme) -> SongSelectLayout {
        let scale = theme.scale(height);
        let margin = theme.song_margin * scale;
        let panel = width * theme.song_panel_ratio;
        let x = width - margin - panel;
        let header = Rect {
            x,
            y: margin,
            width: panel,
            height: theme.song_header_height * scale,
        };
        let hint_height = theme.song_detail_size * scale * 1.5;
        let hint = Rect {
            x,
            y: height - margin - hint_height,
            width: panel,
            height: hint_height,
        };
        let top = header.y + header.height + margin / 2.0;
        let list = Rect {
            x,
            y: top,
            width: panel,
            height: (hint.y - margin / 2.0 - top).max(0.0),
        };
        SongSelectLayout {
            header,
            list,
            hint,
            mods: ModSelectLayout::new(width, height, theme),
            row_height: theme.song_row_height * scale,
            indent: theme.song_indent * scale,
            scale,
        }
    }

    /// How many rows fit the list area (fractional).
    pub fn viewport_rows(&self) -> f32 {
        self.list.height / self.row_height
    }

    /// Top of row `index` when the list is scrolled by `scroll` rows.
    pub fn row_y(&self, index: usize, scroll: f32) -> f32 {
        self.list.y + (index as f32 - scroll) * self.row_height
    }

    /// Flat indices of the rows that touch the list area, out of `total`.
    pub fn visible_rows(&self, scroll: f32, total: usize) -> Range<usize> {
        let first = scroll.max(0.0).floor() as usize;
        let last = (scroll.max(0.0) + self.viewport_rows()).ceil() as usize;
        first.min(total)..last.min(total)
    }

    /// The row under the point, with the list scrolled by `scroll` rows.
    pub fn row_at(
        &self,
        x: f32,
        y: f32,
        scroll: f32,
        list: &SongList,
        selected_set: usize,
    ) -> Option<Row> {
        if !self.list.contains(x, y) {
            return None;
        }
        let index = (y - self.list.y) / self.row_height + scroll;
        if index < 0.0 {
            return None;
        }
        list.row(selected_set, index.floor() as usize)
    }
}

/// What the draw side needs; cloning copies an `Arc`, no allocation.
#[derive(Clone, Debug)]
pub struct SongSelectSnapshot {
    pub list: Arc<SongList>,
    pub selected_set: usize,
    pub selected_difficulty: usize,
    /// Rows scrolled off the top.
    pub scroll: f32,
    pub hovered: Option<Row>,
    pub status: SongSelectStatus,
    /// The localised "scanning" message, made by the application; the view re-lays it
    /// out when the `Arc` changes (by identity).
    pub scanning_text: Arc<str>,
    /// Changes whenever the list is replaced; the view's row texts are valid for one
    /// generation.
    pub generation: u64,
    pub mods: ModSelectSnapshot,
}

/// The screen's state on the update thread.
#[derive(Clone, Debug)]
pub struct SongSelect {
    list: Arc<SongList>,
    set: usize,
    difficulty: usize,
    scroll: f32,
    pointer: Option<(f32, f32)>,
    hovered: Option<Row>,
    status: SongSelectStatus,
    scanning_text: Arc<str>,
    generation: u64,
    mods: ModSelect,
    /// Rows that fit the list area at the last input; keeps the selection in view.
    viewport: f32,
}

impl Default for SongSelect {
    fn default() -> SongSelect {
        SongSelect::new()
    }
}

impl SongSelect {
    pub fn new() -> SongSelect {
        SongSelect {
            list: Arc::default(),
            set: 0,
            difficulty: 0,
            scroll: 0.0,
            pointer: None,
            hovered: None,
            status: SongSelectStatus::default(),
            scanning_text: Arc::from(""),
            generation: 0,
            mods: ModSelect::default(),
            viewport: 10.0,
        }
    }

    pub fn status(&self) -> SongSelectStatus {
        self.status
    }

    pub fn set_status(&mut self, status: SongSelectStatus) {
        self.status = status;
    }

    /// Sets the "scanning" message shown while the status is `Scanning`. The application
    /// formats it off the update thread and keeps its own clone alive.
    pub fn set_scanning_text(&mut self, text: Arc<str>) {
        self.scanning_text = text;
    }

    /// The list shown.
    pub fn list(&self) -> &SongList {
        &self.list
    }

    /// The selected `(set, difficulty)`, if the list has a set.
    pub fn selected(&self) -> Option<(usize, usize)> {
        (self.set < self.list.sets.len()).then_some((self.set, self.difficulty))
    }

    /// Replaces the list, keeping the selected set by its key when it is still there
    /// (the difficulty index is clamped), else the selection stays at the same position.
    pub fn set_list(&mut self, list: Arc<SongList>) {
        let key = self.list.sets.get(self.set).map(|s| s.key);
        let found = key.and_then(|key| list.sets.iter().position(|s| s.key == key));
        self.set = found.unwrap_or(self.set);
        self.list = list;
        self.generation += 1;
        self.set = self.set.min(self.list.sets.len().saturating_sub(1));
        self.difficulty = self
            .difficulty
            .min(self.list.difficulty_count(self.set).saturating_sub(1));
        self.hovered = None;
        self.clamp_scroll();
        self.ensure_visible();
    }

    pub fn snapshot(&self) -> SongSelectSnapshot {
        SongSelectSnapshot {
            list: Arc::clone(&self.list),
            selected_set: self.set,
            selected_difficulty: self.difficulty,
            scroll: self.scroll,
            hovered: self.hovered,
            status: self.status,
            scanning_text: Arc::clone(&self.scanning_text),
            generation: self.generation,
            mods: self.mods.snapshot(),
        }
    }

    /// The mod select panel.
    pub fn mods(&self) -> &ModSelect {
        &self.mods
    }

    /// The panel, for the application to write the active flags and to set its columns.
    pub fn mods_mut(&mut self) -> &mut ModSelect {
        &mut self.mods
    }

    fn selected_row(&self) -> usize {
        self.set + 1 + self.difficulty
    }

    fn max_scroll(&self) -> f32 {
        (self.list.row_count(self.set) as f32 - self.viewport).max(0.0)
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    /// Scrolls the least that shows the selected row.
    fn ensure_visible(&mut self) {
        if self.list.sets.is_empty() {
            return;
        }
        let row = self.selected_row() as f32;
        if row < self.scroll {
            // Show the set's own row too.
            self.scroll = self.set as f32;
        } else if row + 1.0 > self.scroll + self.viewport {
            self.scroll = row + 1.0 - self.viewport;
        }
        self.clamp_scroll();
    }

    fn select_set(&mut self, set: usize) {
        self.set = set.min(self.list.sets.len().saturating_sub(1));
        self.difficulty = 0;
    }

    fn next(&mut self) {
        if self.difficulty + 1 < self.list.difficulty_count(self.set) {
            self.difficulty += 1;
        } else if self.set + 1 < self.list.sets.len() {
            self.select_set(self.set + 1);
        }
    }

    fn previous(&mut self) {
        if self.difficulty > 0 {
            self.difficulty -= 1;
        } else if self.set > 0 {
            self.set -= 1;
            self.difficulty = self.list.difficulty_count(self.set).saturating_sub(1);
        }
    }

    fn play(&self) -> Option<SongSelectAction> {
        let (set, difficulty) = self.selected()?;
        (difficulty < self.list.difficulty_count(set))
            .then_some(SongSelectAction::Play { set, difficulty })
    }

    /// Applies `input`; `layout` places the rows for the pointer and sizes the viewport.
    /// Does not allocate.
    pub fn input(
        &mut self,
        input: SongSelectInput,
        layout: &SongSelectLayout,
    ) -> Option<SongSelectAction> {
        use SongSelectInput as I;
        // The open panel takes every input; Back closes it instead of leaving.
        if self.mods.is_open() {
            if let I::PointerMove { x, y } | I::PointerPress { x, y } = input {
                self.pointer = Some((x, y));
            }
            return self
                .mods
                .input(input, &layout.mods)
                .map(|action| match action {
                    ModSelectAction::Toggle { column, index } => {
                        SongSelectAction::ToggleMod { column, index }
                    }
                    ModSelectAction::DeselectAll => SongSelectAction::DeselectMods,
                });
        }
        if input == I::Back {
            return Some(SongSelectAction::Back);
        }
        if self.status == SongSelectStatus::Loading {
            return None;
        }
        self.viewport = layout.viewport_rows();
        let page = (self.viewport as usize).max(1);
        let last = self.list.sets.len().saturating_sub(1);
        let mut action = None;
        let mut moved = true;
        match input {
            I::Up => self.previous(),
            I::Down => self.next(),
            I::Left => self.select_set(self.set.saturating_sub(1)),
            I::Right => self.select_set(self.set + 1),
            I::PageUp => self.select_set(self.set.saturating_sub(page)),
            I::PageDown => self.select_set(self.set.saturating_add(page)),
            I::Home => self.select_set(0),
            I::End => self.select_set(last),
            I::Activate => action = self.play(),
            I::Back | I::ModHotkey { .. } => moved = false,
            I::ToggleMods => {
                self.mods.open();
                moved = false;
            }
            I::DeselectMods => {
                action = Some(SongSelectAction::DeselectMods);
                moved = false;
            }
            I::Scroll { lines } => {
                self.scroll += lines;
                self.clamp_scroll();
                moved = false;
            }
            I::PointerMove { x, y } => {
                self.pointer = Some((x, y));
                moved = false;
            }
            I::PointerPress { x, y } => {
                self.pointer = Some((x, y));
                moved = false;
                match layout.row_at(x, y, self.scroll, &self.list, self.set) {
                    Some(Row::Set(set)) if set != self.set => self.select_set(set),
                    Some(Row::Difficulty { difficulty, .. }) => {
                        if difficulty == self.difficulty {
                            action = self.play();
                        }
                        self.difficulty = difficulty;
                    }
                    _ => {}
                }
            }
        }
        if moved {
            self.ensure_visible();
        }
        self.clamp_scroll();
        self.hovered = self
            .pointer
            .and_then(|(x, y)| layout.row_at(x, y, self.scroll, &self.list, self.set));
        action
    }
}

#[cfg(test)]
mod tests;
