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

use super::{Row, SongList, SongSelectLayout, SongSelectSnapshot, SongSelectStatus};
use crate::menu::Rect;
use crate::menu::view::{Frame, Line, Typesetter};
use crate::screen::Pose;
use crate::theme::{Theme, faded};

const ELLIPSIS: char = '\u{2026}';
/// Bytes reserved up front in every text buffer.
const TEXT_CAPACITY: usize = 256;

/// The screen's localised texts, already formatted.
#[derive(Clone)]
pub struct SongSelectLabels {
    pub no_folder: String,
    pub failed: String,
    pub loading: String,
    pub empty: String,
    pub hint: String,
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
    hint: Line,
    slots: Vec<RowSlot>,
    /// Widths of the ellipsis at the title and detail sizes.
    ellipsis: [f32; 2],
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
fn fit(type_setter: &mut Typesetter, line: &mut Line, size: f32, avail: f32, ellipsis: f32) {
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
            hint: Line::new(&labels.hint),
            slots: Vec::new(),
            ellipsis: [0.0; 2],
            theme,
        };
        view.scanning.text.reserve(TEXT_CAPACITY);
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
        ] {
            self.type_setter.lay_out(line, status);
        }
        self.type_setter
            .lay_out(&mut self.hint, self.theme.song_detail_size * scale);
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
            &self.hint,
        ];
        let rows = self
            .slots
            .iter()
            .flat_map(|slot| [&slot.primary, &slot.secondary]);
        statics
            .into_iter()
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

        let shift = pose.offset * width;
        let opacity = pose.opacity;
        let theme = &self.theme;
        let layout = &self.layout;
        let pad = theme.song_text_pad * layout.scale;
        let gap = theme.song_row_gap * layout.scale;
        let list = &*snapshot.list;
        let total = list.row_count(snapshot.selected_set);
        let selected_index = snapshot.selected_set + 1 + snapshot.selected_difficulty;

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
        if list.sets.is_empty() {
            if snapshot.status == SongSelectStatus::Ready {
                left(
                    &self.empty,
                    layout.list.x,
                    layout.list.y + layout.row_height / 2.0,
                    theme.disabled_text,
                );
            }
        } else {
            for index in layout.visible_rows(snapshot.scroll, total) {
                let Some(row) = list.row(snapshot.selected_set, index) else {
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
                let fill = if index == selected_index {
                    theme.button_hover_fill
                } else if snapshot.hovered == Some(row) {
                    theme.row_hover_fill
                } else {
                    theme.button_fill
                };
                rect(&area, fill);
                let x = area.x + pad;
                if matches!(row, Row::Set(_)) {
                    let (top, bottom) = (
                        row_text.primary.layout.height(),
                        row_text.secondary.layout.height(),
                    );
                    let start = area.y + (area.height - top - bottom) / 2.0;
                    left(&row_text.primary, x, start + top / 2.0, theme.text);
                    left(
                        &row_text.secondary,
                        x,
                        start + top + bottom / 2.0,
                        theme.disabled_text,
                    );
                } else {
                    left(&row_text.primary, x, area.y + area.height / 2.0, theme.text);
                }
            }
        }
        left(
            &self.hint,
            layout.hint.x,
            layout.hint.y + layout.hint.height / 2.0,
            theme.disabled_text,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
