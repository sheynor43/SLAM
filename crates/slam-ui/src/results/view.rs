//! Draw side of the results. Texts are laid out when the screen is entered (the data's id
//! changes) and again when the window size changes; a steady frame only pushes sprites and
//! does not allocate. Text buffers are reserved up front.

use std::fmt::Write as _;
use std::sync::Arc;

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey, push_layout};

use super::{
    ResultsButton, ResultsData, ResultsLayout, ResultsSnapshot, StatKind, write_accuracy,
    write_grouped, write_stat,
};
use crate::menu::Rect;
use crate::menu::view::{Frame, Line, Typesetter};
use crate::screen::Pose;
use crate::song_select::view::fit;
use crate::theme::{Theme, faded};

const ELLIPSIS: char = '\u{2026}';
/// Bytes reserved up front in every text buffer.
const TEXT_CAPACITY: usize = 256;
/// Rows of the left column.
const INFO_ROWS: usize = 4;

/// The screen's localised texts, already formatted.
#[derive(Clone)]
pub struct ResultsLabels {
    pub score: String,
    pub accuracy: String,
    pub max_combo: String,
    pub mods: String,
    /// Shown instead of the mods when there are none.
    pub no_mods: String,
    /// Names of the judgement rows, in [`StatKind::ALL`] order.
    pub stats: [String; StatKind::COUNT],
    pub retry: String,
    pub back: String,
    pub hint: String,
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
    rank: Line,
    info_labels: [Line; INFO_ROWS],
    info_values: [Line; INFO_ROWS],
    no_mods: Line,
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
            rank: line(),
            info_labels: [
                labelled(&labels.score),
                labelled(&labels.accuracy),
                labelled(&labels.max_combo),
                labelled(&labels.mods),
            ],
            info_values: std::array::from_fn(|_| line()),
            no_mods: labelled(&labels.no_mods),
            stat_labels: labels.stats.each_ref().map(|text| labelled(text)),
            stat_values: std::array::from_fn(|_| line()),
            buttons: [labelled(&labels.retry), labelled(&labels.back)],
            hint: labelled(&labels.hint),
            theme,
        };
        view.lay_out();
        view
    }

    fn text_size(&self) -> f32 {
        self.theme.results_text_size * self.layout.scale
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
        for button in &mut self.buttons {
            self.type_setter
                .lay_out(button, self.theme.button_text_size * scale);
        }
        self.type_setter
            .lay_out(&mut self.hint, self.theme.song_detail_size * scale);
        let mut probe = Line::new(&ELLIPSIS.to_string());
        self.type_setter
            .lay_out(&mut probe, self.theme.results_title_size * scale);
        self.ellipsis = probe.layout.width();
        // The title and rank depend on the data; they are laid out again from it.
        self.holds = None;
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

        let [score, accuracy, combo, mods] = &mut self.info_values;
        for line in [&mut *score, &mut *accuracy, &mut *combo, &mut *mods] {
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
        for line in [score, accuracy, combo, mods] {
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

    /// Asks `glyphs` for every glyph of the texts laid out so far. Returns how many are
    /// not ready yet; repeat with [`GlyphCache::process`] in between.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        self.info_labels
            .iter()
            .chain(&self.info_values)
            .chain(&self.stat_labels)
            .chain(&self.stat_values)
            .chain(&self.buttons)
            .chain([&self.title, &self.rank, &self.no_mods, &self.hint])
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
        shapes: &mut SpriteBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        width: f32,
        height: f32,
        snapshot: &ResultsSnapshot,
        pose: Pose,
        white: AtlasRegion,
    ) {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
        self.prepare(&snapshot.data);

        let shift = pose.offset * width;
        let opacity = pose.opacity;
        let theme = &self.theme;
        let layout = &self.layout;
        let pad = theme.song_text_pad * layout.scale;
        let data = &*snapshot.data;

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
        let mut place = |line: &Line, x: f32, centre_y: f32, colour: Color| {
            let position = [x + shift, centre_y - line.layout.height() / 2.0];
            push_layout(text, &line.layout, position, faded(colour, opacity), glyphs);
        };

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
            rect(area, theme.button_fill);
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
            let area = &layout.info[INFO_ROWS - 1];
            place(
                &self.no_mods,
                area.x + area.width - pad - self.no_mods.layout.width(),
                area.y + area.height / 2.0,
                theme.disabled_text,
            );
        }

        for (i, button) in ResultsButton::ALL.into_iter().enumerate() {
            let fill = if snapshot.hovered == Some(button) {
                theme.button_hover_fill
            } else {
                theme.button_fill
            };
            rect(&layout.buttons[i], fill);
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
