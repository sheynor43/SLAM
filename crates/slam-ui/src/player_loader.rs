//! The screen before play (ADR-0190), after lazer's `PlayerLoader`: the beatmap that is
//! about to start in a card in the middle, and a column of quick settings on the right
//! made of the settings screen's own rows. Play starts once it is prepared, no sooner than
//! [`Theme::loader_push_delay_ms`] after the screen came in, and not while the pointer is
//! on the quick settings.
//!
//! The update side ([`PlayerLoader`]) keeps the rows' widgets, follows the pointer and
//! decides when the screen may go; the draw side ([`PlayerLoaderView`]) draws a
//! [`LoaderFrame`] and a [`LoaderCard`]. Neither allocates once warm; the card's texts are
//! laid out (allocating) when another beatmap is shown.

use std::sync::Arc;

use slam_render::atlas::AtlasRegion;
use slam_render::shape::{Shape, ShapeBatch};
use slam_render::sprite::{Affine, SpriteBatch};
use slam_text::{FontRegistry, GlyphCache};

use crate::anim::{ease_out_cubic, ease_out_quint, progress};
use crate::menu::Rect;
use crate::menu::view::{Line, Typesetter, push_centred};
use crate::settings_screen::panel::panel_width;
use crate::settings_screen::{
    RowValue, RowWidget, SettingsInput, SettingsLayout, SettingsView, route_value,
};
use crate::song_select::ScoreMods;
use crate::star_colour;
use crate::theme::{Theme, faded};
use crate::toolbar::push_cover;
use crate::widgets::{Look, draw, push_left, push_shadow};

/// The most quick settings the column holds.
pub const MAX_QUICK_ROWS: usize = 12;

/// The groups of the quick settings, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickGroup {
    Visual,
    Audio,
    Input,
}

impl QuickGroup {
    pub const ALL: [QuickGroup; 3] = [QuickGroup::Visual, QuickGroup::Audio, QuickGroup::Input];

    fn index(self) -> usize {
        match self {
            QuickGroup::Visual => 0,
            QuickGroup::Audio => 1,
            QuickGroup::Input => 2,
        }
    }
}

/// One quick setting: a row of the settings screen, its group and its widget's state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuickRow {
    /// The row's index on the settings screen.
    pub row: u16,
    pub group: QuickGroup,
    pub widget: RowWidget,
}

/// What the screen asks the application to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoaderEvent {
    /// The input was the screen's and asks for nothing.
    Absorbed,
    /// A quick setting changed: the settings screen's row `row` takes `value`.
    Changed { row: usize, value: RowValue },
    /// Back to the song select; the play being prepared is dropped.
    Back,
}

/// Where the screen's parts are in a window.
#[derive(Clone, Copy, Debug)]
pub struct LoaderLayout {
    window: [f32; 2],
    scale: f32,
    theme: Theme,
    /// Each row's place, in the order of the screen's rows.
    slots: [Rect; MAX_QUICK_ROWS],
    /// Each group's plate and header; empty when the group has no rows.
    groups: [Rect; 3],
    headers: [Rect; 3],
    /// All the groups together.
    column: Rect,
}

impl LoaderLayout {
    /// The layout of `rows` in a `width` × `height` window.
    pub fn new(width: f32, height: f32, theme: &Theme, rows: &[QuickRow]) -> LoaderLayout {
        let scale = theme.scale(height);
        let column_width = theme.loader_column_width * scale;
        let margin = theme.loader_margin * scale;
        let pad = theme.widget_pad * scale;
        let row_height = theme.widget_height * scale * theme.loader_row_scale;
        let row_gap = theme.widget_pad * scale;
        let header = theme.loader_header * scale;
        let x = (width - column_width - margin).max(0.0);
        let mut y = margin;
        let mut slots = [Rect::default(); MAX_QUICK_ROWS];
        let mut groups = [Rect::default(); 3];
        let mut headers = [Rect::default(); 3];
        for group in QuickGroup::ALL {
            let members = rows.iter().take(MAX_QUICK_ROWS).enumerate();
            let mut members = members.filter(|(_, r)| r.group == group).peekable();
            if members.peek().is_none() {
                continue;
            }
            let top = y;
            headers[group.index()] = Rect {
                x: x + pad,
                y: top + pad / 2.0,
                width: column_width - 2.0 * pad,
                height: header,
            };
            y = top + pad / 2.0 + header;
            for (i, _) in members {
                slots[i] = Rect {
                    x: x + pad,
                    y,
                    width: column_width - 2.0 * pad,
                    height: row_height,
                };
                y += row_height + row_gap;
            }
            y += pad - row_gap;
            groups[group.index()] = Rect {
                x,
                y: top,
                width: column_width,
                height: y - top,
            };
            y += theme.loader_group_gap * scale;
        }
        let column = Rect {
            x,
            y: margin,
            width: column_width,
            height: (y - margin - theme.loader_group_gap * scale).max(0.0),
        };
        LoaderLayout {
            window: [width, height],
            scale,
            theme: *theme,
            slots,
            groups,
            headers,
            column,
        }
    }

    /// The layout of row `i` of the screen, as a single settings row.
    pub fn row(&self, i: usize) -> SettingsLayout {
        let slot = self.slots.get(i).copied().unwrap_or_default();
        SettingsLayout::single(slot, self.window[1], &self.theme)
    }

    /// All the quick settings: the pointer on it holds play back.
    pub fn column(&self) -> Rect {
        self.column
    }

    /// Where the pointer reveals the hidden quick settings: right of this.
    pub fn reveal_x(&self) -> f32 {
        self.window[0] - self.theme.loader_reveal_zone * self.scale
    }

    /// The middle of the window, where the card goes: the quick settings are hidden
    /// until asked for and slide over the right edge (ADR-0191).
    pub fn card_centre(&self) -> [f32; 2] {
        [self.window[0] / 2.0, self.window[1] / 2.0]
    }
}

/// The screen on the update thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerLoader {
    since_ns: u64,
    rows: [Option<QuickRow>; MAX_QUICK_ROWS],
    count: usize,
    /// The quick settings are shown: the pointer came to the right edge and stays by them.
    revealed: bool,
    /// The row under the pointer: it takes the arrows.
    hover_row: Option<usize>,
    /// The play is prepared and waits for the screen.
    prepared: bool,
    leaving_since: Option<u64>,
}

impl PlayerLoader {
    /// The screen coming in at `now_ns` with the quick settings `rows` (the first
    /// [`MAX_QUICK_ROWS`]).
    pub fn new(rows: &[QuickRow], now_ns: u64) -> PlayerLoader {
        let mut all = [None; MAX_QUICK_ROWS];
        let count = rows.len().min(MAX_QUICK_ROWS);
        for (slot, row) in all.iter_mut().zip(rows) {
            *slot = Some(*row);
        }
        PlayerLoader {
            since_ns: now_ns,
            rows: all,
            count,
            revealed: false,
            hover_row: None,
            prepared: false,
            leaving_since: None,
        }
    }

    /// The quick settings, in order.
    pub fn rows(&self) -> impl Iterator<Item = &QuickRow> {
        self.rows[..self.count].iter().flatten()
    }

    /// The quick settings, for the application to refresh their values.
    pub fn rows_mut(&mut self) -> impl Iterator<Item = &mut QuickRow> {
        self.rows[..self.count].iter_mut().flatten()
    }

    /// A copy of the quick settings for a layout.
    pub fn row_list(&self) -> ([QuickRow; MAX_QUICK_ROWS], usize) {
        let filler = QuickRow {
            row: 0,
            group: QuickGroup::Visual,
            widget: RowWidget::Note,
        };
        let mut list = [filler; MAX_QUICK_ROWS];
        for (slot, row) in list.iter_mut().zip(self.rows()) {
            *slot = *row;
        }
        (list, self.count)
    }

    /// Whether the play is prepared.
    pub fn set_prepared(&mut self, prepared: bool) {
        self.prepared = prepared;
    }

    /// Whether something on the screen holds play back: the pointer on the quick settings,
    /// a slider dragged or typed into, a list open.
    pub fn holds(&self) -> bool {
        self.revealed
            || self.rows().any(|r| match r.widget {
                RowWidget::Slider(s) => s.is_dragging() || s.is_editing(),
                RowWidget::Dropdown(d) => d.is_open(),
                _ => false,
            })
    }

    /// Whether the screen may start going to play at `now_ns`: prepared, shown long
    /// enough, nothing holding it, not going already.
    pub fn may_leave(&self, now_ns: u64, theme: &Theme) -> bool {
        let delay = (f64::from(theme.loader_push_delay_ms.max(0.0)) * 1e6) as u64;
        self.leaving_since.is_none()
            && self.prepared
            && now_ns >= self.since_ns.saturating_add(delay)
            && !self.holds()
    }

    /// Starts going to play.
    pub fn leave(&mut self, now_ns: u64) {
        self.leaving_since.get_or_insert(now_ns);
        self.revealed = false;
        self.hover_row = None;
    }

    /// Whether the screen has gone out and play may start; at once without effects.
    pub fn gone(&self, now_ns: u64, theme: &Theme) -> bool {
        let out = if theme.effects {
            (f64::from(theme.loader_out_ms.max(0.0)) * 1e6) as u64
        } else {
            0
        };
        self.leaving_since
            .is_some_and(|since| now_ns >= since.saturating_add(out))
    }

    /// What the draw side shows.
    pub fn frame(&self) -> LoaderFrame {
        let (rows, count) = self.row_list();
        LoaderFrame {
            since_ns: self.since_ns,
            leaving_since: self.leaving_since,
            loading: !self.prepared,
            held: self.prepared && self.holds() && self.leaving_since.is_none(),
            revealed: self.revealed,
            rows,
            count,
        }
    }

    fn slot_at(&self, x: f32, y: f32, layout: &LoaderLayout) -> Option<usize> {
        (0..self.count).find(|&i| layout.slots[i].contains(x, y))
    }

    fn open_dropdown(&self) -> Option<usize> {
        (0..self.count).find(|&i| {
            matches!(self.rows[i], Some(QuickRow { widget: RowWidget::Dropdown(d), .. }) if d.is_open())
        })
    }

    fn editing(&self) -> Option<usize> {
        (0..self.count).find(|&i| {
            matches!(self.rows[i], Some(QuickRow { widget: RowWidget::Slider(s), .. }) if s.is_editing())
        })
    }

    fn route(
        &mut self,
        i: usize,
        input: SettingsInput,
        layout: &LoaderLayout,
    ) -> Option<LoaderEvent> {
        let row = self.rows.get_mut(i)?.as_mut()?;
        let value = route_value(&mut row.widget, input, &layout.row(i), 0)?;
        Some(LoaderEvent::Changed {
            row: usize::from(row.row),
            value,
        })
    }

    /// The pointer moved to `x`, `y`: the hover, and a slider's drag or a list's highlight.
    pub fn pointer(&mut self, x: f32, y: f32, layout: &LoaderLayout) -> Option<LoaderEvent> {
        if self.leaving_since.is_some() {
            return None;
        }
        // Shown from the right edge on; they stay while the pointer is by them.
        self.revealed = if self.revealed {
            x >= layout.column.x - layout.theme.widget_pad * layout.scale * 2.0
        } else {
            x >= layout.reveal_x()
        };
        self.hover_row = if self.revealed {
            self.slot_at(x, y, layout)
        } else {
            None
        };
        let input = SettingsInput::PointerMove { x, y };
        if let Some(i) = self.open_dropdown() {
            return self.route(i, input, layout);
        }
        let dragging = (0..self.count).find(|&i| {
            matches!(self.rows[i], Some(QuickRow { widget: RowWidget::Slider(s), .. }) if s.is_dragging())
        });
        dragging.and_then(|i| self.route(i, input, layout))
    }

    /// A press of the primary button at `x`, `y`.
    pub fn press(&mut self, x: f32, y: f32, layout: &LoaderLayout) -> Option<LoaderEvent> {
        if self.leaving_since.is_some() {
            return None;
        }
        let input = SettingsInput::PointerPress { x, y };
        if let Some(i) = self.editing().or_else(|| self.open_dropdown()) {
            return self.route(i, input, layout).or(Some(LoaderEvent::Absorbed));
        }
        if !self.revealed {
            return None;
        }
        let i = self.slot_at(x, y, layout)?;
        self.route(i, input, layout).or(Some(LoaderEvent::Absorbed))
    }

    /// The primary button released: ends a slider's drag.
    pub fn release(&mut self, layout: &LoaderLayout) -> Option<LoaderEvent> {
        let mut event = None;
        for i in 0..self.count {
            if matches!(
                self.rows[i],
                Some(QuickRow {
                    widget: RowWidget::Slider(_),
                    ..
                })
            ) {
                event = self
                    .route(i, SettingsInput::PointerRelease, layout)
                    .or(event);
            }
        }
        event
    }

    /// A key, in the settings screen's terms: arrows step the row under the pointer, Enter
    /// starts and commits typing a slider's value, typing edits it; Back leaves the typing
    /// or the list first, then the screen.
    pub fn key(&mut self, input: SettingsInput, layout: &LoaderLayout) -> Option<LoaderEvent> {
        if self.leaving_since.is_some() {
            return None;
        }
        if let Some(i) = self.editing().or_else(|| self.open_dropdown()) {
            return self.route(i, input, layout).or(Some(LoaderEvent::Absorbed));
        }
        match input {
            SettingsInput::Back => Some(LoaderEvent::Back),
            SettingsInput::Left | SettingsInput::Right | SettingsInput::Activate => {
                let i = self.hover_row?;
                self.route(i, input, layout).or(Some(LoaderEvent::Absorbed))
            }
            _ => None,
        }
    }
}

/// The screen as the draw side sees it; `Copy`.
#[derive(Clone, Copy, Debug)]
pub struct LoaderFrame {
    pub since_ns: u64,
    pub leaving_since: Option<u64>,
    /// The play is still being prepared.
    pub loading: bool,
    /// Prepared, but held back by the pointer on the quick settings.
    pub held: bool,
    /// The quick settings are shown.
    pub revealed: bool,
    pub rows: [QuickRow; MAX_QUICK_ROWS],
    pub count: usize,
}

/// The beatmap the card shows; cloning it shares the strings.
#[derive(Clone, Debug, Default)]
pub struct LoaderCard {
    /// Changes with the beatmap: the draw side lays the texts out again when it does.
    pub key: u64,
    pub title: Arc<str>,
    pub artist: Arc<str>,
    pub version: Arc<str>,
    pub source: Arc<str>,
    pub mapper: Arc<str>,
    /// Star rating with the selected mods, when known.
    pub stars: Option<f32>,
    pub mods: ScoreMods,
}

/// The texts of the screen that do not come from the beatmap.
#[derive(Clone, Debug, Default)]
pub struct LoaderLabels {
    /// Group headers: visual, audio, input.
    pub groups: [String; 3],
    pub source: String,
    pub mapper: String,
    /// Over the cover while the pointer holds play back.
    pub held: String,
}

/// How many mod acronyms the card shows.
const CARD_MODS: usize = crate::song_select::MAX_SCORE_MODS;

/// The draw side of the screen.
pub struct PlayerLoaderView {
    theme: Theme,
    type_setter: Typesetter,
    scale: f32,
    seen: Option<u64>,
    title: Line,
    artist: Line,
    version: Line,
    stars: Line,
    star_value: Option<f32>,
    source_label: Line,
    /// The card has a source to show: without one its row is left out.
    show_source: bool,
    mapper_label: Line,
    source: Line,
    mapper: Line,
    mods: [Line; CARD_MODS],
    mod_count: usize,
    groups: [Line; 3],
    held: Line,
    /// How far the quick settings are in, 0 hidden to 1 shown, and when it was stepped.
    reveal: f32,
    reveal_ns: Option<u64>,
}

impl PlayerLoaderView {
    pub fn new(
        registry: Arc<FontRegistry>,
        theme: Theme,
        labels: &LoaderLabels,
    ) -> PlayerLoaderView {
        PlayerLoaderView {
            theme,
            type_setter: Typesetter::new(registry, theme.font_weight),
            scale: 0.0,
            seen: None,
            title: Line::new(""),
            artist: Line::new(""),
            version: Line::new(""),
            stars: Line::new(""),
            star_value: None,
            source_label: Line::new(&labels.source),
            show_source: false,
            mapper_label: Line::new(&labels.mapper),
            source: Line::new(""),
            mapper: Line::new(""),
            mods: std::array::from_fn(|_| Line::new("")),
            mod_count: 0,
            groups: std::array::from_fn(|i| Line::new(&labels.groups[i])),
            held: Line::new(&labels.held),
            reveal: 0.0,
            reveal_ns: None,
        }
    }

    /// Takes another theme; the texts are laid out again.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.scale = 0.0;
    }

    /// Lays the texts out for `scale` and `card`, when either changed. Allocates then.
    fn lay_out(&mut self, card: &LoaderCard, scale: f32) {
        if self.scale == scale && self.seen == Some(card.key) {
            return;
        }
        let theme = self.theme;
        let fill = |line: &mut Line, text: &str| {
            line.text.clear();
            line.text.push_str(if text.is_empty() { "-" } else { text });
        };
        fill(&mut self.title, &card.title);
        fill(&mut self.artist, &card.artist);
        fill(&mut self.version, &card.version);
        fill(&mut self.source, &card.source);
        fill(&mut self.mapper, &card.mapper);
        self.stars.text.clear();
        self.star_value = card.stars;
        self.show_source = !card.source.is_empty();
        if let Some(stars) = card.stars {
            crate::song_select::push_stars(&mut self.stars.text, stars);
        }
        self.mod_count = 0;
        for (line, acronym) in self.mods.iter_mut().zip(card.mods.iter()) {
            line.text.clear();
            line.text.push_str(acronym);
            self.mod_count += 1;
        }
        let ts = &mut self.type_setter;
        ts.lay_out_face(
            &mut self.title,
            theme.heading_face(theme.loader_title_size * scale),
        );
        ts.lay_out_face(
            &mut self.artist,
            theme.semibold_face(theme.loader_artist_size * scale),
        );
        ts.lay_out_face(
            &mut self.version,
            theme.semibold_face(theme.loader_artist_size * scale),
        );
        let small = theme.widget_text_size * scale * 0.75;
        ts.lay_out_face(&mut self.stars, theme.heading_face(small));
        for line in [
            &mut self.source_label,
            &mut self.mapper_label,
            &mut self.source,
            &mut self.mapper,
            &mut self.held,
        ] {
            ts.lay_out_face(line, theme.regular_face(small));
        }
        for line in &mut self.mods[..self.mod_count] {
            ts.lay_out_face(line, theme.heading_face(small * 0.85));
        }
        for line in &mut self.groups {
            ts.lay_out_face(line, theme.heading_face(small * 0.9));
        }
        self.scale = scale;
        self.seen = Some(card.key);
    }

    /// Draws the screen at `now_ns` with the layer's `opacity` (its transition, ADR-0126).
    /// `cover` is the beatmap's background, if ready; `rows` draws the quick settings.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        rows: &mut SettingsView,
        frame: &LoaderFrame,
        card: &LoaderCard,
        cover: Option<AtlasRegion>,
        width: f32,
        height: f32,
        opacity: f32,
        now_ns: u64,
        white: AtlasRegion,
    ) {
        let theme = self.theme;
        let layout = LoaderLayout::new(width, height, &theme, &frame.rows[..frame.count]);
        let scale = layout.scale;
        self.lay_out(card, scale);
        let ms = |ms: f32| (f64::from(ms.max(0.0)) * 1e6) as u64;
        let (fade, grow, out, away) = if theme.effects {
            let out_since = frame.leaving_since.unwrap_or(u64::MAX);
            (
                ease_out_cubic(progress(frame.since_ns, now_ns, ms(theme.loader_fade_ms))),
                ease_out_quint(progress(frame.since_ns, now_ns, ms(theme.loader_grow_ms))),
                ease_out_cubic(progress(out_since, now_ns, ms(theme.loader_out_ms))),
                ease_out_quint(progress(out_since, now_ns, ms(2.0 * theme.loader_out_ms))),
            )
        } else {
            let gone = if frame.leaving_since.is_some() {
                1.0
            } else {
                0.0
            };
            (1.0, 1.0, gone, gone)
        };
        let slide = self.step_reveal(frame, now_ns);
        // The card.
        let first = (shapes.len(), text.len());
        self.push_card(shapes, text, glyphs, &layout, frame, cover, now_ns, white);
        let from = theme.loader_card_from;
        let size = (from + (1.0 - from) * grow) * (1.0 - (1.0 - from) * away);
        let alpha = (fade * (1.0 - out) * opacity).clamp(0.0, 1.0);
        let map = Affine::about(layout.card_centre(), size, 0.0, [0.0, 0.0]);
        shapes.transform_from(first.0, &map, [1.0, 1.0, 1.0, alpha]);
        text.transform_from(first.1, &map, [1.0, 1.0, 1.0, alpha]);
        // The quick settings: hidden off the right edge, a faint mark there showing where
        // they are; they slide in while revealed.
        let hint = (1.0 - slide) * (1.0 - out) * opacity;
        if hint > 0.0 {
            let w = 4.0 * scale;
            let mark = crate::menu::Rect {
                x: width - 3.0 * w,
                y: height * 0.4,
                width: w,
                height: height * 0.2,
            };
            draw(
                shapes,
                white,
                &mark,
                faded(theme.accent, 0.4 * hint),
                Look::Pill,
                &theme,
                scale,
            );
        }
        if slide > 0.0 {
            let first = (shapes.len(), text.len());
            self.push_column(
                shapes, text, glyphs, rows, &layout, frame, 1.0, now_ns, white,
            );
            let margin = width - layout.column.x;
            let dx = margin * (1.0 - slide);
            let alpha = (slide * (1.0 - out) * opacity).clamp(0.0, 1.0);
            let map = Affine::about([0.0, 0.0], 1.0, 0.0, [dx, 0.0]);
            shapes.transform_from(first.0, &map, [1.0, 1.0, 1.0, alpha]);
            text.transform_from(first.1, &map, [1.0, 1.0, 1.0, alpha]);
        }
        // Going to play, everything goes to black: the play comes in from it (ADR-0191).
        if out > 0.0 {
            let black = slam_render::Color {
                a: (out * opacity).clamp(0.0, 1.0),
                ..slam_render::Color::rgb(0.0, 0.0, 0.0)
            };
            let all = crate::menu::Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            };
            crate::widgets::fill(shapes, white, &all, black);
        }
    }

    /// Steps how far the quick settings are in towards the frame's wish at `now_ns`; the
    /// eased share, 0 to 1. At once without effects.
    fn step_reveal(&mut self, frame: &LoaderFrame, now_ns: u64) -> f32 {
        let target = if frame.revealed && frame.leaving_since.is_none() {
            1.0
        } else {
            0.0
        };
        let dt = self
            .reveal_ns
            .map_or(0, |last| now_ns.saturating_sub(last).min(100_000_000));
        self.reveal_ns = Some(now_ns);
        let ms = self.theme.loader_column_ms;
        if !self.theme.effects || ms <= 0.0 {
            self.reveal = target;
        } else {
            let step = (dt as f64 / (f64::from(ms) * 1e6)) as f32;
            self.reveal = if target > self.reveal {
                (self.reveal + step).min(target)
            } else {
                (self.reveal - step).max(target)
            };
        }
        ease_out_cubic(self.reveal)
    }

    #[allow(clippy::too_many_arguments)]
    fn push_card(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        layout: &LoaderLayout,
        frame: &LoaderFrame,
        cover: Option<AtlasRegion>,
        now_ns: u64,
        white: AtlasRegion,
    ) {
        let theme = self.theme;
        let scale = layout.scale;
        let [cx, cy] = layout.card_centre();
        let gap = theme.widget_pad * scale * 2.0;
        let small = theme.widget_height * scale * 0.75;
        let cover_size = [
            theme.loader_cover_width * scale,
            theme.loader_cover_height * scale,
        ];
        let heights = [
            self.title.layout.height(),
            self.artist.layout.height(),
            cover_size[1] + 2.0 * gap,
            small,
            gap,
            if self.show_source { 2.0 } else { 1.0 } * self.source.layout.height(),
            gap,
            if self.mod_count > 0 { small } else { 0.0 },
        ];
        let mut y = cy - heights.iter().sum::<f32>() / 2.0;
        let text_colour = theme.text;
        // Title and artist.
        push_centred(
            text,
            &self.title,
            [cx, y + heights[0] / 2.0],
            text_colour,
            glyphs,
        );
        y += heights[0];
        push_centred(
            text,
            &self.artist,
            [cx, y + heights[1] / 2.0],
            faded(text_colour, 0.85),
            glyphs,
        );
        y += heights[1] + gap;
        // The cover on a leaning plate, the loading ring or the held note on it.
        let plate = Shape {
            radius: theme.panel_radius * scale,
            skew: theme.button_skew(),
            ..Shape::new(
                white.texture,
                white.uv,
                [cx - cover_size[0] / 2.0, y],
                cover_size,
                theme.widget_popup_fill,
            )
        };
        push_shadow(shapes, &plate, &theme, scale, 1.0);
        if theme.effects {
            shapes.push_shape(&Shape {
                glow: theme.glow_width * scale,
                colour: faded(theme.accent, theme.glow_alpha * 0.5),
                ..plate
            });
        }
        shapes.push_shape(&plate);
        if let Some(region) = cover {
            push_cover(shapes, region, &plate, 1.0);
        }
        let seconds = (now_ns as f64 / 1e9) as f32;
        let wave = 0.5 + 0.5 * (seconds * std::f32::consts::TAU).sin();
        if frame.held {
            // As lazer's blocking layer: tinted, breathing between 0.6 and 1.
            shapes.push_shape(&Shape {
                colour: faded(theme.accent, 0.45 * (0.6 + 0.4 * wave)),
                ..plate
            });
            push_centred(
                text,
                &self.held,
                [cx, y + cover_size[1] / 2.0],
                text_colour,
                glyphs,
            );
        } else if frame.loading {
            let side = cover_size[1] * 0.3;
            let ring = Rect {
                x: cx + cover_size[0] / 2.0 - side * 1.6,
                y: y + cover_size[1] - side * 1.4,
                width: side,
                height: side,
            };
            shapes.push_shape(&Shape {
                radius: side / 2.0,
                outline: theme.outline_width * scale * 2.0,
                ..Shape::new(
                    white.texture,
                    white.uv,
                    [ring.x, ring.y],
                    [side, side],
                    faded(theme.accent, 0.4 + 0.6 * wave),
                )
            });
        }
        y += cover_size[1] + gap;
        // The difficulty and its stars.
        let version_w = self.version.layout.width();
        let has_stars = self.star_value.is_some();
        let pill_w = if has_stars {
            self.stars.layout.width() + small
        } else {
            0.0
        };
        let row_w = version_w + if has_stars { gap + pill_w } else { 0.0 };
        let left = cx - row_w / 2.0;
        push_centred(
            text,
            &self.version,
            [left + version_w / 2.0, y + small / 2.0],
            text_colour,
            glyphs,
        );
        if let Some(stars) = self.star_value {
            let pill = Rect {
                x: left + version_w + gap,
                y: y + small * 0.15,
                width: pill_w,
                height: small * 0.7,
            };
            let colour = star_colour::for_stars(stars);
            draw(shapes, white, &pill, colour, Look::Pill, &theme, scale);
            push_centred(
                text,
                &self.stars,
                pill.centre(),
                star_colour::text_for_stars(stars),
                glyphs,
            );
        }
        y += small + gap;
        // Source and mapper: names right of the middle, labels left of it.
        let line_h = self.source.layout.height();
        let rows = [
            (&self.source_label, &self.source),
            (&self.mapper_label, &self.mapper),
        ];
        let skip = usize::from(!self.show_source);
        for (label, value) in rows.into_iter().skip(skip) {
            let mid = y + line_h / 2.0;
            let label_pos = [cx - gap / 2.0 - label.layout.width() / 2.0, mid];
            push_centred(text, label, label_pos, faded(text_colour, 0.65), glyphs);
            let value_pos = [cx + gap / 2.0 + value.layout.width() / 2.0, mid];
            push_centred(text, value, value_pos, text_colour, glyphs);
            y += line_h;
        }
        y += gap;
        // The mods, as leaning pills.
        if self.mod_count > 0 {
            let pad = small * 0.4;
            let widths = self.mods[..self.mod_count]
                .iter()
                .map(|l| l.layout.width() + 2.0 * pad);
            let total: f32 = widths.clone().sum::<f32>() + gap * (self.mod_count - 1) as f32 / 2.0;
            let mut x = cx - total / 2.0;
            for (line, w) in self.mods[..self.mod_count].iter().zip(widths) {
                let pill = Rect {
                    x,
                    y,
                    width: w,
                    height: small * 0.8,
                };
                draw(
                    shapes,
                    white,
                    &pill,
                    theme.widget_accent_fill,
                    Look::Round,
                    &theme,
                    scale,
                );
                push_centred(text, line, pill.centre(), theme.text, glyphs);
                x += w + gap / 2.0;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_column(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        rows: &mut SettingsView,
        layout: &LoaderLayout,
        frame: &LoaderFrame,
        alpha: f32,
        now_ns: u64,
        white: AtlasRegion,
    ) {
        let theme = self.theme;
        let scale = layout.scale;
        let [width, height] = layout.window;
        for group in QuickGroup::ALL {
            let i = group.index();
            let plate = layout.groups[i];
            if plate.height <= 0.0 {
                continue;
            }
            draw(
                shapes,
                white,
                &plate,
                faded(theme.widget_popup_fill, alpha),
                Look::Round,
                &theme,
                scale,
            );
            push_left(
                text,
                &self.groups[i],
                &layout.headers[i],
                0.0,
                faded(theme.text, alpha),
                glyphs,
            );
        }
        for (i, row) in frame.rows[..frame.count].iter().enumerate() {
            rows.push_row(
                shapes,
                text,
                glyphs,
                &layout.row(i),
                panel_width(width, height, &theme),
                height,
                usize::from(row.row),
                &row.widget,
                alpha,
                white,
                now_ns,
            );
        }
    }
}

impl Default for LoaderFrame {
    fn default() -> LoaderFrame {
        PlayerLoader::new(&[], 0).frame()
    }
}

#[cfg(test)]
mod tests;
