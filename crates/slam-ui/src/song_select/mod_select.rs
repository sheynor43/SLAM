//! The mod select panel of the song select: columns of mod tiles the player toggles with the
//! keyboard or the mouse. The panel knows only acronyms and an active flag per mod; the
//! application owns the mod set, decides what a toggle does (incompatible mods drop out) and
//! writes the result back with [`ModSelect::set_active`], as lazer's overlay does with an
//! external selection.

use super::SongSelectInput;
use crate::menu::Rect;
use crate::theme::Theme;

mod settings;

pub use settings::{
    Customisation, CustomisationLayout, CustomisationSnapshot, CustomisationState, MAX_CHOICES,
    MAX_SETTING_ROWS, SettingRow, SettingRowKind, SettingValue,
};

/// Columns of the panel, lazer's: difficulty reduction, difficulty increase, automation,
/// conversion, fun (ADR-0152). A column without mods is not shown.
pub const MAX_COLUMNS: usize = 5;
/// Most tiles of a column; the active flags of a column are a `u16`.
pub const MAX_COLUMN_MODS: usize = 16;
/// The first columns have keyboard hotkeys, as in lazer; conversion and fun do not.
pub const HOTKEY_COLUMNS: usize = 3;

/// The acronyms of each column, in display order.
pub type ModColumns = [&'static [&'static str]; MAX_COLUMNS];

/// What the panel asks the application to do; the application then reports the new flags.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModSelectAction {
    /// The tile `index` of `column` was toggled.
    Toggle {
        column: usize,
        index: usize,
    },
    DeselectAll,
    /// The tile `index` of `column` was right-clicked at `x`, `y`: the application shows
    /// that mod's settings there ([`ModSelect::open_customisation_at`]).
    Settings {
        column: usize,
        index: usize,
        x: f32,
        y: f32,
    },
    /// The settings panel changed the row `row` it was given.
    SetSetting {
        row: usize,
        value: SettingValue,
    },
    /// The settings panel resets the row `row` to its default.
    ResetSetting {
        row: usize,
    },
}

/// Where the panel's texts and tiles are: a function of the window size, the theme and
/// which columns have mods ([`ModSelectLayout::place`]), shared by hit testing (update) and
/// drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModSelectLayout {
    pub title: Rect,
    /// The columns' headers, in their colours; a column not shown has an empty one.
    pub column_titles: [Rect; MAX_COLUMNS],
    pub hint: Rect,
    /// The settings panel, as it stands before the player places it.
    pub customise: CustomisationLayout,
    /// Left edge of each column at the columns' middle height, before the lean.
    column_x: [f32; MAX_COLUMNS],
    /// Which columns are shown: those with tiles.
    shown: [bool; MAX_COLUMNS],
    /// The tiles in each column, from the last [`ModSelectLayout::place`].
    counts: [usize; MAX_COLUMNS],
    width: f32,
    margin: f32,
    max_tile_width: f32,
    tile_width: f32,
    gap: f32,
    /// Where the columns hang: right under the title.
    min_top: f32,
    /// Top and bottom of the tallest column, set by [`ModSelectLayout::place`].
    top: f32,
    bottom: f32,
    header_height: f32,
    pad: f32,
    tiles_top: f32,
    tile_height: f32,
    pitch: f32,
    /// How far right a point of a column is per unit up (`tan` of the button skew): the
    /// columns lean as lazer's sheared ones.
    lean: f32,
}

impl ModSelectLayout {
    /// The layout with every column shown; [`ModSelectLayout::place`] hides the empty ones.
    /// The columns hang under the header, as lazer's (ADR-0152).
    pub fn new(width: f32, height: f32, theme: &Theme) -> ModSelectLayout {
        let scale = theme.scale(height);
        let margin = theme.song_margin * scale;
        let gap = theme.mod_column_gap * scale;
        // Under the toolbar (ADR-0168), above the footer (ADR-0175, ADR-0177).
        let title = Rect {
            x: margin,
            y: (theme.toolbar_height + theme.song_top_gap) * scale,
            width: width - 2.0 * margin,
            height: theme.song_header_height * scale,
        };
        let label_height = theme.song_detail_size * scale * 1.5;
        let header_height = theme.mod_column_header_height * scale;
        let pad = theme.mod_column_pad * scale;
        let footer_top = height - theme.footer_height * scale;
        let hint = Rect {
            x: margin,
            y: footer_top - margin / 2.0 - label_height,
            width: width - 2.0 * margin,
            height: label_height,
        };
        let min_top = (height * theme.mod_panel_top).max(title.y + title.height + margin);
        let (top, bottom) = (min_top, min_top + header_height);
        let mut layout = ModSelectLayout {
            title,
            column_titles: [Rect::default(); MAX_COLUMNS],
            hint,
            customise: CustomisationLayout::new(width, height, theme),
            column_x: [0.0; MAX_COLUMNS],
            shown: [false; MAX_COLUMNS],
            counts: [0; MAX_COLUMNS],
            width,
            margin,
            max_tile_width: theme.mod_tile_width * scale,
            tile_width: theme.mod_tile_width * scale,
            gap,
            min_top,
            top,
            bottom,
            header_height,
            pad,
            tiles_top: min_top + header_height + pad,
            tile_height: theme.mod_tile_height * scale,
            pitch: theme.mod_tile_pitch * scale,
            lean: theme.button_skew().tan(),
        };
        layout.place([1; MAX_COLUMNS]);
        layout
    }

    /// Shows the columns with tiles (`counts` per column), centred in the window and narrowed
    /// to fit it. Does not allocate.
    pub fn place(&mut self, counts: [usize; MAX_COLUMNS]) {
        // The columns are as tall as their content and hang right under the header, as in
        // lazer's overlay; the room under them stays free.
        self.counts = counts;
        let tallest = counts.iter().copied().max().unwrap_or(0);
        let content = self.column_height(tallest);
        self.top = self.min_top;
        self.bottom = self.top + content;
        self.tiles_top = self.top + self.header_height + self.pad;
        let shown = counts.iter().filter(|&&n| n > 0).count();
        let gaps = self.gap * shown.saturating_sub(1) as f32;
        // The lean pushes the top right and the bottom left: room for both.
        let lean_room = (self.bottom - self.top) * self.lean;
        let room = self.width - 2.0 * self.margin - gaps - lean_room;
        self.tile_width = if shown == 0 {
            self.max_tile_width
        } else {
            self.max_tile_width
                .min(room / shown as f32 - 2.0 * self.pad)
                .max(0.0)
        };
        let column_width = self.tile_width + 2.0 * self.pad;
        let total = column_width * shown as f32 + gaps;
        let left = (self.width - total) / 2.0;
        let mut slot = 0;
        for (column, &count) in counts.iter().enumerate() {
            self.shown[column] = count > 0;
            if count == 0 {
                self.column_x[column] = 0.0;
                self.column_titles[column] = Rect::default();
                continue;
            }
            let x = left + slot as f32 * (column_width + self.gap);
            slot += 1;
            self.column_x[column] = x;
            self.column_titles[column] = self.leaned(Rect {
                x,
                y: self.top,
                width: column_width,
                height: self.header_height,
            });
        }
    }

    /// The height every column leans about: the middle of the tallest plate, so the whole
    /// set shares one slanted line and the columns do not stagger.
    fn lean_middle(&self) -> f32 {
        self.top + (self.bottom - self.top) / 2.0
    }

    /// `rect` moved sideways by the lean at its middle height, about the common middle.
    fn leaned(&self, rect: Rect) -> Rect {
        Rect {
            x: rect.x + (self.lean_middle() - (rect.y + rect.height / 2.0)) * self.lean,
            ..rect
        }
    }

    /// How far right the columns lean per unit up: draw their plates with it.
    pub fn lean(&self) -> f32 {
        self.lean
    }

    /// Whether `column` is shown.
    pub fn is_shown(&self, column: usize) -> bool {
        self.shown.get(column).copied().unwrap_or(false)
    }

    /// Height of a column plate holding `count` tiles: its header, the tiles and the padding.
    fn column_height(&self, count: usize) -> f32 {
        if count == 0 {
            return self.header_height;
        }
        self.header_height
            + self.pad
            + (count - 1) as f32 * self.pitch
            + self.tile_height
            + self.pad
    }

    /// The plate of `column` holding `count` tiles, from the top of its header down to its
    /// last tile and a padding.
    pub fn column_rect(&self, column: usize, count: usize) -> Rect {
        // The plate is drawn leaning about its own middle; shifting it by the difference
        // to the common middle puts its edges on the shared slanted line.
        let height = self.column_height(count);
        Rect {
            x: self.column_x[column.min(MAX_COLUMNS - 1)]
                + (self.lean_middle() - (self.top + height / 2.0)) * self.lean,
            y: self.top,
            width: self.tile_width + 2.0 * self.pad,
            height,
        }
    }

    /// The bottom of the tallest column plate.
    pub fn content_bottom(&self) -> f32 {
        self.bottom
    }

    /// The tile `index` of `column`, moved by the lean at its height.
    pub fn tile_rect(&self, column: usize, index: usize) -> Rect {
        self.leaned(Rect {
            x: self.column_x[column.min(MAX_COLUMNS - 1)] + self.pad,
            y: self.tiles_top + index as f32 * self.pitch,
            width: self.tile_width,
            height: self.tile_height,
        })
    }

    /// The tile under the point, with `counts` tiles in each column.
    pub fn tile_at(&self, x: f32, y: f32, counts: [usize; MAX_COLUMNS]) -> Option<(usize, usize)> {
        let offset = y - self.tiles_top;
        if offset < 0.0 {
            return None;
        }
        let index = (offset / self.pitch).floor() as usize;
        (0..MAX_COLUMNS)
            .find(|&c| {
                self.shown[c] && index < counts[c] && self.tile_rect(c, index).contains(x, y)
            })
            .map(|c| (c, index))
    }
}

/// The tiles of each column.
pub fn column_counts(columns: &ModColumns) -> [usize; MAX_COLUMNS] {
    columns.map(|c| c.len().min(MAX_COLUMN_MODS))
}

/// A setting of an active mod the selected mods line shows after its acronym, as lazer's
/// `ExtendedIconInformation`. Fixed point, so that the snapshot stays `Eq`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TileDetail {
    #[default]
    None,
    /// A speed change in hundredths: "1.05x".
    Speed(u16),
    /// An approach rate in tenths: "AR9.4".
    ApproachRate(i16),
}

/// The details of every tile, by column.
pub type TileDetails = [[TileDetail; MAX_COLUMN_MODS]; MAX_COLUMNS];

/// The score multiplier of every tile, by column.
pub type TileMultipliers = [[f64; MAX_COLUMN_MODS]; MAX_COLUMNS];

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/ModUtils.cs (FormatScoreMultiplier)
/// Writes a score multiplier as lazer shows it: rounded to hundredths away from 1 (up above
/// 1, down below), then "1.12x". Appends to `text`; does not allocate beyond it.
pub fn push_multiplier(text: &mut String, multiplier: f64) {
    use std::fmt::Write;
    let hundredths = (multiplier * 100.0 * 1.0e4).round_ties_even() / 1.0e4;
    let hundredths = if multiplier > 1.0 {
        hundredths.ceil()
    } else {
        hundredths.floor()
    };
    let _ = write!(text, "{:.2}x", hundredths / 100.0);
}

/// Which colour a multiplier is shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiplierTone {
    /// Above 1: harder, scores more.
    Up,
    Down,
    Neutral,
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Overlays/Mods/ModSelectOverlay.cs (updateMultiplier), osu.Game/Overlays/Mods/ModsEffectDisplay.cs
impl MultiplierTone {
    pub fn of(multiplier: f64) -> MultiplierTone {
        if multiplier > 1.0 {
            MultiplierTone::Up
        } else if multiplier < 1.0 {
            MultiplierTone::Down
        } else {
            MultiplierTone::Neutral
        }
    }
}

/// What the draw side needs; `Copy`, so a snapshot allocates nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModSelectSnapshot {
    pub open: bool,
    /// Bit `i` of column `c`: the tile `i` of that column is active.
    pub active: [u16; MAX_COLUMNS],
    pub details: TileDetails,
    /// The score multiplier of each tile's mod, and of the whole selection.
    pub multipliers: TileMultipliers,
    pub total_multiplier: f64,
    pub hover: Option<(usize, usize)>,
    pub customise: CustomisationSnapshot,
}

impl Default for ModSelectSnapshot {
    fn default() -> ModSelectSnapshot {
        ModSelectSnapshot {
            open: false,
            active: [0; MAX_COLUMNS],
            details: TileDetails::default(),
            multipliers: [[1.0; MAX_COLUMN_MODS]; MAX_COLUMNS],
            total_multiplier: 1.0,
            hover: None,
            customise: CustomisationSnapshot::default(),
        }
    }
}

/// The panel's state on the update thread.
#[derive(Clone, Debug)]
pub struct ModSelect {
    columns: ModColumns,
    open: bool,
    active: [u16; MAX_COLUMNS],
    details: TileDetails,
    multipliers: TileMultipliers,
    total_multiplier: f64,
    hover: Option<(usize, usize)>,
    customise: Customisation,
    /// Where the pointer was last, where a settings panel opened by the keyboard or by a
    /// mod's selection goes.
    pointer: Option<(f32, f32)>,
    /// Where the settings panel goes without a pointer: its place in the layout last seen.
    anchor: (f32, f32),
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
            details: TileDetails::default(),
            multipliers: [[1.0; MAX_COLUMN_MODS]; MAX_COLUMNS],
            total_multiplier: 1.0,
            hover: None,
            customise: Customisation::default(),
            pointer: None,
            anchor: (0.0, 0.0),
        }
    }

    /// Tells the panel where the settings panel goes when nothing else says.
    pub fn set_anchor(&mut self, layout: &ModSelectLayout) {
        self.anchor = layout.customise.origin();
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
        self.customise.collapse();
    }

    /// Replaces the rows of the settings panel; see [`Customisation::set_rows`].
    pub fn set_setting_rows(&mut self, rows: &[SettingRow]) {
        self.customise.set_rows(rows);
    }

    /// Collapses the settings panel (the selected mods changed).
    pub fn collapse_customisation(&mut self) {
        self.customise.collapse();
    }

    /// Opens the settings panel with its header's corner at `(x, y)`; its place before is
    /// forgotten (ADR-0177). Does nothing without a setting, or while the mod select is
    /// closed.
    pub fn open_customisation_at(&mut self, x: f32, y: f32) {
        if self.open {
            self.customise.open_at((x, y));
        }
    }

    /// Opens the settings panel for a mod that needs configuring, at the pointer.
    pub fn expand_customisation_by_mod(&mut self) {
        let (x, y) = self.pointer.unwrap_or(self.anchor);
        self.open_customisation_at(x, y);
    }

    pub fn customisation(&self) -> &Customisation {
        &self.customise
    }

    /// Whether the tile `index` of `column` is active.
    pub fn is_active(&self, column: usize, index: usize) -> bool {
        index < MAX_COLUMN_MODS && self.active.get(column).is_some_and(|a| a >> index & 1 != 0)
    }

    /// The active flags, bit `i` of column `c` for the tile `i` of that column.
    pub fn active_flags(&self) -> [u16; MAX_COLUMNS] {
        self.active
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

    /// Sets the detail of a tile; out-of-range tiles are ignored.
    pub fn set_detail(&mut self, column: usize, index: usize, detail: TileDetail) {
        if index < self.columns.get(column).map_or(0, |c| c.len()) {
            self.details[column][index] = detail;
        }
    }

    /// Sets the score multiplier of a tile's mod; out-of-range tiles are ignored.
    pub fn set_multiplier(&mut self, column: usize, index: usize, multiplier: f64) {
        if index < self.columns.get(column).map_or(0, |c| c.len()) {
            self.multipliers[column][index] = multiplier;
        }
    }

    /// Sets the score multiplier of the whole selection.
    pub fn set_total_multiplier(&mut self, multiplier: f64) {
        self.total_multiplier = multiplier;
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
            details: self.details,
            multipliers: self.multipliers,
            total_multiplier: self.total_multiplier,
            hover: self.hover,
            customise: self.customise.snapshot(),
        }
    }

    fn counts(&self) -> [usize; MAX_COLUMNS] {
        column_counts(&self.columns)
    }

    /// Applies `input` while the panel is open: it takes every key. Closing and hovering are
    /// handled here; a toggle is for the application to resolve. Does not allocate.
    pub fn input(
        &mut self,
        input: SongSelectInput,
        layout: &ModSelectLayout,
    ) -> Option<ModSelectAction> {
        use SongSelectInput as I;
        if self.customise.is_expanded() {
            // The expanded settings panel takes every key and the pointer; the tiles are
            // out of reach, as under lazer's dimmed columns.
            self.hover = None;
            if input == I::ToggleMods {
                self.close();
                return None;
            }
            if let I::PointerMove { x, y } | I::PointerPress { x, y } = input {
                self.pointer = Some((x, y));
            }
            return self.customise.input_expanded(input, &layout.customise);
        }
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
            I::ToggleCustomisation => {
                // From the keyboard: where the pointer is, or the top right without one.
                let origin = self.pointer.unwrap_or_else(|| layout.customise.origin());
                self.customise.open_at(origin);
                None
            }
            I::PointerMove { x, y } => {
                self.pointer = Some((x, y));
                self.hover = layout.tile_at(x, y, self.counts());
                None
            }
            I::PointerPress { x, y } => {
                self.pointer = Some((x, y));
                self.hover = layout.tile_at(x, y, self.counts());
                self.hover
                    .map(|(column, index)| ModSelectAction::Toggle { column, index })
            }
            I::SecondaryPress { x, y } => {
                self.pointer = Some((x, y));
                layout.tile_at(x, y, self.counts()).map(|(column, index)| {
                    ModSelectAction::Settings {
                        column,
                        index,
                        x,
                        y,
                    }
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
