//! The song select screen: its update-side state and layout, input-agnostic (the
//! application maps its events to [`SongSelectInput`]). The draw side is
//! [`view::SongSelectView`].
//!
//! The list is a flat column of rows: every set is a row, and the selected set is expanded,
//! showing its difficulties as indented rows right beneath it. Typing filters the list
//! ([`SearchQuery`], [`SongFilter`]): the sets and difficulties that do not match the query
//! are left out. Everything public speaks in indices into the [`SongList`]; the state
//! behind it keeps positions among the visible sets and difficulties.

use std::ops::Range;
use std::sync::Arc;

use crate::menu::Rect;
use crate::theme::Theme;

pub mod filter;
pub mod mod_select;
pub mod view;

pub use filter::{QUERY_CAPACITY, SearchQuery, SongFilter, search_text};

pub use mod_select::{
    CustomisationLayout, CustomisationSnapshot, CustomisationState, MAX_CHOICES, MAX_SETTING_ROWS,
    ModColumns, ModSelect, ModSelectAction, ModSelectLayout, ModSelectSnapshot, SettingRow,
    SettingRowKind, SettingValue, TileDetail, TileDetails,
};
pub use view::{DetailLabels, SongSelectLabels, SongSelectView};

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
    /// What the search matches: the set's fields lowercased and joined by `'\n'`, made by
    /// [`search_text`].
    pub search: String,
    pub difficulties: Vec<DifficultyRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DifficultyRow {
    pub version: String,
    /// What the search matches: the version lowercased, made by [`search_text`].
    pub search: String,
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
    /// Escape: clears the query, or, with none, leaves the screen.
    Back,
    /// F1: opens the mod select panel (and closes it).
    ToggleMods,
    /// Backspace: erases the last character of the query, or, with none, deselects every
    /// mod.
    DeselectMods,
    /// A mod hotkey: the tile `index` of `column` of the mod select panel. The application
    /// maps its key rows to the columns that have hotkeys.
    ModHotkey {
        column: usize,
        index: usize,
    },
    /// Tab: expands or collapses the mod settings panel while the mod select is open.
    ToggleCustomisation,
    /// Delete: resets the selected row of the expanded settings panel to its default.
    ResetSetting,
    /// Space: like Activate in the expanded settings panel, nothing elsewhere. Typed text
    /// arrives as [`SongSelectInput::Type`] instead.
    Space,
    /// A typed character: appended to the search query. Control characters are ignored,
    /// and so is a space on an empty query. Nothing happens while the mod panel is open
    /// (letters are mod hotkeys then) or a difficulty is loading.
    Type(char),
    /// A key repeat of Backspace: erases the last character of the query, nothing else.
    Erase,
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
    /// The primary button was released: ends a slider drag in the settings panel.
    PointerRelease,
}

/// What the screen asks the application to do; indices are into the current list.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// The settings panel changed row `row` (an index into the rows the application gave
    /// [`ModSelect::set_setting_rows`]).
    SetModSetting {
        row: usize,
        value: SettingValue,
    },
    /// The settings panel resets row `row` to its default.
    ResetModSetting {
        row: usize,
    },
    /// The "choose folder" button was used: the application shows its folder picker.
    ChooseFolder,
}

/// One row of the flat column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Set(usize),
    Difficulty { set: usize, difficulty: usize },
}

/// Where the list, status line and hint are: a panel on the right of the window. A pure
/// function of the window size and theme, shared by hit testing (update) and drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SongSelectLayout {
    /// Status line at the top: the first line of `details`, shown instead of it until the
    /// list is ready.
    pub header: Rect,
    /// Two lines of the selected beatmap's details at the top of the panel (ADR-0067).
    pub details: Rect,
    /// The search line between the details and the list.
    pub search: Rect,
    /// The rows' area; row 0 is at its top when not scrolled.
    pub list: Rect,
    /// Key hints below the list.
    pub hint: Rect,
    /// The "choose folder" button: the list's second row, below the empty-list text.
    pub choose_folder: Rect,
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
        let hint_height = theme.song_detail_size * scale * 1.5;
        let header = Rect {
            x,
            y: margin,
            width: panel,
            height: hint_height,
        };
        let hint = Rect {
            x,
            y: height - margin - hint_height,
            width: panel,
            height: hint_height,
        };
        let row_height = theme.song_row_height * scale;
        let details = Rect {
            x,
            y: header.y,
            width: panel,
            height: 2.0 * hint_height,
        };
        let search = Rect {
            x,
            y: details.y + details.height + margin / 2.0,
            width: panel,
            height: row_height - theme.song_row_gap * scale,
        };
        let top = search.y + row_height;
        let list = Rect {
            x,
            y: top,
            width: panel,
            height: (hint.y - margin / 2.0 - top).max(0.0),
        };
        let choose_folder = Rect {
            x,
            y: list.y + row_height,
            width: panel,
            height: row_height - theme.song_row_gap * scale,
        };
        SongSelectLayout {
            header,
            details,
            search,
            list,
            hint,
            choose_folder,
            mods: ModSelectLayout::new(width, height, theme),
            row_height,
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

    /// The row under the point, with the list scrolled by `scroll` rows and the visible set
    /// at position `selected_set` expanded.
    pub fn row_at(
        &self,
        x: f32,
        y: f32,
        scroll: f32,
        filter: &SongFilter,
        selected_set: usize,
    ) -> Option<Row> {
        if !self.list.contains(x, y) {
            return None;
        }
        let index = (y - self.list.y) / self.row_height + scroll;
        if index < 0.0 {
            return None;
        }
        filter.row(selected_set, index.floor() as usize)
    }
}

/// How many rows the snapshot carries: more than the list area shows at any window size
/// (its height in rows is the same at every size: about 11 with the default theme) plus
/// the rows cut at both ends. A theme with a taller area is cut to this many rows.
pub const MAX_VISIBLE_ROWS: usize = 16;

/// What the draw side needs; cloning copies an `Arc` and some fixed-size data, no
/// allocation.
#[derive(Clone, Debug)]
pub struct SongSelectSnapshot {
    pub list: Arc<SongList>,
    /// The selected `(set, difficulty)` as indices into the list, as
    /// [`SongSelect::selected`].
    pub selected: Option<(usize, usize)>,
    /// What the player typed.
    pub query: SearchQuery,
    /// Rows of the flat column, filter applied.
    pub row_count: usize,
    /// Flat index of the first of `rows`.
    pub first_row: usize,
    rows: [Row; MAX_VISIBLE_ROWS],
    /// Rows scrolled off the top.
    pub scroll: f32,
    pub hovered: Option<Row>,
    /// The pointer is over the "choose folder" button, which is shown.
    pub folder_hovered: bool,
    pub status: SongSelectStatus,
    /// The localised "scanning" message, made by the application; the view re-lays it
    /// out when the `Arc` changes (by identity).
    pub scanning_text: Arc<str>,
    /// Changes whenever the list is replaced; the view's row texts are valid for one
    /// generation.
    pub generation: u64,
    pub mods: ModSelectSnapshot,
    /// The selected difficulty's details, as [`SongSelect::set_details`] gave them.
    pub details: Option<BeatmapDetails>,
}

/// What the song select shows of the selected difficulty (ADR-0067), computed by the
/// application with the selected mods.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatmapDetails {
    pub circle_size: f32,
    pub approach_rate: f32,
    pub overall_difficulty: f32,
    pub drain_rate: f32,
    /// `None` until the file is read.
    pub stats: Option<DetailStats>,
}

/// The read part of [`BeatmapDetails`], at the mods' rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DetailStats {
    /// Whole milliseconds.
    pub length_ms: u64,
    pub bpm_minimum: i32,
    pub bpm_maximum: i32,
    pub bpm_most_common: i32,
    pub circles: u32,
    pub sliders: u32,
    pub spinners: u32,
}

impl SongSelectSnapshot {
    /// The rows that are in or near the list area, from flat index `first_row`.
    pub fn rows(&self) -> &[Row] {
        let len = self.row_count.saturating_sub(self.first_row);
        &self.rows[..len.min(MAX_VISIBLE_ROWS)]
    }
}

/// The screen's state on the update thread.
#[derive(Clone, Debug)]
pub struct SongSelect {
    list: Arc<SongList>,
    filter: SongFilter,
    query: SearchQuery,
    /// Positions among the visible sets, and among the visible difficulties of that set.
    set: usize,
    difficulty: usize,
    scroll: f32,
    pointer: Option<(f32, f32)>,
    hovered: Option<Row>,
    folder_hovered: bool,
    status: SongSelectStatus,
    scanning_text: Arc<str>,
    generation: u64,
    mods: ModSelect,
    /// Rows that fit the list area at the last input; keeps the selection in view.
    viewport: f32,
    details: Option<BeatmapDetails>,
}

impl Default for SongSelect {
    fn default() -> SongSelect {
        SongSelect::new()
    }
}

/// Whether there is nothing to play and the folder can be chosen: an empty list that is
/// neither being scanned nor loading.
pub(crate) fn can_choose_folder(list: &SongList, status: SongSelectStatus) -> bool {
    list.sets.is_empty()
        && matches!(
            status,
            SongSelectStatus::NoFolder | SongSelectStatus::Failed | SongSelectStatus::Ready
        )
}

impl SongSelect {
    pub fn new() -> SongSelect {
        SongSelect {
            list: Arc::default(),
            filter: SongFilter::default(),
            query: SearchQuery::default(),
            set: 0,
            difficulty: 0,
            scroll: 0.0,
            pointer: None,
            hovered: None,
            folder_hovered: false,
            status: SongSelectStatus::default(),
            scanning_text: Arc::from(""),
            generation: 0,
            mods: ModSelect::default(),
            viewport: 10.0,
            details: None,
        }
    }

    /// Sets the selected difficulty's details for the next snapshots.
    pub fn set_details(&mut self, details: Option<BeatmapDetails>) {
        self.details = details;
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

    /// What the player typed.
    pub fn query(&self) -> &SearchQuery {
        &self.query
    }

    /// The sets and difficulties that pass the query.
    pub fn filter(&self) -> &SongFilter {
        &self.filter
    }

    /// The selected `(set, difficulty)` as indices into the list, if a set is visible.
    pub fn selected(&self) -> Option<(usize, usize)> {
        let set = self.filter.set_index(self.set)?;
        Some((set, self.filter.difficulty_index(self.set, self.difficulty)))
    }

    /// Replaces the list, keeping the selected set by its key when it is still visible
    /// (the difficulty position is clamped), else the selection stays at the same position.
    /// The query stays: `filter` is the buffers for the new list ([`SongFilter::for_list`]),
    /// made off the update thread. Returns the filter it replaces, for the caller to free
    /// off the update thread too.
    pub fn set_list(&mut self, list: Arc<SongList>, filter: SongFilter) -> SongFilter {
        let key = self
            .selected()
            .and_then(|(set, _)| self.list.sets.get(set))
            .map(|s| s.key);
        let old = std::mem::replace(&mut self.filter, filter);
        self.list = list;
        self.filter.apply(&self.list, &self.query);
        let found = key
            .and_then(|key| self.list.sets.iter().position(|s| s.key == key))
            .and_then(|index| self.filter.set_position(index));
        self.set = found.unwrap_or(self.set);
        self.generation += 1;
        self.set = self.set.min(self.filter.set_count().saturating_sub(1));
        self.difficulty = self
            .difficulty
            .min(self.filter.difficulty_count(self.set).saturating_sub(1));
        self.hovered = None;
        self.clamp_scroll();
        self.ensure_visible();
        old
    }

    /// Filters the list again after the query changed. The selection stays on its
    /// difficulty if that is still visible, else moves to the first visible difficulty of
    /// its set, else to the first visible set.
    fn refilter(&mut self) {
        let before = self.selected();
        self.filter.apply(&self.list, &self.query);
        let (set, difficulty) = before
            .and_then(|(set, difficulty)| {
                let set = self.filter.set_position(set)?;
                let difficulty = self.filter.difficulty_position(set, difficulty);
                Some((set, difficulty.unwrap_or(0)))
            })
            .unwrap_or((0, 0));
        self.set = set;
        self.difficulty = difficulty;
        self.hovered = None;
        self.clamp_scroll();
        self.ensure_visible();
    }

    /// Appends a typed character to the query.
    fn type_char(&mut self, c: char) {
        if self.push_typed(c) {
            self.refilter();
        }
    }

    /// Appends a typed character to the query without filtering again; returns whether it
    /// changed.
    fn push_typed(&mut self, c: char) -> bool {
        if c.is_control() {
            return false;
        }
        let c = if c.is_whitespace() { ' ' } else { c };
        if c == ' ' && self.query.is_empty() {
            return false;
        }
        self.query.push(c)
    }

    /// Types `text` as [`SongSelectInput::Type`] does each of its characters, filtering the
    /// list once at the end (a long IME commit is one pass over the library, not one per
    /// character). Does not allocate.
    pub fn type_text(&mut self, text: &str, layout: &SongSelectLayout) {
        if self.mods.is_open() || self.status == SongSelectStatus::Loading {
            return;
        }
        let mut typed = false;
        for c in text.chars() {
            typed |= self.push_typed(c);
        }
        if typed {
            self.refilter();
            self.hovered = self
                .pointer
                .and_then(|(x, y)| layout.row_at(x, y, self.scroll, &self.filter, self.set));
        }
    }

    /// Erases the last character of the query; returns whether there was one.
    fn erase(&mut self) -> bool {
        let erased = self.query.pop();
        if erased {
            self.refilter();
        }
        erased
    }

    /// Whether the "choose folder" button is shown.
    pub fn shows_choose_folder(&self) -> bool {
        can_choose_folder(&self.list, self.status)
    }

    pub fn snapshot(&self) -> SongSelectSnapshot {
        let row_count = self.filter.row_count(self.set);
        let first_row = (self.scroll.max(0.0).floor() as usize).min(row_count);
        // Every slot `rows` returns is filled, however many rows the view shows.
        let mut rows = [Row::Set(0); MAX_VISIBLE_ROWS];
        for (slot, index) in rows.iter_mut().zip(first_row..row_count) {
            if let Some(row) = self.filter.row(self.set, index) {
                *slot = row;
            }
        }
        SongSelectSnapshot {
            list: Arc::clone(&self.list),
            selected: self.selected(),
            query: self.query,
            row_count,
            first_row,
            rows,
            scroll: self.scroll,
            hovered: self.hovered,
            folder_hovered: self.folder_hovered,
            status: self.status,
            scanning_text: Arc::clone(&self.scanning_text),
            generation: self.generation,
            mods: self.mods.snapshot(),
            details: self.details,
        }
    }

    /// The mod select panel.
    pub fn mods(&self) -> &ModSelect {
        &self.mods
    }

    /// Advances the time-driven parts of the screen (the settings panel's collapse delay) to
    /// `now_ns`, a monotonic clock in nanoseconds. Does not allocate.
    pub fn tick(&mut self, now_ns: u64, layout: &SongSelectLayout) {
        self.viewport = layout.viewport_rows();
        if self.mods.is_open() {
            self.mods.tick(now_ns, &layout.mods);
        }
    }

    /// The panel, for the application to write the active flags and to set its columns.
    pub fn mods_mut(&mut self) -> &mut ModSelect {
        &mut self.mods
    }

    fn selected_row(&self) -> usize {
        self.set + 1 + self.difficulty
    }

    fn max_scroll(&self) -> f32 {
        (self.filter.row_count(self.set) as f32 - self.viewport).max(0.0)
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    /// Scrolls the least that shows the selected row.
    fn ensure_visible(&mut self) {
        if self.filter.set_count() == 0 {
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
        self.set = set.min(self.filter.set_count().saturating_sub(1));
        self.difficulty = 0;
    }

    fn next(&mut self) {
        if self.difficulty + 1 < self.filter.difficulty_count(self.set) {
            self.difficulty += 1;
        } else if self.set + 1 < self.filter.set_count() {
            self.select_set(self.set + 1);
        }
    }

    fn previous(&mut self) {
        if self.difficulty > 0 {
            self.difficulty -= 1;
        } else if self.set > 0 {
            self.set -= 1;
            self.difficulty = self.filter.difficulty_count(self.set).saturating_sub(1);
        }
    }

    fn play(&self) -> Option<SongSelectAction> {
        let (set, difficulty) = self.selected()?;
        (self.difficulty < self.filter.difficulty_count(self.set))
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
                    ModSelectAction::SetSetting { row, value } => {
                        SongSelectAction::SetModSetting { row, value }
                    }
                    ModSelectAction::ResetSetting { row } => {
                        SongSelectAction::ResetModSetting { row }
                    }
                });
        }
        let loading = self.status == SongSelectStatus::Loading;
        if input == I::Back {
            if !loading && !self.query.is_empty() {
                self.query.clear();
                self.refilter();
                return None;
            }
            return Some(SongSelectAction::Back);
        }
        if loading {
            return None;
        }
        self.viewport = layout.viewport_rows();
        let page = (self.viewport as usize).max(1);
        let last = self.filter.set_count().saturating_sub(1);
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
            I::Activate => {
                action = if self.shows_choose_folder() {
                    Some(SongSelectAction::ChooseFolder)
                } else {
                    self.play()
                }
            }
            I::Back
            | I::ModHotkey { .. }
            | I::ToggleCustomisation
            | I::ResetSetting
            | I::Space
            | I::PointerRelease => moved = false,
            I::Type(c) => {
                self.type_char(c);
                moved = false;
            }
            I::Erase => {
                self.erase();
                moved = false;
            }
            I::ToggleMods => {
                self.mods.open();
                moved = false;
            }
            I::DeselectMods => {
                if !self.erase() {
                    action = Some(SongSelectAction::DeselectMods);
                }
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
                if self.shows_choose_folder() && layout.choose_folder.contains(x, y) {
                    action = Some(SongSelectAction::ChooseFolder);
                }
                match layout.row_at(x, y, self.scroll, &self.filter, self.set) {
                    Some(Row::Set(set)) => {
                        if let Some(position) = self.filter.set_position(set)
                            && position != self.set
                        {
                            self.select_set(position);
                        }
                    }
                    Some(Row::Difficulty { difficulty, .. }) => {
                        if let Some(position) =
                            self.filter.difficulty_position(self.set, difficulty)
                        {
                            if position == self.difficulty {
                                action = self.play();
                            }
                            self.difficulty = position;
                        }
                    }
                    None => {}
                }
            }
        }
        if moved {
            self.ensure_visible();
        }
        self.clamp_scroll();
        self.hovered = self
            .pointer
            .and_then(|(x, y)| layout.row_at(x, y, self.scroll, &self.filter, self.set));
        self.folder_hovered = self.shows_choose_folder()
            && self
                .pointer
                .is_some_and(|(x, y)| layout.choose_folder.contains(x, y));
        action
    }
}

#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod tests;
