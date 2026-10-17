//! Draw side of the settings screen. Texts are laid out again only when the window size
//! changes; once warm, `push` does not allocate.

use std::sync::Arc;

use slam_render::atlas::AtlasRegion;
use slam_render::sprite::SpriteBatch;
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey};

use super::{
    MAX_ROWS, RowKind, RowSpec, RowWidget, SettingsLayout, SettingsScreen, SettingsSnapshot,
};
use crate::menu::view::{Frame, Line, Typesetter};
use crate::screen::Pose;
use crate::theme::{Theme, faded};
use crate::widgets::{
    DropdownLayout, DropdownView, SliderLayout, SliderView, ToggleLayout, ToggleView, fill,
    push_left, push_right,
};

/// The texts of one row, already localised.
#[derive(Clone, Debug, Default)]
pub struct RowLabels {
    pub label: String,
    /// The unit shown after a slider's value, with its leading space if any.
    pub unit: String,
    /// The texts of a dropdown's options.
    pub options: Vec<String>,
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
}

enum RowView {
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
        let (width, height) = (1280.0, theme.reference_height);
        let mut view = SettingsView {
            type_setter: Typesetter::new(registry, theme.font_weight),
            frame: Frame { width, height },
            layout: SettingsLayout::new(width, height, &theme),
            title: Line::new(&labels.title),
            tabs: labels.sections.iter().map(|s| Line::new(s)).collect(),
            labels: (0..rows.len())
                .map(|i| Line::new(labels.rows.get(i).map_or("", |r| &r.label)))
                .collect(),
            marker: Line::new(&labels.restart),
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
    }

    /// Asks `glyphs` for every glyph the screen shows. Returns how many are not ready yet;
    /// repeat with [`GlyphCache::process`] in between until none are.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        let own = std::iter::once(&self.title)
            .chain(&self.tabs)
            .chain(&self.labels)
            .chain(std::iter::once(&self.marker))
            .flat_map(|line| line.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count();
        own + self
            .views
            .iter()
            .map(|view| match view {
                RowView::Toggle(_) => 0,
                RowView::Slider(v) => v.request_glyphs(glyphs),
                RowView::Dropdown(v) => v.request_glyphs(glyphs),
            })
            .sum::<usize>()
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
                    let bounds = DropdownLayout::new(widget, dropdown.count(), layout.scale);
                    view.push(shapes, text, glyphs, dropdown, &bounds, opacity, white);
                }
                _ => {}
            }
        }
        if let Some(i) = open_row
            && let (RowView::Dropdown(view), RowWidget::Dropdown(dropdown)) =
                (&mut self.views[i], &snapshot.widgets[i])
        {
            let bounds = DropdownLayout::new(
                layout.widget(self.meta[i].1),
                dropdown.count(),
                layout.scale,
            );
            view.push(shapes, text, glyphs, dropdown, &bounds, opacity, white);
        }
    }
}
