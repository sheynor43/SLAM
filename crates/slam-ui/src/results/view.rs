//! Draw side of the results. Texts are laid out when the screen is entered (the data's id
//! changes) and again when the window size changes; a steady frame only pushes sprites and
//! does not allocate. Text buffers are reserved up front.

use std::fmt::Write as _;
use std::sync::Arc;

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::shape::ShapeBatch;
use slam_render::sprite::{Sprite, SpriteBatch};

use crate::widgets::Look;
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey, push_layout};

use super::stable::{self, HeaderKind, Item, Metrics, Part, StableSkin, StableSprite};
use super::{
    ResultsButton, ResultsData, ResultsLayout, ResultsSnapshot, StatKind, write_accuracy,
    write_fixed2, write_grouped, write_stat,
};
use crate::menu::Rect;
use crate::menu::view::{Face, Frame, Line, Typesetter};
use crate::screen::Pose;
use crate::song_select::view::{fit, fit_face};
use crate::theme::{Theme, faded};

const ELLIPSIS: char = '\u{2026}';
/// Bytes reserved up front in every text buffer.
const TEXT_CAPACITY: usize = 256;
/// Rows of the left column.
const INFO_ROWS: usize = 6;
/// The mods' row, where "none" is drawn when there are no mods.
const MODS_ROW: usize = 3;

/// The screen's localised texts, already formatted.
#[derive(Clone)]
pub struct ResultsLabels {
    pub score: String,
    pub accuracy: String,
    pub max_combo: String,
    pub mods: String,
    /// Shown instead of the mods when there are none.
    pub no_mods: String,
    pub average_hit_error: String,
    pub unstable_rate: String,
    /// Shown instead of a hit error statistic the play has no value for.
    pub not_available: String,
    /// Names of the judgement rows, in [`StatKind::ALL`] order.
    pub stats: [String; StatKind::COUNT],
    pub retry: String,
    pub back: String,
    pub hint: String,
    /// Heading of the details page of the stable look (ADR-0161).
    pub details_title: String,
}

impl std::fmt::Debug for ResultsLabels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultsLabels")
            .field("hint", &self.hint)
            .finish_non_exhaustive()
    }
}

fn line() -> Line {
    let mut line = Line::new("");
    line.text.reserve(TEXT_CAPACITY);
    line
}

/// The health line above half health and below it, as stable draws them.
const HEALTH_HIGH: Color = Color::rgb(0.0, 1.0, 0.0);
const HEALTH_LOW: Color = Color::rgb(1.0, 0.0, 0.0);

/// A line from `from` to `to` as one rotated sprite `thickness` thick.
fn line_segment(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    from: [f32; 2],
    to: [f32; 2],
    thickness: f32,
    colour: Color,
) {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let mut sprite = Sprite::new(
        white.texture,
        white.uv,
        from,
        [(dx * dx + dy * dy).sqrt(), thickness],
    );
    sprite.origin = [0.0, thickness / 2.0];
    sprite.rotation = dy.atan2(dx);
    sprite.colour = colour;
    shapes.push(&sprite);
}

/// A skin button drawn into `rect`: grown by `1 + 0.01 a` about its centre and brightened
/// by [`stable::hover_factor`], `a` being the hover animation. The sprite itself gets
/// brighter; nothing is drawn behind or over it.
// Ported from McOsu db2add2: src/App/Osu/OsuUIBackButton.cpp (draw)
fn push_button(
    shapes: &mut ShapeBatch,
    sprite: &StableSprite,
    rect: &Rect,
    amount: f32,
    opacity: f32,
) {
    let grow = 1.0 + 0.01 * amount;
    let size = [rect.width * grow, rect.height * grow];
    let at = [
        rect.x + rect.width / 2.0 - size[0] / 2.0,
        rect.y + rect.height / 2.0 - size[1] / 2.0,
    ];
    let gain = stable::hover_factor(amount);
    let mut quad = Sprite::new(sprite.texture, sprite.uv, at, size);
    quad.colour = faded(Color::rgb(gain, gain, gain), opacity);
    shapes.push(&quad);
}

/// A row of the details grid: a label and its value, in the left or the right column.
struct GridRow {
    label: Line,
    value: Line,
    right: bool,
}

/// The draw side of [`super::Results`].
pub struct ResultsView {
    theme: Theme,
    type_setter: Typesetter,
    frame: Frame,
    layout: ResultsLayout,
    /// The data id the dynamic texts are for.
    holds: Option<u64>,
    ellipsis: f32,
    title: Line,
    /// The beatmap line in the stable look's top bar, and what it was laid out for.
    stable_title: Line,
    stable_by: Line,
    stable_played: Line,
    details_title: Line,
    details: Vec<Line>,
    grid: Vec<GridRow>,
    stable_holds: Option<(u64, u32, u32, u32)>,
    /// The frame's time, for the animations of the stable look.
    now_ns: u64,
    rank: Line,
    info_labels: [Line; INFO_ROWS],
    info_values: [Line; INFO_ROWS],
    no_mods: Line,
    not_available: Line,
    stat_labels: [Line; StatKind::COUNT],
    stat_values: [Line; StatKind::COUNT],
    buttons: [Line; 2],
    hint: Line,
}

impl std::fmt::Debug for ResultsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultsView")
            .field("holds", &self.holds)
            .finish_non_exhaustive()
    }
}

impl ResultsView {
    /// A view with the fonts of `registry`. Lays the static texts out once: call it off
    /// the hot paths.
    pub fn new(registry: Arc<FontRegistry>, theme: Theme, labels: &ResultsLabels) -> ResultsView {
        let (width, height) = (1280.0, theme.reference_height);
        let labelled = |text: &str| Line::new(text);
        let mut view = ResultsView {
            type_setter: Typesetter::new(registry, theme.font_weight),
            frame: Frame { width, height },
            layout: ResultsLayout::new(width, height, &theme),
            holds: None,
            ellipsis: 0.0,
            title: line(),
            stable_title: line(),
            stable_by: line(),
            stable_played: line(),
            details_title: labelled(&labels.details_title),
            details: Vec::new(),
            grid: Vec::new(),
            stable_holds: None,
            now_ns: 0,
            rank: line(),
            info_labels: [
                labelled(&labels.score),
                labelled(&labels.accuracy),
                labelled(&labels.max_combo),
                labelled(&labels.mods),
                labelled(&labels.average_hit_error),
                labelled(&labels.unstable_rate),
            ],
            info_values: std::array::from_fn(|_| line()),
            no_mods: labelled(&labels.no_mods),
            not_available: labelled(&labels.not_available),
            stat_labels: labels.stats.each_ref().map(|text| labelled(text)),
            stat_values: std::array::from_fn(|_| line()),
            buttons: [labelled(&labels.retry), labelled(&labels.back)],
            hint: labelled(&labels.hint),
            theme,
        };
        view.lay_out();
        view
    }

    /// Sets the time the animations of the next `push` are drawn at.
    pub fn set_time(&mut self, now_ns: u64) {
        self.now_ns = now_ns;
    }

    fn text_size(&self) -> f32 {
        self.theme.results_text_size * self.layout.scale
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
        self.layout = ResultsLayout::new(width, height, &self.theme);
        let scale = self.layout.scale;
        let size = self.text_size();
        for line in self
            .info_labels
            .iter_mut()
            .chain(&mut self.stat_labels)
            .chain(&mut self.info_values)
            .chain(&mut self.stat_values)
        {
            self.type_setter.lay_out(line, size);
        }
        self.type_setter.lay_out(&mut self.no_mods, size);
        self.type_setter.lay_out(&mut self.not_available, size);
        for button in &mut self.buttons {
            self.type_setter
                .lay_out(button, self.theme.button_text_size * scale);
        }
        self.type_setter
            .lay_out(&mut self.hint, self.theme.song_detail_size * scale);
        let heading = Face {
            size: self.theme.results_title_size * scale * 1.5,
            weight: self.theme.caption_weight,
            width: slam_text::NORMAL_WIDTH,
        };
        self.type_setter
            .lay_out_face(&mut self.details_title, heading);
        let mut probe = Line::new(&ELLIPSIS.to_string());
        self.type_setter
            .lay_out(&mut probe, self.theme.results_title_size * scale);
        self.ellipsis = probe.layout.width();
        // The title and rank depend on the data; they are laid out again from it.
        self.holds = None;
        self.stable_holds = None;
    }

    /// Writes the texts of `data` and lays them out.
    fn fill(&mut self, data: &ResultsData) {
        let size = self.text_size();
        let scale = self.layout.scale;
        self.title.text.clear();
        self.title.text.push_str(&data.title);
        fit(
            &mut self.type_setter,
            &mut self.title,
            self.theme.results_title_size * scale,
            self.layout.title.width * 0.9,
            self.ellipsis,
        );
        self.rank.text.clear();
        self.rank.text.push_str(data.rank.letters());
        self.type_setter
            .lay_out(&mut self.rank, self.theme.results_rank_size * scale);

        let [score, accuracy, combo, mods, average, rate] = &mut self.info_values;
        for line in [
            &mut *score,
            &mut *accuracy,
            &mut *combo,
            &mut *mods,
            &mut *average,
            &mut *rate,
        ] {
            line.text.clear();
        }
        let _ = write_grouped(&mut score.text, data.score);
        let _ = write_accuracy(&mut accuracy.text, data.accuracy);
        let _ = write!(combo.text, "{}x/{}x", data.combo, data.max_combo);
        for (i, acronym) in data.mods.iter().enumerate() {
            if i > 0 {
                mods.text.push(' ');
            }
            mods.text.push_str(acronym.as_str());
        }
        // Without a value the row shows the "not available" text instead, see `push`.
        if let Some(text) = &data.average_hit_error_text {
            average.text.push_str(text);
        }
        if let Some(rate_value) = data.unstable_rate {
            let _ = write_fixed2(&mut rate.text, rate_value);
        }
        for line in [score, accuracy, combo, mods, average, rate] {
            self.type_setter.lay_out(line, size);
        }
        for ((kind, stat), line) in StatKind::ALL
            .into_iter()
            .zip(data.stats)
            .zip(&mut self.stat_values)
        {
            line.text.clear();
            let _ = write_stat(&mut line.text, kind, stat);
            self.type_setter.lay_out(line, size);
        }
    }

    /// Writes the details grid: the play's numbers in the left column and the judgement
    /// counts in the right one. Allocates; runs when the data or the window changes.
    fn fill_grid(&mut self, data: &ResultsData, light: Face, strong: Face) {
        let rows = INFO_ROWS + data.rows().count();
        self.grid.truncate(rows);
        while self.grid.len() < rows {
            self.grid.push(GridRow {
                label: line(),
                value: line(),
                right: false,
            });
        }
        let unavailable = |i: usize| match i {
            3 => data.mods.is_empty().then_some(&self.no_mods),
            4 => data
                .average_hit_error_text
                .is_none()
                .then_some(&self.not_available),
            5 => data.unstable_rate.is_none().then_some(&self.not_available),
            _ => None,
        };
        let mut sources: [Option<(usize, bool)>; INFO_ROWS + StatKind::COUNT] =
            [None; INFO_ROWS + StatKind::COUNT];
        for (i, source) in sources.iter_mut().enumerate().take(INFO_ROWS) {
            *source = Some((i, false));
        }
        for (n, (kind, _)) in data.rows().enumerate() {
            sources[INFO_ROWS + n] = Some((kind.index(), true));
        }
        for (row, source) in self.grid.iter_mut().zip(sources) {
            let Some((i, right)) = source else { continue };
            row.right = right;
            let (label, value) = if right {
                (&self.stat_labels[i], &self.stat_values[i])
            } else {
                (
                    &self.info_labels[i],
                    unavailable(i).unwrap_or(&self.info_values[i]),
                )
            };
            row.label.text.clear();
            row.label.text.push_str(&label.text);
            row.value.text.clear();
            row.value.text.push_str(&value.text);
            self.type_setter.lay_out_face(&mut row.label, light);
            self.type_setter.lay_out_face(&mut row.value, strong);
        }
    }

    /// The stable look's art: the details page, the graph, the skin's sprites, the black
    /// bar with the header in it, the Back button and the scroll-down hint.
    #[allow(clippy::too_many_arguments)]
    fn push_art(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        snapshot: &ResultsSnapshot,
        pose: Pose,
        white: AtlasRegion,
        skin: &StableSkin,
    ) {
        let Frame { width, height } = self.frame;
        let shift = pose.offset * width;
        let opacity = pose.opacity;
        let tint = faded(Color::WHITE, opacity);
        let data = &*snapshot.data;
        let now = self.now_ns;
        let unit = Metrics::new(width, height).ui(1.0);
        let geometry = stable::geometry(width, height, &skin.art());
        let scroll = snapshot.scroll_at(now);
        let scrolled = scroll * geometry.scroll_area;
        let bar = geometry.bar_height;

        // Texts first: laid out again when the data, the window or the bar changes.
        let key = (data.id, width as u32, height as u32, bar as u32);
        let (lines, count) = stable::header_lines(unit, bar, !data.extra.beatmap_by.is_empty());
        let em = stable::header_em(unit);
        if self.stable_holds != Some(key) {
            self.stable_holds = Some(key);
            let margin = unit * 5.0;
            self.stable_title.text.clear();
            self.stable_title.text.push_str(&data.title);
            let regular = |size| Face {
                size,
                weight: self.theme.results_header_weight,
                width: slam_text::NORMAL_WIDTH,
            };
            let face = regular(em * lines[0].scale);
            let mut probe = Line::new(&ELLIPSIS.to_string());
            self.type_setter.lay_out_face(&mut probe, face);
            fit_face(
                &mut self.type_setter,
                &mut self.stable_title,
                face,
                (width - 2.0 * margin).max(0.0),
                probe.layout.width(),
            );
            for (line, source, scale) in [
                (&mut self.stable_by, &data.extra.beatmap_by, lines[1].scale),
                (
                    &mut self.stable_played,
                    &data.extra.played_by,
                    lines[count - 1].scale,
                ),
            ] {
                line.text.clear();
                line.text.push_str(source);
                self.type_setter.lay_out_face(line, regular(em * scale));
            }
            let scale = self.layout.scale;
            let size = self.theme.results_text_size * scale;
            let light = Face {
                size,
                weight: self.theme.results_header_weight,
                width: slam_text::NORMAL_WIDTH,
            };
            let strong = Face {
                weight: self.theme.caption_weight,
                ..light
            };
            self.details.truncate(data.extra.details.len());
            while self.details.len() < data.extra.details.len() {
                self.details.push(line());
            }
            for (line, source) in self.details.iter_mut().zip(&data.extra.details) {
                line.text.clear();
                line.text.push_str(source);
                self.type_setter.lay_out_face(line, light);
            }
            self.fill_grid(data, light, strong);
        }

        // The scrolled content, under the bar.
        self.push_details(shapes, text, glyphs, &geometry, scrolled, pose, white);
        let f = geometry.screen_scale;
        if let Some(graph) = &skin.images.graph {
            let size = [
                graph.size[0] / graph.density * f,
                graph.size[1] / graph.density * f,
            ];
            let at = [
                stable::GRAPH_AT[0] * f + shift,
                stable::GRAPH_AT[1] * f - scrolled,
            ];
            let mut quad = Sprite::new(graph.texture, graph.uv, at, size);
            quad.colour = tint;
            shapes.push(&quad);
            let rect = Rect {
                x: at[0],
                y: at[1],
                width: size[0],
                height: size[1],
            };
            let thickness = (2.0 * f).max(1.0);
            stable::health_segments(rect, &data.extra.health, |from, to, low| {
                let colour = if low { HEALTH_LOW } else { HEALTH_HIGH };
                line_segment(shapes, white, from, to, thickness, faded(colour, opacity));
            });
        }
        if let Some(retry) = &skin.images.retry {
            let mut rect = geometry.retry;
            rect.x += shift;
            rect.y -= scrolled;
            let amount = snapshot.hover_amount(ResultsButton::Retry, now);
            push_button(shapes, retry, &rect, amount, opacity);
        }
        stable::lay_out(
            skin,
            data,
            width,
            height,
            stable::header_min_height(unit),
            |item| match item {
                Item::Bar { height } => {
                    let mut bar =
                        Sprite::new(white.texture, white.uv, [shift, 0.0], [width, height]);
                    bar.colour = faded(
                        Color {
                            a: stable::BAR_ALPHA,
                            ..Color::BLACK
                        },
                        opacity,
                    );
                    shapes.push(&bar);
                }
                Item::Sprite {
                    part,
                    sprite,
                    centre,
                    size,
                } => {
                    let dy = if part == Part::Title { 0.0 } else { -scrolled };
                    let at = [
                        centre[0] - size[0] / 2.0 + shift,
                        centre[1] - size[1] / 2.0 + dy,
                    ];
                    let mut quad = Sprite::new(sprite.texture, sprite.uv, at, size);
                    quad.colour = tint;
                    shapes.push(&quad);
                }
            },
        );
        for line_at in &lines[..count] {
            let line = match line_at.kind {
                HeaderKind::Title => &self.stable_title,
                HeaderKind::By => &self.stable_by,
                HeaderKind::Played => &self.stable_played,
            };
            push_layout(
                text,
                &line.layout,
                [line_at.x + shift, line_at.y],
                tint,
                glyphs,
            );
        }
        if let Some(back) = &skin.images.back {
            let mut rect = geometry.back;
            rect.x += shift;
            let amount = snapshot.hover_amount(ResultsButton::Back, now);
            push_button(shapes, back, &rect, amount, opacity);
        }
    }

    /// The details page under the first screen: a translucent plate with the heading, the
    /// two-column grid of the play's numbers and the extra lines. Rows above the bar are
    /// skipped (texts are drawn over the shapes).
    #[allow(clippy::too_many_arguments)]
    fn push_details(
        &self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        geometry: &stable::Geometry,
        scrolled: f32,
        pose: Pose,
        white: AtlasRegion,
    ) {
        let Frame { width, .. } = self.frame;
        if self.details.is_empty() && self.grid.is_empty() {
            return;
        }
        let theme = &self.theme;
        let scale = self.layout.scale;
        let shift = pose.offset * width;
        let pad = theme.song_text_pad * scale * 1.5;
        let margin = theme.results_margin * scale;
        let heading = theme.results_title_size * scale * 1.5;
        let left = width * 0.05;
        let left_rows = self.grid.iter().filter(|row| !row.right).count();
        let right_rows = self.grid.len() - left_rows;
        let top = geometry.bar_height + geometry.scroll_area - scrolled + margin;
        let metrics = stable::details_metrics(
            left_rows.max(right_rows),
            self.details.len(),
            theme.results_row_height * scale,
            heading,
            pad,
            geometry.scroll_area - margin,
        );
        let panel = Rect {
            x: left - pad + shift,
            y: top,
            width: width - 2.0 * (left - pad),
            height: metrics.height,
        };
        let colour = faded(theme.button_fill, pose.opacity);
        crate::widgets::draw(shapes, white, &panel, colour, Look::Round, theme, scale);
        let strong = faded(theme.text, pose.opacity);
        let dim = faded(theme.disabled_text, pose.opacity);
        let visible = |y: f32, line: &Line| y + line.layout.height() > geometry.bar_height;
        let centred = |line: &Line, y: f32| y + (metrics.pitch - line.layout.height()) / 2.0;

        let mut y = top + pad;
        let at = [
            left + shift,
            y + (heading - self.details_title.layout.height()) / 2.0,
        ];
        if visible(at[1], &self.details_title) {
            push_layout(text, &self.details_title.layout, at, strong, glyphs);
        }
        y += heading;

        let column = ((width - 2.0 * left) - theme.results_column_gap * scale) / 2.0;
        let grid_top = y;
        let mut next = [0_usize; 2];
        for row in &self.grid {
            let side = usize::from(row.right);
            let row_y = grid_top + next[side] as f32 * metrics.pitch;
            next[side] += 1;
            let x = left + side as f32 * (column + theme.results_column_gap * scale) + shift;
            let ly = centred(&row.label, row_y);
            if visible(ly, &row.label) {
                push_layout(text, &row.label.layout, [x, ly], dim, glyphs);
                let vx = x + column - row.value.layout.width();
                push_layout(
                    text,
                    &row.value.layout,
                    [vx, centred(&row.value, row_y)],
                    strong,
                    glyphs,
                );
            }
        }
        y = grid_top + metrics.grid_rows as f32 * metrics.pitch;
        if !self.grid.is_empty() && !self.details.is_empty() {
            y += metrics.pitch * 0.5;
        }
        for line in &self.details {
            let ly = centred(line, y);
            if visible(ly, line) {
                push_layout(text, &line.layout, [left + shift, ly], dim, glyphs);
            }
            y += metrics.pitch;
        }
    }

    /// Asks `glyphs` for every glyph of the texts laid out so far. Returns how many are
    /// not ready yet; repeat with [`GlyphCache::process`] in between.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        self.info_labels
            .iter()
            .chain(&self.info_values)
            .chain(&self.stat_labels)
            .chain(&self.stat_values)
            .chain(&self.buttons)
            .chain(&self.details)
            .chain(self.grid.iter().flat_map(|row| [&row.label, &row.value]))
            .chain([
                &self.title,
                &self.stable_title,
                &self.stable_by,
                &self.stable_played,
                &self.details_title,
                &self.rank,
                &self.no_mods,
                &self.not_available,
                &self.hint,
            ])
            .flat_map(|line| line.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count()
    }

    /// Lays out the texts of `data` (the draw thread does it on entering), so that
    /// [`ResultsView::request_glyphs`] sees their glyphs.
    pub fn prepare(&mut self, data: &ResultsData) {
        if self.holds != Some(data.id) {
            self.fill(data);
            self.holds = Some(data.id);
        }
    }

    /// Writes the screen at `pose` on a `width` x `height` window: rectangles into
    /// `shapes` (tinting `white`, an opaque white image) and texts into `text` (drawn with
    /// the MSDF shader). Allocation-free once the data's texts are laid out.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        width: f32,
        height: f32,
        snapshot: &ResultsSnapshot,
        pose: Pose,
        white: AtlasRegion,
    ) {
        self.push_with(
            shapes, text, glyphs, width, height, snapshot, pose, white, None,
        );
    }

    /// [`push`](Self::push) in stable's look when the skin gives one (ADR-0158): the
    /// skin's panel, letter and counters replace SLAM's rows; the buttons stay.
    #[allow(clippy::too_many_arguments)]
    pub fn push_stable(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        width: f32,
        height: f32,
        snapshot: &ResultsSnapshot,
        pose: Pose,
        white: AtlasRegion,
        skin: &StableSkin,
    ) {
        self.push_with(
            shapes,
            text,
            glyphs,
            width,
            height,
            snapshot,
            pose,
            white,
            Some(skin),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn push_with(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        width: f32,
        height: f32,
        snapshot: &ResultsSnapshot,
        pose: Pose,
        white: AtlasRegion,
        stable: Option<&StableSkin>,
    ) {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
        self.prepare(&snapshot.data);
        if let Some(skin) = stable {
            self.push_art(shapes, text, glyphs, snapshot, pose, white, skin);
        }

        let shift = pose.offset * width;
        let opacity = pose.opacity;
        let theme = &self.theme;
        let layout = &self.layout;
        let layout = &match stable {
            Some(skin) => layout.stable(width, height, &skin.art()),
            None => *layout,
        };
        let pad = theme.song_text_pad * layout.scale;
        let data = &*snapshot.data;

        let scale = layout.scale;
        let mut rect = |rect: &Rect, colour: Color, look: Look| {
            let moved = Rect {
                x: rect.x + shift,
                ..*rect
            };
            let colour = faded(colour, opacity);
            crate::widgets::draw(shapes, white, &moved, colour, look, theme, scale);
        };
        let mut place = |line: &Line, x: f32, centre_y: f32, colour: Color| {
            let position = [x + shift, centre_y - line.layout.height() / 2.0];
            push_layout(text, &line.layout, position, faded(colour, opacity), glyphs);
        };

        if stable.is_none() {
            place(
                &self.title,
                layout.title.centre()[0] - self.title.layout.width() / 2.0,
                layout.title.centre()[1],
                theme.text,
            );
            let rank_colour = if data.rank.silver() {
                theme.rank_silver
            } else {
                theme.text
            };
            place(
                &self.rank,
                layout.rank.centre()[0] - self.rank.layout.width() / 2.0,
                layout.rank.centre()[1],
                rank_colour,
            );

            let mut row = |area: &Rect, label: &Line, value: &Line| {
                rect(area, theme.button_fill, Look::Round);
                let y = area.y + area.height / 2.0;
                place(label, area.x + pad, y, theme.disabled_text);
                place(
                    value,
                    area.x + area.width - pad - value.layout.width(),
                    y,
                    theme.text,
                );
            };
            for (i, area) in layout.info.iter().enumerate() {
                row(area, &self.info_labels[i], &self.info_values[i]);
            }
            for (n, (kind, _)) in data.rows().enumerate() {
                let i = kind.index();
                row(
                    &layout.stat_row(n),
                    &self.stat_labels[i],
                    &self.stat_values[i],
                );
            }
            // Mods with none: the "none" text over the empty value.
            if data.mods.is_empty() {
                let area = &layout.info[MODS_ROW];
                place(
                    &self.no_mods,
                    area.x + area.width - pad - self.no_mods.layout.width(),
                    area.y + area.height / 2.0,
                    theme.disabled_text,
                );
            }
            // The hit error rows without a value: the "not available" text over the empty one.
            for (row, present) in [
                (4, data.average_hit_error_text.is_some()),
                (5, data.unstable_rate.is_some()),
            ] {
                if !present {
                    let area = &layout.info[row];
                    place(
                        &self.not_available,
                        area.x + area.width - pad - self.not_available.layout.width(),
                        area.y + area.height / 2.0,
                        theme.disabled_text,
                    );
                }
            }
        }

        // The stable look has the skin's buttons and no hint line.
        if stable.is_some() {
            return;
        }
        for (i, button) in ResultsButton::ALL.into_iter().enumerate() {
            let fill = if snapshot.hovered == Some(button) {
                theme.button_hover_fill
            } else {
                theme.button_fill
            };
            let look = Look::Button {
                highlighted: snapshot.hovered == Some(button),
                seed: i as f32 * 0.37,
                fade: opacity,
            };
            rect(&layout.buttons[i], fill, look);
        }
        for (i, area) in layout.buttons.iter().enumerate() {
            let c = area.centre();
            let line = &self.buttons[i];
            place(line, c[0] - line.layout.width() / 2.0, c[1], theme.text);
        }
        let c = layout.hint.centre();
        place(
            &self.hint,
            c[0] - self.hint.layout.width() / 2.0,
            c[1],
            theme.disabled_text,
        );
    }
}
