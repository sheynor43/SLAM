//! Draw side of the settings screen. Texts are laid out again only when the window size
//! changes; once warm, `push` does not allocate.

use std::sync::Arc;

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::shape::{Shape, ShapeBatch};
use slam_render::sprite::{Affine, SpriteBatch};
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey};

use super::{
    AreaPreview, Conflict, KeyValue, MAX_ROWS, MouseKey, PREVIEW_LINES, RowKind, RowSpec,
    RowWidget, SettingsLayout, SettingsScreen, SettingsSnapshot, icons,
};
use crate::marquee::{Marquee, MarqueeParams, ScrollTracker, push_marquee};
use crate::menu::Rect;
use crate::menu::view::{Face, Frame, Line, Typesetter};
use crate::screen::Pose;
use crate::song_select::{QUERY_CAPACITY, SearchQuery};
use crate::theme::{Theme, faded};
use crate::widgets::{
    DropdownView, Look, SliderLayout, SliderView, ToggleLayout, ToggleView, draw, push_left,
    push_right,
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
    /// The row's name can be replaced at runtime ([`SettingsScreen::set_dynamic_labels`]);
    /// the view reserves room for it up front.
    pub dynamic_label: bool,
}

/// Bytes reserved up front for a row's runtime name; longer ones are cut with an ellipsis.
pub const DYNAMIC_LABEL_CAPACITY: usize = 64;

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
    Preview,
    Toggle(ToggleView),
    Slider(SliderView),
    Dropdown(DropdownView),
}

/// How much larger than a row's text a section header's is (ADR-0133).
const HEADER_TEXT: f32 = 1.25;

/// The icon's side as a share of its tab's.
const ICON_SHARE: f32 = 0.56;

/// Draws the tablet-area preview in `rect`: the surface fitted to it, the area turned about
/// its centre. Does not allocate; nothing is drawn for a surface without area.
fn push_preview(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    rect: &Rect,
    preview: &AreaPreview,
    theme: &Theme,
    scale: f32,
    opacity: f32,
) {
    let [sw, sh] = preview.surface;
    if !(sw > 0.0 && sh > 0.0 && rect.width > 0.0 && rect.height > 0.0 && opacity > 0.0) {
        return;
    }
    let pad = theme.widget_pad * scale;
    let room = [
        (rect.width - 2.0 * pad).max(0.0),
        (rect.height - 2.0 * pad).max(0.0),
    ];
    let fit = (room[0] / sw).min(room[1] / sh);
    if !(fit > 0.0 && fit.is_finite()) {
        return;
    }
    let size = [sw * fit, sh * fit];
    let origin = [
        rect.x + (rect.width - size[0]) / 2.0,
        rect.y + (rect.height - size[1]) / 2.0,
    ];
    let outline = (theme.widget_track_height * scale / 3.0).max(1.0);
    let radius = theme.widget_pad * scale;
    let plate =
        |position: [f32; 2], size: [f32; 2], colour: Color, outline: f32, radius: f32| Shape {
            radius: radius.min(size[0].min(size[1]) / 2.0),
            outline,
            ..Shape::new(white.texture, white.uv, position, size, colour)
        };
    shapes.push_shape(&plate(
        origin,
        size,
        faded(theme.button_fill, opacity),
        0.0,
        radius,
    ));
    shapes.push_shape(&plate(
        origin,
        size,
        faded(theme.row_hover_fill, opacity),
        outline,
        radius,
    ));
    let area = [preview.size[0] * fit, preview.size[1] * fit];
    let centre = [
        origin[0] + preview.centre[0] * fit,
        origin[1] + preview.centre[1] * fit,
    ];
    let corner = [centre[0] - area[0] / 2.0, centre[1] - area[1] / 2.0];
    let first = shapes.len();
    shapes.push_shape(&plate(
        corner,
        area,
        faded(theme.widget_accent_fill, opacity * AREA_FILL_OPACITY),
        0.0,
        0.0,
    ));
    shapes.push_shape(&plate(
        corner,
        area,
        faded(theme.accent, opacity),
        outline,
        0.0,
    ));
    let turn = Affine::about(centre, 1.0, preview.rotation_deg.to_radians(), [0.0; 2]);
    shapes.transform_from(first, &turn, [1.0; 4]);
    let dot = (2.0 * outline).max(3.0 * scale);
    shapes.push_shape(&plate(
        [centre[0] - dot / 2.0, centre[1] - dot / 2.0],
        [dot, dot],
        faded(theme.accent, opacity),
        0.0,
        dot / 2.0,
    ));
}

/// How opaque the area's fill is, against its outline.
const AREA_FILL_OPACITY: f32 = 0.35;

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
    /// The typing feedback of the search text (ADR-0167).
    search_type: crate::typing::TypeFeedback,
    tabs: Vec<Line>,
    /// The section names as headers of the list, larger than the tabs'.
    headers: Vec<Line>,
    labels: Vec<Line>,
    /// The rows' whole names: `labels` holds them as fitted to the label column.
    label_texts: Vec<String>,
    marker: Line,
    /// Per row: its section, its position in it and whether it needs a restart.
    meta: Vec<(usize, usize, bool)>,
    views: Vec<RowView>,
    /// Per row: the lines of its hint, empty without one.
    hints: Vec<Vec<Line>>,
    /// The ride of each row's name, and of each line of its hint (ADR-0148).
    label_rides: Vec<Marquee>,
    hint_rides: Vec<Vec<Marquee>>,
    /// Applies the snapshot's sideways wheel and drags to the marquees.
    scroll_tracker: ScrollTracker,
    value: Option<ValueButton>,
    keys: KeyLines,
    /// The rows whose names can change, with their static labels.
    dynamic: Vec<(usize, Box<str>)>,
    /// The generation of the dynamic labels laid out.
    dynamic_seen: u64,
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
    let face = Face {
        size,
        weight: type_setter.weight(),
        width: slam_text::NORMAL_WIDTH,
    };
    lay_out_fitted_face(type_setter, line, face, width);
}

/// [`lay_out_fitted`] in `face`.
fn lay_out_fitted_face(type_setter: &mut Typesetter, line: &mut Line, face: Face, width: f32) {
    type_setter.lay_out_face(line, face);
    let laid = line.layout.width();
    if laid > width && laid > 0.0 && width > 0.0 {
        type_setter.lay_out_face(line, face.scaled((width / laid).max(MIN_FIT)));
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
                    RowKind::Preview => RowView::Preview,
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
            search_type: crate::typing::TypeFeedback::new(),
            tabs: labels.sections.iter().map(|s| Line::new(s)).collect(),
            headers: labels.sections.iter().map(|s| Line::new(s)).collect(),
            labels: (0..rows.len())
                .map(|i| {
                    let mut line = Line::new(labels.rows.get(i).map_or("", |r| &r.label));
                    // Room for the ellipsis a cut name ends with.
                    line.text.reserve(ELLIPSIS.len_utf8());
                    if labels.rows.get(i).is_some_and(|r| r.dynamic_label) {
                        line.text.reserve(DYNAMIC_LABEL_CAPACITY);
                    }
                    line
                })
                .collect(),
            label_texts: (0..rows.len())
                .map(|i| {
                    let row = labels.rows.get(i);
                    let mut text = String::new();
                    if row.is_some_and(|r| r.dynamic_label) {
                        text.reserve(DYNAMIC_LABEL_CAPACITY);
                    }
                    text.push_str(row.map_or("", |r| &r.label));
                    text
                })
                .collect(),
            dynamic: labels
                .rows
                .iter()
                .enumerate()
                .take(rows.len())
                .filter(|(_, r)| r.dynamic_label)
                .map(|(i, r)| (i, r.label.as_str().into()))
                .collect(),
            dynamic_seen: 0,
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
            label_rides: vec![Marquee::default(); rows.len()],
            hint_rides: Vec::new(),
            scroll_tracker: ScrollTracker::default(),
            theme,
        };
        view.hint_rides = view
            .hints
            .iter()
            .map(|lines| vec![Marquee::default(); lines.len()])
            .collect();
        view.lay_out();
        view.warm_dynamic();
        view
    }

    /// Lays every dynamic row out once with a name as long as [`DYNAMIC_LABEL_CAPACITY`]
    /// allows, so that their layouts' buffers hold any later name and
    /// [`SettingsView::refresh_dynamic`] does not grow them on the draw thread; then puts
    /// the static labels back. A name in a fallback font not seen before may still grow
    /// the font caches once. Allocates: called by [`SettingsView::new`].
    fn warm_dynamic(&mut self) {
        let Some(last) = self.dynamic.iter().map(|(row, _)| *row).max() else {
            return;
        };
        // The widest glyphs, then the most words (a line break after every space): the
        // most glyphs and break opportunities a name can bring.
        for unit in ["W", "W "] {
            let long: Box<str> = unit.repeat(DYNAMIC_LABEL_CAPACITY / unit.len()).into();
            let names = vec![long; last + 1];
            self.refresh_dynamic(&names);
        }
        self.refresh_dynamic(&[]);
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

    /// Row `row`'s name as shown (cut to its column) and its width, if the row exists.
    pub fn shown_label(&self, row: usize) -> Option<(&str, f32)> {
        self.labels
            .get(row)
            .map(|line| (line.text.as_str(), line.layout.width()))
    }

    /// The section row `row` is in, if the row exists.
    pub fn section_of(&self, row: usize) -> Option<usize> {
        self.meta.get(row).map(|(section, _, _)| *section)
    }

    /// Draws row `row` alone, as the toolbar's popup shows it (ADR-0168): its name and its
    /// widget in the state `widget`, laid out by `layout` ([`SettingsLayout::single`]) at
    /// slot 0. `panel_width` and `height` are the sizes the texts are laid out for, as
    /// [`SettingsView::push`] takes them. Buttons, keys and notes draw nothing but the
    /// name. Does not allocate once laid out.
    #[allow(clippy::too_many_arguments)]
    pub fn push_row(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        layout: &SettingsLayout,
        panel_width: f32,
        height: f32,
        row: usize,
        widget: &RowWidget,
        opacity: f32,
        white: AtlasRegion,
        now_ns: u64,
    ) {
        let frame = Frame {
            width: panel_width,
            height,
        };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
        let (theme, scale) = (self.theme, layout.scale);
        let pad = theme.widget_pad * scale;
        let (Some(view), Some(label)) = (self.views.get_mut(row), self.labels.get(row)) else {
            return;
        };
        push_left(
            text,
            label,
            &layout.label(0),
            pad,
            faded(theme.text, opacity),
            glyphs,
        );
        match (view, widget) {
            (RowView::Toggle(view), RowWidget::Toggle(toggle)) => {
                let bounds = ToggleLayout::new(layout.row(0), &theme, scale);
                view.push(shapes, toggle, &bounds, opacity, white);
            }
            (RowView::Slider(view), RowWidget::Slider(slider)) => {
                let bounds = SliderLayout::new(layout.widget(0), &theme, scale);
                view.push(shapes, text, glyphs, slider, &bounds, opacity, white);
            }
            (RowView::Dropdown(view), RowWidget::Dropdown(dropdown)) => {
                let bounds = layout.dropdown_at(0, dropdown.count());
                view.set_theme(&theme);
                view.push(
                    shapes, text, glyphs, dropdown, &bounds, opacity, white, now_ns,
                );
            }
            _ => {}
        }
    }

    /// Draws row `row`'s name in `rect`, `pad` from its left edge.
    pub fn push_label(
        &self,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        row: usize,
        rect: &Rect,
        pad: f32,
        colour: Color,
    ) {
        if let Some(line) = self.labels.get(row) {
            push_left(text, line, rect, pad, colour, glyphs);
        }
    }

    /// The width of line `line` of row `row`'s hint as laid out, if it exists.
    pub fn hint_width(&self, row: usize, line: usize) -> Option<f32> {
        self.hints
            .get(row)
            .and_then(|lines| lines.get(line))
            .map(|line| line.layout.width())
    }

    /// Row `row`'s name ride, read-only. For tests.
    #[doc(hidden)]
    pub fn label_ride(&self, row: usize) -> Option<&Marquee> {
        self.label_rides.get(row)
    }

    /// The ride of line `line` of row `row`'s hint, read-only. For tests.
    #[doc(hidden)]
    pub fn hint_ride(&self, row: usize, line: usize) -> Option<&Marquee> {
        self.hint_rides.get(row)?.get(line)
    }

    /// The rides of the dropdown at `row` (its box's, then its options'), read-only. For
    /// tests.
    #[doc(hidden)]
    pub fn dropdown_rides(&self, row: usize) -> Option<Vec<Marquee>> {
        match self.views.get(row)? {
            RowView::Dropdown(view) => Some(view.marquees().copied().collect()),
            _ => None,
        }
    }

    /// Forgets where the marquees were drawn, but for the dropdown of row `keep`: a text
    /// not drawn again takes no input.
    fn clear_ride_windows(&mut self, keep: Option<usize>) {
        for ride in self
            .label_rides
            .iter_mut()
            .chain(self.hint_rides.iter_mut().flatten())
        {
            ride.place(Rect::default());
        }
        for (i, view) in self.views.iter_mut().enumerate() {
            if let RowView::Dropdown(view) = view
                && keep != Some(i)
            {
                for ride in view.marquees_mut() {
                    ride.place(Rect::default());
                }
            }
        }
    }

    fn lay_out(&mut self) {
        let Frame { width, height } = self.frame;
        self.layout = SettingsLayout::new(width, height, &self.theme);
        let scale = self.layout.scale;
        let text = self.theme.widget_text_size * scale;
        let pad = 2.0 * self.theme.widget_pad * scale;
        // In the panel a long name may not fit its column: it shrinks, down to a floor, and
        // past that it rides (ADR-0148).
        let label = self.layout.label(0).width - pad;
        let caption = self.theme.caption_face(scale);
        for (line, whole) in self.labels.iter_mut().zip(&self.label_texts) {
            line.text.clone_from(whole);
            lay_out_fitted_face(&mut self.type_setter, line, caption, label);
        }
        for line in &mut self.tabs {
            self.type_setter.lay_out(line, text);
        }
        for line in &mut self.headers {
            let face = self.theme.heading_face(text * HEADER_TEXT);
            self.type_setter.lay_out_face(line, face);
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
        let note = self.theme.note_face(scale);
        let widget = self.layout.widget(0).width - pad;
        // A hint shares the widget column with a toggle, which keeps the right end; it
        // shrinks to fit the rest, down to a floor.
        let toggle = (self.theme.widget_toggle_width + self.theme.widget_pad) * scale;
        for (lines, view) in self.hints.iter_mut().zip(&self.views) {
            let room = match view {
                RowView::Toggle(_) => widget - toggle,
                _ => widget,
            };
            for line in lines {
                lay_out_fitted_face(&mut self.type_setter, line, note, room.max(0.0));
            }
        }
        for view in &mut self.views {
            if let RowView::Button(line) = view {
                lay_out_fitted(&mut self.type_setter, line, text, widget);
            }
        }
        if let Some(value) = &mut self.value {
            let mut probe = Line::new(&ELLIPSIS.to_string());
            self.type_setter
                .lay_out_face(&mut probe, self.theme.caption_face(scale));
            value.ellipsis = probe.layout.width();
        }
        self.lay_out_value();
    }

    /// Copies the runtime names of the dynamic rows from the snapshot into their reserved
    /// buffers (cut at a character boundary to [`DYNAMIC_LABEL_CAPACITY`], with an ellipsis)
    /// and lays them out again. Does not allocate once [`SettingsView::warm_dynamic`] has
    /// grown the buffers (but for font caches meeting a new fallback font).
    fn refresh_dynamic(&mut self, overrides: &[Box<str>]) {
        let scale = self.layout.scale;
        let pad = 2.0 * self.theme.widget_pad * scale;
        let label = self.layout.label(0).width - pad;
        let caption = self.theme.caption_face(scale);
        for (row, fallback) in &self.dynamic {
            let (Some(whole), Some(line)) =
                (self.label_texts.get_mut(*row), self.labels.get_mut(*row))
            else {
                continue;
            };
            let source = overrides
                .get(*row)
                .map(|text| &**text)
                .filter(|text| !text.is_empty())
                .unwrap_or(fallback);
            whole.clear();
            if source.len() <= DYNAMIC_LABEL_CAPACITY {
                whole.push_str(source);
            } else {
                let mut cut = DYNAMIC_LABEL_CAPACITY - ELLIPSIS.len_utf8();
                while !source.is_char_boundary(cut) {
                    cut -= 1;
                }
                whole.push_str(&source[..cut]);
                whole.push(ELLIPSIS);
            }
            line.text.clone_from(whole);
            lay_out_fitted_face(&mut self.type_setter, line, caption, label);
        }
    }

    /// Lays the value text out, cut from the left with an ellipsis to fit its button. Reuses
    /// the buffers.
    fn lay_out_value(&mut self) {
        let Some(value) = &mut self.value else {
            return;
        };
        let face = self.theme.caption_face(self.layout.scale);
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
        self.type_setter.lay_out_face(&mut value.line, face);
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
        self.type_setter.lay_out_face(&mut value.line, face);
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
                RowView::Toggle(_) | RowView::Key | RowView::Note | RowView::Preview => 0,
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
    ///
    /// Returns where the hovered tab's tooltip starts in `shapes` and `text` when it is shown:
    /// the caller draws from there as a layer of its own, over the rows' texts.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        width: f32,
        height: f32,
        snapshot: &SettingsSnapshot,
        pose: Pose,
        white: AtlasRegion,
        now_ns: u64,
    ) -> Option<(u32, u32)> {
        let frame = Frame { width, height };
        if frame != self.frame {
            self.frame = frame;
            self.lay_out();
        }
        if self.dynamic_seen != snapshot.dynamic_generation {
            self.dynamic_seen = snapshot.dynamic_generation;
            self.refresh_dynamic(&snapshot.dynamic_labels);
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
            self.search_type.observe(snapshot.search.as_str(), now_ns);
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
        let params = MarqueeParams::new(&theme, layout.scale);
        // The marquees take the sideways wheel and drags first, by the windows of the last
        // frame; then the windows are cleared for this frame's texts to set. An open list
        // covers the rows: only its own texts take input then.
        let open = snapshot.widgets[..snapshot.len.min(MAX_ROWS)]
            .iter()
            .position(|w| matches!(w, RowWidget::Dropdown(d) if d.is_open()));
        if open.is_some() {
            self.clear_ride_windows(open);
        }
        self.scroll_tracker.apply(
            &snapshot.text,
            now_ns,
            &params,
            self.label_rides
                .iter_mut()
                .chain(self.hint_rides.iter_mut().flatten())
                .chain(
                    self.views
                        .iter_mut()
                        .flat_map(|view| match view {
                            RowView::Dropdown(view) => Some(view.marquees_mut()),
                            _ => None,
                        })
                        .flatten(),
                ),
        );
        self.clear_ride_windows(None);

        let search = layout.search();
        let field = faded(theme.button_fill, opacity);
        draw(
            shapes,
            white,
            &search,
            field,
            Look::Round,
            &theme,
            layout.scale,
        );
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
            // The newest character slides in; an erase pops the text (ADR-0167).
            let line = &self.search;
            let pose = self.search_type.pose(now_ns, &theme, layout.scale);
            let position = [
                search.x + pad,
                search.y + (search.height - line.layout.height()) / 2.0,
            ];
            slam_text::push_layout_styled(
                text,
                &line.layout,
                position,
                glyphs,
                crate::typing::styler(&line.layout, faded(theme.text, opacity), pose),
            );
        }
        let pointer = snapshot.text.pointer;
        let mut hovered = None;
        for i in 0..self.tabs.len() {
            // One colour for all tabs; the current section is highlighted, the hovered one
            // lit like any button (ADR-0165).
            let rect = layout.tabs[i];
            let hover = rect.contains(pointer[0], pointer[1]);
            if hover {
                hovered = Some(i);
            }
            let current = i == snapshot.section;
            let fill_colour = if hover || current {
                theme.button_hover_fill
            } else {
                theme.button_fill
            };
            let look = Look::Button {
                highlighted: current,
                seed: i as f32 * 0.41,
                fade: opacity,
            };
            draw(
                shapes,
                white,
                &rect,
                faded(fill_colour, opacity),
                look,
                &theme,
                layout.scale,
            );
            let ink = theme.text;
            icons::push_icon(
                shapes,
                white,
                &rect,
                ICON_SHARE,
                faded(ink, opacity),
                icons::icon(i),
            );
        }
        // The name of the hovered section, in a plate beside its tab; the plate and its text
        // are drawn last:
        // the caller draws the rest first, then the shapes and texts from the returned split
        // on, so the plate covers the list's texts.
        let tip = hovered.filter(|_| snapshot.conflict.is_none()).map(|i| {
            let pad = theme.widget_pad * layout.scale;
            let tab = layout.tabs[i];
            (
                i,
                Rect {
                    x: tab.x + tab.width + pad,
                    y: tab.y,
                    width: self.tabs[i].layout.width() + 2.0 * pad,
                    height: tab.height,
                },
            )
        });

        if let Some(conflict) = snapshot.conflict {
            self.push_conflict(shapes, text, glyphs, &layout, &conflict, opacity, white);
            return None;
        }

        // An open list covers the widget column of rows below or above it. Its shapes are
        // drawn last, over theirs, but all texts are drawn over all shapes: those rows keep
        // their labels (left of the list) and shapes-only widgets, not the widgets' texts.
        let open_list = (0..snapshot.len).find_map(|i| match &snapshot.widgets[i] {
            RowWidget::Dropdown(d) if d.is_open() && !snapshot.hidden[i] => {
                Some((usize::from(snapshot.slots[i]), d.count()))
            }
            _ => None,
        });
        // The open list is drawn last, over the other widgets' rectangles.
        let mut open_row = None;
        let mut any = false;
        // Only the lines that touch the content area are laid out and drawn (ADR-0133).
        let visible = layout.visible_slots();
        for (section, header) in snapshot.headers.iter().enumerate().take(self.headers.len()) {
            let Some(slot) = header.map(usize::from).filter(|s| visible.contains(s)) else {
                continue;
            };
            let fraction = layout.fraction(slot);
            // A header is a short text at the left of its row: shown unless it reaches the
            // list.
            let reaches_list = open_list.is_some_and(|(open, count)| {
                layout.covered_by_list(open, count, slot)
                    && layout.row(slot).x + pad + self.headers[section].layout.width()
                        > layout.widget(slot).x
            });
            if fraction <= 0.0 || reaches_list {
                continue;
            }
            push_left(
                text,
                &self.headers[section],
                &layout.row(slot),
                pad,
                faded(theme.text, opacity * fraction),
                glyphs,
            );
        }
        for i in 0..snapshot.len.min(self.meta.len()) {
            let (_, _, restart) = self.meta[i];
            let slot = usize::from(snapshot.slots[i]);
            any |= !snapshot.hidden[i];
            if snapshot.hidden[i] || !visible.contains(&slot) {
                continue;
            }
            let covered =
                open_list.is_some_and(|(open, count)| layout.covered_by_list(open, count, slot));
            let fraction = layout.fraction(slot);
            if fraction <= 0.0 {
                continue;
            }
            // A row partly scrolled out fades by the part that is out: nothing clips it.
            let opacity = opacity * fraction;
            // A note runs the whole row, under the list too.
            if covered && matches!(self.views[i], RowView::Note) {
                continue;
            }
            if matches!(self.views[i], RowView::Preview) {
                // The preview spans this row and the ones after it, which are notes.
                let last = layout.fraction(slot + PREVIEW_LINES - 1);
                let rect = {
                    let (first, end) = (layout.row(slot), layout.row(slot + PREVIEW_LINES - 1));
                    Rect {
                        height: (end.y + end.height - first.y).max(0.0),
                        ..first
                    }
                };
                push_preview(
                    shapes,
                    white,
                    &rect,
                    &snapshot.preview,
                    &theme,
                    layout.scale,
                    opacity * last,
                );
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
                let focus = faded(theme.row_hover_fill, opacity);
                let row = layout.row(slot);
                draw(
                    shapes,
                    white,
                    &row,
                    focus,
                    Look::Round,
                    &theme,
                    layout.scale,
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
            push_marquee(
                text,
                &self.labels[i],
                &label,
                pad,
                faded(label_colour, opacity),
                glyphs,
                &mut self.label_rides[i],
                &params,
                now_ns,
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
            if covered {
                // Only a toggle is all shapes, which the list hides where it lies over it.
                if let (RowView::Toggle(view), RowWidget::Toggle(toggle)) =
                    (&mut self.views[i], &snapshot.widgets[i])
                {
                    let bounds = ToggleLayout::new(layout.row(slot), &theme, layout.scale);
                    view.push(shapes, toggle, &bounds, widget_opacity, white);
                }
                continue;
            }
            match (&mut self.views[i], &snapshot.widgets[i]) {
                (RowView::Key, RowWidget::Key(key)) => {
                    let colour = if key.is_listening() {
                        theme.button_hover_fill
                    } else {
                        theme.button_fill
                    };
                    let colour = faded(colour, opacity);
                    draw(
                        shapes,
                        white,
                        &widget,
                        colour,
                        Look::Round,
                        &theme,
                        layout.scale,
                    );
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
                    let look = Look::Button {
                        highlighted: i == snapshot.focus,
                        seed: i as f32 * 0.23,
                        fade: opacity,
                    };
                    let colour = faded(colour, opacity);
                    draw(shapes, white, &widget, colour, look, &theme, layout.scale);
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
                    view.set_theme(&self.theme);
                    view.push(
                        shapes, text, glyphs, dropdown, &bounds, opacity, white, now_ns,
                    );
                }
                _ => {}
            }
            if hinted {
                // The lines are stacked and centred in the widget column.
                let lines = &self.hints[i];
                let total: f32 = lines.iter().map(|l| l.layout.height()).sum();
                let mut y = widget.y + (widget.height - total) / 2.0;
                // A toggle keeps the right end of the column.
                let room = match self.views[i] {
                    RowView::Toggle(_) => {
                        widget.width - (theme.widget_toggle_width + theme.widget_pad) * layout.scale
                    }
                    _ => widget.width,
                };
                for (line, ride) in lines.iter().zip(&mut self.hint_rides[i]) {
                    let rect = Rect {
                        y,
                        height: line.layout.height(),
                        width: room.max(0.0),
                        ..widget
                    };
                    push_marquee(
                        text,
                        line,
                        &rect,
                        pad,
                        faded(theme.disabled_text, opacity),
                        glyphs,
                        ride,
                        &params,
                        now_ns,
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
            view.set_theme(&self.theme);
            view.push(
                shapes, text, glyphs, dropdown, &bounds, opacity, white, now_ns,
            );
        }
        if let Some((i, rect)) = tip {
            let split = (shapes.len() as u32, text.len() as u32);
            let pad = theme.widget_pad * layout.scale;
            draw(
                shapes,
                white,
                &rect,
                faded(theme.widget_popup_fill, opacity),
                Look::Round,
                &theme,
                layout.scale,
            );
            push_left(
                text,
                &self.tabs[i],
                &rect,
                pad,
                faded(theme.text, opacity),
                glyphs,
            );
            return Some(split);
        }
        None
    }

    /// The question about a taken key, in place of the rows.
    #[allow(clippy::too_many_arguments)]
    fn push_conflict(
        &self,
        shapes: &mut ShapeBatch,
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
        let popup = faded(theme.widget_popup_fill, opacity);
        draw(
            shapes,
            white,
            &layout.dialog(),
            popup,
            Look::Panel,
            theme,
            layout.scale,
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
            let look = Look::Button {
                highlighted: chosen,
                seed: rect.x * 0.01,
                fade: opacity,
            };
            let fill_colour = faded(fill_colour, opacity);
            draw(shapes, white, &rect, fill_colour, look, theme, layout.scale);
            push_left(text, line, &rect, pad, colour, glyphs);
        }
    }
}
