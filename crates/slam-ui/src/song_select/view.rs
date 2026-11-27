//! Draw side of the song select. Allocation-free once warm: row texts live in a fixed
//! pool of slots (sized for the visible rows when the window size changes), each slot
//! reuses its strings and layouts. A row that scrolls into view is written into the slot
//! `index % pool size` with `clear` and `push_str`, so a steady scroll allocates only when
//! a text is longer than any before it in that slot (buffers grow) and never frees on the
//! draw thread. A new list generation only marks the slots stale. Row texts are cut to the
//! row's width with an ellipsis, which bounds the glyphs per row.

use crate::anim::{ease_out_quint, progress};
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
    CustomisationState, MAX_COLUMNS, MAX_SETTING_ROWS, MultiplierTone, SettingRowKind, TileDetail,
    TileDetails, TileMultipliers, push_multiplier,
};
use super::{
    FooterButton, FooterState, MAX_VISIBLE_ROWS, ModColumns, ModSelectSnapshot, Row, SearchQuery,
    SongList, SongSelectLayout, SongSelectSnapshot, SongSelectStatus, Thumbs, can_choose_folder,
};
use crate::marquee::{Marquee, MarqueeParams, ScrollTracker, push_marquee};
use crate::menu::Rect;
use crate::menu::view::{Face, Frame, Line, Typesetter};
use crate::screen::Pose;
use crate::theme::{Theme, faded};
use crate::widgets::Look;
use crate::widgets::ToggleLayout;

const ELLIPSIS: char = '\u{2026}';
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
/// Room for a tile's "1.12x".
const MULTIPLIER_CAPACITY: usize = 16;

const VALUE_CHARS: &str = "0123456789.-x";

/// Where a frame's quads divide into layers drawn one over the other: each is the count of
/// shapes and of glyph quads so far (ADR-0152, ADR-0177).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Splits {
    /// Under it the list's texts; over it the corner logo (ADR-0186).
    pub logo: Option<(u32, u32)>,
    /// Under it the list and the footer; over it the mod select panel.
    pub panel: Option<(u32, u32)>,
    /// Under it the panel; over it the floating settings panel.
    pub popup: Option<(u32, u32)>,
}

/// How long the screen may go undrawn before its footer slides in again.
const FOOTER_AWAY_NS: u64 = 250_000_000;

/// The screen's localised texts, already formatted.
#[derive(Clone)]
pub struct SongSelectLabels {
    pub no_folder: String,
    pub failed: String,
    pub loading: String,
    pub empty: String,
    /// The button shown when there is nothing to play.
    pub choose_folder: String,
    /// Shown above the footer after a play was refused for want of an audio output.
    pub no_audio: String,
    /// The footer's buttons (ADR-0175): back, the pin of the current mods, reset.
    pub footer_back: String,
    pub footer_pin: String,
    pub footer_reset: String,
    /// Follows a difficulty's SS performance: "pp" in "412pp" (ADR-0151).
    pub pp_suffix: String,
    /// Shown in the search line while the query is empty.
    pub search_placeholder: String,
    /// Shown instead of the rows when the query matches nothing.
    pub search_no_matches: String,
    /// The mod select panel's title; also prefixes the selected mods shown under the list.
    pub mods_title: String,
    pub mods_hint: String,
    /// The label before the selection's score multiplier.
    pub mods_multiplier: String,
    /// The header of the settings panel.
    pub mods_customise: String,
    /// The texts of the settings panel by Fluent key: section headers (mod names), setting
    /// labels and the names of choice values. A key a row uses and this lacks draws nothing.
    pub setting_texts: Vec<(&'static str, String)>,
    pub mod_column_titles: [String; MAX_COLUMNS],
    /// Each tile's mod name and description, by column, in the order of `mod_columns`
    /// (ADR-0152); a tile without one shows its acronym only.
    pub mod_texts: [Vec<(String, String)>; MAX_COLUMNS],
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
    /// The labels before a score row's accuracy and max combo.
    pub accuracy: String,
    pub combo: String,
}

impl std::fmt::Debug for SongSelectLabels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SongSelectLabels")
            .field("no_audio", &self.no_audio)
            .finish_non_exhaustive()
    }
}

/// Texts of one row: a set has both lines, a difficulty only the first.
struct RowSlot {
    /// Generation and row the texts are for; `None` when stale.
    holds: Option<(u64, Row)>,
    primary: Line,
    secondary: Line,
    /// A difficulty's SS performance, "412pp" (ADR-0151).
    pp: Line,
    /// Hundredths of the stars and whole pp the texts show (`i32::MIN` for none), so a
    /// frame lays a text out again only when the number it rolls to changed.
    rolled_stars: i32,
    rolled_pp: i32,
    /// Room of the text that rides (the mapper of a set, the name of a difficulty), as
    /// filled.
    avail: f32,
    /// The ride of that text (ADR-0148).
    marquee: Marquee,
    /// Whether the texts' glyphs were asked of the cache since they were laid out, so a
    /// row about to scroll in has them ready.
    requested: bool,
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
            pp: line(),
            rolled_stars: i32::MIN,
            rolled_pp: i32::MIN,
            avail: 0.0,
            marquee: Marquee::default(),
            requested: false,
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
    no_audio: Line,
    /// The footer's labels, and the pinned presets' (ADR-0175).
    footer_back: Line,
    footer_pin: Line,
    footer_reset: Line,
    presets: Vec<Line>,
    /// How many of `presets` hold a label.
    preset_count: usize,
    /// When the footer started sliding in, and when the view was last drawn.
    footer_since: u64,
    /// The footer buttons' hover, press, flash and slide state (ADR-0175).
    footer_anim: super::FooterAnim,
    last_push: Option<u64>,
    /// [`SongSelectLabels::pp_suffix`].
    pp_suffix: String,
    /// The query, for the query it was laid out for.
    search: Line,
    search_for: Option<SearchQuery>,
    /// The typing feedback of the search text (ADR-0167).
    search_type: crate::typing::TypeFeedback,
    search_placeholder: Line,
    search_no_matches: Line,
    mods_title: Line,
    mods_hint: Line,
    /// "Score multiplier 1.12x" of the selection, and the multipliers shown on the tiles.
    mods_multiplier: Line,
    mods_multiplier_label: String,
    tile_multipliers: Vec<Vec<Line>>,
    /// What the multiplier lines were written for.
    multipliers_for: Option<(TileMultipliers, f64)>,
    mod_column_titles: [Line; MAX_COLUMNS],
    customise_title: Line,
    setting_lines: Vec<(&'static str, Line)>,
    value_slots: Vec<ValueSlot>,
    value_charset: Line,
    /// The acronym of every tile, by column.
    mod_tiles: Vec<Vec<Line>>,
    /// Each tile's name and description, fitted to the tile; `mod_texts` keeps them whole.
    mod_names: Vec<Vec<(Line, Line)>>,
    mod_texts: [Vec<(String, String)>; MAX_COLUMNS],
    /// "Mods: HD DT" under the list, for the flags it was written for.
    mods_selected: Line,
    mods_selected_for: Option<([u16; MAX_COLUMNS], TileDetails, u64)>,
    mod_columns: ModColumns,
    slots: Vec<RowSlot>,
    /// Widths of the ellipsis at the title and detail sizes and in the caption and note
    /// faces.
    ellipsis: [f32; 4],
    /// The statistics and local scores on the left.
    info: InfoPanel,
    /// When each thumbnail seen was first drawn; replaced round-robin.
    fades: [Option<(TextureHandle, u64)>; FADES],
    fade_next: usize,
    /// The set whose difficulties are opening, and the one closing (ADR-0136).
    expansion: Expansion,
    /// Where each row shown is drawn, easing to its place (ADR-0172).
    row_motion: RowMotion,
    /// The rows' rating colours, easing to new ratings.
    star_motion: StarMotion,
    /// The performance each difficulty row shows, rolling as its rating does.
    pp_motion: StarMotion,
    /// The mod select panel's slide in and out (ADR-0152).
    mods_slide: PanelSlide,
    shift: Shift,
    /// The thumbnails were on at the last frame: the row texts are cut for it.
    thumbs_on: bool,
    /// The list scrollbar's fades (ADR-0174).
    bar_look: crate::scrollbar::ScrollbarLook,
    /// Applies the snapshot's sideways wheel and drags to the rows' marquees.
    scroll_tracker: ScrollTracker,
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
        // Only set rows carry a thumbnail.
        let row = theme.song_set_height * layout.scale;
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

/// The part of an image a full-plate thumbnail shows (ADR-0174): the image covers the
/// shape's bounding box (`width` plus the skew's lean over `full_height`) as `cover` does,
/// centred, and the shape shows the rows `top` to `bottom` (fractions of `full_height`,
/// for a plate scrolled part-way out of the list). `uv` is the image's place in its
/// texture and `image` its size in pixels. The scale stays the same whatever is cut off.
pub(crate) fn plate_uv(
    uv: UvRect,
    image: [f32; 2],
    width: f32,
    full_height: f32,
    tan: f32,
    (top, bottom): (f32, f32),
) -> UvRect {
    let lean = tan.abs();
    let box_full = (width + lean * full_height).max(f32::EPSILON);
    let box_cut = width + lean * full_height * (bottom - top).clamp(0.0, 1.0);
    let target = box_full / full_height.max(f32::EPSILON);
    let aspect = image[0] / image[1].max(1.0);
    let (keep_u, keep_v) = if aspect >= target {
        (target / aspect.max(f32::EPSILON), 1.0)
    } else {
        (1.0, aspect / target.max(f32::EPSILON))
    };
    let du = (uv.u1 - uv.u0) * keep_u * box_cut / box_full;
    let dv = (uv.v1 - uv.v0) * keep_v;
    let (cu, cv) = ((uv.u0 + uv.u1) / 2.0, (uv.v0 + uv.v1) / 2.0);
    let v_top = cv - dv / 2.0;
    UvRect {
        u0: cu - du / 2.0,
        u1: cu + du / 2.0,
        v0: v_top + top * dv,
        v1: v_top + bottom * dv,
    }
}

/// The shape of a full-plate thumbnail over the plate `plate` (cut to the list), whose
/// whole height is `full_height` from `full_top` (ADR-0174): the plate's rounding and
/// `song_plate_dim_right` at the right, at `alpha`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plate_shape(
    image: AtlasRegion,
    plate: &Rect,
    full_top: f32,
    full_height: f32,
    shift: f32,
    alpha: f32,
    theme: &Theme,
    scale: f32,
) -> Shape {
    let tan = theme.button_skew().tan();
    let top = ((plate.y - full_top) / full_height.max(f32::EPSILON)).clamp(0.0, 1.0);
    let bottom =
        ((plate.y + plate.height - full_top) / full_height.max(f32::EPSILON)).clamp(top, 1.0);
    let dim = |level: f32| Color {
        r: level,
        g: level,
        b: level,
        a: alpha,
    };
    Shape {
        radius: theme.button_radius * scale,
        skew: theme.button_skew(),
        upright: true,
        gradient: Some(slam_render::shape::Gradient::Horizontal(dim(
            theme.song_plate_dim_right
        ))),
        texture: image.texture,
        uv: plate_uv(
            image.uv,
            [image.size.width as f32, image.size.height as f32],
            plate.width,
            full_height,
            tan,
            (top, bottom),
        ),
        ..Shape::new(
            image.texture,
            image.uv,
            [plate.x + shift, plate.y],
            [plate.width, plate.height],
            dim(theme.song_plate_dim_left),
        )
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
    let face = Face {
        size,
        weight: type_setter.weight(),
        width: slam_text::NORMAL_WIDTH,
    };
    fit_face(type_setter, line, face, avail, ellipsis);
}

/// [`fit`] in `face`; `ellipsis` is measured in the same face.
pub(crate) fn fit_face(
    type_setter: &mut Typesetter,
    line: &mut Line,
    face: Face,
    avail: f32,
    ellipsis: f32,
) {
    type_setter.lay_out_face(line, face);
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
    type_setter.lay_out_face(line, face);
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
            layout: SongSelectLayout::new(width, height, &theme)
                .with_mod_columns(&labels.mod_columns),
            no_folder: Line::new(&labels.no_folder),
            scanning: Line::new(""),
            scanning_seen: Arc::clone(&initial),
            _scanning_initial: initial,
            failed: Line::new(&labels.failed),
            loading: Line::new(&labels.loading),
            empty: Line::new(&labels.empty),
            choose_folder: Line::new(&labels.choose_folder),
            no_audio: Line::new(&labels.no_audio),
            footer_back: Line::new(&labels.footer_back),
            footer_pin: Line::new(&labels.footer_pin),
            footer_reset: Line::new(&labels.footer_reset),
            presets: (0..super::MAX_PRESETS)
                .map(|_| {
                    let mut line = Line::new("");
                    line.text.reserve(TEXT_CAPACITY);
                    line
                })
                .collect(),
            preset_count: 0,
            footer_since: 0,
            footer_anim: super::FooterAnim::default(),
            last_push: None,
            pp_suffix: labels.pp_suffix.clone(),
            search: Line::new(""),
            search_for: None,
            search_type: crate::typing::TypeFeedback::new(),
            search_placeholder: Line::new(&labels.search_placeholder),
            search_no_matches: Line::new(&labels.search_no_matches),
            mods_title: Line::new(&labels.mods_title),
            mods_hint: Line::new(&labels.mods_hint),
            mods_multiplier: Line::new(""),
            mods_multiplier_label: labels.mods_multiplier.clone(),
            tile_multipliers: labels
                .mod_columns
                .iter()
                .map(|c| c.iter().map(|_| Line::new("")).collect())
                .collect(),
            multipliers_for: None,
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
            mod_names: labels
                .mod_texts
                .iter()
                .map(|c| {
                    c.iter()
                        .map(|(name, desc)| (Line::new(name), Line::new(desc)))
                        .collect()
                })
                .collect(),
            mod_texts: labels.mod_texts.clone(),
            mods_selected: Line::new(""),
            mods_selected_for: None,
            mod_columns: labels.mod_columns,
            slots: Vec::new(),
            ellipsis: [0.0; 4],
            info: InfoPanel::new(&labels.details),
            fades: [None; FADES],
            expansion: Expansion::default(),
            row_motion: RowMotion::default(),
            star_motion: StarMotion::default(),
            pp_motion: StarMotion::default(),
            mods_slide: PanelSlide::default(),
            shift: Shift::default(),
            fade_next: 0,
            thumbs_on: false,
            bar_look: crate::scrollbar::ScrollbarLook::new(),
            scroll_tracker: ScrollTracker::default(),
            theme,
        };
        view.scanning.text.reserve(TEXT_CAPACITY);
        view.search.text.reserve(TEXT_CAPACITY);
        view.mods_selected.text.reserve(TEXT_CAPACITY);
        view.mods_multiplier.text.reserve(TEXT_CAPACITY);
        for line in view.tile_multipliers.iter_mut().flatten() {
            line.text.reserve(MULTIPLIER_CAPACITY);
        }
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
        self.layout =
            SongSelectLayout::new(width, height, &self.theme).with_mod_columns(&self.mod_columns);
        let scale = self.layout.scale;
        let status = self.theme.song_status_size * scale;
        for line in [
            &mut self.no_folder,
            &mut self.scanning,
            &mut self.failed,
            &mut self.loading,
            &mut self.empty,
        ] {
            self.type_setter.lay_out(line, status);
        }
        // The button's label is Bold (ADR-0174).
        self.type_setter
            .lay_out_face(&mut self.choose_folder, self.theme.heading_face(status));
        for line in [
            &mut self.no_audio,
            &mut self.footer_back,
            &mut self.footer_pin,
            &mut self.footer_reset,
        ] {
            self.type_setter
                .lay_out(line, self.theme.song_detail_size * scale);
        }
        for line in &mut self.presets {
            line.text.clear();
        }
        self.preset_count = 0;
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
        let text_scale = self.theme.mod_tile_text_scale;
        for line in self.mod_tiles.iter_mut().flatten() {
            self.type_setter
                .lay_out_face(line, enlarged(self.theme.caption_face(scale), text_scale));
        }
        self.multipliers_for = None;
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
        self.type_setter
            .lay_out_face(&mut probe, self.theme.caption_face(scale));
        self.ellipsis[2] = probe.layout.width();
        self.type_setter
            .lay_out_face(&mut probe, self.theme.note_face(scale));
        self.ellipsis[3] = probe.layout.width();
        // A tile's name and description, cut to the room right of its switch (ADR-0152).
        let tile = self.layout.mods.tile_rect(0, 0);
        // The multiplier at the tile's right end takes room from the texts.
        let multiplier_room = self.theme.note_size * text_scale * scale * MULTIPLIER_ROOM_EMS;
        let room = (tile.width
            - tile.height
            - mod_text_gap(&self.theme, tile.height, scale)
            - multiplier_room)
            .max(0.0);
        for (lines, texts) in self.mod_names.iter_mut().zip(&self.mod_texts) {
            for ((name, desc), (name_text, desc_text)) in lines.iter_mut().zip(texts) {
                name.text.clone_from(name_text);
                desc.text.clone_from(desc_text);
                // Mod tile names are SemiBold (ADR-0174).
                let caption = Face {
                    weight: self.theme.semibold_weight,
                    ..enlarged(self.theme.caption_face(scale), text_scale)
                };
                fit_face(&mut self.type_setter, name, caption, room, self.ellipsis[2]);
                let note = enlarged(self.theme.note_face(scale), text_scale);
                fit_face(&mut self.type_setter, desc, note, room, self.ellipsis[3]);
            }
        }
        let count = self.layout.viewport_rows().ceil() as usize + 2 + 2 * PREFILL_ROWS;
        self.slots.clear();
        self.slots.resize_with(count, RowSlot::new);
    }

    /// How many rows have their texts laid out: the rows in view and a few past its ends
    /// (ADR-0172).
    pub fn laid_out_rows(&self) -> usize {
        self.slots.iter().filter(|s| s.holds.is_some()).count()
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
            &self.no_audio,
            &self.footer_back,
            &self.footer_pin,
            &self.footer_reset,
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
            .chain(self.tile_multipliers.iter().flatten())
            .chain([&self.mods_multiplier])
            .chain(&self.presets)
            .chain(
                self.mod_names
                    .iter()
                    .flatten()
                    .flat_map(|(name, desc)| [name, desc]),
            )
            .chain([&self.customise_title, &self.value_charset])
            .chain(self.info.lines())
            .chain(self.setting_lines.iter().map(|(_, line)| line))
            .chain(self.value_slots.iter().map(|slot| &slot.line));
        let rows = self
            .slots
            .iter()
            .flat_map(|slot| [&slot.primary, &slot.secondary, &slot.pp]);
        statics
            .into_iter()
            .chain(mods)
            .chain(rows)
            .flat_map(|line| line.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count()
    }

    /// Makes a difficulty row's pill and performance show the eased numbers, as lazer's
    /// `StarRatingDisplay` rolls its number: a text is laid out again only when its shown
    /// number changed. A row new to the tables shows its numbers at once. Does not
    /// allocate once the texts have their room.
    fn roll(
        slot: &mut RowSlot,
        type_setter: &mut Typesetter,
        (star_motion, pp_motion): (&mut StarMotion, &mut StarMotion),
        theme: &Theme,
        layout: &SongSelectLayout,
        pp_suffix: &str,
        d: &crate::song_select::DifficultyRow,
    ) {
        let key = d.key & !SET_KEY_BIT;
        let info = theme.song_difficulty_info;
        if info.stars()
            && let Some(to) = d.stars
            && slot.rolled_stars != i32::MIN
        {
            let shown = star_motion.track(key, to);
            let q = (shown * 100.0).round() as i32;
            if q != slot.rolled_stars {
                slot.rolled_stars = q;
                slot.secondary.text.clear();
                super::push_stars(&mut slot.secondary.text, shown);
                type_setter.lay_out_face(
                    &mut slot.secondary,
                    theme.heading_face(theme.song_detail_size * layout.scale),
                );
                slot.requested = false;
            }
        }
        if info.pp()
            && let Some(to) = d.pp
            && slot.rolled_pp != i32::MIN
        {
            let shown = pp_motion.track(key, to);
            let q = shown.max(0.0).round() as i32;
            if q != slot.rolled_pp {
                slot.rolled_pp = q;
                slot.pp.text.clear();
                super::push_pp(&mut slot.pp.text, shown, pp_suffix);
                type_setter.lay_out_face(&mut slot.pp, theme.caption_face(layout.scale));
                slot.requested = false;
            }
        }
    }

    /// Writes `row`'s texts into `slot` unless it already holds them.
    #[allow(clippy::too_many_arguments)]
    fn fill(
        slot: &mut RowSlot,
        type_setter: &mut Typesetter,
        theme: &Theme,
        layout: &SongSelectLayout,
        ellipsis: [f32; 4],
        pp_suffix: &str,
        generation: u64,
        list: &SongList,
        row: Row,
        text_shift: f32,
        now_ns: u64,
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
        // The text fits the row shifted as it is when not selected (ADR-0149), so it does
        // not change as the row slides.
        let most_shift = match row {
            Row::Set(_) => theme.song_set_shift,
            Row::Difficulty { .. } => theme.song_difficulty_shift,
        } * layout.scale;
        // Nor as it moves along the arc (ADR-0172): the texts shift right with it, the dots,
        // pp and pill keep to the list's right end, so the room is the arc's narrowest, at
        // the list's ends.
        let arc = layout.arc_x(layout.list.y);
        let mut avail =
            (layout.list.width - indent - most_shift - arc - 2.0 * pad - text_shift).max(0.0);
        slot.holds = None;
        slot.primary.text.clear();
        slot.secondary.text.clear();
        slot.pp.text.clear();
        slot.rolled_stars = i32::MIN;
        slot.rolled_pp = i32::MIN;
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
                let info = theme.song_difficulty_info;
                // The star pill keeps the right end; its text is the second line.
                if info.stars()
                    && let Some(stars) = d.stars
                {
                    super::push_stars(&mut slot.secondary.text, stars);
                    slot.rolled_stars = (stars * 100.0).round() as i32;
                    // The star pill is Bold in lazer (ADR-0174).
                    type_setter.lay_out_face(
                        &mut slot.secondary,
                        theme.heading_face(theme.song_detail_size * layout.scale),
                    );
                    avail = (avail
                        - super::info_panel::pill_width(&slot.secondary, theme, layout.scale))
                    .max(0.0);
                }
                // The SS performance, left of the pill (ADR-0151).
                if info.pp()
                    && let Some(pp) = d.pp
                {
                    super::push_pp(&mut slot.pp.text, pp, pp_suffix);
                    slot.rolled_pp = pp.max(0.0).round() as i32;
                    type_setter.lay_out_face(&mut slot.pp, theme.caption_face(layout.scale));
                    avail = (avail - slot.pp.layout.width() - pad).max(0.0);
                }
            }
        }
        let scale = layout.scale;
        if matches!(row, Row::Set(_)) {
            // The set's title is Bold (lazer's Heading1, ADR-0174).
            fit_face(
                type_setter,
                &mut slot.primary,
                theme.heading_face(theme.song_title_size * scale),
                avail,
                ellipsis[0],
            );
            // The mapper is a note (lazer: 14 SemiBold); too long, it rides.
            type_setter.lay_out_face(&mut slot.secondary, theme.note_face(scale));
        } else {
            // The difficulty's name is SemiBold (ADR-0174).
            type_setter.lay_out_face(
                &mut slot.primary,
                Face {
                    weight: theme.semibold_weight,
                    ..theme.caption_face(scale)
                },
            );
        }
        slot.avail = avail;
        slot.marquee.restart(now_ns);
        slot.requested = false;
        slot.holds = Some((generation, row));
        true
    }

    /// Writes the screen at `pose` on a `width` x `height` window: row rectangles into
    /// `shapes` (tinting `white`, an opaque white image) and texts into `text` (drawn with
    /// the MSDF shader). Allocation-free while the same rows are in view; see the module
    /// docs.
    ///
    /// Returns where the mod select panel starts in `shapes` and `text` when it is drawn:
    /// the caller draws from there as a layer of its own, so the list stays seen under the
    /// panel and its texts do not cover the columns (ADR-0152).
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
    ) -> Splits {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
        // Only the small thumbnails take room from the texts (ADR-0174).
        let reserves = thumbs.enabled && !thumbs.full;
        if reserves != self.thumbs_on {
            // The texts of the set rows are cut to another width.
            self.thumbs_on = reserves;
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

        self.refresh_selected(&snapshot.mods, thumbs.now_ns);
        self.refresh_presets(&snapshot.presets, &snapshot.footer);
        self.refresh_multipliers(&snapshot.mods);
        self.refresh_search(&snapshot.query, thumbs.now_ns);
        // The mod select panel's slide (ADR-0152): the backdrop without delay, the last
        // shown column with the most.
        let now = thumbs.now_ns;
        self.mods_slide.follow(snapshot.mods.open, now, &self.theme);
        let shown_columns = (0..MAX_COLUMNS)
            .filter(|&c| self.layout.mods.is_shown(c))
            .count();
        let last_delay = shown_columns.saturating_sub(1) as f32 * self.theme.mod_column_stagger_ms;
        let panel_back = self.mods_slide.visibility(now, 0.0, &self.theme);
        let panel_columns = self.mods_slide.visibility(now, last_delay, &self.theme) > 0.0;
        self.info.refresh(
            &mut self.type_setter,
            &self.theme,
            &self.layout,
            [self.ellipsis[0], self.ellipsis[1]],
            snapshot,
        );
        // The footer slides in with the screen: from the first frame it shows, or the first
        // after the screen was away (ADR-0175).
        let entered = pose.opacity > 0.0
            && self
                .last_push
                .is_none_or(|last| now.saturating_sub(last) > FOOTER_AWAY_NS);
        if entered {
            self.footer_since = now;
        }
        self.last_push = Some(now);
        let footer_slide = crate::anim::ease_out_quint(crate::anim::progress(
            self.footer_since,
            now,
            (self.theme.footer_slide_ms.max(0.0) * 1.0e6) as u64,
        ));
        let target = self.layout.footer.buttons(snapshot.footer);
        self.footer_anim.update(
            now,
            &target,
            snapshot.footer_hover,
            snapshot.footer_pressed,
            &self.theme,
        );
        let shift = pose.offset * width;
        let opacity = pose.opacity;
        let theme = &self.theme;
        let layout = &self.layout;
        let pad = theme.song_text_pad * layout.scale;
        let list = &*snapshot.list;

        let scale = layout.scale;
        // Rows emerging as their set expands fade in on top of the layer (ADR-0136).
        let row_alpha = std::cell::Cell::new(1.0f32);
        // The list under the sliding mod select panel, or the panel (ADR-0152).
        let layer = std::cell::Cell::new(1.0f32);
        // Shared by `rect` and the thumbnails, which go straight in.
        let shape_target = std::cell::RefCell::new(&mut *shapes);
        let mut rect = |rect: &Rect, colour: Color, look: Look| {
            let moved = Rect {
                x: rect.x + shift,
                ..*rect
            };
            let colour = faded(colour, opacity * row_alpha.get() * layer.get());
            let shapes = &mut **shape_target.borrow_mut();
            crate::widgets::draw(shapes, white, &moved, colour, look, theme, scale);
        };
        let params = MarqueeParams::new(theme, scale);
        let now_ns = thumbs.now_ns;
        // The rows' marquees take the sideways wheel and drags first, by the windows of
        // the last frame; then the windows are cleared for this frame's rows to set.
        self.scroll_tracker.apply(
            &snapshot.text,
            now_ns,
            &params,
            self.slots.iter_mut().map(|slot| &mut slot.marquee),
        );
        for slot in &mut self.slots {
            slot.marquee.place(Rect::default());
        }
        let target = std::cell::RefCell::new((&mut *text, &mut *glyphs));
        let mut left = |line: &Line, x: f32, centre_y: f32, colour: Color| {
            let position = [x + shift, centre_y - line.layout.height() / 2.0];
            let colour = faded(colour, opacity * row_alpha.get() * layer.get());
            let (text, glyphs) = &mut *target.borrow_mut();
            push_layout(text, &line.layout, position, colour, glyphs);
        };
        // Draws a text that rides in `avail` pixels from `x`, centred on `centre_y`.
        let ride = |line: &Line,
                    marquee: &mut Marquee,
                    avail: f32,
                    x: f32,
                    centre_y: f32,
                    colour: Color| {
            let height = line.layout.height();
            let rect = Rect {
                x: x + shift,
                y: centre_y - height / 2.0,
                width: avail,
                height,
            };
            let colour = faded(colour, opacity * row_alpha.get() * layer.get());
            let (text, glyphs) = &mut *target.borrow_mut();
            push_marquee(
                text, line, &rect, 0.0, colour, glyphs, marquee, &params, now_ns,
            );
        };

        // The list stays under the mod select panel, dimmed by its backdrop (ADR-0152).
        {
            layer.set(1.0);
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
            // The selected difficulty's statistics and local scores, on the left; they give
            // way to the mod panel, whose multiplier stands over them (ADR-0152).
            layer.set(1.0 - panel_back);
            self.info.push(
                &mut self.type_setter,
                layout,
                theme,
                snapshot,
                // The wedges' outlines are the looks', not the closure's: they fade with the
                // panel too, like the fills and texts.
                opacity * (1.0 - panel_back),
                now_ns,
                &mut rect,
                &mut left,
                &mut |line, marquee, avail, x, y, colour| ride(line, marquee, avail, x, y, colour),
            );
            layer.set(1.0);
            // The search line.
            let search = leaned(&layout.search);
            // The box runs on to the window's right edge, past its rounded corners, as the
            // plates do; the text and caret keep to the list's width.
            let search_box = Rect {
                width: (layout.window_right - search.x + 2.0 * theme.button_radius * scale)
                    .max(search.width),
                ..search
            };
            rect(&search_box, theme.song_search_fill, Look::Round);
            if snapshot.query.is_empty() {
                // The status line takes the placeholder's place (ADR-0177).
                let (line, colour) = match status_line {
                    Some(line) => (line, theme.text),
                    None => (&self.search_placeholder, theme.disabled_text),
                };
                left(line, search.x + pad, search.y + search.height / 2.0, colour);
            } else {
                // The newest character slides in; an erase pops the text (ADR-0167).
                let line = &self.search;
                let pose = self.search_type.pose(now_ns, theme, scale);
                let colour = faded(theme.text, opacity * row_alpha.get() * layer.get());
                let position = [
                    search.x + pad + shift,
                    search.y + search.height / 2.0 - line.layout.height() / 2.0,
                ];
                let (text, glyphs) = &mut *target.borrow_mut();
                slam_text::push_layout_styled(
                    text,
                    &line.layout,
                    position,
                    glyphs,
                    crate::typing::styler(&line.layout, colour, pose),
                );
            }
            // The caret: at the end of the typed text, at the start of the placeholder.
            let caret_alpha =
                self.search_type
                    .caret_alpha(now_ns, theme, !snapshot.query.is_empty());
            if caret_alpha > 0.0 {
                let reach = if snapshot.query.is_empty() {
                    0.0
                } else {
                    self.search.layout.width()
                };
                let caret = crate::typing::caret_rect(
                    search.x + pad + reach,
                    search.y + search.height / 2.0,
                    self.search.layout.height(),
                    theme,
                    scale,
                );
                let colour = Color {
                    a: theme.caret.a * caret_alpha,
                    ..theme.caret
                };
                rect(&caret, colour, Look::Pill);
            }
            if list.sets.is_empty() {
                if snapshot.status == SongSelectStatus::Ready {
                    let y = layout.list.y + theme.song_set_height * scale / 2.0;
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
                    let y = layout.list.y + theme.song_set_height * scale / 2.0;
                    left(
                        &self.search_no_matches,
                        lean(layout.list.x, y),
                        y,
                        theme.disabled_text,
                    );
                }
                // Thumbnails are pushed after the rows: their shapes wait here.
                let mut thumb_shapes: [Option<Shape>; 2 * MAX_VISIBLE_ROWS] =
                    [None; 2 * MAX_VISIBLE_ROWS];
                let mut thumb_count = 0;
                let thumb = ThumbGeometry::new(theme, layout);
                let expansion = self
                    .expansion
                    .track(list, snapshot.selected, thumbs.now_ns, theme);
                // Rows pulled in from past the window's ends while the list moves.
                let geometry = &snapshot.geometry;
                let slide = self
                    .shift
                    .track(list, snapshot.selected, thumbs.now_ns, theme);
                let visible = layout.visible_rows(geometry, snapshot.shown_scroll);
                let reach = expansion.reach();
                // A few rows past the view are laid out and their glyphs asked for, so
                // their texts are ready when they scroll in (and while eased rows are
                // drawn inside the view whose places are outside it).
                let reach = reach + PREFILL_ROWS;
                let rows_to_draw = visible.start.saturating_sub(reach)
                    ..(visible.end + reach).min(snapshot.row_count);
                // Rows ease to their places, each on its own, as lazer's panels do: a quick
                // change of selection moves them, never leaves holes (ADR-0172).
                let ease = self.row_motion.begin(thumbs.now_ns, theme.song_row_ease_ms);
                let roll_ns = (theme.star_colour_ease_ms.max(0.0) as f64 * 1e6) as u64;
                self.star_motion.begin(thumbs.now_ns, roll_ns);
                self.pp_motion.begin(thumbs.now_ns, roll_ns);
                let mut set_top: Option<(usize, f32)> = None;
                for index in rows_to_draw {
                    let Some(row) = snapshot.row_at(index) else {
                        continue;
                    };
                    let slot_count = self.slots.len();
                    let slot = &mut self.slots[index % slot_count];
                    let text_shift = if reserves && matches!(row, Row::Set(_)) {
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
                        &self.pp_suffix,
                        snapshot.generation,
                        list,
                        row,
                        text_shift,
                        now_ns,
                    ) {
                        continue;
                    }
                    if let Row::Difficulty { set, difficulty } = row
                        && let Some(d) = list
                            .sets
                            .get(set)
                            .and_then(|s| s.difficulties.get(difficulty))
                    {
                        Self::roll(
                            slot,
                            &mut self.type_setter,
                            (&mut self.star_motion, &mut self.pp_motion),
                            theme,
                            layout,
                            &self.pp_suffix,
                            d,
                        );
                    }
                    if !slot.requested {
                        slot.requested = true;
                        let cache = &mut *target.borrow_mut().1;
                        for line in [&slot.primary, &slot.secondary, &slot.pp] {
                            for g in line.layout.glyphs() {
                                let _ = cache.glyph(GlyphKey::of(g));
                            }
                        }
                    }
                    let (primary_line, secondary_line, pp_line) =
                        (&slot.primary, &slot.secondary, &slot.pp);
                    let (avail, marquee) = (slot.avail, &mut slot.marquee);
                    let indent = match row {
                        Row::Set(_) => 0.0,
                        Row::Difficulty { .. } => layout.indent,
                    };
                    // The opening set's difficulties fade in (ADR-0136); where rows are is
                    // the easing's.
                    let (_, alpha) = expansion.offset(row);
                    row_alpha.set(alpha);
                    let target = geometry.top(index);
                    let (key, start) = match row {
                        Row::Set(set) => {
                            (list.sets.get(set).map_or(0, |s| s.key) | SET_KEY_BIT, None)
                        }
                        // A difficulty new to the list slides out from under its set.
                        Row::Difficulty { set, difficulty } => (
                            list.sets
                                .get(set)
                                .and_then(|s| s.difficulties.get(difficulty))
                                .map_or(0, |d| d.key)
                                & !SET_KEY_BIT,
                            set_top.filter(|&(s, _)| s == set).map(|(_, top)| top),
                        ),
                    };
                    let top = self.row_motion.track(key, target, start, ease);
                    if let Row::Set(set) = row {
                        set_top = Some((set, top));
                    }
                    let y = layout.list.y + (top - snapshot.shown_scroll) * scale;
                    let height = layout.plate_height(geometry, index);
                    // The rows sit on an arc (ADR-0172).
                    let lean = layout.arc_x(y + height / 2.0);
                    // Rows that are not selected start further right (ADR-0149).
                    let moved = slide.of(row) * scale;
                    let area = Rect {
                        x: layout.list.x + indent + moved + lean,
                        y,
                        width: (layout.list.width - indent - moved - lean).max(0.0),
                        height,
                    };
                    // Rows scrolled part-way out are cut at the list's edges, and their texts
                    // show only whole, so nothing spills over the search line or the hints.
                    let (top_edge, bottom_edge) =
                        (layout.list.y, layout.list.y + layout.list.height);
                    let clipped_top = area.y.max(top_edge);
                    let clipped_bottom = (area.y + area.height).min(bottom_edge);
                    if clipped_bottom <= clipped_top {
                        continue;
                    }
                    let inside = |line: &Line, centre_y: f32| {
                        let half = line.layout.height() / 2.0;
                        centre_y - half >= top_edge && centre_y + half <= bottom_edge
                    };
                    // The selected set's row is lit as the selected difficulty's (ADR-0149).
                    let selected = match row {
                        Row::Difficulty { set, difficulty } => {
                            snapshot.selected == Some((set, difficulty))
                        }
                        Row::Set(set) => snapshot.selected.is_some_and(|(s, _)| s == set),
                    };
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
                    let (set_index, raw_stars) = match row {
                        Row::Set(set) => (
                            set,
                            // A set takes the colour of its hardest difficulty (ADR-0136).
                            list.sets.get(set).and_then(|s| {
                                s.difficulties
                                    .iter()
                                    .filter_map(|d| d.stars)
                                    .reduce(f32::max)
                            }),
                        ),
                        Row::Difficulty { set, difficulty } => (
                            set,
                            list.sets
                                .get(set)
                                .and_then(|s| s.difficulties.get(difficulty))
                                .and_then(|d| d.stars),
                        ),
                    };
                    // The rating's colour eases to a new rating, as the info panel's does.
                    let eased = raw_stars.map(|to| self.star_motion.track(key, to));
                    let stars = match row {
                        Row::Set(_) => None,
                        Row::Difficulty { .. } => eased,
                    };
                    let seed = index as f32 * 0.29;
                    let look = match row {
                        // The selected difficulty glows with the beat (ADR-0167).
                        Row::Difficulty { .. } if selected && snapshot.beat.is_some() => {
                            Look::BeatButton {
                                seed,
                                fade: opacity * alpha,
                                tint: crate::star_colour::glow_for(stars, theme.accent),
                                beat: snapshot.beat.unwrap_or(0.0),
                            }
                        }
                        // A difficulty glows and is outlined in its star colour (ADR-0131).
                        Row::Difficulty { .. } => Look::TintedButton {
                            highlighted: selected,
                            seed,
                            fade: opacity * alpha,
                            tint: crate::star_colour::glow_for(stars, theme.accent),
                        },
                        // A set in the colour of its hardest difficulty (ADR-0136).
                        Row::Set(_) => Look::TintedButton {
                            highlighted: selected,
                            seed,
                            fade: opacity * alpha,
                            tint: crate::star_colour::glow_for(eased, theme.accent),
                        },
                    };
                    // The plate runs on past the window's right edge, as lazer's; the
                    // texts keep to the list.
                    let plate = Rect {
                        width: (layout.window_right - clipped.x
                            + 2.0 * theme.button_radius * scale)
                            .max(clipped.width),
                        ..clipped
                    };
                    rect(&plate, fill, look);
                    let x = area.x + pad + text_shift;
                    if thumbs.enabled
                        && thumbs.full
                        && matches!(row, Row::Set(_))
                        && let Some(image) = index
                            .checked_sub(snapshot.first_row)
                            .and_then(|i| thumbs.rows.get(i))
                            .copied()
                            .flatten()
                    {
                        // The set's background over the whole plate, fading in (ADR-0174).
                        let alpha = fade_alpha(
                            &mut self.fades,
                            &mut self.fade_next,
                            image.handle,
                            thumbs.now_ns,
                            theme.song_plate_fade_ms,
                        );
                        thumb_shapes[thumb_count] = Some(plate_shape(
                            image.region,
                            &plate,
                            area.y,
                            area.height,
                            shift,
                            ease_out_quint(alpha) * opacity,
                            theme,
                            scale,
                        ));
                        thumb_count += 1;
                    } else if thumbs.enabled
                        && !thumbs.full
                        && matches!(row, Row::Set(_))
                        && let Some((top, bottom)) = thumb.span(&area, top_edge, bottom_edge)
                    {
                        let (left_x, full_top) =
                            (area.x + thumb.inset + shift, area.y + thumb.inset);
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
                            // Upright in the leaning plate (ADR-0167): the image spans the
                            // plate's bounding box, so it keeps its aspect by losing as much
                            // of its height.
                            let uv = image.region.uv;
                            let lean = (theme.button_skew().tan() * thumb.height).abs();
                            let kept = thumb.width / (thumb.width + lean);
                            let middle = (uv.v0 + uv.v1) / 2.0;
                            let dv = (uv.v1 - uv.v0) * kept;
                            let v0 = middle - dv / 2.0;
                            let cut = |at: f32| v0 + (at - full_top) / thumb.height * dv;
                            thumb_shapes[thumb_count] = Some(Shape {
                                colour: faded(Color::rgb(1.0, 1.0, 1.0), alpha * opacity),
                                texture: image.region.texture,
                                uv: UvRect {
                                    v0: cut(top),
                                    v1: cut(bottom),
                                    ..uv
                                },
                                upright: true,
                                ..base
                            });
                            thumb_count += 1;
                        }
                    }
                    if matches!(row, Row::Set(_)) {
                        let (top, bottom) =
                            (primary_line.layout.height(), secondary_line.layout.height());
                        let start = area.y + (area.height - top - bottom) / 2.0;
                        let (primary, secondary) = (start + top / 2.0, start + top + bottom / 2.0);
                        if inside(primary_line, primary) {
                            left(primary_line, x, primary, theme.text);
                        }
                        if inside(secondary_line, secondary) {
                            ride(
                                secondary_line,
                                marquee,
                                avail,
                                x,
                                secondary,
                                theme.disabled_text,
                            );
                        }
                    } else {
                        let centre = area.y + area.height / 2.0;
                        // With the stars shown, the name rises and the star counter goes under
                        // it, as lazer's `PanelBeatmap` (ADR-0151).
                        let counter = theme.song_difficulty_info.stars() && stars.is_some();
                        let star = theme.difficulty_star_size * scale;
                        let gap = theme.difficulty_star_gap * scale;
                        let name_centre = if counter {
                            let total = primary_line.layout.height() + gap + star;
                            centre - total / 2.0 + primary_line.layout.height() / 2.0
                        } else {
                            centre
                        };
                        if inside(primary_line, name_centre) {
                            ride(primary_line, marquee, avail, x, name_centre, theme.text);
                        }
                        if counter && let Some(stars) = stars {
                            let star_centre =
                                name_centre + primary_line.layout.height() / 2.0 + gap + star / 2.0;
                            if star_centre - star / 2.0 >= top_edge
                                && star_centre + star / 2.0 <= bottom_edge
                            {
                                push_star_counter(&mut rect, stars, x, star_centre, theme, scale);
                            }
                        }
                    }
                    let centre = area.y + area.height / 2.0;
                    let right = area.x + area.width - pad;
                    match row {
                        Row::Difficulty { .. } => {
                            // The pill's text is empty when the setting hides the stars.
                            let pill = match stars {
                                Some(stars) if !secondary_line.text.is_empty() => Some(stars),
                                _ => None,
                            };
                            let mut end = right;
                            if let Some(stars) = pill {
                                // The star pill, in the colour of the rating.
                                let line = secondary_line;
                                let width = super::info_panel::star_pill_width(line, theme, scale);
                                end -= width + pad;
                                if inside(secondary_line, centre) {
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
                            // The SS performance, left of the pill (ADR-0151).
                            if !pp_line.text.is_empty() && inside(pp_line, centre) {
                                left(pp_line, end - pp_line.layout.width(), centre, theme.text);
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
                            if centre - size / 2.0 >= top_edge && centre + size / 2.0 <= bottom_edge
                            {
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
                row_alpha.set(1.0);
                for shape in thumb_shapes[..thumb_count].iter().flatten() {
                    let shape = Shape {
                        colour: faded(shape.colour, layer.get()),
                        ..*shape
                    };
                    shape_target.borrow_mut().push_shape(&shape);
                }
                // The scrollbar over the plates, which run past the window's edge
                // (ADR-0174); it follows the rows in the mod panel's fade.
                let mut bar = super::scrollbar_of(geometry, layout);
                bar.track.x += shift;
                self.bar_look.push_carousel(
                    &mut shape_target.borrow_mut(),
                    white,
                    &bar,
                    snapshot.shown_scroll,
                    snapshot.text.pointer,
                    snapshot.scrollbar_held,
                    opacity * layer.get(),
                    theme,
                    scale,
                    now_ns,
                );
            }
            layer.set(1.0);
            // The refused play's reason, above the footer, on a backing of its own.
            if snapshot.no_audio {
                let hint = layout.hint;
                rect(&hint, theme.song_search_fill, Look::Round);
                let line = &self.no_audio;
                left(line, hint.x + pad, hint.y + hint.height / 2.0, theme.text);
            }
            // The footer bar slides up from below with the screen (ADR-0175).
            let bar = layout.footer.bar;
            let up = (1.0 - footer_slide) * bar.height;
            rect(
                &Rect {
                    y: bar.y + up,
                    ..bar
                },
                theme.footer_fill,
                Look::Plain,
            );
            let anim = &self.footer_anim;
            let buttons = layout.footer.animated(anim.presence());
            let presence = anim.presence();
            let flash = anim.mods_flash(now, theme);
            let mix = |a: Color, b: Color, t: f32| Color {
                r: a.r + (b.r - a.r) * t,
                g: a.g + (b.g - a.g) * t,
                b: a.b + (b.b - a.b) * t,
                a: a.a + (b.a - a.a) * t,
            };
            // `open` is how far the button's place is open (its fade); `selected` keeps a
            // preset lit while the current mods are it.
            let draw_button =
                |rect_of: Rect, button: FooterButton, seed: f32, open: f32, lit: bool| {
                    let hot = anim.hover(button);
                    let press = anim.press(button);
                    let mut fill = mix(theme.button_fill, theme.button_hover_fill, hot.max(press));
                    if button == FooterButton::Mods {
                        fill = mix(fill, theme.accent, flash * theme.footer_flash_mix);
                    }
                    // Pressed in, the button pops a little larger, as lazer's does.
                    let grow = press * theme.footer_pop;
                    let area = Rect {
                        x: rect_of.x - rect_of.width * grow / 2.0,
                        y: rect_of.y + up - rect_of.height * grow / 2.0,
                        width: rect_of.width * (1.0 + grow),
                        height: rect_of.height * (1.0 + grow),
                    };
                    let on =
                        lit || press > 0.5 || (button == FooterButton::Mods && snapshot.mods.open);
                    rect(
                        &area,
                        fill,
                        Look::Button {
                            highlighted: on,
                            seed,
                            fade: opacity * open,
                        },
                    );
                };
            let centred_label = |line: &Line, area: &Rect| {
                (
                    area.x + (area.width - line.layout.width()) / 2.0,
                    area.y + area.height / 2.0 + up,
                )
            };
            draw_button(buttons.back, FooterButton::Back, 0.1, 1.0, false);
            let (x, y) = centred_label(&self.footer_back, &buttons.back);
            left(&self.footer_back, x, y, theme.text);
            draw_button(buttons.mods, FooterButton::Mods, 0.2, 1.0, false);
            let selected = snapshot.mods.active != [0; MAX_COLUMNS];
            let inset = buttons.mods.height * theme.button_skew().tan() / 2.0 + pad;
            // The new selection's text fades in as the button flashes.
            let text_in = 1.0 - flash;
            if selected {
                left(
                    &self.mods_title,
                    buttons.mods.x + inset,
                    buttons.mods.y + up + buttons.mods.height * 0.3,
                    theme.text,
                );
                left(
                    &self.mods_selected,
                    buttons.mods.x + inset,
                    buttons.mods.y + up + buttons.mods.height * 0.71,
                    faded(tone_colour(theme, snapshot.mods.total_multiplier), text_in),
                );
            } else {
                left(
                    &self.mods_title,
                    buttons.mods.x + inset,
                    buttons.mods.y + up + buttons.mods.height / 2.0,
                    theme.text,
                );
            }
            for i in 0..buttons.preset_count.min(self.presets.len()) {
                let open = presence.presets[i];
                let lit = snapshot.current_preset == Some(i);
                draw_button(
                    buttons.presets[i],
                    FooterButton::Preset(i),
                    0.3 + i as f32 * 0.07,
                    open,
                    lit,
                );
                let (x, y) = centred_label(&self.presets[i], &buttons.presets[i]);
                left(&self.presets[i], x, y, faded(theme.text, open));
            }
            if let Some(area) = buttons.pin {
                draw_button(area, FooterButton::Pin, 0.7, presence.pin, false);
                let (x, y) = centred_label(&self.footer_pin, &area);
                left(&self.footer_pin, x, y, faded(theme.text, presence.pin));
            }
            if let Some(area) = buttons.reset {
                draw_button(area, FooterButton::Reset, 0.8, presence.reset, false);
                let (x, y) = centred_label(&self.footer_reset, &area);
                left(&self.footer_reset, x, y, faded(theme.text, presence.reset));
            }
        }
        // The beating logo in the corner, over the list's texts and under the mod select
        // panel (ADR-0186).
        let logo_split = thumbs.logo.map(|_| {
            (
                shape_target.borrow().len() as u32,
                target.borrow().0.len() as u32,
            )
        });
        if let Some(art) = thumbs.logo {
            let layout = &self.layout;
            crate::menu::view::push_logo(
                &mut shape_target.borrow_mut(),
                art,
                white,
                theme,
                &snapshot.logo,
                [
                    layout.logo_centre[0] + pose.offset * width,
                    layout.logo_centre[1],
                ],
                layout.logo_size,
                pose.opacity,
                None,
            );
        }
        let panel = panel_back > 0.0 || panel_columns;
        let mut popup_split = None;
        let split = panel.then(|| {
            (
                shape_target.borrow().len() as u32,
                target.borrow().0.len() as u32,
            )
        });
        if panel {
            // The backdrop, title, hint and settings fade; the columns slide (ADR-0152).
            layer.set(panel_back);
            // Down to the footer, which stays in reach (ADR-0175).
            let full = Rect {
                x: 0.0,
                y: 0.0,
                width,
                height: layout.footer.bar.y,
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
            // The selection's score multiplier at the left of the title's row.
            left(
                &self.mods_multiplier,
                panel.title.x,
                panel.title.y + panel.title.height / 2.0,
                tone_colour(theme, snapshot.mods.total_multiplier),
            );
            let custom = &snapshot.mods.customise;
            let expanded = custom.state != CustomisationState::Collapsed;
            let dim = if expanded { DIMMED } else { 1.0 };
            let mut slot = 0;
            for (column, titles) in self.mod_column_titles.iter().enumerate() {
                if !panel.is_shown(column) {
                    continue;
                }
                // Each column after the first a little later, from below and above in turn
                // (ADR-0152, ADR-0167).
                let direction = slide_direction(slot);
                let delay = slot as f32 * theme.mod_column_stagger_ms;
                slot += 1;
                let shown = self.mods_slide.visibility(now_ns, delay, theme);
                if shown <= 0.0 {
                    continue;
                }
                layer.set(shown);
                let dy = direction * (1.0 - shown) * theme.mod_panel_distance * scale;
                let down = |r: Rect| Rect { y: r.y + dy, ..r };
                // Each column in the colour of its mod type (ADR-0136): a plate with a
                // header in that colour, as lazer's `ModColumn`.
                let tint = theme.mod_column_tints[column % theme.mod_column_tints.len()];
                let count = self.mod_tiles[column].len();
                let lean = panel.lean();
                rect(
                    &down(panel.column_rect(column, count)),
                    faded(theme.mod_column_fill, dim),
                    Look::LeaningPanel { lean },
                );
                let header = down(panel.column_titles[column]);
                rect(&header, faded(tint, dim), Look::LeaningPanel { lean });
                // The title from the left, as lazer's column headers.
                let (x, y) = (
                    header.x + 2.0 * theme.widget_pad * scale,
                    header.y + header.height / 2.0,
                );
                let on_tint = Color {
                    a: 1.0,
                    ..theme.mod_column_fill
                };
                left(titles, x, y, faded(on_tint, dim));
                for (index, line) in self.mod_tiles[column].iter().enumerate() {
                    let area = down(panel.tile_rect(column, index));
                    let on = snapshot.mods.active[column] >> index & 1 != 0;
                    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Overlays/Mods/ModSelectPanel.cs (BackgroundColour, ForegroundColour)
                    // On: the column's colour darkened, its switch in the colour itself.
                    let fill = if on {
                        darkened(tint, theme.mod_tile_active_darken)
                    } else if snapshot.mods.hover == Some((column, index)) {
                        theme.button_hover_fill
                    } else {
                        theme.button_fill
                    };
                    let look = Look::TintedButton {
                        highlighted: on,
                        seed: (column * 7 + index) as f32 * 0.31,
                        fade: opacity * shown,
                        tint,
                    };
                    rect(&area, faded(fill, dim), look);
                    // The acronym in the switch on the left, in the column's colour; the
                    // name and description beside it, as lazer's `ModPanel` (ADR-0152).
                    let switch = Rect {
                        width: area.height,
                        ..area
                    };
                    let (x, y) = centred(line, &switch);
                    if on {
                        // Leaning with the tile, as lazer's sheared switch.
                        rect(&switch, faded(tint, dim), Look::Strip);
                        let ink = Color {
                            a: 1.0,
                            ..theme.background
                        };
                        left(line, x, y, faded(ink, dim));
                    } else {
                        left(line, x, y, faded(tint, dim));
                    }
                    if let Some(line) = self.tile_multipliers.get(column).and_then(|c| c.get(index))
                    {
                        let value = snapshot.mods.multipliers[column][index];
                        let ink = if on {
                            Color {
                                a: 1.0,
                                ..theme.background
                            }
                        } else {
                            tone_colour(theme, value)
                        };
                        left(
                            line,
                            area.x + area.width
                                - theme.widget_pad * scale
                                - line.layout.width()
                                - mod_text_gap(theme, area.height, scale) * 0.5,
                            area.y + area.height / 2.0,
                            faded(ink, dim),
                        );
                    }
                    if let Some((name, desc)) = self
                        .mod_names
                        .get(column)
                        .and_then(|names| names.get(index))
                    {
                        // Clear of the switch's leaning top corner.
                        let text_x = area.x + area.height + mod_text_gap(theme, area.height, scale);
                        // Dark on an active tile's bright wash, as lazer's active panels.
                        let ink = Color {
                            a: 1.0,
                            ..theme.background
                        };
                        let (name_ink, desc_ink) = if on {
                            (ink, faded(ink, 0.75))
                        } else {
                            (theme.text, theme.disabled_text)
                        };
                        left(
                            name,
                            text_x,
                            area.y + area.height * 0.34,
                            faded(name_ink, dim),
                        );
                        left(
                            desc,
                            text_x,
                            area.y + area.height * 0.72,
                            faded(desc_ink, dim),
                        );
                    }
                }
            }
            layer.set(panel_back);
            // The floating settings panel (ADR-0177) is a layer of its own, so that the
            // columns' texts do not show through its plate.
            let cl = &panel.customise.at(custom.origin, custom.len);
            if expanded {
                popup_split = Some((
                    shape_target.borrow().len() as u32,
                    target.borrow().0.len() as u32,
                ));
                rect(
                    &cl.header,
                    Color {
                        a: 1.0,
                        ..if custom.moving {
                            theme.button_fill
                        } else {
                            theme.button_hover_fill
                        }
                    },
                    Look::Round,
                );
                left(
                    &self.customise_title,
                    cl.header.x + cl.pad,
                    cl.header.y + cl.header.height / 2.0,
                    theme.text,
                );
                rect(
                    &cl.content(custom.len),
                    // Opaque: the columns' plates and tiles must not show through.
                    Color {
                        a: 1.0,
                        ..theme.widget_popup_fill
                    },
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
        }
        Splits {
            logo: logo_split,
            panel: split,
            popup: popup_split,
        }
    }

    /// Lays the query out unless it already shows it.
    fn refresh_search(&mut self, query: &SearchQuery, now_ns: u64) {
        if self.search_for == Some(*query) {
            return;
        }
        self.search_for = Some(*query);
        self.search_type.observe(query.as_str(), now_ns);
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

    /// Writes the tiles' multipliers and the selection's unless they already show them.
    fn refresh_multipliers(&mut self, mods: &ModSelectSnapshot) {
        let key = (mods.multipliers, mods.total_multiplier);
        if self.multipliers_for == Some(key) {
            return;
        }
        self.multipliers_for = Some(key);
        let scale = self.layout.scale;
        let face = enlarged(self.theme.note_face(scale), self.theme.mod_tile_text_scale);
        for (lines, values) in self.tile_multipliers.iter_mut().zip(&mods.multipliers) {
            for (line, &value) in lines.iter_mut().zip(values) {
                line.text.clear();
                push_multiplier(&mut line.text, value);
                self.type_setter.lay_out_face(line, face);
            }
        }
        let line = &mut self.mods_multiplier;
        line.text.clear();
        line.text.push_str(&self.mods_multiplier_label);
        line.text.push(' ');
        push_multiplier(&mut line.text, mods.total_multiplier);
        self.type_setter
            .lay_out(line, self.theme.song_status_size * scale);
    }

    /// Writes "HD DT1.05x  1.12x" for the active tiles, their details and the multiplier of
    /// the selection (the footer's Mods button) unless it already shows them.
    fn refresh_selected(&mut self, mods: &ModSelectSnapshot, now_ns: u64) {
        let key = (mods.active, mods.details, mods.total_multiplier.to_bits());
        if self.mods_selected_for == Some(key) {
            return;
        }
        // The Mods button flashes when the selection changes, not when it first shows.
        if self.mods_selected_for.is_some() {
            self.footer_anim.flash_mods(now_ns, &self.theme);
        }
        self.mods_selected_for = Some(key);
        let line = &mut self.mods_selected;
        line.text.clear();
        for (column, acronyms) in self.mod_columns.iter().enumerate() {
            for (index, acronym) in acronyms.iter().enumerate() {
                if mods.active[column] >> index & 1 != 0 {
                    if !line.text.is_empty() {
                        line.text.push(' ');
                    }
                    line.text.push_str(acronym);
                    push_detail(&mut line.text, mods.details[column][index]);
                }
            }
        }
        if !line.text.is_empty() {
            line.text.push_str("  ");
            push_multiplier(&mut line.text, mods.total_multiplier);
        }
        let size = self.theme.song_detail_size * self.layout.scale;
        let room = self.layout.footer.buttons(Default::default()).mods.width
            - 2.0 * self.theme.widget_pad * self.layout.scale
            - self.layout.footer.bar.height * 0.3;
        fit_tail(
            &mut self.type_setter,
            line,
            size,
            room.max(0.0),
            self.ellipsis[1],
        );
    }

    /// Lays the pinned presets' labels out for the footer's buttons unless they show them.
    fn refresh_presets(&mut self, presets: &[Arc<str>], footer: &FooterState) {
        let count = footer.presets.min(presets.len()).min(self.presets.len());
        let same = count == self.preset_count
            && self.presets[..count]
                .iter()
                .zip(presets)
                .all(|(line, label)| line.text == **label);
        if same {
            return;
        }
        self.preset_count = count;
        let size = self.theme.song_detail_size * self.layout.scale;
        let room = self.theme.footer_preset_width * self.layout.scale
            - 2.0 * self.theme.widget_pad * self.layout.scale;
        for (line, label) in self.presets.iter_mut().zip(presets).take(count) {
            line.text.clear();
            line.text.push_str(label);
            fit_tail(
                &mut self.type_setter,
                line,
                size,
                room.max(0.0),
                self.ellipsis[1],
            );
        }
    }
}

/// The room between a mod tile's switch and its texts: the switch leans with the tile, so its
/// top corner reaches right by half its height times the skew; then a little padding.
/// How many ems of the note face a tile's multiplier ("1.00x" and its padding) is given.
const MULTIPLIER_ROOM_EMS: f32 = 3.6;

/// `face` made `by` times larger.
fn enlarged(face: crate::menu::view::Face, by: f32) -> crate::menu::view::Face {
    crate::menu::view::Face {
        size: face.size * by,
        ..face
    }
}

/// Which way the column in `slot` (among the shown ones) slides in from: below for the first,
/// above for the second, and so on (ADR-0167).
fn slide_direction(slot: usize) -> f32 {
    if slot.is_multiple_of(2) { 1.0 } else { -1.0 }
}

/// The colour of a multiplier's text.
fn tone_colour(theme: &Theme, multiplier: f64) -> Color {
    match MultiplierTone::of(multiplier) {
        MultiplierTone::Up => theme.mod_multiplier_up,
        MultiplierTone::Down => theme.mod_multiplier_down,
        MultiplierTone::Neutral => theme.text,
    }
}

fn mod_text_gap(theme: &Theme, tile_height: f32, scale: f32) -> f32 {
    theme.button_skew().tan() * tile_height / 2.0 + theme.widget_pad * scale * 0.5
}

/// `colour` darkened as osu-framework's `Color4.Darken`: each channel divided by
/// `1 + amount`, alpha kept.
// Ported from osu-framework 2026.921.1: osu.Framework/Extensions/Color4Extensions/Color4Extensions.cs (Darken)
fn darkened(colour: Color, amount: f32) -> Color {
    let scale = 1.0 / (1.0 + amount);
    Color {
        r: colour.r * scale,
        g: colour.g * scale,
        b: colour.b * scale,
        a: colour.a,
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/StarCounter.cs
/// Lazer's star counter: [`STAR_COUNT`] stars from `x`, centred on `centre_y`, each dim
/// behind and filled in the rating's colour as far as the rating reaches it; a partly reached
/// star is drawn smaller (ADR-0151).
fn push_star_counter(
    rect: &mut impl FnMut(&Rect, Color, Look),
    stars: f32,
    x: f32,
    centre_y: f32,
    theme: &Theme,
    scale: f32,
) {
    let size = theme.difficulty_star_size * scale;
    let pitch = size + theme.difficulty_star_spacing * scale;
    let colour = crate::star_colour::for_stars(stars);
    for i in 0..STAR_COUNT {
        let cx = x + i as f32 * pitch + size / 2.0;
        let at = |s: f32| Rect {
            x: cx - s / 2.0,
            y: centre_y - s / 2.0,
            width: s,
            height: s,
        };
        rect(
            &at(size),
            faded(theme.text, theme.difficulty_star_dim),
            Look::Star,
        );
        let fill = (stars - i as f32).clamp(0.0, 1.0);
        if fill > 0.0 {
            rect(&at(size * fill), colour, Look::Star);
        }
    }
}

/// Stars of [`push_star_counter`], as lazer's `StarCounter` default.
const STAR_COUNT: usize = 10;

/// The mod select panel's slide, followed on the draw side by the frame's time as the
/// snapshot opens and closes the panel (ADR-0152). A turn half way goes on from where the
/// panel is.
#[derive(Clone, Copy, Debug, Default)]
struct PanelSlide {
    /// Whether a snapshot was followed yet: the first one shows the panel as it is, so a
    /// view made while it is open (another language or scale) does not slide it in again.
    seen: bool,
    open: bool,
    start_ns: u64,
    /// The visibility when the slide started.
    from: f32,
}

impl PanelSlide {
    /// Starts sliding the other way when `open` changes.
    fn follow(&mut self, open: bool, now_ns: u64, theme: &Theme) {
        if !self.seen {
            self.seen = true;
            *self = PanelSlide {
                seen: true,
                open,
                start_ns: now_ns,
                from: if open { 1.0 } else { 0.0 },
            };
            return;
        }
        if open != self.open {
            // Where the panel is now, so that a turn does not jump.
            self.from = self.visibility(now_ns, 0.0, theme);
            self.open = open;
            self.start_ns = now_ns;
        }
    }

    /// How far a part `delay_ms` behind the first is in: 0 hidden, 1 in place.
    fn visibility(&self, now_ns: u64, delay_ms: f32, theme: &Theme) -> f32 {
        let elapsed = now_ns.saturating_sub(self.start_ns) as f32 / 1.0e6 - delay_ms;
        let part = |duration: f32| {
            if duration <= 0.0 {
                1.0
            } else {
                (elapsed / duration).clamp(0.0, 1.0)
            }
        };
        if self.open {
            self.from + (1.0 - self.from) * ease_out_quint(part(theme.mod_panel_in_ms))
        } else {
            self.from * (1.0 - ease_out_quint(part(theme.mod_panel_out_ms)))
        }
    }
}

/// Rows past the view's ends that are laid out ahead (ADR-0172).
const PREFILL_ROWS: usize = 3;

/// Marks a set row's key in [`RowMotion`], apart from its difficulties'.
const SET_KEY_BIT: u64 = 1 << 63;
/// Rows [`RowMotion`] follows at once.
const MOTION_ROWS: usize = 4 * MAX_VISIBLE_ROWS;

/// Where the rows shown are drawn, by key, in the list's reference pixels: each eases to its
/// place (lazer's `DampContinuously` of a panel's `DrawYPosition`, ADR-0172). Rows not drawn
/// for a frame are forgotten. Allocation-free.
#[derive(Clone, Debug)]
struct RowMotion {
    keys: [u64; MOTION_ROWS],
    tops: [f32; MOTION_ROWS],
    /// The frame each row was last drawn in; 0: free.
    seen: [u32; MOTION_ROWS],
    frame: u32,
    last_ns: u64,
}

impl Default for RowMotion {
    fn default() -> Self {
        RowMotion {
            keys: [0; MOTION_ROWS],
            tops: [0.0; MOTION_ROWS],
            seen: [0; MOTION_ROWS],
            frame: 0,
            last_ns: 0,
        }
    }
}

impl RowMotion {
    /// Starts a frame at `now_ns`; the part of the way left each row goes this frame, with
    /// `half_ms` the time it takes to halve it.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Carousel/Carousel.cs (Update: DampContinuously)
    fn begin(&mut self, now_ns: u64, half_ms: f32) -> f32 {
        let elapsed_ms = now_ns.saturating_sub(self.last_ns) as f64 / 1e6;
        let first = self.last_ns == 0;
        self.last_ns = now_ns;
        self.frame = self.frame.wrapping_add(1).max(2);
        if first || half_ms <= 0.0 {
            return 1.0;
        }
        (1.0 - 0.5f64.powf(elapsed_ms / f64::from(half_ms))) as f32
    }

    /// Where row `key` is drawn this frame, going to `target`; a row not drawn last frame
    /// starts at `start` (or at its place).
    fn track(&mut self, key: u64, target: f32, start: Option<f32>, ease: f32) -> f32 {
        let last = self.frame.wrapping_sub(1);
        if let Some(i) = (0..MOTION_ROWS)
            .find(|&i| self.keys[i] == key && (self.seen[i] == last || self.seen[i] == self.frame))
        {
            if self.seen[i] != self.frame {
                self.tops[i] += (target - self.tops[i]) * ease;
                self.seen[i] = self.frame;
            }
            return self.tops[i];
        }
        let top = start.unwrap_or(target);
        // A slot no row of this frame or the last holds.
        if let Some(i) =
            (0..MOTION_ROWS).find(|&i| self.seen[i] != last && self.seen[i] != self.frame)
        {
            self.keys[i] = key;
            self.tops[i] = top;
            self.seen[i] = self.frame;
        }
        top
    }
}

/// The rating each row's colour shows, by key: it eases to a new rating over a set time (the
/// info panel's 750 ms OutQuint) instead of jumping when mods or new values change it. A row
/// seen for the first time shows its rating at once. Allocation-free.
#[derive(Clone, Debug)]
struct StarMotion {
    keys: [u64; MOTION_ROWS],
    from: [f32; MOTION_ROWS],
    to: [f32; MOTION_ROWS],
    since_ns: [u64; MOTION_ROWS],
    seen: [u32; MOTION_ROWS],
    frame: u32,
    now_ns: u64,
    duration_ns: u64,
}

impl Default for StarMotion {
    fn default() -> Self {
        StarMotion {
            keys: [0; MOTION_ROWS],
            from: [0.0; MOTION_ROWS],
            to: [0.0; MOTION_ROWS],
            since_ns: [0; MOTION_ROWS],
            seen: [0; MOTION_ROWS],
            frame: 0,
            now_ns: 0,
            duration_ns: 0,
        }
    }
}

impl StarMotion {
    /// Starts a frame at `now_ns`; ratings ease over `duration_ns`.
    fn begin(&mut self, now_ns: u64, duration_ns: u64) {
        self.frame = self.frame.wrapping_add(1).max(2);
        self.now_ns = now_ns;
        self.duration_ns = duration_ns;
    }

    fn value(&self, i: usize) -> f32 {
        let t = if self.duration_ns == 0 {
            1.0
        } else {
            (self.now_ns.saturating_sub(self.since_ns[i]) as f64 / self.duration_ns as f64)
                .clamp(0.0, 1.0) as f32
        };
        self.from[i] + (self.to[i] - self.from[i]) * ease_out_quint(t)
    }

    /// The rating row `key` shows this frame, going to `to`.
    fn track(&mut self, key: u64, to: f32) -> f32 {
        let last = self.frame.wrapping_sub(1);
        if let Some(i) = (0..MOTION_ROWS)
            .find(|&i| self.keys[i] == key && (self.seen[i] == last || self.seen[i] == self.frame))
        {
            if (self.to[i] - to).abs() > f32::EPSILON {
                self.from[i] = self.value(i);
                self.to[i] = to;
                self.since_ns[i] = self.now_ns;
            }
            self.seen[i] = self.frame;
            return self.value(i);
        }
        if let Some(i) =
            (0..MOTION_ROWS).find(|&i| self.seen[i] != last && self.seen[i] != self.frame)
        {
            self.keys[i] = key;
            self.from[i] = to;
            self.to[i] = to;
            self.since_ns[i] = self.now_ns;
            self.seen[i] = self.frame;
        }
        to
    }
}

/// The expanded set and when it changed, so its difficulties slide out from under its row
/// and the rows below follow, while the set that closed slides shut (ADR-0136).
#[derive(Clone, Copy, Debug, Default)]
struct Expansion {
    /// Key of the expanded set.
    key: Option<u64>,
    /// When it last changed; 0 before any change.
    start_ns: u64,
    /// Sets still closing, newest first: a quick change closes a half-open set from where
    /// it is, and the ones closing before go on (ADR-0167).
    closing: [Option<Closing>; CLOSING],
}

/// Sets closing at once at most; an older one is shut at once.
const CLOSING: usize = 4;

/// A set closing: its key, difficulty count, how far open it was when it began to close
/// (0 to 1) and when that was.
#[derive(Clone, Copy, Debug)]
struct Closing {
    key: u64,
    count: usize,
    from: f32,
    start_ns: u64,
}

/// One frame of an [`Expansion`]: where the opening and the closing sets are in the list.
#[derive(Clone, Copy, Debug)]
struct ExpansionFrame {
    /// How far the change is, 0 to 1, eased.
    t: f32,
    /// The opening set's index and difficulty count.
    open: Option<(usize, usize)>,
    /// The closing sets' indices, difficulty counts and how open they still are, 0 to 1.
    closed: [Option<(usize, usize, f32)>; CLOSING],
}

impl Expansion {
    /// Follows the selection: a new expanded set starts the animation from `now_ns`. The
    /// same set in a republished list (another index, same key) does not.
    fn track(
        &mut self,
        list: &SongList,
        selected: Option<(usize, usize)>,
        now_ns: u64,
        theme: &Theme,
    ) -> ExpansionFrame {
        let open = selected.and_then(|(set, _)| Some((set, list.sets.get(set)?)));
        let key = open.map(|(_, s)| s.key);
        let duration = (theme.song_expand_ms.max(0.0) * 1e6) as u64;
        let eased = |start: u64| {
            if start == 0 {
                1.0
            } else {
                ease_out_quint(progress(start, now_ns, duration))
            }
        };
        if key != self.key {
            let closed = self.key.and_then(|old| {
                let count = list.sets.iter().find(|s| s.key == old)?.difficulties.len();
                Some(Closing {
                    key: old,
                    count,
                    from: eased(self.start_ns),
                    start_ns: now_ns.max(1),
                })
            });
            self.closing.rotate_right(1);
            self.closing[0] = closed;
            // The first selection opens at once.
            self.start_ns = if self.key.is_some() { now_ns.max(1) } else { 0 };
            self.key = key;
        }
        // Shut ones are forgotten.
        for slot in &mut self.closing {
            if slot.is_some_and(|c| eased(c.start_ns) >= 1.0) {
                *slot = None;
            }
        }
        let t = eased(self.start_ns);
        if t >= 1.0 && self.closing.iter().all(Option::is_none) {
            // Settled: nothing moves, and the list is not searched.
            return ExpansionFrame {
                t: 1.0,
                open: None,
                closed: [None; CLOSING],
            };
        }
        // No further than the snapshot carries rows past the window, so nothing slides in
        // from where there is no row to draw.
        let cap = |count: usize| count.min(super::ROW_PAD);
        let closed = self.closing.map(|slot| {
            let c = slot?;
            let index = list.sets.iter().position(|s| s.key == c.key)?;
            Some((index, cap(c.count), c.from * (1.0 - eased(c.start_ns))))
        });
        ExpansionFrame {
            t,
            open: open
                .filter(|_| t < 1.0)
                .map(|(index, s)| (index, cap(s.difficulties.len()))),
            closed,
        }
    }
}

/// How far right the rows that are not selected start, sliding when the selection changes
/// (ADR-0149, lazer's `Panel.updateXOffset`): the set and the difficulty selected before and
/// when it changed, by key and by index into their set.
#[derive(Debug, Default)]
struct Shift {
    set: Option<u64>,
    difficulty: Option<(u64, usize)>,
    old_set: Option<u64>,
    old_difficulty: Option<(u64, usize)>,
    /// When the set and the difficulty last changed; 0 before any change (no slide).
    set_start_ns: u64,
    difficulty_start_ns: u64,
    /// How far the old set and difficulty had slid in when they were left, 0 to 1.
    old_set_from: f32,
    old_difficulty_from: f32,
}

/// One frame of a [`Shift`]: the selection by index and the slides' progress.
#[derive(Clone, Copy, Debug)]
struct ShiftFrame {
    set: Option<usize>,
    difficulty: Option<(usize, usize)>,
    old_set: Option<usize>,
    old_difficulty: Option<(usize, usize)>,
    set_t: f32,
    difficulty_t: f32,
    old_set_from: f32,
    old_difficulty_from: f32,
    set_shift: f32,
    difficulty_shift: f32,
}

impl Shift {
    fn track(
        &mut self,
        list: &SongList,
        selected: Option<(usize, usize)>,
        now_ns: u64,
        theme: &Theme,
    ) -> ShiftFrame {
        let set_key = selected.and_then(|(set, _)| Some(list.sets.get(set)?.key));
        let difficulty = selected.zip(set_key).map(|((_, d), key)| (key, d));
        let duration = (theme.song_shift_ms.max(0.0) * 1e6) as u64;
        let t = |start: u64| {
            if start == 0 {
                1.0
            } else {
                ease_out_quint(progress(start, now_ns, duration))
            }
        };
        // The first selection is there at once. One left before its slide ended slides
        // back from where it got to (ADR-0167).
        if set_key != self.set {
            self.old_set = self.set;
            self.old_set_from = t(self.set_start_ns);
            self.set_start_ns = if self.set.is_some() { now_ns.max(1) } else { 0 };
            self.set = set_key;
        }
        if difficulty != self.difficulty {
            self.old_difficulty = self.difficulty;
            self.old_difficulty_from = t(self.difficulty_start_ns);
            self.difficulty_start_ns = if self.difficulty.is_some() {
                now_ns.max(1)
            } else {
                0
            };
            self.difficulty = difficulty;
        }
        let (set_t, difficulty_t) = (t(self.set_start_ns), t(self.difficulty_start_ns));
        let index = |key: u64| list.sets.iter().position(|s| s.key == key);
        // Only while a slide runs is the list searched for the old keys.
        let old_set = self.old_set.filter(|_| set_t < 1.0).and_then(index);
        let old_difficulty = self
            .old_difficulty
            .filter(|_| difficulty_t < 1.0)
            .and_then(|(key, d)| Some((index(key)?, d)));
        ShiftFrame {
            set: selected.map(|(set, _)| set),
            difficulty: selected,
            old_set,
            old_difficulty,
            set_t,
            difficulty_t,
            old_set_from: self.old_set_from,
            old_difficulty_from: self.old_difficulty_from,
            set_shift: theme.song_set_shift,
            difficulty_shift: theme.song_difficulty_shift,
        }
    }
}

impl ShiftFrame {
    /// How far right `row` starts, in reference pixels.
    fn of(&self, row: Row) -> f32 {
        match row {
            Row::Set(set) => {
                let full = self.set_shift;
                if self.set == Some(set) {
                    full * (1.0 - self.set_t)
                } else if self.old_set == Some(set) {
                    full * (1.0 - self.old_set_from * (1.0 - self.set_t))
                } else {
                    full
                }
            }
            Row::Difficulty { set, difficulty } => {
                let full = self.difficulty_shift;
                if self.difficulty == Some((set, difficulty)) {
                    full * (1.0 - self.difficulty_t)
                } else if self.old_difficulty == Some((set, difficulty)) {
                    full * (1.0 - self.old_difficulty_from * (1.0 - self.difficulty_t))
                } else {
                    full
                }
            }
        }
    }
}

impl ExpansionFrame {
    /// Rows the list may move by: drawn past the window's ends meanwhile.
    fn reach(&self) -> usize {
        self.open.map_or(0, |o| o.1) + self.closed.iter().flatten().map(|c| c.1).sum::<usize>()
    }

    /// Rows `row` is moved down by (negative: up), and its opacity.
    fn offset(&self, row: Row) -> (f32, f32) {
        let set = match row {
            Row::Set(set) | Row::Difficulty { set, .. } => set,
        };
        let mut rows = 0.0;
        let mut alpha = 1.0;
        let left = 1.0 - self.t;
        if let Some((open, count)) = self.open {
            match row {
                // Out from under the set's row, the later ones from further.
                Row::Difficulty { set, difficulty } if set == open => {
                    rows -= left * (difficulty as f32 + 1.0);
                    alpha = self.t;
                }
                _ if set > open => rows -= left * count as f32,
                _ => {}
            }
        }
        for &(closed, count, still_open) in self.closed.iter().flatten() {
            if set > closed {
                rows += still_open * count as f32;
            }
        }
        (rows, alpha)
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

    fn thumb(width: u32, height: u32) -> AtlasRegion {
        use slam_render::{Extent, Origin};
        AtlasRegion {
            texture: slam_render::textures::TextureId(5),
            origin: Origin::new(0, 0),
            size: Extent::new(width, height),
            uv: UvRect::FULL,
        }
    }

    /// The width of `text` laid out in `face`.
    fn width_in(type_setter: &mut Typesetter, text: &str, face: Face) -> f32 {
        let mut line = Line::new(text);
        type_setter.lay_out_face(&mut line, face);
        line.layout.width()
    }

    #[test]
    fn lazers_faces_are_used_per_element() {
        use crate::song_select::{DifficultyRow, SearchSource, SetRow};
        let theme = Theme::default();
        let layout = SongSelectLayout::new(1280.0, 720.0, &theme);
        let mut type_setter =
            Typesetter::new(Arc::new(FontRegistry::with_builtin()), theme.font_weight);
        let list = SongList {
            sets: vec![SetRow {
                key: 1,
                artist: "Artist".to_owned(),
                title: "Title of the song".to_owned(),
                creator: "mapper".to_owned(),
                difficulties: vec![DifficultyRow::new(SearchSource {
                    difficulty: "Insane difficulty",
                    ..SearchSource::default()
                })],
            }],
        };
        let mut fill = |row: Row| {
            let mut slot = RowSlot::new();
            let ellipsis = [0.0; 4];
            assert!(SongSelectView::fill(
                &mut slot,
                &mut type_setter,
                &theme,
                &layout,
                ellipsis,
                "pp",
                1,
                &list,
                row,
                0.0,
                0,
            ));
            slot
        };
        let set = fill(Row::Set(0));
        let difficulty = fill(Row::Difficulty {
            set: 0,
            difficulty: 0,
        });
        let scale = layout.scale;
        // The set's title is bold: as wide as the heading face makes it, wider than regular.
        let title = set.primary.text.clone();
        let bold = width_in(
            &mut type_setter,
            &title,
            theme.heading_face(theme.song_title_size * scale),
        );
        let regular = width_in(
            &mut type_setter,
            &title,
            theme.regular_face(theme.song_title_size * scale),
        );
        assert!((set.primary.layout.width() - bold).abs() < 1e-3);
        assert!(bold > regular);
        // The mapper and the difficulty's name are semibold.
        let semibold = |text: &str, face: Face, type_setter: &mut Typesetter| {
            width_in(
                type_setter,
                text,
                Face {
                    weight: theme.semibold_weight,
                    ..face
                },
            )
        };
        let note = semibold("mapper", theme.note_face(scale), &mut type_setter);
        assert!((set.secondary.layout.width() - note).abs() < 1e-3);
        let name = semibold(
            "Insane difficulty",
            theme.caption_face(scale),
            &mut type_setter,
        );
        assert!((difficulty.primary.layout.width() - name).abs() < 1e-3);
        assert!(
            name > width_in(
                &mut type_setter,
                "Insane difficulty",
                theme.caption_face(scale)
            )
        );
    }

    #[test]
    fn a_full_plate_thumbnail_is_the_plate_dimmed_from_left_to_right() {
        let theme = Theme::default();
        let plate = Rect {
            x: 100.0,
            y: 40.0,
            width: 500.0,
            height: 72.0,
        };
        let shape = plate_shape(thumb(700, 100), &plate, 40.0, 72.0, 3.0, 0.8, &theme, 1.0);
        assert_eq!(shape.position, [103.0, 40.0]);
        assert_eq!(shape.size, [500.0, 72.0]);
        assert_eq!(shape.skew, theme.button_skew());
        assert_eq!(shape.radius, theme.button_radius);
        assert!(shape.upright);
        assert_eq!(shape.texture, slam_render::textures::TextureId(5));
        // Darker on the left than on the right, faded as a whole.
        assert_eq!(shape.colour.r, theme.song_plate_dim_left);
        assert_eq!(shape.colour.a, 0.8);
        match shape.gradient {
            Some(slam_render::shape::Gradient::Horizontal(end)) => {
                assert_eq!(end.r, theme.song_plate_dim_right);
                assert_eq!(end.a, 0.8);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_full_plate_image_covers_the_plate_whatever_its_aspect() {
        let (w, h, tan) = (500.0, 72.0, 0.2);
        let box_aspect = (w + tan * h) / h;
        for (iw, ih) in [(1000.0, 100.0), (100.0, 1000.0), (1920.0, 1080.0)] {
            let uv = plate_uv(UvRect::FULL, [iw, ih], w, h, tan, (0.0, 1.0));
            let shown = ((uv.u1 - uv.u0) * iw) / ((uv.v1 - uv.v0) * ih);
            assert!((shown - box_aspect).abs() < 1e-3, "{iw}x{ih}: {shown}");
            assert!(uv.u0 >= -1e-6 && uv.u1 <= 1.0 + 1e-6, "{uv:?}");
            assert!(uv.v0 >= -1e-6 && uv.v1 <= 1.0 + 1e-6, "{uv:?}");
        }
        // A plate cut at the top shows the lower part of the same window, at the same scale.
        let whole = plate_uv(UvRect::FULL, [1920.0, 1080.0], w, h, tan, (0.0, 1.0));
        let cut = plate_uv(UvRect::FULL, [1920.0, 1080.0], w, h, tan, (0.5, 1.0));
        assert!((cut.v1 - whole.v1).abs() < 1e-6);
        assert!((cut.v0 - (whole.v0 + whole.v1) / 2.0).abs() < 1e-6);
    }

    fn sets(counts: &[usize]) -> SongList {
        use crate::song_select::{DifficultyRow, SearchSource, SetRow};
        SongList {
            sets: counts
                .iter()
                .enumerate()
                .map(|(i, &n)| SetRow {
                    key: 100 + i as u64,
                    artist: String::new(),
                    title: String::new(),
                    creator: String::new(),
                    difficulties: (0..n)
                        .map(|_| DifficultyRow::new(SearchSource::default()))
                        .collect(),
                })
                .collect(),
        }
    }

    const MS: u64 = 1_000_000;

    #[test]
    fn rows_ease_to_new_places_and_new_ones_start_where_asked() {
        let mut m = RowMotion::default();
        let e = m.begin(1_000 * MS, 50.0);
        assert_eq!(e, 1.0, "the first frame is in place");
        assert_eq!(m.track(1, 100.0, None, e), 100.0);
        // The row's place moves 60 down: after one half time it is half way.
        let e = m.begin(1_050 * MS, 50.0);
        let y = m.track(1, 160.0, None, e);
        assert!((y - 130.0).abs() < 1e-3, "{y}");
        // A new row starts where it is told, then eases.
        assert_eq!(m.track(2, 200.0, Some(130.0), e), 130.0);
        let e = m.begin(1_100 * MS, 50.0);
        let y = m.track(2, 200.0, None, e);
        assert!((y - 165.0).abs() < 1e-3, "{y}");
        // A row not drawn for a frame is forgotten: back at its place at once.
        let e = m.begin(1_150 * MS, 50.0);
        let _ = m.begin(1_200 * MS, 50.0);
        assert_eq!(m.track(1, 300.0, None, e), 300.0);
    }

    /// A difficulty row rated `stars` and `pp`, and the text of a slot filled for it.
    fn rated(stars: f32, pp: f32) -> crate::song_select::DifficultyRow {
        let mut d = crate::song_select::DifficultyRow::new(crate::song_select::SearchSource {
            difficulty: "Insane",
            ..Default::default()
        });
        d.stars = Some(stars);
        d.pp = Some(pp);
        d
    }

    fn roll_rig() -> (Typesetter, Theme, SongSelectLayout, RowSlot) {
        let theme = Theme {
            song_difficulty_info: crate::song_select::DifficultyInfo::StarsAndPp,
            ..Theme::default()
        };
        let layout = SongSelectLayout::new(1280.0, 720.0, &theme);
        let type_setter = Typesetter::new(
            std::sync::Arc::new(slam_text::FontRegistry::with_builtin()),
            700,
        );
        (type_setter, theme, layout, RowSlot::new())
    }

    /// Fills `slot` with `d` as `fill` does for a difficulty row.
    fn fill_slot(slot: &mut RowSlot, d: &crate::song_select::DifficultyRow) {
        slot.secondary.text.clear();
        super::super::push_stars(&mut slot.secondary.text, d.stars.unwrap());
        slot.rolled_stars = (d.stars.unwrap() * 100.0).round() as i32;
        slot.pp.text.clear();
        super::super::push_pp(&mut slot.pp.text, d.pp.unwrap(), "pp");
        slot.rolled_pp = d.pp.unwrap().round() as i32;
    }

    #[test]
    fn a_new_rating_rolls_the_rows_stars_and_pp_and_a_first_row_does_not() {
        let (mut ts, theme, layout, mut slot) = roll_rig();
        let (mut stars, mut pp) = (StarMotion::default(), StarMotion::default());
        let d = 750 * MS;
        let mut frame = |slot: &mut RowSlot, row: &crate::song_select::DifficultyRow, now: u64| {
            stars.begin(now, d);
            pp.begin(now, d);
            SongSelectView::roll(
                slot,
                &mut ts,
                (&mut stars, &mut pp),
                &theme,
                &layout,
                "pp",
                row,
            );
            (slot.secondary.text.clone(), slot.pp.text.clone())
        };
        // First appearance: the numbers are at once what they are.
        let low = rated(2.0, 100.0);
        fill_slot(&mut slot, &low);
        assert_eq!(
            frame(&mut slot, &low, 1_000 * MS),
            ("2.00".into(), "100pp".into())
        );
        assert_eq!(
            frame(&mut slot, &low, 1_016 * MS),
            ("2.00".into(), "100pp".into())
        );
        // A mod rates it higher; the republished row's text is the target at once, and the
        // roll takes it back to where it was.
        let high = rated(6.0, 500.0);
        fill_slot(&mut slot, &high);
        let (s, p) = frame(&mut slot, &high, 1_032 * MS);
        assert_eq!((s.as_str(), p.as_str()), ("2.00", "100pp"));
        let (s, p) = frame(&mut slot, &high, 1_032 * MS + 200 * MS);
        let (s, p): (f32, f32) = (
            s.parse().unwrap(),
            p.trim_end_matches("pp").parse().unwrap(),
        );
        assert!(s > 2.5 && s < 6.0 && p > 150.0 && p < 500.0, "{s} {p}");
        // The text is laid out again with it.
        assert!(slot.secondary.layout.width() > 0.0);
        let end = frame(&mut slot, &high, 1_032 * MS + 800 * MS);
        assert_eq!(end, ("6.00".into(), "500pp".into()));
    }

    #[test]
    fn a_new_rating_eases_the_rows_colour_and_a_new_row_shows_it_at_once() {
        let mut m = StarMotion::default();
        let d = 750 * MS;
        m.begin(1_000 * MS, d);
        // A row first seen shows its rating at once.
        assert_eq!(m.track(7, 3.0), 3.0);
        m.begin(1_016 * MS, d);
        assert_eq!(m.track(7, 3.0), 3.0);
        // A mod changes the rating: the shown value starts from the old one...
        m.begin(1_032 * MS, d);
        let first = m.track(7, 6.0);
        assert!((first - 3.0).abs() < 1e-3, "{first}");
        // ...is on its way part-way (OutQuint is past half at a third of the time)...
        m.begin(1_032 * MS + 250 * MS, d);
        let mid = m.track(7, 6.0);
        assert!(mid > 4.5 && mid < 6.0, "{mid}");
        // ...and rests on the new rating.
        m.begin(1_032 * MS + 800 * MS, d);
        assert_eq!(m.track(7, 6.0), 6.0);
        // A change mid-way starts from where the value is.
        m.begin(2_000 * MS, d);
        m.track(7, 2.0);
        m.begin(2_000 * MS + 100 * MS, d);
        let cut = m.track(7, 2.0);
        m.begin(2_000 * MS + 100 * MS + MS, d);
        let turned = m.track(7, 9.0);
        assert!((turned - cut).abs() < 0.2, "{turned} vs {cut}");
    }

    #[test]
    fn the_first_selection_opens_at_once() {
        let theme = Theme::default();
        let list = sets(&[3, 2]);
        let mut e = Expansion::default();
        let frame = e.track(&list, Some((0, 0)), 5 * MS, &theme);
        assert_eq!(frame.reach(), 0);
        assert_eq!(
            frame.offset(Row::Difficulty {
                set: 0,
                difficulty: 2
            }),
            (0.0, 1.0)
        );
    }

    #[test]
    fn a_new_set_slides_its_difficulties_out_and_the_old_one_shut() {
        let theme = Theme::default();
        let list = sets(&[3, 2, 4]);
        let mut e = Expansion::default();
        e.track(&list, Some((0, 0)), 0, &theme);
        // Set 2 opens at 1 s; set 0 (3 difficulties) closes.
        let start = e.track(&list, Some((2, 0)), 1_000 * MS, &theme);
        assert_eq!(start.t, 0.0);
        assert_eq!(start.reach(), 4 + 3);
        // Its difficulties start under its row (which is where it was: below the closing
        // set's rows), unseen.
        let (rows, alpha) = start.offset(Row::Difficulty {
            set: 2,
            difficulty: 1,
        });
        let (set_rows, _) = start.offset(Row::Set(2));
        assert_eq!(set_rows, 3.0);
        assert_eq!((rows - set_rows, alpha), (-2.0, 0.0));
        // Rows between the two sets stay where they were: below the closing set's rows.
        assert_eq!(start.offset(Row::Set(1)), (3.0, 1.0));
        assert_eq!(start.offset(Row::Set(0)), (0.0, 1.0));
        // Halfway they are on their way; after the duration everything is in place.
        let mid = e.track(&list, Some((2, 1)), 1_000 * MS + 60 * MS, &theme);
        let (rows, alpha) = mid.offset(Row::Difficulty {
            set: 2,
            difficulty: 1,
        });
        let (set_rows, _) = mid.offset(Row::Set(2));
        let under = rows - set_rows;
        assert!(under > -2.0 && under < 0.0 && alpha > 0.0 && alpha < 1.0);
        let done = e.track(&list, Some((2, 1)), 2_000 * MS, &theme);
        assert_eq!(done.reach(), 0);
        assert_eq!(done.offset(Row::Set(1)), (0.0, 1.0));
    }

    #[test]
    fn a_quick_change_closes_a_half_open_set_from_where_it_is() {
        let theme = Theme::default();
        let list = sets(&[3, 2, 4]);
        let mut e = Expansion::default();
        e.track(&list, Some((0, 0)), 0, &theme);
        // Set 1 opens at 1 s, and is left for set 2 a little later, half open.
        e.track(&list, Some((1, 0)), 1_000 * MS, &theme);
        let half = e.track(&list, Some((1, 0)), 1_000 * MS + 20 * MS, &theme);
        let open_part = half.t;
        assert!(open_part > 0.0 && open_part < 1.0);
        // Set 2 stays where it is drawn: its row moves up by set 1's two difficulties in
        // the list, and the offset makes up for them and for set 0 still closing.
        let before = half.offset(Row::Set(2)).0;
        let after = e
            .track(&list, Some((2, 0)), 1_000 * MS + 20 * MS, &theme)
            .offset(Row::Set(2))
            .0;
        assert!((before - (after - 2.0)).abs() < 1e-4, "{before} -> {after}");
    }

    #[test]
    fn a_selection_left_mid_slide_slides_back_from_there() {
        let theme = Theme::default();
        let list = sets(&[3, 2, 4]);
        let mut shift = Shift::default();
        shift.track(&list, Some((0, 0)), 0, &theme);
        shift.track(&list, Some((1, 0)), 1_000 * MS, &theme);
        let mid = shift.track(&list, Some((1, 0)), 1_000 * MS + 20 * MS, &theme);
        let before = mid.of(Row::Set(1));
        let after = shift
            .track(&list, Some((2, 0)), 1_000 * MS + 20 * MS, &theme)
            .of(Row::Set(1));
        assert!((before - after).abs() < 1e-4, "{before} -> {after}");
    }

    #[test]
    fn a_republished_list_with_the_same_set_does_not_animate() {
        let theme = Theme::default();
        let mut e = Expansion::default();
        e.track(&sets(&[3, 2]), Some((1, 0)), 0, &theme);
        // The set moved to index 0 (its key is 101), as a resort would do.
        let mut moved = sets(&[2, 3]);
        moved.sets.swap(0, 1);
        moved.sets[0].key = 101;
        moved.sets[1].key = 100;
        let frame = e.track(&moved, Some((0, 0)), 500 * MS, &theme);
        assert_eq!(frame.reach(), 0);
    }

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
    fn columns_slide_in_from_below_and_above_in_turn() {
        assert_eq!(slide_direction(0), 1.0);
        assert_eq!(slide_direction(1), -1.0);
        assert_eq!(slide_direction(2), 1.0);
        assert_eq!(slide_direction(3), -1.0);
        let theme = Theme::default();
        assert_eq!(theme.mod_column_stagger_ms, 30.0);
        assert_eq!(theme.mod_panel_in_ms, 400.0);
        assert_eq!(theme.mod_panel_distance, 700.0);
        assert!(theme.mod_panel_out_ms <= 250.0);
    }

    #[test]
    fn the_multiplier_is_coloured_by_its_side_of_one() {
        let theme = Theme::default();
        assert_eq!(tone_colour(&theme, 1.2), theme.mod_multiplier_up);
        assert_eq!(tone_colour(&theme, 0.8), theme.mod_multiplier_down);
        assert_eq!(tone_colour(&theme, 1.0), theme.text);
    }

    #[test]
    fn the_mod_panel_slides_in_and_out_and_turns_where_it_is() {
        let theme = Theme::default();
        let ms = 1_000_000u64;
        let mut slide = PanelSlide::default();
        slide.follow(false, 0, &theme);
        assert_eq!(slide.visibility(0, 0.0, &theme), 0.0);
        slide.follow(true, 1000 * ms, &theme);
        let at = |slide: &PanelSlide, t: u64, delay: f32| slide.visibility(t * ms, delay, &theme);
        assert_eq!(at(&slide, 1000, 0.0), 0.0);
        // OutQuint: most of the way early; a later column lags behind.
        let early = at(&slide, 1100, 0.0);
        assert!(early > 0.7 && early < 1.0, "{early}");
        assert!(at(&slide, 1100, 30.0) < early);
        assert_eq!(at(&slide, 1000 + theme.mod_panel_in_ms as u64, 0.0), 1.0);
        assert_eq!(
            at(&slide, 1000 + theme.mod_panel_in_ms as u64 + 30, 30.0),
            1.0
        );
        // Closing half way through opening goes on from where the panel is.
        let mut turn = slide;
        let now = at(&turn, 1050, 0.0);
        turn.follow(false, 1050 * ms, &theme);
        assert!((at(&turn, 1050, 0.0) - now).abs() < 1e-6);
        assert!(at(&turn, 1100, 0.0) < now);
        assert_eq!(at(&turn, 1050 + theme.mod_panel_out_ms as u64, 0.0), 0.0);
        // Following the same state again changes nothing.
        let before = at(&slide, 1200, 0.0);
        slide.follow(true, 1200 * ms, &theme);
        assert_eq!(at(&slide, 1200, 0.0), before);
    }
}
