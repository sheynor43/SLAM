//! Draw side of the settings screen. Texts are laid out again only when the window size
//! changes; once warm, `push` does not allocate.

use std::sync::Arc;

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::sprite::SpriteBatch;
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey};

use super::{
    Conflict, KeyValue, MAX_ROWS, MouseKey, RowKind, RowSpec, RowWidget, SettingsLayout,
    SettingsScreen, SettingsSnapshot,
};
use crate::menu::Rect;
use crate::menu::view::{Frame, Line, Typesetter};
use crate::screen::Pose;
use crate::song_select::{QUERY_CAPACITY, SearchQuery};
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
    /// A slider shows its value without trailing zeros (`0.##`).
    pub trim_zeros: bool,
    /// A slider shows this instead of its value at zero; empty shows the number.
    pub zero_text: String,
    /// A note on the row, shown while [`SettingsScreen::set_hint`] says so: in the room of
    /// a slider (which is then not drawn), beside a toggle. Lines are split by `\n`, at
    /// most [`MAX_HINT_LINES`]; each must fit the widget column on one line.
    pub hint: String,
}

/// Most lines of a hint; more are dropped.
pub const MAX_HINT_LINES: usize = 2;
/// How much a disabled row is faded.
const DISABLED_OPACITY: f32 = 0.5;

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
    /// The prefixes of the modifiers of a hotkey: "Ctrl+", "Shift+", "Alt+".
    pub mods: [String; 3],
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
    /// Shown in the empty search box (ADR-0095).
    pub search_placeholder: String,
    /// Shown instead of rows when nothing matches the search.
    pub no_results: String,
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
    mods: Vec<Line>,
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
            mods: labels.mods.iter().map(|k| Line::new(k)).collect(),
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
        .chain(&self.mods)
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
        .chain(&mut self.mods)
    }

    /// The label of `value`'s key; [`KeyLines::prefixes`] has its modifiers.
    fn line(&self, value: KeyValue) -> &Line {
        match value {
            KeyValue::Empty => &self.empty,
            KeyValue::Key(code) | KeyValue::Chord { key: code, .. } => {
                self.keys.get(usize::from(code)).unwrap_or(&self.empty)
            }
            KeyValue::Mouse(button) => self
                .mouse
                .get(MouseKey::index(button))
                .unwrap_or(&self.empty),
        }
    }
}

impl KeyLines {
    /// The modifier prefixes of `value`, in order.
    fn prefixes(&self, value: KeyValue) -> impl Iterator<Item = &Line> {
        let mods = match value {
            KeyValue::Chord { mods, .. } => mods,
            _ => 0,
        };
        [KeyValue::CTRL, KeyValue::SHIFT, KeyValue::ALT]
            .into_iter()
            .zip(&self.mods)
            .filter(move |(bit, _)| mods & bit != 0)
            .map(|(_, line)| line)
    }

    /// Draws `value`, its modifiers first, left in `rect`.
    #[allow(clippy::too_many_arguments)]
    fn push(
        &self,
        text: &mut SpriteBatch,
        value: KeyValue,
        rect: &Rect,
        pad: f32,
        colour: Color,
        glyphs: &mut GlyphCache,
    ) {
        let mut x = pad;
        for prefix in self.prefixes(value) {
            push_left(text, prefix, rect, x, colour, glyphs);
            x += prefix.layout.width();
        }
        push_left(text, self.line(value), rect, x, colour, glyphs);
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

// The views are built once and live in a `Vec`; boxing them would only add a hop per frame.
#[allow(clippy::large_enum_variant)]
enum RowView {
    Key,
    Button(Line),
    Note,
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
    placeholder: Line,
    no_results: Line,
    /// The search as typed, laid out when it changes; its buffers are reserved up front.
    search: Line,
    search_seen: SearchQuery,
    tabs: Vec<Line>,
    labels: Vec<Line>,
    marker: Line,
    /// Per row: its section, its position in it and whether it needs a restart.
    meta: Vec<(usize, usize, bool)>,
    views: Vec<RowView>,
    /// Per row: the lines of its hint, empty without one.
    hints: Vec<Vec<Line>>,
    value: Option<ValueButton>,
    keys: KeyLines,
}

impl std::fmt::Debug for SettingsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsView")
            .field("placeholder", &self.placeholder.text)
            .finish_non_exhaustive()
    }
}

/// The smallest a text shrinks to fit its place, as a share of its size; past it, it
/// overflows.
const MIN_FIT: f32 = 0.7;

/// Lays `line` out at `size`, smaller if that is wider than `width`, down to [`MIN_FIT`].
fn lay_out_fitted(type_setter: &mut Typesetter, line: &mut Line, size: f32, width: f32) {
    type_setter.lay_out(line, size);
    let laid = line.layout.width();
    if laid > width && laid > 0.0 && width > 0.0 {
        type_setter.lay_out(line, size * (width / laid).max(MIN_FIT));
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
                    RowKind::Note => RowView::Note,
                    RowKind::Toggle { .. } => RowView::Toggle(ToggleView::new(theme)),
                    RowKind::Slider { step, .. } => {
                        let view = SliderView::new(
                            Arc::clone(&registry),
                            theme,
                            decimals(step),
                            &row.unit,
                        );
                        let view = if row.trim_zeros { view.trimmed() } else { view };
                        RowView::Slider(if row.zero_text.is_empty() {
                            view
                        } else {
                            view.with_zero_text(&row.zero_text)
                        })
                    }
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
            placeholder: Line::new(&labels.search_placeholder),
            no_results: Line::new(&labels.no_results),
            search: {
                let mut line = Line::new("");
                line.text.reserve(QUERY_CAPACITY);
                line
            },
            search_seen: SearchQuery::default(),
            tabs: labels.sections.iter().map(|s| Line::new(s)).collect(),
            labels: (0..rows.len())
                .map(|i| Line::new(labels.rows.get(i).map_or("", |r| &r.label)))
                .collect(),
            marker: Line::new(&labels.restart),
            keys: KeyLines::new(&labels.keys),
            meta,
            views,
            hints: (0..rows.len())
                .map(|i| {
                    labels
                        .rows
                        .get(i)
                        .map_or("", |r| r.hint.as_str())
                        .lines()
                        .take(MAX_HINT_LINES)
                        .map(Line::new)
                        .collect()
                })
                .collect(),
            theme,
        };
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

    /// Lays the texts out for the settings panel on a `width` × `height` window
    /// ([`super::panel::panel_width`]), as [`SettingsView::lay_out_for`]. Allocates: call it
    /// off the hot paths.
    pub fn lay_out_for_panel(&mut self, width: f32, height: f32) {
        let panel = super::panel::panel_width(width, height, &self.theme);
        self.lay_out_for(panel, height);
    }

    fn lay_out(&mut self) {
        let Frame { width, height } = self.frame;
        self.layout = SettingsLayout::new(width, height, &self.theme);
        let scale = self.layout.scale;
        let text = self.theme.widget_text_size * scale;
        let pad = 2.0 * self.theme.widget_pad * scale;
        // In the panel a long name may not fit its column: it shrinks, down to a floor.
        let label = self.layout.label(0).width - pad;
        for line in &mut self.labels {
            lay_out_fitted(&mut self.type_setter, line, text, label);
        }
        let tab = self.layout.tabs[0].width - pad;
        for line in &mut self.tabs {
            lay_out_fitted(&mut self.type_setter, line, text, tab);
        }
        for line in [
            &mut self.placeholder,
            &mut self.no_results,
            &mut self.search,
        ] {
            self.type_setter.lay_out(line, text);
        }
        self.type_setter
            .lay_out(&mut self.marker, self.theme.settings_marker_size * scale);
        for line in self.keys.all_mut() {
            self.type_setter.lay_out(line, text);
        }
        let hint = self.theme.settings_marker_size * scale;
        for line in self.hints.iter_mut().flatten() {
            self.type_setter.lay_out(line, hint);
        }
        let widget = self.layout.widget(0).width - pad;
        for view in &mut self.views {
            if let RowView::Button(line) = view {
                lay_out_fitted(&mut self.type_setter, line, text, widget);
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
        // Only the width is used, which is the same for every slot, so the row's position
        // (which hiding rows changes) does not matter.
        let avail = self.layout.widget(0).width - 2.0 * self.theme.widget_pad * self.layout.scale;
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

    /// The view of the dropdown row `row`, to move it to a screen that replaces this one
    /// (ADR-0091); `None` for other rows.
    pub fn dropdown_mut(&mut self, row: usize) -> Option<&mut DropdownView> {
        match self.views.get_mut(row) {
            Some(RowView::Dropdown(view)) => Some(view),
            _ => None,
        }
    }

    /// Asks `glyphs` for every glyph the screen shows. Returns how many are not ready yet;
    /// repeat with [`GlyphCache::process`] in between until none are.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        let own = [&self.placeholder, &self.no_results, &self.search]
            .into_iter()
            .chain(&self.tabs)
            .chain(&self.labels)
            .chain(std::iter::once(&self.marker))
            .chain(self.keys.all())
            .chain(self.hints.iter().flatten())
            .flat_map(|line| line.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count();
        own + self
            .views
            .iter()
            .map(|view| match view {
                RowView::Toggle(_) | RowView::Key | RowView::Note => 0,
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
        if self.search_seen != snapshot.search {
            self.search_seen = snapshot.search;
            self.search.text.clear();
            self.search.text.push_str(snapshot.search.as_str());
            self.type_setter.lay_out(
                &mut self.search,
                self.theme.widget_text_size * self.layout.scale,
            );
        }
        let layout = self
            .layout
            .shifted(pose.offset * width)
            .scrolled(snapshot.scroll);
        let (theme, opacity) = (self.theme, pose.opacity);
        let pad = theme.widget_pad * layout.scale;

        let search = layout.search();
        fill(shapes, white, &search, faded(theme.button_fill, opacity));
        if snapshot.search.is_empty() {
            push_left(
                text,
                &self.placeholder,
                &search,
                pad,
                faded(theme.disabled_text, opacity),
                glyphs,
            );
        } else {
            push_left(
                text,
                &self.search,
                &search,
                pad,
                faded(theme.text, opacity),
                glyphs,
            );
        }
        for (i, tab) in self.tabs.iter().enumerate() {
            let fill_colour = if !snapshot.searching && i == snapshot.section {
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

        // An open list covers rows below or above it, whose texts would show through (all
        // texts are drawn over all rectangles): those rows are left out.
        let open_list = (0..snapshot.len).find_map(|i| match &snapshot.widgets[i] {
            RowWidget::Dropdown(d)
                if d.is_open()
                    && (snapshot.searching || self.meta[i].0 == snapshot.section)
                    && !snapshot.hidden[i] =>
            {
                Some((usize::from(snapshot.slots[i]), d.count()))
            }
            _ => None,
        });
        // The open list is drawn last, over the other widgets' rectangles.
        let mut open_row = None;
        let mut any = false;
        for i in 0..snapshot.len.min(self.meta.len()) {
            let (section, _, restart) = self.meta[i];
            let slot = usize::from(snapshot.slots[i]);
            let in_view =
                (snapshot.searching || section == snapshot.section) && !snapshot.hidden[i];
            any |= in_view;
            if !in_view
                || !layout.shows(slot)
                || open_list.is_some_and(|(open, count)| layout.covered_by_list(open, count, slot))
            {
                continue;
            }
            if matches!(self.views[i], RowView::Note) {
                // The whole row, left-aligned; there is no theme token for a warning.
                push_left(
                    text,
                    &self.labels[i],
                    &layout.row(slot),
                    pad,
                    faded(theme.disabled_text, opacity),
                    glyphs,
                );
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
            let disabled = snapshot.disabled[i];
            let hinted = snapshot.hinted[i] && !self.hints[i].is_empty();
            let label = layout.label(slot);
            let label_colour = if disabled {
                theme.disabled_text
            } else {
                theme.text
            };
            push_left(
                text,
                &self.labels[i],
                &label,
                pad,
                faded(label_colour, opacity),
                glyphs,
            );
            let widget_opacity = if disabled {
                opacity * DISABLED_OPACITY
            } else {
                opacity
            };
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
                    let colour = if key.is_listening() {
                        theme.button_hover_fill
                    } else {
                        theme.button_fill
                    };
                    fill(shapes, white, &widget, faded(colour, opacity));
                    let text_colour = faded(theme.text, opacity);
                    if key.is_listening() {
                        push_left(text, &self.keys.prompt, &widget, pad, text_colour, glyphs);
                    } else {
                        self.keys
                            .push(text, key.value(), &widget, pad, text_colour, glyphs);
                    }
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
                    view.push(shapes, toggle, &bounds, widget_opacity, white);
                }
                // A hint takes the room of the slider.
                (RowView::Slider(_), RowWidget::Slider(_)) if hinted => {}
                (RowView::Slider(view), RowWidget::Slider(slider)) => {
                    let bounds = SliderLayout::new(widget, &theme, layout.scale);
                    view.push(shapes, text, glyphs, slider, &bounds, widget_opacity, white);
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
            if hinted {
                // The lines are stacked and centred in the widget column.
                let lines = &self.hints[i];
                let total: f32 = lines.iter().map(|l| l.layout.height()).sum();
                let mut y = widget.y + (widget.height - total) / 2.0;
                for line in lines {
                    let rect = Rect {
                        y,
                        height: line.layout.height(),
                        ..widget
                    };
                    push_left(
                        text,
                        line,
                        &rect,
                        pad,
                        faded(theme.disabled_text, opacity),
                        glyphs,
                    );
                    y += rect.height;
                }
            }
        }
        if snapshot.searching && !any {
            push_left(
                text,
                &self.no_results,
                &layout.dialog_row(0),
                pad,
                faded(theme.disabled_text, opacity),
                glyphs,
            );
        }
        if let Some(i) = open_row
            && let (RowView::Dropdown(view), RowWidget::Dropdown(dropdown)) =
                (&mut self.views[i], &snapshot.widgets[i])
        {
            let bounds = layout.dropdown_at(usize::from(snapshot.slots[i]), dropdown.count());
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
        self.keys.push(
            text,
            conflict.key,
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
