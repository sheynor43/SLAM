//! Draw side of the settings screen. Texts are laid out again only when the window size
//! changes; once warm, `push` does not allocate.

use std::sync::Arc;

use slam_render::atlas::AtlasRegion;
use slam_render::sprite::SpriteBatch;
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey};

use super::{
    Conflict, KeyValue, MAX_ROWS, MouseKey, RowKind, RowSpec, RowWidget, SettingsLayout,
    SettingsScreen, SettingsSnapshot,
};
use crate::menu::view::{Frame, Line, Typesetter};
use crate::screen::Pose;
use crate::theme::{Theme, faded};
use crate::widgets::{
    DropdownView, SliderLayout, SliderView, ToggleLayout, ToggleView, fill, push_left, push_right,
};

/// The texts of one row, already localised.
#[derive(Clone, Debug, Default)]
pub struct RowLabels {
    pub label: String,
    /// The unit shown after a slider's value, with its leading space if any.
    pub unit: String,
    /// The texts of a dropdown's options.
    pub options: Vec<String>,
    /// The text on a button row's button.
    pub button: String,
    /// The button row shows the screen's value text ([`SettingsScreen::set_value_text`])
    /// instead of `button`, which then is the placeholder for an empty value. At most one
    /// row of a screen does.
    pub shows_value: bool,
}

/// The texts of key bindings and of the question about a taken key. The application
/// formats them all when the screen opens, so that nothing is formatted later.
#[derive(Clone, Debug, Default)]
pub struct KeyLabels {
    /// An empty slot.
    pub empty: String,
    /// Shown by a key row that waits for a key.
    pub prompt: String,
    /// The name of each keyboard key, indexed by scancode. A scancode past the end shows
    /// as an empty slot.
    pub keys: Vec<String>,
    /// The name of each mouse button, in the order of [`MouseKey::ALL`].
    pub mouse: [String; 5],
    /// "This input is already bound:".
    pub conflict_title: String,
    /// The answer that keeps the old binding.
    pub conflict_keep: String,
    /// The answer that takes the new one.
    pub conflict_apply: String,
}

/// The screen's localised texts, already formatted.
#[derive(Clone, Debug, Default)]
pub struct SettingsLabels {
    pub title: String,
    /// Section names, in section order.
    pub sections: Vec<String>,
    /// One entry per row, in row order.
    pub rows: Vec<RowLabels>,
    /// The marker of a row that needs a restart.
    pub restart: String,
    pub keys: KeyLabels,
}

/// The lines of [`KeyLabels`], laid out.
struct KeyLines {
    empty: Line,
    prompt: Line,
    keys: Vec<Line>,
    mouse: Vec<Line>,
    title: Line,
    keep: Line,
    apply: Line,
}

impl KeyLines {
    fn new(labels: &KeyLabels) -> KeyLines {
        KeyLines {
            empty: Line::new(&labels.empty),
            prompt: Line::new(&labels.prompt),
            keys: labels.keys.iter().map(|k| Line::new(k)).collect(),
            mouse: labels.mouse.iter().map(|k| Line::new(k)).collect(),
            title: Line::new(&labels.conflict_title),
            keep: Line::new(&labels.conflict_keep),
            apply: Line::new(&labels.conflict_apply),
        }
    }

    fn all(&self) -> impl Iterator<Item = &Line> {
        [
            &self.empty,
            &self.prompt,
            &self.title,
            &self.keep,
            &self.apply,
        ]
        .into_iter()
        .chain(&self.keys)
        .chain(&self.mouse)
    }

    fn all_mut(&mut self) -> impl Iterator<Item = &mut Line> {
        [
            &mut self.empty,
            &mut self.prompt,
            &mut self.title,
            &mut self.keep,
            &mut self.apply,
        ]
        .into_iter()
        .chain(&mut self.keys)
        .chain(&mut self.mouse)
    }

    /// The label of `value`.
    fn line(&self, value: KeyValue) -> &Line {
        match value {
            KeyValue::Empty => &self.empty,
            KeyValue::Key(code) => self.keys.get(usize::from(code)).unwrap_or(&self.empty),
            KeyValue::Mouse(button) => self
                .mouse
                .get(MouseKey::index(button))
                .unwrap_or(&self.empty),
        }
    }
}

/// The button row that shows a runtime text, such as a folder path.
struct ValueButton {
    row: usize,
    /// The text shown, cut from the left to fit the button; buffers reserved up front.
    line: Line,
    /// The whole text `line` was made from, to tell when it changes.
    seen: String,
    /// Width of the ellipsis at the widget text size.
    ellipsis: f32,
}

/// Bytes reserved up front for the value text; longer ones are cut from the left first.
const VALUE_CAPACITY: usize = 512;
const ELLIPSIS: char = '\u{2026}';

enum RowView {
    Key,
    Button(Line),
    Toggle(ToggleView),
    Slider(SliderView),
    Dropdown(DropdownView),
}

/// The draw side of [`SettingsScreen`].
pub struct SettingsView {
    theme: Theme,
    type_setter: Typesetter,
    frame: Frame,
    layout: SettingsLayout,
    title: Line,
    tabs: Vec<Line>,
    labels: Vec<Line>,
    marker: Line,
    /// Per row: its section, its position in it and whether it needs a restart.
    meta: Vec<(usize, usize, bool)>,
    views: Vec<RowView>,
    value: Option<ValueButton>,
    keys: KeyLines,
}

impl std::fmt::Debug for SettingsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsView")
            .field("title", &self.title.text)
            .finish_non_exhaustive()
    }
}

/// Decimals a slider with this `step` shows.
fn decimals(step: f64) -> usize {
    let mut decimals = 0;
    let mut scaled = step;
    while decimals < 6 && (scaled - scaled.round()).abs() > 1e-9 {
        scaled *= 10.0;
        decimals += 1;
    }
    decimals
}

impl SettingsView {
    /// A view of `rows` (as given to [`SettingsScreen::new`]) with the fonts of `registry`.
    /// Lays the texts out once, so that later layouts do not allocate: call it off the hot
    /// paths.
    pub fn new(
        registry: Arc<FontRegistry>,
        theme: Theme,
        rows: &[RowSpec],
        labels: &SettingsLabels,
    ) -> SettingsView {
        let rows = &rows[..rows.len().min(MAX_ROWS)];
        // The same placement as the screen's, which owns the rule.
        let screen = SettingsScreen::new(rows, labels.sections.len());
        let meta = rows
            .iter()
            .enumerate()
            .map(|(i, spec)| (screen.section_of(i), screen.slot(i), spec.restart_required))
            .collect();
        let views = rows
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let row = labels.rows.get(i).cloned().unwrap_or_default();
                match spec.kind {
                    RowKind::Key { .. } => RowView::Key,
                    RowKind::Button => RowView::Button(Line::new(&row.button)),
                    RowKind::Toggle { .. } => RowView::Toggle(ToggleView::new(theme)),
                    RowKind::Slider { step, .. } => RowView::Slider(SliderView::new(
                        Arc::clone(&registry),
                        theme,
                        decimals(step),
                        &row.unit,
                    )),
                    RowKind::Dropdown { .. } => RowView::Dropdown(DropdownView::new(
                        Arc::clone(&registry),
                        theme,
                        &row.options,
                    )),
                }
            })
            .collect();
        let value = rows
            .iter()
            .enumerate()
            .zip(&labels.rows)
            .find(|(_, row)| row.shows_value)
            .map(|((row, _), _)| {
                let mut line = Line::new("");
                line.text.reserve(VALUE_CAPACITY + ELLIPSIS.len_utf8());
                ValueButton {
                    row,
                    line,
                    seen: String::with_capacity(VALUE_CAPACITY + ELLIPSIS.len_utf8()),
                    ellipsis: 0.0,
                }
            });
        let (width, height) = (1280.0, theme.reference_height);
        let mut view = SettingsView {
            value,
            type_setter: Typesetter::new(registry, theme.font_weight),
            frame: Frame { width, height },
            layout: SettingsLayout::new(width, height, &theme),
            title: Line::new(&labels.title),
            tabs: labels.sections.iter().map(|s| Line::new(s)).collect(),
            labels: (0..rows.len())
                .map(|i| Line::new(labels.rows.get(i).map_or("", |r| &r.label)))
                .collect(),
            marker: Line::new(&labels.restart),
            keys: KeyLines::new(&labels.keys),
            meta,
            views,
            theme,
        };
        view.lay_out();
        view
    }

    fn lay_out(&mut self) {
        let Frame { width, height } = self.frame;
        self.layout = SettingsLayout::new(width, height, &self.theme);
        let scale = self.layout.scale;
        let text = self.theme.widget_text_size * scale;
        self.type_setter
            .lay_out(&mut self.title, self.theme.settings_title_size * scale);
        for line in self.tabs.iter_mut().chain(&mut self.labels) {
            self.type_setter.lay_out(line, text);
        }
        self.type_setter
            .lay_out(&mut self.marker, self.theme.settings_marker_size * scale);
        for line in self.keys.all_mut() {
            self.type_setter.lay_out(line, text);
        }
        for view in &mut self.views {
            if let RowView::Button(line) = view {
                self.type_setter.lay_out(line, text);
            }
        }
        if let Some(value) = &mut self.value {
            let mut probe = Line::new(&ELLIPSIS.to_string());
            self.type_setter.lay_out(&mut probe, text);
            value.ellipsis = probe.layout.width();
        }
        self.lay_out_value();
    }

    /// Lays the value text out, cut from the left with an ellipsis to fit its button. Reuses
    /// the buffers.
    fn lay_out_value(&mut self) {
        let Some(value) = &mut self.value else {
            return;
        };
        let size = self.theme.widget_text_size * self.layout.scale;
        let slot = self.meta.get(value.row).map_or(0, |m| m.1);
        let avail =
            self.layout.widget(slot).width - 2.0 * self.theme.widget_pad * self.layout.scale;
        // Start from the end when the text is longer than the buffer.
        let mut start = value.seen.len().saturating_sub(VALUE_CAPACITY);
        while !value.seen.is_char_boundary(start) {
            start += 1;
        }
        value.line.text.clear();
        if start > 0 {
            value.line.text.push(ELLIPSIS);
        }
        value.line.text.push_str(&value.seen[start..]);
        self.type_setter.lay_out(&mut value.line, size);
        let total = value.line.layout.width();
        if total <= avail {
            return;
        }
        // Drop the head until the rest and an ellipsis fit.
        let need = total - (avail - value.ellipsis);
        let cut = value
            .line
            .layout
            .glyphs()
            .iter()
            .find(|g| g.x >= need)
            .map_or(value.line.text.len(), |g| g.cluster as usize)
            .min(value.line.text.len());
        let mut cut = cut;
        while !value.line.text.is_char_boundary(cut) {
            cut += 1;
        }
        // `need` is positive, so the cut is past the first glyph (which may be an ellipsis
        // already).
        value.line.text.replace_range(..cut, "");
        value.line.text.insert(0, ELLIPSIS);
        self.type_setter.lay_out(&mut value.line, size);
    }

    /// Exchanges `view` with the view of the dropdown row `row`; returns whether `row` is a
    /// dropdown (otherwise `view` is left as it is). The caller frees the old view off the
    /// hot paths. Does not allocate; the new view lays itself out again at the current
    /// scale when next drawn.
    pub fn swap_dropdown(&mut self, row: usize, view: &mut DropdownView) -> bool {
        match self.views.get_mut(row) {
            Some(RowView::Dropdown(own)) => {
                std::mem::swap(own, view);
                true
            }
            _ => false,
        }
    }

    /// Asks `glyphs` for every glyph the screen shows. Returns how many are not ready yet;
    /// repeat with [`GlyphCache::process`] in between until none are.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        let own = std::iter::once(&self.title)
            .chain(&self.tabs)
            .chain(&self.labels)
            .chain(std::iter::once(&self.marker))
            .chain(self.keys.all())
            .flat_map(|line| line.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count();
        own + self
            .views
            .iter()
            .map(|view| match view {
                RowView::Toggle(_) | RowView::Key => 0,
                RowView::Button(line) => line
                    .layout
                    .glyphs()
                    .iter()
                    .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
                    .count(),
                RowView::Slider(v) => v.request_glyphs(glyphs),
                RowView::Dropdown(v) => v.request_glyphs(glyphs),
            })
            .sum::<usize>()
            + self.value.as_ref().map_or(0, |value| {
                value
                    .line
                    .layout
                    .glyphs()
                    .iter()
                    .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
                    .count()
            })
    }

    /// Writes the screen at `pose` on a `width` × `height` window: rectangles into `shapes`
    /// (tinting `white`, an opaque white image) and texts into `text` (drawn with the MSDF
    /// shader). Allocation-free once warm and once the batches have room.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut SpriteBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        width: f32,
        height: f32,
        snapshot: &SettingsSnapshot,
        pose: Pose,
        white: AtlasRegion,
    ) {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
        if self
            .value
            .as_ref()
            .is_some_and(|value| value.seen != *snapshot.value_text)
        {
            if let Some(value) = &mut self.value {
                value.seen.clear();
                value.seen.push_str(&snapshot.value_text);
            }
            self.lay_out_value();
        }
        let layout = self.layout.shifted(pose.offset * width);
        let (theme, opacity) = (self.theme, pose.opacity);
        let pad = theme.widget_pad * layout.scale;

        push_left(
            text,
            &self.title,
            &layout.title,
            0.0,
            faded(theme.text, opacity),
            glyphs,
        );
        for (i, tab) in self.tabs.iter().enumerate() {
            let fill_colour = if i == snapshot.section {
                theme.button_hover_fill
            } else {
                theme.button_fill
            };
            fill(shapes, white, &layout.tabs[i], faded(fill_colour, opacity));
            push_left(
                text,
                tab,
                &layout.tabs[i],
                pad,
                faded(theme.text, opacity),
                glyphs,
            );
        }

        if let Some(conflict) = snapshot.conflict {
            self.push_conflict(shapes, text, glyphs, &layout, &conflict, opacity, white);
            return;
        }

        // An open list covers the rows below it, whose texts would show through (all
        // texts are drawn over all rectangles): those rows are left out.
        let open_slot = (0..snapshot.len).find_map(|i| match &snapshot.widgets[i] {
            RowWidget::Dropdown(d) if d.is_open() && self.meta[i].0 == snapshot.section => {
                Some(self.meta[i].1)
            }
            _ => None,
        });
        // The open list is drawn last, over the other widgets' rectangles.
        let mut open_row = None;
        for i in 0..snapshot.len.min(self.meta.len()) {
            let (section, slot, restart) = self.meta[i];
            if section != snapshot.section || open_slot.is_some_and(|open| slot > open) {
                continue;
            }
            if i == snapshot.focus {
                fill(
                    shapes,
                    white,
                    &layout.row(slot),
                    faded(theme.row_hover_fill, opacity),
                );
            }
            let label = layout.label(slot);
            push_left(
                text,
                &self.labels[i],
                &label,
                pad,
                faded(theme.text, opacity),
                glyphs,
            );
            if restart {
                push_right(
                    text,
                    &self.marker,
                    &label,
                    pad,
                    faded(theme.disabled_text, opacity),
                    glyphs,
                );
            }
            let widget = layout.widget(slot);
            match (&mut self.views[i], &snapshot.widgets[i]) {
                (RowView::Key, RowWidget::Key(key)) => {
                    let (colour, line) = if key.is_listening() {
                        (theme.button_hover_fill, &self.keys.prompt)
                    } else {
                        (theme.button_fill, self.keys.line(key.value()))
                    };
                    fill(shapes, white, &widget, faded(colour, opacity));
                    push_left(text, line, &widget, pad, faded(theme.text, opacity), glyphs);
                }
                (RowView::Button(line), RowWidget::Button) => {
                    let colour = if i == snapshot.focus {
                        theme.button_hover_fill
                    } else {
                        theme.button_fill
                    };
                    fill(shapes, white, &widget, faded(colour, opacity));
                    let line = match &self.value {
                        Some(value) if value.row == i && !value.seen.is_empty() => &value.line,
                        _ => &*line,
                    };
                    push_left(text, line, &widget, pad, faded(theme.text, opacity), glyphs);
                }
                (RowView::Toggle(view), RowWidget::Toggle(toggle)) => {
                    let bounds = ToggleLayout::new(layout.row(slot), &theme, layout.scale);
                    view.push(shapes, toggle, &bounds, opacity, white);
                }
                (RowView::Slider(view), RowWidget::Slider(slider)) => {
                    let bounds = SliderLayout::new(widget, &theme, layout.scale);
                    view.push(shapes, text, glyphs, slider, &bounds, opacity, white);
                }
                (RowView::Dropdown(view), RowWidget::Dropdown(dropdown)) => {
                    if dropdown.is_open() {
                        open_row = Some(i);
                        continue;
                    }
                    let bounds = layout.dropdown_at(slot, dropdown.count());
                    view.push(shapes, text, glyphs, dropdown, &bounds, opacity, white);
                }
                _ => {}
            }
        }
        if let Some(i) = open_row
            && let (RowView::Dropdown(view), RowWidget::Dropdown(dropdown)) =
                (&mut self.views[i], &snapshot.widgets[i])
        {
            let bounds = layout.dropdown_at(self.meta[i].1, dropdown.count());
            view.push(shapes, text, glyphs, dropdown, &bounds, opacity, white);
        }
    }

    /// The question about a taken key, in place of the rows.
    #[allow(clippy::too_many_arguments)]
    fn push_conflict(
        &self,
        shapes: &mut SpriteBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        layout: &SettingsLayout,
        conflict: &Conflict,
        opacity: f32,
        white: AtlasRegion,
    ) {
        let theme = &self.theme;
        let pad = theme.widget_pad * layout.scale;
        let colour = faded(theme.text, opacity);
        fill(
            shapes,
            white,
            &layout.dialog(),
            faded(theme.widget_popup_fill, opacity),
        );
        push_left(
            text,
            &self.keys.title,
            &layout.dialog_row(0),
            pad,
            colour,
            glyphs,
        );
        push_left(
            text,
            self.keys.line(conflict.key),
            &layout.dialog_row(1),
            pad,
            colour,
            glyphs,
        );
        if let Some(other) = conflict.other.and_then(|row| self.labels.get(row)) {
            push_left(text, other, &layout.dialog_row(2), pad, colour, glyphs);
        }
        let [keep, apply] = layout.conflict_buttons();
        for (rect, line, chosen) in [
            (keep, &self.keys.keep, !conflict.apply),
            (apply, &self.keys.apply, conflict.apply),
        ] {
            let fill_colour = if chosen {
                theme.button_hover_fill
            } else {
                theme.button_fill
            };
            fill(shapes, white, &rect, faded(fill_colour, opacity));
            push_left(text, line, &rect, pad, colour, glyphs);
        }
    }
}
