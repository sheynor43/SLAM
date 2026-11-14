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
use slam_render::atlas::UvRect;
use slam_render::shape::{Shape, ShapeBatch};
use slam_render::sprite::SpriteBatch;
use slam_render::upload::TextureHandle;
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey, push_layout};

use super::info_panel::InfoPanel;
use super::mod_select::{
    CustomisationState, MAX_COLUMNS, MAX_SETTING_ROWS, SettingRowKind, TileDetail, TileDetails,
};
use super::{
    MAX_VISIBLE_ROWS, ModColumns, ModSelectSnapshot, Row, SearchQuery, SongList, SongSelectLayout,
    SongSelectSnapshot, SongSelectStatus, Thumbs, can_choose_folder,
};
use crate::menu::Rect;
use crate::menu::view::{Frame, Line, Typesetter};
use crate::screen::Pose;
use crate::theme::{Theme, faded};
use crate::widgets::Look;
use crate::widgets::ToggleLayout;

const ELLIPSIS: char = '\u{2026}';
/// The ellipsis as text.
pub(crate) const ELLIPSIS_STR: &str = "\u{2026}";
/// Bytes reserved up front in every text buffer.
pub(super) const TEXT_CAPACITY: usize = 256;
/// How much of its colour a dimmed element keeps: the mod tiles under the expanded settings
/// panel, an inactive panel header.
const DIMMED: f32 = 0.35;
/// Thumbnails whose fade-in start is remembered: the memory cache's size (ADR-0127).
pub(crate) const FADES: usize = 64;
/// How much of the accent a thumbnail's placeholder shows.
const PLACEHOLDER_ALPHA: f32 = 0.25;
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
    /// Shown instead of the hints after a play was refused for want of an audio output.
    pub no_audio: String,
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
    /// The title of the local scores list, and the line shown when there are none.
    pub scores: String,
    pub no_scores: String,
}

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

/// Width of `count` difficulty dots at the right end of a set row, with the gap before them
/// (ADR-0131); 0 for none.
fn dots_width(count: usize, theme: &Theme, scale: f32) -> f32 {
    let count = count.min(MAX_DOTS);
    if count == 0 {
        return 0.0;
    }
    let (size, gap) = (
        theme.difficulty_dot_size * scale,
        theme.difficulty_dot_gap * scale,
    );
    count as f32 * (size + gap) + gap * 2.0
}

/// The most difficulty dots a set row shows.
const MAX_DOTS: usize = 12;

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
    no_audio: Line,
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
    /// The statistics and local scores on the left.
    info: InfoPanel,
    /// When each thumbnail seen was first drawn; replaced round-robin.
    fades: [Option<(TextureHandle, u64)>; FADES],
    fade_next: usize,
    /// The thumbnails were on at the last frame: the row texts are cut for it.
    thumbs_on: bool,
}

impl std::fmt::Debug for SongSelectView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SongSelectView")
            .field("slots", &self.slots.len())
            .finish_non_exhaustive()
    }
}

/// Where a set row's thumbnail goes, relative to the row (ADR-0127).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ThumbGeometry {
    /// Gap between the row's edge and the thumbnail.
    pub inset: f32,
    pub width: f32,
    pub height: f32,
    /// How far the row's texts move right to make room.
    pub text_shift: f32,
}

impl ThumbGeometry {
    pub(crate) fn new(theme: &Theme, layout: &SongSelectLayout) -> ThumbGeometry {
        let inset = theme.song_thumbnail_inset * layout.scale;
        let row = layout.row_height - theme.song_row_gap * layout.scale;
        let height = (row - 2.0 * inset).max(0.0);
        let width = height * theme.song_thumbnail_aspect;
        ThumbGeometry {
            inset,
            width,
            height,
            text_shift: inset + width,
        }
    }

    /// The thumbnail's top and bottom for a row at `area`, cut to the list's edges; `None`
    /// when nothing of it is inside.
    pub(crate) fn span(&self, area: &Rect, top_edge: f32, bottom_edge: f32) -> Option<(f32, f32)> {
        let top = area.y + self.inset;
        let (top, bottom) = (top.max(top_edge), (top + self.height).min(bottom_edge));
        (bottom > top).then_some((top, bottom))
    }
}

/// How opaque a thumbnail is `fade_ms` after it was first drawn; remembers the first draw
/// of up to [`FADES`] thumbnails, replacing the oldest. Allocation-free.
pub(crate) fn fade_alpha(
    fades: &mut [Option<(TextureHandle, u64)>; FADES],
    next: &mut usize,
    handle: TextureHandle,
    now_ns: u64,
    fade_ms: f32,
) -> f32 {
    let start = match fades.iter().flatten().find(|(h, _)| *h == handle) {
        Some((_, start)) => *start,
        None => {
            fades[*next] = Some((handle, now_ns));
            *next = (*next + 1) % FADES;
            now_ns
        }
    };
    let elapsed_ms = now_ns.saturating_sub(start) as f32 / 1.0e6;
    if fade_ms <= 0.0 {
        1.0
    } else {
        (elapsed_ms / fade_ms).clamp(0.0, 1.0)
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
            no_audio: Line::new(&labels.no_audio),
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
            info: InfoPanel::new(&labels.details),
            fades: [None; FADES],
            fade_next: 0,
            thumbs_on: false,
            theme,
        };
        view.scanning.text.reserve(TEXT_CAPACITY);
        view.search.text.reserve(TEXT_CAPACITY);
        view.mods_selected.text.reserve(TEXT_CAPACITY);
        view.lay_out();
        view
    }

    /// Lays the texts out for a `width` × `height` window ahead of the first `push`, so that
    /// a view made off the draw thread does not lay them out there (ADR-0091). Allocates:
    /// call it off the hot paths.
    pub fn lay_out_for(&mut self, width: f32, height: f32) {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
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
        for line in [&mut self.hint, &mut self.no_audio] {
            self.type_setter
                .lay_out(line, self.theme.song_detail_size * scale);
        }
        self.info.lay_out(&mut self.type_setter, &self.theme, scale);
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
            &self.no_audio,
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
            .chain(self.info.lines())
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
        text_shift: f32,
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
        let mut avail = (layout.list.width - indent - 2.0 * pad - text_shift).max(0.0);
        slot.holds = None;
        slot.primary.text.clear();
        slot.secondary.text.clear();
        match row {
            Row::Set(_) => {
                let _ = write!(slot.primary.text, "{} - {}", set_row.artist, set_row.title);
                slot.secondary.text.push_str(&set_row.creator);
                // The difficulty dots keep the right end.
                avail =
                    (avail - dots_width(set_row.difficulties.len(), theme, layout.scale)).max(0.0);
            }
            Row::Difficulty { difficulty, .. } => {
                let Some(d) = set_row.difficulties.get(difficulty) else {
                    return false;
                };
                slot.primary.text.push_str(&d.version);
                // The star pill keeps the right end; its text is the second line.
                if let Some(stars) = d.stars {
                    super::push_stars(&mut slot.secondary.text, stars);
                    type_setter.lay_out(&mut slot.secondary, theme.song_detail_size * layout.scale);
                    avail = (avail
                        - super::info_panel::pill_width(&slot.secondary, theme, layout.scale))
                    .max(0.0);
                }
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
        if matches!(row, Row::Set(_)) {
            fit(
                type_setter,
                &mut slot.secondary,
                theme.song_detail_size * scale,
                avail,
                ellipsis[1],
            );
        }
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
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        width: f32,
        height: f32,
        snapshot: &SongSelectSnapshot,
        pose: Pose,
        white: AtlasRegion,
        thumbs: Thumbs<'_>,
    ) {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
        if thumbs.enabled != self.thumbs_on {
            // The texts of the set rows are cut to another width.
            self.thumbs_on = thumbs.enabled;
            for slot in &mut self.slots {
                slot.holds = None;
            }
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
        self.info.refresh(
            &mut self.type_setter,
            &self.theme,
            &self.layout,
            self.ellipsis,
            snapshot,
        );
        let shift = pose.offset * width;
        let opacity = pose.opacity;
        let theme = &self.theme;
        let layout = &self.layout;
        let pad = theme.song_text_pad * layout.scale;
        let gap = theme.song_row_gap * layout.scale;
        let list = &*snapshot.list;

        let scale = layout.scale;
        let mut rect = |rect: &Rect, colour: Color, look: Look| {
            let moved = Rect {
                x: rect.x + shift,
                ..*rect
            };
            let colour = faded(colour, opacity);
            crate::widgets::draw(shapes, white, &moved, colour, look, theme, scale);
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
            rect(&full, theme.mod_backdrop_fill, Look::Plain);
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
                // Each column in the colour of its mod type (ADR-0136).
                let tint = theme.mod_column_tints[column % theme.mod_column_tints.len()];
                let (x, y) = centred(titles, &panel.column_titles[column]);
                left(titles, x, y, faded(tint, dim));
                for (index, line) in self.mod_tiles[column].iter().enumerate() {
                    let area = panel.tile_rect(column, index);
                    let on = snapshot.mods.active[column] >> index & 1 != 0;
                    let fill = if !on && snapshot.mods.hover == Some((column, index)) {
                        theme.button_hover_fill
                    } else {
                        theme.button_fill
                    };
                    let look = Look::TintedButton {
                        highlighted: on,
                        seed: (column * 7 + index) as f32 * 0.31,
                        fade: opacity,
                        tint,
                    };
                    rect(&area, faded(fill, dim), look);
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
            rect(&cl.header, faded(header_fill, active), Look::Round);
            left(
                &self.customise_title,
                cl.header.x + cl.pad,
                cl.header.y + cl.header.height / 2.0,
                faded(theme.text, active),
            );
            if expanded {
                rect(
                    &cl.content(custom.len),
                    theme.widget_popup_fill,
                    Look::Panel,
                );
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
                        rect(&area, theme.button_fill, Look::Round);
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
                        rect(&area, theme.button_hover_fill, Look::Round);
                    } else if custom.hover == Some(index) {
                        rect(&area, theme.row_hover_fill, Look::Round);
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
                            rect(&sl.track, theme.button_fill, Look::Pill);
                            let filled = Rect {
                                width: sl.track.width * fraction,
                                ..sl.track
                            };
                            rect(&filled, faded(theme.widget_accent_fill, tint), Look::Pill);
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
                                Look::Pill,
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
                            rect(&sw.track, fill, Look::Pill);
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
                                Look::Pill,
                            );
                        }
                        SettingRowKind::Choice {
                            index: chosen,
                            count,
                            values,
                        } => {
                            rect(&control, theme.button_fill, Look::Round);
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
        // Everything on the panel leans with it (ADR-0121).
        let lean = |x: f32, centre_y: f32| x + layout.lean_x(centre_y);
        let leaned = |r: &Rect| Rect {
            x: lean(r.x, r.y + r.height / 2.0),
            ..*r
        };
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
                lean(
                    layout.header.x,
                    layout.header.y + layout.header.height / 2.0,
                ),
                layout.header.y + layout.header.height / 2.0,
                theme.text,
            );
        }
        // The selected difficulty's statistics and local scores, on the left.
        self.info
            .push(layout, theme, snapshot, &mut rect, &mut left);
        // The search line.
        let search = leaned(&layout.search);
        rect(&search, theme.song_search_fill, Look::Round);
        let line = if snapshot.query.is_empty() {
            (&self.search_placeholder, theme.disabled_text)
        } else {
            (&self.search, theme.text)
        };
        left(
            line.0,
            search.x + pad,
            search.y + search.height / 2.0,
            line.1,
        );
        if list.sets.is_empty() {
            if snapshot.status == SongSelectStatus::Ready {
                let y = layout.list.y + layout.row_height / 2.0;
                left(&self.empty, lean(layout.list.x, y), y, theme.disabled_text);
            }
            if can_choose_folder(list, snapshot.status) {
                let button = &leaned(&layout.choose_folder);
                let fill = if snapshot.folder_hovered {
                    theme.button_hover_fill
                } else {
                    theme.button_fill
                };
                rect(
                    button,
                    fill,
                    Look::Button {
                        highlighted: snapshot.folder_hovered,
                        seed: 0.0,
                        fade: opacity,
                    },
                );
                left(
                    &self.choose_folder,
                    button.x + pad,
                    button.y + button.height / 2.0,
                    theme.text,
                );
            }
        } else {
            if snapshot.row_count == 0 {
                let y = layout.list.y + layout.row_height / 2.0;
                left(
                    &self.search_no_matches,
                    lean(layout.list.x, y),
                    y,
                    theme.disabled_text,
                );
            }
            let rows = snapshot.rows();
            // Thumbnails are pushed after the rows: their shapes wait here.
            let mut thumb_shapes: [Option<Shape>; 2 * MAX_VISIBLE_ROWS] =
                [None; 2 * MAX_VISIBLE_ROWS];
            let mut thumb_count = 0;
            let thumb = ThumbGeometry::new(theme, layout);
            for index in layout.visible_rows(snapshot.shown_scroll, snapshot.row_count) {
                let Some(row) = index
                    .checked_sub(snapshot.first_row)
                    .and_then(|i| rows.get(i))
                    .copied()
                else {
                    continue;
                };
                let slot_count = self.slots.len();
                let slot = &mut self.slots[index % slot_count];
                let text_shift = if thumbs.enabled && matches!(row, Row::Set(_)) {
                    thumb.text_shift
                } else {
                    0.0
                };
                if !Self::fill(
                    slot,
                    &mut self.type_setter,
                    theme,
                    layout,
                    self.ellipsis,
                    snapshot.generation,
                    list,
                    row,
                    text_shift,
                ) {
                    continue;
                }
                let row_text = &*slot;
                let indent = match row {
                    Row::Set(_) => 0.0,
                    Row::Difficulty { .. } => layout.indent,
                };
                let y = layout.row_y(index, snapshot.shown_scroll);
                // The list leans along the rows' skew (ADR-0121).
                let lean = layout.lean_x(y + (layout.row_height - gap) / 2.0);
                let area = Rect {
                    x: layout.list.x + indent + lean,
                    y,
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
                // The selection is the row's wash, outline and glow, not a fill (ADR-0136).
                let fill = if !selected && snapshot.hovered == Some(row) {
                    theme.button_hover_fill
                } else {
                    theme.button_fill
                };
                let clipped = Rect {
                    y: clipped_top,
                    height: clipped_bottom - clipped_top,
                    ..area
                };
                let (set_index, stars) = match row {
                    Row::Set(set) => (set, None),
                    Row::Difficulty { set, difficulty } => (
                        set,
                        list.sets
                            .get(set)
                            .and_then(|s| s.difficulties.get(difficulty))
                            .and_then(|d| d.stars),
                    ),
                };
                let seed = index as f32 * 0.29;
                let look = match row {
                    // A difficulty glows and is outlined in its star colour (ADR-0131).
                    Row::Difficulty { .. } => Look::TintedButton {
                        highlighted: selected,
                        seed,
                        fade: opacity,
                        tint: crate::star_colour::glow_for(stars, theme.accent),
                    },
                    // A set in the colour of its hardest difficulty (ADR-0136).
                    Row::Set(set) => Look::TintedButton {
                        highlighted: selected,
                        seed,
                        fade: opacity,
                        tint: crate::star_colour::glow_for(
                            list.sets.get(set).and_then(|s| {
                                s.difficulties
                                    .iter()
                                    .filter_map(|d| d.stars)
                                    .reduce(f32::max)
                            }),
                            theme.accent,
                        ),
                    },
                };
                rect(&clipped, fill, look);
                let x = area.x + pad + text_shift;
                if thumbs.enabled
                    && matches!(row, Row::Set(_))
                    && let Some((top, bottom)) = thumb.span(&area, top_edge, bottom_edge)
                {
                    let (left_x, full_top) = (area.x + thumb.inset + shift, area.y + thumb.inset);
                    let base = Shape {
                        radius: theme.button_radius * scale,
                        skew: theme.button_skew(),
                        ..Shape::new(
                            white.texture,
                            white.uv,
                            [left_x, top],
                            [thumb.width, bottom - top],
                            faded(theme.accent, PLACEHOLDER_ALPHA * opacity),
                        )
                    };
                    thumb_shapes[thumb_count] = Some(base);
                    thumb_count += 1;
                    if let Some(image) = index
                        .checked_sub(snapshot.first_row)
                        .and_then(|i| thumbs.rows.get(i))
                        .copied()
                        .flatten()
                    {
                        let alpha = fade_alpha(
                            &mut self.fades,
                            &mut self.fade_next,
                            image.handle,
                            thumbs.now_ns,
                            theme.song_thumbnail_fade_ms,
                        );
                        let uv = image.region.uv;
                        let dv = uv.v1 - uv.v0;
                        let cut = |at: f32| uv.v0 + (at - full_top) / thumb.height * dv;
                        thumb_shapes[thumb_count] = Some(Shape {
                            colour: faded(Color::rgb(1.0, 1.0, 1.0), alpha * opacity),
                            texture: image.region.texture,
                            uv: UvRect {
                                v0: cut(top),
                                v1: cut(bottom),
                                ..uv
                            },
                            ..base
                        });
                        thumb_count += 1;
                    }
                }
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
                let centre = area.y + area.height / 2.0;
                let right = area.x + area.width - pad;
                match row {
                    Row::Difficulty { .. } => {
                        if let Some(stars) = stars
                            && inside(&row_text.secondary, centre)
                        {
                            // The star pill, in the colour of the rating.
                            let line = &row_text.secondary;
                            let width = super::info_panel::star_pill_width(line, theme, scale);
                            super::info_panel::push_star_pill(
                                &mut rect,
                                &mut left,
                                line,
                                stars,
                                theme,
                                scale,
                                right - width,
                                centre,
                            );
                        }
                    }
                    Row::Set(_) => {
                        // One dot per difficulty, in its colour, by stars.
                        let dots = list
                            .sets
                            .get(set_index)
                            .map_or(&[][..], |s| s.difficulties.as_slice());
                        let size = theme.difficulty_dot_size * scale;
                        let dot_gap = theme.difficulty_dot_gap * scale;
                        if centre - size / 2.0 >= top_edge && centre + size / 2.0 <= bottom_edge {
                            for (n, d) in dots.iter().take(MAX_DOTS).rev().enumerate() {
                                rect(
                                    &Rect {
                                        x: right - (n + 1) as f32 * (size + dot_gap) + dot_gap,
                                        y: centre - size / 2.0,
                                        width: size,
                                        height: size,
                                    },
                                    crate::star_colour::glow_for(d.stars, theme.accent),
                                    Look::Pill,
                                );
                            }
                        }
                    }
                }
            }
            for shape in thumb_shapes[..thumb_count].iter().flatten() {
                shapes.push_shape(shape);
            }
        }
        // The refused play's reason stands out from the hints it replaces.
        let (hint, colour) = if snapshot.no_audio {
            (&self.no_audio, theme.text)
        } else {
            (&self.hint, theme.song_hint_text)
        };
        left(
            hint,
            lean(layout.hint.x, layout.hint.y + layout.hint.height / 2.0),
            layout.hint.y + layout.hint.height / 2.0,
            colour,
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
}
