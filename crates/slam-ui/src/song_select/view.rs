//! Draw side of the song select. Allocation-free once warm: row texts live in a fixed
//! pool of slots (sized for the visible rows when the window size changes), each slot
//! reuses its strings and layouts. A row that scrolls into view is written into the slot
//! `index % pool size` with `clear` and `push_str`, so a steady scroll allocates only when
//! a text is longer than any before it in that slot (buffers grow) and never frees on the
//! draw thread. A new list generation only marks the slots stale. Row texts are cut to the
//! row's width with an ellipsis, which bounds the glyphs per row.

use std::fmt::Write as _;
use std::sync::Arc;

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey, push_layout};

use super::mod_select::{
    CustomisationState, MAX_COLUMNS, MAX_SETTING_ROWS, SettingRowKind, TileDetail, TileDetails,
};
use super::{
    BeatmapDetails, DetailStats, ModColumns, ModSelectSnapshot, Row, SearchQuery, SongList,
    SongSelectLayout, SongSelectSnapshot, SongSelectStatus, can_choose_folder,
};
use crate::menu::Rect;
use crate::menu::view::{Frame, Line, Typesetter};
use crate::screen::Pose;
use crate::theme::{Theme, faded};
use crate::widgets::ToggleLayout;

const ELLIPSIS: char = '\u{2026}';
/// Bytes reserved up front in every text buffer.
const TEXT_CAPACITY: usize = 256;
/// How much of its colour a dimmed element keeps: the mod tiles under the expanded settings
/// panel, an inactive panel header.
const DIMMED: f32 = 0.35;
/// What the settings values are made of; asked for up front.
const VALUE_CHARS: &str = "0123456789.-x";

/// The screen's localised texts, already formatted.
#[derive(Clone)]
pub struct SongSelectLabels {
    pub no_folder: String,
    pub failed: String,
    pub loading: String,
    pub empty: String,
    /// The button shown when there is nothing to play.
    pub choose_folder: String,
    pub hint: String,
    /// Shown in the search line while the query is empty.
    pub search_placeholder: String,
    /// Shown instead of the rows when the query matches nothing.
    pub search_no_matches: String,
    /// The mod select panel's title; also prefixes the selected mods shown under the list.
    pub mods_title: String,
    pub mods_hint: String,
    /// The header of the settings panel.
    pub mods_customise: String,
    /// The texts of the settings panel by Fluent key: section headers (mod names), setting
    /// labels and the names of choice values. A key a row uses and this lacks draws nothing.
    pub setting_texts: Vec<(&'static str, String)>,
    pub mod_column_titles: [String; MAX_COLUMNS],
    /// The acronyms of the panel's columns: the same as [`super::ModSelect::new`] gets.
    pub mod_columns: ModColumns,
    /// The names before the selected difficulty's details.
    pub details: DetailLabels,
}

/// The names of the selected difficulty's details (ADR-0067).
#[derive(Clone, Debug, Default)]
pub struct DetailLabels {
    pub length: String,
    pub bpm: String,
    pub circles: String,
    pub sliders: String,
    pub spinners: String,
    pub circle_size: String,
    pub approach_rate: String,
    pub overall_difficulty: String,
    pub drain_rate: String,
}

/// What the details' values are made of; asked for up front.
const DETAIL_CHARS: &str = "0123456789.:-() \u{b7}";

/// Separates the details of one line.
const DETAIL_SEPARATOR: &str = "  \u{b7}  ";

impl std::fmt::Debug for SongSelectLabels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SongSelectLabels")
            .field("hint", &self.hint)
            .finish_non_exhaustive()
    }
}

/// Texts of one row: a set has both lines, a difficulty only the first.
struct RowSlot {
    /// Generation and row the texts are for; `None` when stale.
    holds: Option<(u64, Row)>,
    primary: Line,
    secondary: Line,
}

impl RowSlot {
    fn new() -> RowSlot {
        let line = || {
            let mut line = Line::new("");
            line.text.reserve(TEXT_CAPACITY);
            line
        };
        RowSlot {
            holds: None,
            primary: line(),
            secondary: line(),
        }
    }
}

/// The text of one visible settings row's number, laid out again only when it changes.
struct ValueSlot {
    shown: Option<(u64, u8)>,
    line: Line,
}

/// The draw side of [`super::SongSelect`].
pub struct SongSelectView {
    theme: Theme,
    type_setter: Typesetter,
    frame: Frame,
    layout: SongSelectLayout,
    no_folder: Line,
    scanning: Line,
    /// The scanning text last copied into `scanning`.
    scanning_seen: Arc<str>,
    /// The first `scanning_seen`, kept so that replacing it frees nothing in a frame.
    _scanning_initial: Arc<str>,
    failed: Line,
    loading: Line,
    empty: Line,
    choose_folder: Line,
    hint: Line,
    /// The query, for the query it was laid out for.
    search: Line,
    search_for: Option<SearchQuery>,
    search_placeholder: Line,
    search_no_matches: Line,
    mods_title: Line,
    mods_hint: Line,
    mod_column_titles: [Line; MAX_COLUMNS],
    customise_title: Line,
    setting_lines: Vec<(&'static str, Line)>,
    value_slots: Vec<ValueSlot>,
    value_charset: Line,
    /// The acronym of every tile, by column.
    mod_tiles: Vec<Vec<Line>>,
    /// "Mods: HD DT" under the list, for the flags it was written for.
    mods_selected: Line,
    mods_selected_for: Option<([u16; MAX_COLUMNS], TileDetails)>,
    mod_columns: ModColumns,
    slots: Vec<RowSlot>,
    /// Widths of the ellipsis at the title and detail sizes.
    ellipsis: [f32; 2],
    detail_labels: DetailLabels,
    /// Length, BPM and objects; then the difficulty: for the details they were written for.
    detail_lines: [Line; 2],
    details_for: Option<Option<BeatmapDetails>>,
    detail_charset: Line,
}

impl std::fmt::Debug for SongSelectView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SongSelectView")
            .field("slots", &self.slots.len())
            .finish_non_exhaustive()
    }
}

/// Lays `line` out at `size`, cutting its text with an ellipsis if it is wider than
/// `avail`. Reuses the line's buffers.
pub(crate) fn fit(
    type_setter: &mut Typesetter,
    line: &mut Line,
    size: f32,
    avail: f32,
    ellipsis: f32,
) {
    type_setter.lay_out(line, size);
    let total = line.layout.width();
    if total <= avail {
        return;
    }
    let limit = avail - ellipsis;
    let glyphs = line.layout.glyphs();
    let cut = glyphs
        .iter()
        .enumerate()
        .find(|(i, _)| glyphs.get(i + 1).map_or(total, |next| next.x) > limit)
        .map_or(line.text.len(), |(_, g)| g.cluster as usize);
    let mut cut = cut.min(line.text.len());
    while !line.text.is_char_boundary(cut) {
        cut -= 1;
    }
    line.text.truncate(cut);
    line.text.push(ELLIPSIS);
    type_setter.lay_out(line, size);
}

/// Like [`fit`], but keeps the end of the text and cuts its start: the search line shows
/// the letters just typed.
fn fit_tail(type_setter: &mut Typesetter, line: &mut Line, size: f32, avail: f32, ellipsis: f32) {
    type_setter.lay_out(line, size);
    let total = line.layout.width();
    if total <= avail {
        return;
    }
    let limit = avail - ellipsis;
    let cut = line
        .layout
        .glyphs()
        .iter()
        .find(|g| total - g.x <= limit)
        .map_or(line.text.len(), |g| g.cluster as usize);
    let mut cut = cut.min(line.text.len());
    while !line.text.is_char_boundary(cut) {
        cut += 1;
    }
    // In place: a query of at most `QUERY_CAPACITY` bytes and the ellipsis fit the line's
    // `TEXT_CAPACITY`.
    let mut buf = [0; 4];
    line.text
        .replace_range(..cut, ELLIPSIS.encode_utf8(&mut buf));
    type_setter.lay_out(line, size);
}

impl SongSelectView {
    /// A view with the fonts of `registry`. Lays the static texts out once: call it off
    /// the hot paths.
    pub fn new(
        registry: Arc<FontRegistry>,
        theme: Theme,
        labels: &SongSelectLabels,
    ) -> SongSelectView {
        let (width, height) = (1280.0, theme.reference_height);
        let initial: Arc<str> = Arc::from("");
        let mut view = SongSelectView {
            type_setter: Typesetter::new(registry, theme.font_weight),
            frame: Frame { width, height },
            layout: SongSelectLayout::new(width, height, &theme),
            no_folder: Line::new(&labels.no_folder),
            scanning: Line::new(""),
            scanning_seen: Arc::clone(&initial),
            _scanning_initial: initial,
            failed: Line::new(&labels.failed),
            loading: Line::new(&labels.loading),
            empty: Line::new(&labels.empty),
            choose_folder: Line::new(&labels.choose_folder),
            hint: Line::new(&labels.hint),
            search: Line::new(""),
            search_for: None,
            search_placeholder: Line::new(&labels.search_placeholder),
            search_no_matches: Line::new(&labels.search_no_matches),
            mods_title: Line::new(&labels.mods_title),
            mods_hint: Line::new(&labels.mods_hint),
            mod_column_titles: std::array::from_fn(|i| Line::new(&labels.mod_column_titles[i])),
            customise_title: Line::new(&labels.mods_customise),
            setting_lines: labels
                .setting_texts
                .iter()
                .map(|(key, text)| (*key, Line::new(text)))
                .collect(),
            value_slots: (0..MAX_SETTING_ROWS)
                .map(|_| {
                    let mut line = Line::new("");
                    line.text.reserve(TEXT_CAPACITY);
                    ValueSlot { shown: None, line }
                })
                .collect(),
            value_charset: Line::new(VALUE_CHARS),
            mod_tiles: labels
                .mod_columns
                .iter()
                .map(|c| c.iter().map(|a| Line::new(a)).collect())
                .collect(),
            mods_selected: Line::new(""),
            mods_selected_for: None,
            mod_columns: labels.mod_columns,
            slots: Vec::new(),
            ellipsis: [0.0; 2],
            detail_labels: labels.details.clone(),
            detail_lines: std::array::from_fn(|_| {
                let mut line = Line::new("");
                line.text.reserve(TEXT_CAPACITY);
                line
            }),
            details_for: None,
            detail_charset: Line::new(DETAIL_CHARS),
            theme,
        };
        view.scanning.text.reserve(TEXT_CAPACITY);
        view.search.text.reserve(TEXT_CAPACITY);
        view.mods_selected.text.reserve(TEXT_CAPACITY);
        view.lay_out();
        view
    }

    fn lay_out(&mut self) {
        let Frame { width, height } = self.frame;
        self.layout = SongSelectLayout::new(width, height, &self.theme);
        let scale = self.layout.scale;
        let status = self.theme.song_status_size * scale;
        for line in [
            &mut self.no_folder,
            &mut self.scanning,
            &mut self.failed,
            &mut self.loading,
            &mut self.empty,
            &mut self.choose_folder,
        ] {
            self.type_setter.lay_out(line, status);
        }
        self.type_setter
            .lay_out(&mut self.hint, self.theme.song_detail_size * scale);
        self.type_setter.lay_out(
            &mut self.detail_charset,
            self.theme.song_detail_size * scale,
        );
        self.details_for = None;
        for line in [&mut self.search_placeholder, &mut self.search_no_matches] {
            self.type_setter
                .lay_out(line, self.theme.song_title_size * scale);
        }
        self.search_for = None;
        let detail = self.theme.song_detail_size * scale;
        self.type_setter.lay_out(&mut self.mods_hint, detail);
        self.type_setter
            .lay_out(&mut self.mods_title, self.theme.song_status_size * scale);
        for line in &mut self.mod_column_titles {
            self.type_setter.lay_out(line, detail);
        }
        for line in self
            .setting_lines
            .iter_mut()
            .map(|(_, line)| line)
            .chain([&mut self.customise_title, &mut self.value_charset])
        {
            self.type_setter.lay_out(line, detail);
        }
        for slot in &mut self.value_slots {
            slot.shown = None;
        }
        for line in self.mod_tiles.iter_mut().flatten() {
            self.type_setter
                .lay_out(line, self.theme.song_title_size * scale);
        }
        self.mods_selected_for = None;
        // The ellipsis widths and the row slots depend on the scale and the window.
        let mut probe = Line::new(&ELLIPSIS.to_string());
        for (width, size) in self
            .ellipsis
            .iter_mut()
            .zip([self.theme.song_title_size, self.theme.song_detail_size])
        {
            self.type_setter.lay_out(&mut probe, size * scale);
            *width = probe.layout.width();
        }
        let count = self.layout.viewport_rows().ceil() as usize + 2;
        self.slots.clear();
        self.slots.resize_with(count, RowSlot::new);
    }

    /// Asks `glyphs` for every glyph of the static texts and of the rows laid out so far.
    /// Returns how many are not ready yet; repeat with [`GlyphCache::process`] in between.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        let statics = [
            &self.no_folder,
            &self.scanning,
            &self.failed,
            &self.loading,
            &self.empty,
            &self.choose_folder,
            &self.hint,
            &self.search,
            &self.search_placeholder,
            &self.search_no_matches,
            &self.mods_title,
            &self.mods_hint,
            &self.mods_selected,
        ];
        let mods = self
            .mod_column_titles
            .iter()
            .chain(self.mod_tiles.iter().flatten())
            .chain([&self.customise_title, &self.value_charset])
            .chain(&self.detail_lines)
            .chain([&self.detail_charset])
            .chain(self.setting_lines.iter().map(|(_, line)| line))
            .chain(self.value_slots.iter().map(|slot| &slot.line));
        let rows = self
            .slots
            .iter()
            .flat_map(|slot| [&slot.primary, &slot.secondary]);
        statics
            .into_iter()
            .chain(mods)
            .chain(rows)
            .flat_map(|line| line.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count()
    }

    /// Writes `row`'s texts into `slot` unless it already holds them.
    #[allow(clippy::too_many_arguments)]
    fn fill(
        slot: &mut RowSlot,
        type_setter: &mut Typesetter,
        theme: &Theme,
        layout: &SongSelectLayout,
        ellipsis: [f32; 2],
        generation: u64,
        list: &SongList,
        row: Row,
    ) -> bool {
        if slot.holds == Some((generation, row)) {
            return true;
        }
        let pad = theme.song_text_pad * layout.scale;
        let (indent, set) = match row {
            Row::Set(set) => (0.0, set),
            Row::Difficulty { set, .. } => (layout.indent, set),
        };
        let Some(set_row) = list.sets.get(set) else {
            return false;
        };
        let avail = (layout.list.width - indent - 2.0 * pad).max(0.0);
        slot.holds = None;
        slot.primary.text.clear();
        slot.secondary.text.clear();
        match row {
            Row::Set(_) => {
                let _ = write!(slot.primary.text, "{} - {}", set_row.artist, set_row.title);
                slot.secondary.text.push_str(&set_row.creator);
            }
            Row::Difficulty { difficulty, .. } => {
                let Some(d) = set_row.difficulties.get(difficulty) else {
                    return false;
                };
                slot.primary.text.push_str(&d.version);
            }
        }
        let scale = layout.scale;
        fit(
            type_setter,
            &mut slot.primary,
            theme.song_title_size * scale,
            avail,
            ellipsis[0],
        );
        fit(
            type_setter,
            &mut slot.secondary,
            theme.song_detail_size * scale,
            avail,
            ellipsis[1],
        );
        slot.holds = Some((generation, row));
        true
    }

    /// Writes the screen at `pose` on a `width` x `height` window: row rectangles into
    /// `shapes` (tinting `white`, an opaque white image) and texts into `text` (drawn with
    /// the MSDF shader). Allocation-free while the same rows are in view; see the module
    /// docs.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut SpriteBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        width: f32,
        height: f32,
        snapshot: &SongSelectSnapshot,
        pose: Pose,
        white: AtlasRegion,
    ) {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
        if matches!(snapshot.status, SongSelectStatus::Scanning { .. })
            && !Arc::ptr_eq(&self.scanning_seen, &snapshot.scanning_text)
        {
            self.scanning_seen = Arc::clone(&snapshot.scanning_text);
            self.scanning.text.clear();
            self.scanning.text.push_str(&self.scanning_seen);
            let size = self.theme.song_status_size * self.layout.scale;
            self.type_setter.lay_out(&mut self.scanning, size);
        }

        self.refresh_selected(&snapshot.mods);
        self.refresh_search(&snapshot.query);
        self.refresh_details(&snapshot.details);
        let shift = pose.offset * width;
        let opacity = pose.opacity;
        let theme = &self.theme;
        let layout = &self.layout;
        let pad = theme.song_text_pad * layout.scale;
        let gap = theme.song_row_gap * layout.scale;
        let list = &*snapshot.list;

        let mut rect = |rect: &Rect, colour: Color| {
            let mut sprite = Sprite::new(
                white.texture,
                white.uv,
                [rect.x + shift, rect.y],
                [rect.width, rect.height],
            );
            sprite.colour = faded(colour, opacity);
            shapes.push(&sprite);
        };
        let mut left = |line: &Line, x: f32, centre_y: f32, colour: Color| {
            let position = [x + shift, centre_y - line.layout.height() / 2.0];
            push_layout(text, &line.layout, position, faded(colour, opacity), glyphs);
        };

        if snapshot.mods.open {
            let full = Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            };
            rect(&full, theme.mod_backdrop_fill);
            let panel = &layout.mods;
            let centred = |line: &Line, area: &Rect| {
                (
                    area.x + (area.width - line.layout.width()) / 2.0,
                    area.y + area.height / 2.0,
                )
            };
            let (x, y) = centred(&self.mods_title, &panel.title);
            left(&self.mods_title, x, y, theme.text);
            let (x, y) = centred(&self.mods_hint, &panel.hint);
            left(&self.mods_hint, x, y, theme.disabled_text);
            let custom = &snapshot.mods.customise;
            let expanded = custom.state != CustomisationState::Collapsed;
            let dim = if expanded { DIMMED } else { 1.0 };
            for (column, titles) in self.mod_column_titles.iter().enumerate() {
                let (x, y) = centred(titles, &panel.column_titles[column]);
                left(titles, x, y, faded(theme.disabled_text, dim));
                for (index, line) in self.mod_tiles[column].iter().enumerate() {
                    let area = panel.tile_rect(column, index);
                    let on = snapshot.mods.active[column] >> index & 1 != 0;
                    let fill = if on {
                        theme.widget_accent_fill
                    } else if snapshot.mods.hover == Some((column, index)) {
                        theme.button_hover_fill
                    } else {
                        theme.button_fill
                    };
                    rect(&area, faded(fill, dim));
                    let (x, y) = centred(line, &area);
                    left(line, x, y, faded(theme.text, dim));
                }
            }
            // The settings panel: its header, and the rows while expanded.
            let cl = &panel.customise;
            let enabled = custom.is_enabled();
            let header_fill = if custom.header_hover || expanded {
                theme.button_hover_fill
            } else {
                theme.button_fill
            };
            let active = if enabled { 1.0 } else { DIMMED };
            rect(&cl.header, faded(header_fill, active));
            left(
                &self.customise_title,
                cl.header.x + cl.pad,
                cl.header.y + cl.header.height / 2.0,
                faded(theme.text, active),
            );
            if expanded {
                rect(&cl.content(custom.len), theme.widget_popup_fill);
                let text_of = |key: &str| {
                    self.setting_lines
                        .iter()
                        .find_map(|(k, line)| (*k == key).then_some(line))
                };
                for slot in 0..cl.visible_rows(custom.len) {
                    let index = custom.first + slot;
                    let Some(row) = custom.rows.get(index).filter(|_| index < custom.len) else {
                        break;
                    };
                    let area = cl.row_rect(slot);
                    let label = text_of(row.label);
                    if matches!(row.kind, SettingRowKind::Header) {
                        rect(&area, theme.button_fill);
                        if let Some(line) = label {
                            left(
                                line,
                                area.x + cl.pad,
                                area.y + area.height / 2.0,
                                theme.text,
                            );
                        }
                        continue;
                    }
                    if index == custom.selected {
                        rect(&area, theme.button_hover_fill);
                    } else if custom.hover == Some(index) {
                        rect(&area, theme.row_hover_fill);
                    }
                    let name = cl.label_rect(&area);
                    if let Some(line) = label {
                        left(line, name.x, name.y + name.height / 2.0, theme.text);
                    }
                    let control = cl.control_rect(&area);
                    match row.kind {
                        SettingRowKind::Header => {}
                        SettingRowKind::Number {
                            value,
                            min,
                            max,
                            decimals,
                            unset,
                            ..
                        } => {
                            let sl = cl.slider(&area);
                            let fraction = if max > min {
                                ((value - min) / (max - min)).clamp(0.0, 1.0) as f32
                            } else {
                                0.0
                            };
                            let tint = if unset { DIMMED } else { 1.0 };
                            rect(&sl.track, theme.button_fill);
                            let filled = Rect {
                                width: sl.track.width * fraction,
                                ..sl.track
                            };
                            rect(&filled, faded(theme.widget_accent_fill, tint));
                            let knob = (
                                theme.widget_knob_width * cl.scale,
                                theme.widget_knob_height * cl.scale,
                            );
                            rect(
                                &Rect {
                                    x: filled.x + filled.width - knob.0 / 2.0,
                                    y: sl.bounds.y + (sl.bounds.height - knob.1) / 2.0,
                                    width: knob.0,
                                    height: knob.1,
                                },
                                faded(theme.text, tint),
                            );
                            let shown = &mut self.value_slots[slot];
                            let key = (value.to_bits(), decimals);
                            if shown.shown != Some(key) {
                                shown.shown = Some(key);
                                shown.line.text.clear();
                                let _ = write!(
                                    shown.line.text,
                                    "{value:.decimals$}{suffix}",
                                    decimals = usize::from(decimals),
                                    suffix = row.suffix
                                );
                                self.type_setter.lay_out(
                                    &mut shown.line,
                                    theme.song_detail_size * layout.scale,
                                );
                            }
                            let colour = if unset {
                                theme.disabled_text
                            } else {
                                theme.text
                            };
                            left(
                                &shown.line,
                                sl.value.x + sl.value.width - shown.line.layout.width(),
                                sl.value.y + sl.value.height / 2.0,
                                colour,
                            );
                        }
                        SettingRowKind::Toggle { value } => {
                            let sw = ToggleLayout::new(control, theme, cl.scale);
                            let fill = if value {
                                theme.widget_accent_fill
                            } else {
                                theme.button_fill
                            };
                            rect(&sw.track, fill);
                            let pad = theme.widget_knob_pad * cl.scale;
                            let size = sw.track.height - 2.0 * pad;
                            let travel = sw.track.width - 2.0 * pad - size;
                            rect(
                                &Rect {
                                    x: sw.track.x + pad + if value { travel } else { 0.0 },
                                    y: sw.track.y + pad,
                                    width: size,
                                    height: size,
                                },
                                theme.text,
                            );
                        }
                        SettingRowKind::Choice {
                            index: chosen,
                            count,
                            values,
                        } => {
                            rect(&control, theme.button_fill);
                            let name = chosen
                                .filter(|&i| i < count.min(values.len()))
                                .and_then(|i| text_of(values[i]));
                            if let Some(line) = name {
                                let (x, y) = centred(line, &control);
                                left(line, x, y, theme.text);
                            }
                        }
                    }
                }
            }
            return;
        }
        // The panel's backing, from the status line to the hints.
        let inset = theme.song_margin * layout.scale / 2.0;
        rect(
            &Rect {
                x: layout.header.x - inset,
                y: layout.header.y - inset,
                width: layout.header.width + 2.0 * inset,
                height: layout.hint.y + layout.hint.height + 2.0 * inset - layout.header.y,
            },
            theme.song_panel_fill,
        );
        let status_line = match snapshot.status {
            SongSelectStatus::NoFolder => Some(&self.no_folder),
            SongSelectStatus::Scanning { .. } => Some(&self.scanning),
            SongSelectStatus::Failed => Some(&self.failed),
            SongSelectStatus::Loading => Some(&self.loading),
            SongSelectStatus::Ready => None,
        };
        if let Some(line) = status_line {
            left(
                line,
                layout.header.x,
                layout.header.y + layout.header.height / 2.0,
                theme.text,
            );
        }
        // The selected difficulty's details, over the search line; the status line takes
        // the first one's place.
        let line_height = layout.details.height / 2.0;
        let from = usize::from(status_line.is_some());
        for (i, line) in self.detail_lines.iter().enumerate().skip(from) {
            let centre = layout.details.y + line_height * (i as f32 + 0.5);
            left(line, layout.details.x, centre, theme.text);
        }
        // The search line.
        rect(&layout.search, theme.song_search_fill);
        let line = if snapshot.query.is_empty() {
            (&self.search_placeholder, theme.disabled_text)
        } else {
            (&self.search, theme.text)
        };
        left(
            line.0,
            layout.search.x + pad,
            layout.search.y + layout.search.height / 2.0,
            line.1,
        );
        if list.sets.is_empty() {
            if snapshot.status == SongSelectStatus::Ready {
                left(
                    &self.empty,
                    layout.list.x,
                    layout.list.y + layout.row_height / 2.0,
                    theme.disabled_text,
                );
            }
            if can_choose_folder(list, snapshot.status) {
                let button = &layout.choose_folder;
                let fill = if snapshot.folder_hovered {
                    theme.button_hover_fill
                } else {
                    theme.button_fill
                };
                rect(button, fill);
                left(
                    &self.choose_folder,
                    button.x + pad,
                    button.y + button.height / 2.0,
                    theme.text,
                );
            }
        } else {
            if snapshot.row_count == 0 {
                left(
                    &self.search_no_matches,
                    layout.list.x,
                    layout.list.y + layout.row_height / 2.0,
                    theme.disabled_text,
                );
            }
            let rows = snapshot.rows();
            for index in layout.visible_rows(snapshot.scroll, snapshot.row_count) {
                let Some(row) = index
                    .checked_sub(snapshot.first_row)
                    .and_then(|i| rows.get(i))
                    .copied()
                else {
                    continue;
                };
                let slot_count = self.slots.len();
                let slot = &mut self.slots[index % slot_count];
                if !Self::fill(
                    slot,
                    &mut self.type_setter,
                    theme,
                    layout,
                    self.ellipsis,
                    snapshot.generation,
                    list,
                    row,
                ) {
                    continue;
                }
                let row_text = &*slot;
                let indent = match row {
                    Row::Set(_) => 0.0,
                    Row::Difficulty { .. } => layout.indent,
                };
                let area = Rect {
                    x: layout.list.x + indent,
                    y: layout.row_y(index, snapshot.scroll),
                    width: layout.list.width - indent,
                    height: layout.row_height - gap,
                };
                // Rows scrolled part-way out are cut at the list's edges, and their texts
                // show only whole, so nothing spills over the search line or the hints.
                let (top_edge, bottom_edge) = (layout.list.y, layout.list.y + layout.list.height);
                let clipped_top = area.y.max(top_edge);
                let clipped_bottom = (area.y + area.height).min(bottom_edge);
                if clipped_bottom <= clipped_top {
                    continue;
                }
                let inside = |line: &Line, centre_y: f32| {
                    let half = line.layout.height() / 2.0;
                    centre_y - half >= top_edge && centre_y + half <= bottom_edge
                };
                let selected = matches!(
                    row,
                    Row::Difficulty { set, difficulty } if snapshot.selected == Some((set, difficulty))
                );
                let fill = if selected {
                    theme.button_hover_fill
                } else if snapshot.hovered == Some(row) {
                    theme.row_hover_fill
                } else {
                    theme.button_fill
                };
                rect(
                    &Rect {
                        y: clipped_top,
                        height: clipped_bottom - clipped_top,
                        ..area
                    },
                    fill,
                );
                let x = area.x + pad;
                if matches!(row, Row::Set(_)) {
                    let (top, bottom) = (
                        row_text.primary.layout.height(),
                        row_text.secondary.layout.height(),
                    );
                    let start = area.y + (area.height - top - bottom) / 2.0;
                    let (primary, secondary) = (start + top / 2.0, start + top + bottom / 2.0);
                    if inside(&row_text.primary, primary) {
                        left(&row_text.primary, x, primary, theme.text);
                    }
                    if inside(&row_text.secondary, secondary) {
                        left(&row_text.secondary, x, secondary, theme.disabled_text);
                    }
                } else {
                    let centre = area.y + area.height / 2.0;
                    if inside(&row_text.primary, centre) {
                        left(&row_text.primary, x, centre, theme.text);
                    }
                }
            }
        }
        left(
            &self.hint,
            layout.hint.x,
            layout.hint.y + layout.hint.height / 2.0,
            theme.song_hint_text,
        );
        // The selection, left of the list panel.
        if snapshot.mods.active != [0; MAX_COLUMNS] {
            let margin = theme.song_margin * layout.scale;
            left(
                &self.mods_selected,
                margin,
                layout.hint.y + layout.hint.height / 2.0,
                theme.text,
            );
        }
    }

    /// Writes the details' two lines unless they already show `details`.
    fn refresh_details(&mut self, details: &Option<BeatmapDetails>) {
        if self.details_for == Some(*details) {
            return;
        }
        self.details_for = Some(*details);
        let labels = &self.detail_labels;
        let [stats_line, difficulty_line] = &mut self.detail_lines;
        stats_line.text.clear();
        difficulty_line.text.clear();
        if let Some(d) = details {
            if let Some(s) = d.stats {
                write_stats(&mut stats_line.text, labels, &s);
            }
            let values = [
                (&labels.circle_size, d.circle_size),
                (&labels.approach_rate, d.approach_rate),
                (&labels.overall_difficulty, d.overall_difficulty),
                (&labels.drain_rate, d.drain_rate),
            ];
            for (i, (label, value)) in values.into_iter().enumerate() {
                if i > 0 {
                    difficulty_line.text.push_str(DETAIL_SEPARATOR);
                }
                let _ = write!(difficulty_line.text, "{label} ");
                push_difficulty(&mut difficulty_line.text, value);
            }
        }
        let scale = self.layout.scale;
        let avail = self.layout.details.width;
        for line in &mut self.detail_lines {
            fit(
                &mut self.type_setter,
                line,
                self.theme.song_detail_size * scale,
                avail,
                self.ellipsis[1],
            );
        }
    }

    /// Lays the query out unless it already shows it.
    fn refresh_search(&mut self, query: &SearchQuery) {
        if self.search_for == Some(*query) {
            return;
        }
        self.search_for = Some(*query);
        let scale = self.layout.scale;
        let pad = self.theme.song_text_pad * scale;
        self.search.text.clear();
        self.search.text.push_str(query.as_str());
        fit_tail(
            &mut self.type_setter,
            &mut self.search,
            self.theme.song_title_size * scale,
            (self.layout.search.width - 2.0 * pad).max(0.0),
            self.ellipsis[0],
        );
    }

    /// Writes "Mods: HD DT 1.05x" for the active tiles and their details unless it already
    /// shows them.
    fn refresh_selected(&mut self, mods: &ModSelectSnapshot) {
        let key = (mods.active, mods.details);
        if self.mods_selected_for == Some(key) {
            return;
        }
        self.mods_selected_for = Some(key);
        let line = &mut self.mods_selected;
        line.text.clear();
        line.text.push_str(&self.mods_title.text);
        line.text.push(':');
        for (column, acronyms) in self.mod_columns.iter().enumerate() {
            for (index, acronym) in acronyms.iter().enumerate() {
                if mods.active[column] >> index & 1 != 0 {
                    line.text.push(' ');
                    line.text.push_str(acronym);
                    push_detail(&mut line.text, mods.details[column][index]);
                }
            }
        }
        let size = self.theme.song_detail_size * self.layout.scale;
        self.type_setter.lay_out(line, size);
    }
}

/// Appends a tile's detail after its acronym: " 1.05x", " AR9.4", " AR10" (lazer's
/// `ToStandardFormattedString(1)` drops a zero decimal).
fn push_detail(text: &mut String, detail: TileDetail) {
    // Writing to a `String` cannot fail.
    let _ = match detail {
        TileDetail::None => Ok(()),
        TileDetail::Speed(hundredths) => write!(text, " {:.2}x", f64::from(hundredths) / 100.0),
        TileDetail::ApproachRate(tenths) if tenths % 10 == 0 => write!(text, " AR{}", tenths / 10),
        TileDetail::ApproachRate(tenths) => write!(text, " AR{:.1}", f64::from(tenths) / 10.0),
    };
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Extensions/TimeDisplayExtensions.cs (ToFormattedDuration)
/// `mm:ss`, `hh:mm:ss` from an hour, `dd:hh:mm:ss` from a day; seconds cut, as `TimeSpan`.
fn push_duration(text: &mut String, ms: u64) {
    let seconds = ms / 1000;
    let (days, hours, minutes, seconds) = (
        seconds / 86_400,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
    );
    let _ = if days > 0 {
        write!(text, "{days:02}:{hours:02}:{minutes:02}:{seconds:02}")
    } else if hours > 0 {
        write!(text, "{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        write!(text, "{minutes:02}:{seconds:02}")
    };
}

/// Formats into a fixed buffer, so that reading a float's digits does not allocate.
struct StackText {
    bytes: [u8; 32],
    len: usize,
}

impl std::fmt::Write for StackText {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len + s.len();
        let slot = self.bytes.get_mut(self.len..end).ok_or(std::fmt::Error)?;
        slot.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// A difficulty value as lazer's `float.ToString("0.##")`: .NET rounds the float to 7
/// significant digits first, then those digits to two decimals half away from zero, and
/// drops trailing zeros.
fn push_difficulty(text: &mut String, value: f32) {
    let mut digits = StackText {
        bytes: [0; 32],
        len: 0,
    };
    // "d.dddddde-N": seven significant digits, correctly rounded.
    if !value.is_finite() || write!(digits, "{:.6e}", value.abs()).is_err() {
        return;
    }
    let written = std::str::from_utf8(&digits.bytes[..digits.len]).unwrap_or("");
    let Some((mantissa, exponent)) = written.split_once('e') else {
        return;
    };
    let Ok(exponent) = exponent.parse::<i32>() else {
        return;
    };
    let significand = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .fold(0_i64, |n, d| n * 10 + i64::from(d - b'0'));
    // The value is `significand * 10^(exponent - 6)`; in hundredths, `10^(exponent - 4)`.
    let shift = exponent - 4;
    let hundredths = if shift >= 0 {
        significand.saturating_mul(10_i64.saturating_pow(shift as u32))
    } else if shift > -18 {
        let divisor = 10_i64.pow((-shift) as u32);
        let (q, r) = (significand / divisor, significand % divisor);
        q + i64::from(2 * r >= divisor)
    } else {
        0
    };
    if value < 0.0 && hundredths != 0 {
        text.push('-');
    }
    let (whole, part) = (hundredths / 100, hundredths % 100);
    let _ = write!(text, "{whole}");
    match part {
        0 => {}
        p if p % 10 == 0 => {
            let _ = write!(text, ".{}", p / 10);
        }
        p => {
            let _ = write!(text, ".{p:02}");
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/BeatmapTitleWedge.cs (updateLengthAndBpmStatistics)
/// "Length 03:25 · BPM 150-200 (180) · Circles 512 · Sliders 300 · Spinners 2".
fn write_stats(text: &mut String, labels: &DetailLabels, s: &DetailStats) {
    let _ = write!(text, "{} ", labels.length);
    push_duration(text, s.length_ms);
    let _ = write!(text, "{DETAIL_SEPARATOR}{} ", labels.bpm);
    let _ = if s.bpm_minimum == s.bpm_maximum {
        write!(text, "{}", s.bpm_minimum)
    } else {
        write!(
            text,
            "{}-{} ({})",
            s.bpm_minimum, s.bpm_maximum, s.bpm_most_common
        )
    };
    for (label, count) in [
        (&labels.circles, s.circles),
        (&labels.sliders, s.sliders),
        (&labels.spinners, s.spinners),
    ] {
        let _ = write!(text, "{DETAIL_SEPARATOR}{label} {count}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_follow_the_acronym() {
        let mut text = String::from("DT");
        push_detail(&mut text, TileDetail::Speed(105));
        push_detail(&mut text, TileDetail::None);
        push_detail(&mut text, TileDetail::ApproachRate(-5));
        push_detail(&mut text, TileDetail::ApproachRate(94));
        push_detail(&mut text, TileDetail::ApproachRate(100));
        push_detail(&mut text, TileDetail::ApproachRate(-100));
        assert_eq!(text, "DT 1.05x AR-0.5 AR9.4 AR10 AR-10");
    }

    #[test]
    fn long_texts_are_cut_to_the_available_width() {
        let registry = Arc::new(FontRegistry::with_builtin());
        let mut type_setter = Typesetter::new(registry, 700);
        let mut probe = Line::new(&ELLIPSIS.to_string());
        type_setter.lay_out(&mut probe, 20.0);
        let ellipsis = probe.layout.width();
        let avail = 300.0;
        let mut line = Line::new(&"W".repeat(5000));
        fit(&mut type_setter, &mut line, 20.0, avail, ellipsis);
        assert!(line.text.ends_with(ELLIPSIS));
        assert!(line.layout.width() <= avail, "{}", line.layout.width());
        assert!(line.layout.width() > avail / 2.0);
        assert!(line.layout.glyphs().len() < 60);
        // Text that fits stays as it is.
        let mut short = Line::new("short");
        fit(&mut type_setter, &mut short, 20.0, avail, ellipsis);
        assert_eq!(short.text, "short");
        // Multi-byte text is cut at a character boundary.
        let mut wide = Line::new(&"\u{44b}".repeat(2000));
        fit(&mut type_setter, &mut wide, 20.0, avail, ellipsis);
        assert!(wide.text.ends_with(ELLIPSIS));
        assert!(wide.layout.width() <= avail);
    }

    #[test]
    fn long_queries_keep_their_end() {
        let registry = Arc::new(FontRegistry::with_builtin());
        let mut type_setter = Typesetter::new(registry, 700);
        let mut probe = Line::new(&ELLIPSIS.to_string());
        type_setter.lay_out(&mut probe, 20.0);
        let ellipsis = probe.layout.width();
        let avail = 300.0;
        let text = format!("{}end", "\u{44b}".repeat(60));
        let mut line = Line::new(&text);
        fit_tail(&mut type_setter, &mut line, 20.0, avail, ellipsis);
        assert!(line.text.starts_with(ELLIPSIS), "{}", line.text);
        assert!(line.text.ends_with("end"));
        assert!(line.layout.width() <= avail, "{}", line.layout.width());
        assert!(line.layout.width() > avail / 2.0);
        let mut short = Line::new("short");
        fit_tail(&mut type_setter, &mut short, 20.0, avail, ellipsis);
        assert_eq!(short.text, "short");
    }

    #[test]
    fn durations_as_lazer_formats_them() {
        let cases = [
            (0, "00:00"),
            (59_999, "00:59"),
            (205_000, "03:25"),
            (3_600_000, "01:00:00"),
            (90_061_000, "01:01:01:01"),
        ];
        for (ms, text) in cases {
            let mut s = String::new();
            push_duration(&mut s, ms);
            assert_eq!(s, text, "{ms}");
        }
    }

    #[test]
    fn difficulty_values_keep_two_decimals_at_most() {
        let cases = [
            (4.0, "4"),
            (9.3, "9.3"),
            (7.666_667, "7.67"),
            (8.005, "8.01"),
            (10.0, "10"),
            (0.05, "0.05"),
            (-0.5, "-0.5"),
            // The float rounds to 2.675000 at seven digits before the decimals: EZ on CS 5.35.
            (5.35f32 * 0.5, "2.68"),
            // HR on CS 4.25: 5.5249996 to 5.525000.
            (4.25f32 * 1.3, "5.53"),
            (12345.678, "12345.68"),
        ];
        for (value, text) in cases {
            let mut s = String::new();
            push_difficulty(&mut s, value);
            assert_eq!(s, text, "{value}");
        }
    }

    #[test]
    fn stats_show_one_bpm_or_the_range_and_the_most_common() {
        let labels = DetailLabels {
            length: "Length".into(),
            bpm: "BPM".into(),
            circles: "Circles".into(),
            sliders: "Sliders".into(),
            spinners: "Spinners".into(),
            ..DetailLabels::default()
        };
        let mut stats = DetailStats {
            length_ms: 205_400,
            bpm_minimum: 180,
            bpm_maximum: 180,
            bpm_most_common: 180,
            circles: 512,
            sliders: 300,
            spinners: 2,
        };
        let mut s = String::new();
        write_stats(&mut s, &labels, &stats);
        assert_eq!(
            s,
            "Length 03:25  \u{b7}  BPM 180  \u{b7}  Circles 512  \u{b7}  Sliders 300  \u{b7}  Spinners 2"
        );
        stats.bpm_minimum = 150;
        stats.bpm_maximum = 200;
        s.clear();
        write_stats(&mut s, &labels, &stats);
        assert!(s.contains("BPM 150-200 (180)"), "{s}");
    }
}
