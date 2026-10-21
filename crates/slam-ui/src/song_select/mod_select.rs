//! The mod select panel of the song select: columns of mod tiles the player toggles with the
//! keyboard or the mouse. The panel knows only acronyms and an active flag per mod; the
//! application owns the mod set, decides what a toggle does (incompatible mods drop out) and
//! writes the result back with [`ModSelect::set_active`], as lazer's overlay does with an
//! external selection.

use super::SongSelectInput;
use crate::menu::Rect;
use crate::theme::Theme;

/// Columns of the panel: lazer's difficulty reduction, difficulty increase and conversion.
pub const MAX_COLUMNS: usize = 3;
/// Most tiles of a column; the active flags of a column are a `u16`.
pub const MAX_COLUMN_MODS: usize = 16;
/// The first columns have keyboard hotkeys; the rest (lazer's conversion column) do not.
pub const HOTKEY_COLUMNS: usize = 2;

/// The acronyms of each column, in display order.
pub type ModColumns = [&'static [&'static str]; MAX_COLUMNS];

/// What the panel asks the application to do; the application then reports the new flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModSelectAction {
    /// The tile `index` of `column` was toggled.
    Toggle {
        column: usize,
        index: usize,
    },
    DeselectAll,
}

/// Where the panel's texts and tiles are: a pure function of the window size and theme,
/// shared by hit testing (update) and drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModSelectLayout {
    pub title: Rect,
    pub column_titles: [Rect; MAX_COLUMNS],
    pub hint: Rect,
    /// Left edge and width of the columns.
    column_x: [f32; MAX_COLUMNS],
    tile_width: f32,
    tiles_top: f32,
    tile_height: f32,
    pitch: f32,
}

impl ModSelectLayout {
    pub fn new(width: f32, height: f32, theme: &Theme) -> ModSelectLayout {
        let scale = theme.scale(height);
        let margin = theme.song_margin * scale;
        let tile_width = theme.mod_tile_width * scale;
        let gap = theme.mod_column_gap * scale;
        let total = tile_width * MAX_COLUMNS as f32 + gap * (MAX_COLUMNS - 1) as f32;
        let left = (width - total) / 2.0;
        let title = Rect {
            x: left,
            y: margin,
            width: total,
            height: theme.song_header_height * scale,
        };
        let column_x: [f32; MAX_COLUMNS] =
            std::array::from_fn(|i| left + i as f32 * (tile_width + gap));
        let label_height = theme.song_detail_size * scale * 1.5;
        let label_y = title.y + title.height + margin / 2.0;
        let column_titles = column_x.map(|x| Rect {
            x,
            y: label_y,
            width: tile_width,
            height: label_height,
        });
        let hint = Rect {
            x: left,
            y: height - margin - label_height,
            width: total,
            height: label_height,
        };
        ModSelectLayout {
            title,
            column_titles,
            hint,
            column_x,
            tile_width,
            tiles_top: label_y + label_height + margin / 2.0,
            tile_height: (theme.song_row_height - theme.song_row_gap) * scale,
            pitch: theme.song_row_height * scale,
        }
    }

    /// The tile `index` of `column`.
    pub fn tile_rect(&self, column: usize, index: usize) -> Rect {
        Rect {
            x: self.column_x[column.min(MAX_COLUMNS - 1)],
            y: self.tiles_top + index as f32 * self.pitch,
            width: self.tile_width,
            height: self.tile_height,
        }
    }

    /// The tile under the point, with `counts` tiles in each column.
    pub fn tile_at(&self, x: f32, y: f32, counts: [usize; MAX_COLUMNS]) -> Option<(usize, usize)> {
        let column = (0..MAX_COLUMNS)
            .find(|&c| x >= self.column_x[c] && x < self.column_x[c] + self.tile_width)?;
        let offset = y - self.tiles_top;
        if offset < 0.0 {
            return None;
        }
        let index = (offset / self.pitch).floor() as usize;
        (index < counts[column] && self.tile_rect(column, index).contains(x, y))
            .then_some((column, index))
    }
}

/// What the draw side needs; `Copy`, so a snapshot allocates nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModSelectSnapshot {
    pub open: bool,
    /// Bit `i` of column `c`: the tile `i` of that column is active.
    pub active: [u16; MAX_COLUMNS],
    pub hover: Option<(usize, usize)>,
}

/// The panel's state on the update thread.
#[derive(Clone, Debug)]
pub struct ModSelect {
    columns: ModColumns,
    open: bool,
    active: [u16; MAX_COLUMNS],
    hover: Option<(usize, usize)>,
}

impl Default for ModSelect {
    fn default() -> ModSelect {
        ModSelect::new([&[]; MAX_COLUMNS])
    }
}

impl ModSelect {
    /// A closed panel with nothing active. Columns longer than [`MAX_COLUMN_MODS`] are cut.
    pub fn new(columns: ModColumns) -> ModSelect {
        ModSelect {
            columns: columns.map(|c| &c[..c.len().min(MAX_COLUMN_MODS)]),
            open: false,
            active: [0; MAX_COLUMNS],
            hover: None,
        }
    }

    pub fn columns(&self) -> &ModColumns {
        &self.columns
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.hover = None;
    }

    /// Whether the tile `index` of `column` is active.
    pub fn is_active(&self, column: usize, index: usize) -> bool {
        index < MAX_COLUMN_MODS && self.active.get(column).is_some_and(|a| a >> index & 1 != 0)
    }

    /// Sets the flag of a tile; out-of-range tiles are ignored.
    pub fn set_active(&mut self, column: usize, index: usize, on: bool) {
        if index >= self.columns.get(column).map_or(0, |c| c.len()) {
            return;
        }
        let bit = 1u16 << index;
        if on {
            self.active[column] |= bit;
        } else {
            self.active[column] &= !bit;
        }
    }

    /// Every acronym of the panel, column by column, with its flag.
    pub fn iter(&self) -> impl Iterator<Item = (usize, usize, &'static str, bool)> + '_ {
        self.columns.iter().enumerate().flat_map(move |(c, mods)| {
            mods.iter()
                .enumerate()
                .map(move |(i, &acronym)| (c, i, acronym, self.is_active(c, i)))
        })
    }

    pub fn snapshot(&self) -> ModSelectSnapshot {
        ModSelectSnapshot {
            open: self.open,
            active: self.active,
            hover: self.hover,
        }
    }

    fn counts(&self) -> [usize; MAX_COLUMNS] {
        self.columns.map(|c| c.len())
    }

    /// Applies `input` while the panel is open: it takes every key. Closing and hovering are
    /// handled here; a toggle is for the application to resolve. Does not allocate.
    pub fn input(
        &mut self,
        input: SongSelectInput,
        layout: &ModSelectLayout,
    ) -> Option<ModSelectAction> {
        use SongSelectInput as I;
        match input {
            // lazer: Back and ToggleModSelection hide the overlay, so does Select without a
            // search.
            I::Back | I::Activate | I::ToggleMods => {
                self.close();
                None
            }
            I::DeselectMods => Some(ModSelectAction::DeselectAll),
            I::ModHotkey { column, index } => (column < HOTKEY_COLUMNS
                && index < self.columns[column].len())
            .then_some(ModSelectAction::Toggle { column, index }),
            I::PointerMove { x, y } => {
                self.hover = layout.tile_at(x, y, self.counts());
                None
            }
            I::PointerPress { x, y } => {
                self.hover = layout.tile_at(x, y, self.counts());
                self.hover
                    .map(|(column, index)| ModSelectAction::Toggle { column, index })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
