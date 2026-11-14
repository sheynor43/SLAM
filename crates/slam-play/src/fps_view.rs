//! The draw side of the FPS counter (ADR-0053, ADR-0138): a rounded panel with a soft
//! shadow over the screen, in one of four modes (ADR-0138), and the thread panel's graphs.
//! Allocation-free once built.

use std::fmt;
use std::sync::Arc;

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::shape::{Shape, ShapeBatch};
use slam_render::sprite::{PushSprite, Sprite, SpriteBatch};
use slam_text::{
    FamilyId, FontRegistry, GlyphCache, GlyphKey, Layout, LayoutContext, TextStats, TextStyle,
    UI_FAMILY, push_layout,
};
use slam_ui::theme::Theme;

use crate::fps_counter::{FpsCounter, colour};
use crate::fps_graph::GRAPH_LEN;
use crate::hud::{NUMBER_TEXT_LEN, NumberText, Template};

/// A figure the counter can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FpsCell {
    /// Draw frames per second.
    Fps,
    /// The draw thread's frame time.
    DrawTime,
    /// The update thread's frame time.
    UpdateTime,
    /// Update ticks per second.
    UpdateRate,
    /// Time between audio callbacks.
    AudioPeriod,
    /// The audio buffer's length.
    AudioBuffer,
    /// Audio buffers missed or underrun.
    AudioXruns,
    /// Input thread wake-ups per second.
    InputPolls,
    /// Input events per second.
    InputEvents,
}

/// Figures there are.
pub const CELLS: usize = 9;

impl FpsCell {
    pub const ALL: [FpsCell; CELLS] = [
        FpsCell::Fps,
        FpsCell::DrawTime,
        FpsCell::UpdateTime,
        FpsCell::UpdateRate,
        FpsCell::AudioPeriod,
        FpsCell::AudioBuffer,
        FpsCell::AudioXruns,
        FpsCell::InputPolls,
        FpsCell::InputEvents,
    ];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// A thread of the thread panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FpsRow {
    Draw,
    Update,
    Audio,
    Input,
}

impl FpsRow {
    pub const ALL: [FpsRow; 4] = [FpsRow::Draw, FpsRow::Update, FpsRow::Audio, FpsRow::Input];

    /// The row's figures, left to right.
    pub fn cells(self) -> &'static [FpsCell] {
        match self {
            FpsRow::Draw => &[FpsCell::DrawTime, FpsCell::Fps],
            FpsRow::Update => &[FpsCell::UpdateTime, FpsCell::UpdateRate],
            FpsRow::Audio => &[
                FpsCell::AudioPeriod,
                FpsCell::AudioBuffer,
                FpsCell::AudioXruns,
            ],
            FpsRow::Input => &[FpsCell::InputPolls, FpsCell::InputEvents],
        }
    }

    /// Whether the row has a graph of frame times under it.
    pub fn has_graph(self) -> bool {
        matches!(self, FpsRow::Draw | FpsRow::Update)
    }
}

/// What the counter shows (ADR-0138).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FpsMode {
    /// The draw rate only.
    Fps,
    /// The draw rate and the draw frame time.
    Draw,
    /// The draw rate and the update thread's frame time: lazer's counter.
    #[default]
    Update,
    /// Every thread, with graphs.
    Full,
}

impl FpsMode {
    pub const ALL: [FpsMode; 4] = [FpsMode::Fps, FpsMode::Draw, FpsMode::Update, FpsMode::Full];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            FpsMode::Fps => "fps",
            FpsMode::Draw => "draw",
            FpsMode::Update => "update",
            FpsMode::Full => "full",
        }
    }

    pub fn from_name(name: &str) -> Option<FpsMode> {
        FpsMode::ALL.into_iter().find(|m| m.name() == name)
    }

    /// The figures shown, in the compact modes left to right.
    pub fn cells(self) -> &'static [FpsCell] {
        match self {
            FpsMode::Fps => &[FpsCell::Fps],
            FpsMode::Draw => &[FpsCell::Fps, FpsCell::DrawTime],
            FpsMode::Update => &[FpsCell::Fps, FpsCell::UpdateTime],
            FpsMode::Full => &FpsCell::ALL,
        }
    }

    pub fn is_full(self) -> bool {
        self == FpsMode::Full
    }
}

/// The window corner the counter sits in (ADR-0060); lazer's is the bottom right.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FpsCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

impl FpsCorner {
    pub const ALL: [FpsCorner; 4] = [
        FpsCorner::TopLeft,
        FpsCorner::TopRight,
        FpsCorner::BottomLeft,
        FpsCorner::BottomRight,
    ];

    fn left(self) -> bool {
        matches!(self, FpsCorner::TopLeft | FpsCorner::BottomLeft)
    }

    fn top(self) -> bool {
        matches!(self, FpsCorner::TopLeft | FpsCorner::TopRight)
    }
}

/// The counter's localised texts: the unit around each number, with
/// [`crate::hud::OFFSET_PLACEHOLDER`] standing for it, and the threads' names.
#[derive(Debug, Clone)]
pub struct FpsLabels {
    /// A time, e.g. `{} ms`.
    pub frame_time: String,
    /// The draw rate, e.g. `{} fps`.
    pub fps: String,
    /// A rate, e.g. `{} Hz`.
    pub hz: String,
    /// Events per second, e.g. `{} ev/s`.
    pub events: String,
    /// The audio buffer, e.g. `buf {} ms`.
    pub buffer: String,
    /// Missed audio buffers, e.g. `{} xruns`.
    pub xruns: String,
    /// The threads' names, drawn small and dim: draw, update, audio, input.
    pub threads: [String; 4],
}

impl FpsLabels {
    /// English texts with the placeholder, for tests and previews.
    pub fn english() -> FpsLabels {
        let p = crate::hud::OFFSET_PLACEHOLDER;
        FpsLabels {
            frame_time: format!("{p} ms"),
            fps: format!("{p} fps"),
            hz: format!("{p} Hz"),
            events: format!("{p} ev/s"),
            buffer: format!("buf {p} ms"),
            xruns: format!("{p} xruns"),
            threads: ["DRAW", "UPDATE", "AUDIO", "INPUT"].map(str::to_owned),
        }
    }
}

/// Window height the sizes are given at; they scale with the window.
const REFERENCE_HEIGHT: f32 = 768.0;
/// The panel's distance from the window's edge.
const MARGIN: f32 = 6.0;
/// Padding inside the panel.
const PAD: f32 = 8.0;
const WEIGHT: u16 = 600;
const LABEL_WEIGHT: u16 = 700;

// The compact panel: one line.
const COMPACT_HEIGHT: f32 = 24.0;
const COMPACT_SIZE: f32 = 13.0;
const ENTRY_GAP: f32 = 12.0;

// The thread panel.
const FULL_SIZE: f32 = 11.0;
const LABEL_SIZE: f32 = 9.0;
const ROW_HEIGHT: f32 = 16.0;
const ROW_GAP: f32 = 2.0;
const BLOCK_GAP: f32 = 5.0;
const GRAPH_HEIGHT: f32 = 18.0;
const GRAPH_GAP: f32 = 2.0;
/// Pitch of the graph's bars and the width of one.
const BAR_PITCH: f32 = 2.0;
const BAR_WIDTH: f32 = 1.0;
const LABEL_WIDTH: f32 = 48.0;
const COLUMN_WIDTH: f32 = 64.0;
const COLUMNS: usize = 3;
const FULL_WIDTH: f32 = 2.0 * PAD + GRAPH_LEN as f32 * BAR_PITCH;
const _: () = assert!(LABEL_WIDTH + COLUMNS as f32 * COLUMN_WIDTH == GRAPH_LEN as f32 * BAR_PITCH);

/// Tops of the thread panel's rows and graphs, from the panel's top, and its height.
const ROW_TOP: [f32; 4] = {
    let draw = PAD;
    let update = draw + ROW_HEIGHT + GRAPH_GAP + GRAPH_HEIGHT + BLOCK_GAP;
    let audio = update + ROW_HEIGHT + GRAPH_GAP + GRAPH_HEIGHT + BLOCK_GAP;
    [draw, update, audio, audio + ROW_HEIGHT + ROW_GAP]
};
const FULL_HEIGHT: f32 = ROW_TOP[3] + ROW_HEIGHT + PAD;

/// The most shapes (rectangles) the counter records: a shadow, the panel and its outline,
/// and the bars and floors of the two graphs.
pub const MAX_FPS_SHAPES: usize = 3 + 2 * (GRAPH_LEN + 1);

/// Plain figures and the panel's text over a dark ground.
const NEUTRAL: Color = Color {
    r: 0.92,
    g: 0.93,
    b: 0.95,
    a: 1.0,
};
/// The threads' names.
const LABEL: Color = Color {
    r: 0.58,
    g: 0.61,
    b: 0.68,
    a: 1.0,
};
/// How much of the counter's alpha the panel's fill keeps, and its outline's.
const PANEL_ALPHA: f32 = 0.78;
const OUTLINE_ALPHA: f32 = 0.10;
/// The shadow, as a share of the theme's.
const SHADOW_SHARE: f32 = 0.8;
/// The graphs' floor.
const FLOOR_ALPHA: f32 = 0.16;
/// The graph's full height, in frame periods at the rate aimed for.
const GRAPH_PERIODS: f32 = 3.0;
/// What a number is laid out with to settle the buffers and warm the glyphs.
const WARM_NUMBER: &str = "0123456789.,0123456789-";
/// How long the compact panel's texts must need less room before it narrows to them.
const SHRINK_AFTER_MS: f64 = 1000.0;

/// Text sizes and positions of one window size.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Frame {
    width: f32,
    height: f32,
    scale: f32,
}

/// The numbers a layout shows, and the mode: `None` lays out on the next update.
type Shown = ([NumberText; CELLS], FpsMode);

/// The draw side of the FPS counter: lays out its texts and the panel behind them.
pub struct FpsView {
    registry: Arc<FontRegistry>,
    family: FamilyId,
    context: LayoutContext,
    frame: Frame,
    corner: FpsCorner,
    mode: FpsMode,
    templates: [Template; CELLS],
    text: String,
    layouts: [Layout; CELLS],
    names: [String; 4],
    name_layouts: [Layout; 4],
    shown: Option<Shown>,
    /// The compact panel's width in pixels: it grows at once and shrinks only after its
    /// texts have needed less for [`SHRINK_AFTER_MS`], so that a digit coming and going
    /// does not make it jitter. Starts over with the window size and the mode.
    box_width: f32,
    /// The width the compact panel's texts need now.
    needed_width: f32,
    /// Counter time since which the texts have needed less than `box_width`.
    narrow_since: Option<f64>,
    colours: [Color; CELLS],
    alpha: f32,
    /// Bars of the draw and update graphs, in milliseconds, and each graph's full height.
    bars: [[f32; GRAPH_LEN]; 2],
    graph_full_ms: [f32; 2],
    /// Bar colours: on target, short of it, far short.
    bar_colours: [Color; 3],
    /// Every template and name laid out with [`WARM_NUMBER`], to warm the glyphs.
    warm: Vec<Layout>,
}

impl fmt::Debug for FpsView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FpsView")
            .field("frame", &self.frame)
            .field("corner", &self.corner)
            .field("mode", &self.mode)
            .field("box_width", &self.box_width)
            .finish_non_exhaustive()
    }
}

impl FpsView {
    /// A view with fonts from `registry` and the localised `labels`. Lays out every
    /// character it will show, so that later layouts do not allocate: call it off the hot
    /// paths.
    pub fn new(registry: Arc<FontRegistry>, labels: &FpsLabels) -> FpsView {
        // Indexed by `FpsCell`.
        let templates = [
            Template::new(&labels.fps),
            Template::new(&labels.frame_time),
            Template::new(&labels.frame_time),
            Template::new(&labels.hz),
            Template::new(&labels.frame_time),
            Template::new(&labels.buffer),
            Template::new(&labels.xruns),
            Template::new(&labels.hz),
            Template::new(&labels.events),
        ];
        let longest = templates.iter().map(Template::len).max().unwrap_or(0);
        // An unknown family lays out nothing; the built-in registry has the UI family.
        let family = registry.family(UI_FAMILY).unwrap_or(FamilyId(u32::MAX));
        let mut view = FpsView {
            registry,
            family,
            context: LayoutContext::new(),
            frame: Frame {
                width: 0.0,
                height: 0.0,
                scale: 0.0,
            },
            corner: FpsCorner::default(),
            mode: FpsMode::default(),
            templates,
            text: String::with_capacity(longest + NUMBER_TEXT_LEN),
            layouts: std::array::from_fn(|_| Layout::new()),
            names: labels.threads.clone(),
            name_layouts: std::array::from_fn(|_| Layout::new()),
            shown: None,
            box_width: 0.0,
            needed_width: 0.0,
            narrow_since: None,
            colours: [NEUTRAL; CELLS],
            alpha: 0.0,
            bars: [[0.0; GRAPH_LEN]; 2],
            graph_full_ms: [1.0; 2],
            bar_colours: [colour(1.0), colour(0.6), colour(0.2)],
            warm: Vec::with_capacity(CELLS + 5),
        };
        let style = TextStyle::new(view.family, WEIGHT, 1.0);
        for i in 0..CELLS {
            view.templates[i].write(WARM_NUMBER, &mut view.text);
            let mut warm = Layout::new();
            view.context
                .layout_into(&view.registry, &view.text, &style, &mut warm);
            view.warm.push(warm);
            view.context
                .layout_into(&view.registry, &view.text, &style, &mut view.layouts[i]);
        }
        let mut number = Layout::new();
        view.context
            .layout_into(&view.registry, WARM_NUMBER, &style, &mut number);
        view.warm.push(number);
        let label_style = TextStyle::new(view.family, LABEL_WEIGHT, 1.0);
        for i in 0..4 {
            view.context.layout_into(
                &view.registry,
                &view.names[i],
                &label_style,
                &mut view.name_layouts[i],
            );
            let mut warm = Layout::new();
            view.context
                .layout_into(&view.registry, &view.names[i], &label_style, &mut warm);
            view.warm.push(warm);
        }
        view
    }

    /// Asks `glyphs` for every glyph the counter shows. Returns how many are not ready
    /// yet; repeat with [`GlyphCache::process`] in between until none are, before the
    /// play starts.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        self.warm
            .iter()
            .flat_map(Layout::glyphs)
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), slam_text::Glyph::Pending))
            .count()
    }

    /// Puts the panel in `corner` from the next frame on.
    pub fn set_corner(&mut self, corner: FpsCorner) {
        self.corner = corner;
    }

    /// Shows `mode` from the next update on.
    pub fn set_mode(&mut self, mode: FpsMode) {
        if mode != self.mode {
            self.mode = mode;
            self.shown = None;
            self.box_width = 0.0;
            self.narrow_since = None;
        }
    }

    /// The counter shows again, with its time from zero: the panel's narrowing starts over.
    pub fn restart(&mut self) {
        self.narrow_since = None;
    }

    pub fn mode(&self) -> FpsMode {
        self.mode
    }

    /// Lays out what changed for `counter` on a window of `width` × `height` pixels.
    /// Allocation-free.
    pub fn update(&mut self, width: f32, height: f32, counter: &FpsCounter) {
        let frame = Frame {
            width,
            height,
            scale: height / REFERENCE_HEIGHT,
        };
        if frame != self.frame {
            self.frame = frame;
            self.shown = None;
            self.box_width = 0.0;
            self.narrow_since = None;
        }
        let texts = counter.texts();
        for (colour, cell) in self.colours.iter_mut().zip(FpsCell::ALL) {
            *colour = texts.colour(cell).unwrap_or(NEUTRAL);
        }
        self.alpha = counter.alpha();
        if self.mode.is_full() {
            let graphs = [
                (counter.draw_graph(), counter.aim_draw()),
                (counter.update_graph(), counter.aim_update()),
            ];
            for (i, (graph, aim)) in graphs.into_iter().enumerate() {
                for (n, bar) in self.bars[i].iter_mut().enumerate() {
                    *bar = graph.bar(n);
                }
                let period = if aim > 0.0 {
                    (1000.0 / aim) as f32
                } else {
                    16.7
                };
                self.graph_full_ms[i] = period * GRAPH_PERIODS;
            }
        }
        let numbers = FpsCell::ALL.map(|cell| texts.number(cell));
        if self.shown == Some((numbers, self.mode)) {
            self.fit_box(counter.time_ms());
            return;
        }
        self.shown = Some((numbers, self.mode));
        let scale = frame.scale;
        let size = if self.mode.is_full() {
            FULL_SIZE
        } else {
            COMPACT_SIZE
        };
        let style = TextStyle::new(self.family, WEIGHT, size * scale);
        let mut width = 0.0;
        for &cell in self.mode.cells() {
            let i = cell.index();
            // lazer's texts start empty; no lone unit before the first numbers.
            if numbers[i].as_str().is_empty() {
                self.text.clear();
            } else {
                self.templates[i].write(numbers[i].as_str(), &mut self.text);
            }
            self.context
                .layout_into(&self.registry, &self.text, &style, &mut self.layouts[i]);
            width += self.layouts[i].width() + ENTRY_GAP * scale;
        }
        if self.mode.is_full() {
            let style = TextStyle::new(self.family, LABEL_WEIGHT, LABEL_SIZE * scale);
            for i in 0..4 {
                self.context.layout_into(
                    &self.registry,
                    &self.names[i],
                    &style,
                    &mut self.name_layouts[i],
                );
            }
        } else {
            let inner = width - ENTRY_GAP * scale;
            self.needed_width = inner + 2.0 * PAD * scale;
            self.fit_box(counter.time_ms());
        }
    }

    /// Widens the compact panel to its texts at once, and narrows it to them once they
    /// have needed less for [`SHRINK_AFTER_MS`] of counter time `now_ms`.
    fn fit_box(&mut self, now_ms: f64) {
        if self.mode.is_full() {
            return;
        }
        // Under half a pixel is layout noise, not a narrower text.
        if self.needed_width >= self.box_width - 0.5 {
            self.box_width = self.box_width.max(self.needed_width);
            self.narrow_since = None;
            return;
        }
        // Time going back is a new counter's, should `restart` have been missed.
        let since = match self.narrow_since {
            Some(since) if since <= now_ms => since,
            _ => *self.narrow_since.insert(now_ms),
        };
        if now_ms - since >= SHRINK_AFTER_MS {
            self.box_width = self.needed_width;
            self.narrow_since = None;
        }
    }

    /// The panel's position and size.
    fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let Frame {
            width,
            height,
            scale,
        } = self.frame;
        let size = if self.mode.is_full() {
            [FULL_WIDTH * scale, FULL_HEIGHT * scale]
        } else {
            [self.box_width, COMPACT_HEIGHT * scale]
        };
        let margin = MARGIN * scale;
        let x = if self.corner.left() {
            margin
        } else {
            width - margin - size[0]
        };
        let y = if self.corner.top() {
            margin
        } else {
            height - margin - size[1]
        };
        ([x, y], size)
    }

    /// Writes the counter's shadow, panel and graphs into `batch`, tinting `white` (an
    /// opaque white image); the panel follows `theme`'s radius and shadow (ADR-0116,
    /// ADR-0136). Allocation-free once `batch` has room for [`MAX_FPS_SHAPES`].
    pub fn push_shapes(&self, batch: &mut ShapeBatch, theme: &Theme, white: AtlasRegion) {
        if self.alpha <= 0.0 {
            return;
        }
        let scale = self.frame.scale;
        let (position, size) = self.bounds();
        let mut panel = Shape::new(
            white.texture,
            white.uv,
            position,
            size,
            Color {
                a: PANEL_ALPHA * self.alpha,
                ..theme.settings_panel_fill
            },
        );
        panel.radius = (theme.panel_radius * scale).min(size[0].min(size[1]) * 0.5);
        let strength = theme.shadow * SHADOW_SHARE;
        if strength > 0.0 {
            batch.push_shape(&Shape {
                position: [position[0], position[1] + theme.shadow_offset * scale],
                glow: theme.shadow_radius * scale * (0.5 + 0.5 * strength),
                colour: Color {
                    a: (theme.shadow_alpha * strength).min(0.95) * self.alpha,
                    ..Color::rgb(0.0, 0.0, 0.0)
                },
                ..panel
            });
        }
        batch.push_shape(&panel);
        batch.push_shape(&Shape {
            outline: scale.max(1.0),
            colour: Color {
                a: OUTLINE_ALPHA * self.alpha,
                ..Color::WHITE
            },
            ..panel
        });
        if self.mode.is_full() {
            for (n, row) in [FpsRow::Draw, FpsRow::Update].into_iter().enumerate() {
                self.push_graph(batch, white, row, n);
            }
        }
    }

    /// The graph under `row`: a floor and a bar per span, drawn from the floor up.
    fn push_graph(&self, batch: &mut impl PushSprite, white: AtlasRegion, row: FpsRow, n: usize) {
        let scale = self.frame.scale;
        let ([x, y], _) = self.bounds();
        let left = x + PAD * scale;
        let top = y + (ROW_TOP[row as usize] + ROW_HEIGHT + GRAPH_GAP) * scale;
        let height = GRAPH_HEIGHT * scale;
        let floor = (scale.round().max(1.0)).min(height);
        let mut sprite = Sprite::new(
            white.texture,
            white.uv,
            [left, top + height - floor],
            [GRAPH_LEN as f32 * BAR_PITCH * scale, floor],
        );
        sprite.colour = Color {
            a: FLOOR_ALPHA * self.alpha,
            ..Color::WHITE
        };
        batch.push_sprite(&sprite);
        let full = self.graph_full_ms[n].max(f32::MIN_POSITIVE);
        let width = (BAR_WIDTH * scale).round().max(1.0);
        for (i, &ms) in self.bars[n].iter().enumerate() {
            if ms <= 0.0 {
                continue;
            }
            // The bar's height is its share of the graph; the colour is how far its
            // period is from the one aimed for.
            let share = (ms / full).min(1.0);
            let bar = (share * (height - floor)).max(1.0 * scale.round().max(1.0));
            let ratio = full / GRAPH_PERIODS / ms;
            let pick = match ratio {
                r if r >= 0.8 => 0,
                r if r >= 0.5 => 1,
                _ => 2,
            };
            let mut sprite = Sprite::new(
                white.texture,
                white.uv,
                [
                    (left + i as f32 * BAR_PITCH * scale).round(),
                    top + height - floor - bar,
                ],
                [width, bar],
            );
            sprite.colour = Color {
                a: self.alpha,
                ..self.bar_colours[pick]
            };
            batch.push_sprite(&sprite);
        }
    }

    /// Writes the counter's text into a batch drawn with the MSDF shader. Glyphs not
    /// ready yet are skipped and counted. Allocation-free once `batch` has room.
    pub fn push_text(&self, batch: &mut SpriteBatch, glyphs: &mut GlyphCache) -> TextStats {
        let mut stats = TextStats::default();
        if self.alpha <= 0.0 {
            return stats;
        }
        let scale = self.frame.scale;
        let ([x, y], size) = self.bounds();
        let mut put = |layout: &Layout, left: f32, cy: f32, tint: Color| {
            let colour = Color {
                a: self.alpha,
                ..tint
            };
            let position = [left, cy - layout.height() * 0.5];
            let s = push_layout(batch, layout, position, colour, glyphs);
            stats.drawn += s.drawn;
            stats.pending += s.pending;
        };
        if self.mode.is_full() {
            for row in FpsRow::ALL {
                let cy = y + (ROW_TOP[row as usize] + ROW_HEIGHT * 0.5) * scale;
                put(&self.name_layouts[row as usize], x + PAD * scale, cy, LABEL);
                let cells = row.cells();
                for (k, &cell) in cells.iter().enumerate() {
                    // Columns count from the right: a short row leaves its left ones empty.
                    let column = COLUMNS - cells.len() + k;
                    let right =
                        x + (PAD + LABEL_WIDTH + (column + 1) as f32 * COLUMN_WIDTH) * scale;
                    let layout = &self.layouts[cell.index()];
                    put(
                        layout,
                        right - layout.width(),
                        cy,
                        self.colours[cell.index()],
                    );
                }
            }
        } else {
            let cy = y + size[1] * 0.5;
            let mut left = x + PAD * scale;
            for &cell in self.mode.cells() {
                let layout = &self.layouts[cell.index()];
                put(layout, left, cy, self.colours[cell.index()]);
                left += layout.width() + ENTRY_GAP * scale;
            }
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fps_counter::{AudioFigures, InputFigures, ThreadFigures};

    const MS: u64 = 1_000_000;

    fn view() -> FpsView {
        FpsView::new(
            Arc::new(FontRegistry::with_builtin()),
            &FpsLabels::english(),
        )
    }

    /// A counter that has run steadily at 1000 Hz for two seconds.
    fn counter() -> FpsCounter {
        let mut c = FpsCounter::new();
        for t in 1..2000 {
            c.frame(t * MS, MS, 1000.0, 1000.0);
        }
        c.set_figures(ThreadFigures {
            update_hz: 1000.0,
            audio: Some(AudioFigures {
                callbacks_hz: 375.0,
                buffer_ms: 2.7,
                xruns: 3,
            }),
            input: Some(InputFigures {
                polls_hz: 800.0,
                events_hz: 125.0,
            }),
        });
        c
    }

    fn white() -> AtlasRegion {
        AtlasRegion {
            texture: slam_render::textures::TextureId(0),
            origin: slam_render::Origin { x: 0, y: 0 },
            size: slam_render::Extent {
                width: 4,
                height: 4,
            },
            uv: slam_render::atlas::UvRect::FULL,
        }
    }

    #[test]
    fn modes_round_trip_through_their_names() {
        for mode in FpsMode::ALL {
            assert_eq!(FpsMode::from_name(mode.name()), Some(mode));
        }
        assert_eq!(FpsMode::from_name("nope"), None);
        // lazer's counter is the default.
        assert_eq!(FpsMode::default(), FpsMode::Update);
    }

    #[test]
    fn each_mode_shows_its_lines() {
        assert_eq!(FpsMode::Fps.cells(), &[FpsCell::Fps]);
        assert_eq!(FpsMode::Draw.cells(), &[FpsCell::Fps, FpsCell::DrawTime]);
        assert_eq!(
            FpsMode::Update.cells(),
            &[FpsCell::Fps, FpsCell::UpdateTime]
        );
        // The thread panel has every figure once, in its rows.
        let rows: Vec<FpsCell> = FpsRow::ALL
            .iter()
            .flat_map(|r| r.cells())
            .copied()
            .collect();
        assert_eq!(rows.len(), CELLS);
        for cell in FpsCell::ALL {
            assert!(FpsMode::Full.cells().contains(&cell));
            assert_eq!(rows.iter().filter(|c| **c == cell).count(), 1, "{cell:?}");
            assert_eq!(FpsCell::ALL[cell.index()], cell);
        }
        assert!(FpsRow::ALL.iter().all(|r| r.cells().len() <= COLUMNS));
        assert!(FpsRow::Draw.has_graph() && FpsRow::Update.has_graph());
        assert!(!FpsRow::Audio.has_graph() && !FpsRow::Input.has_graph());
    }

    #[test]
    fn the_compact_panel_sits_in_the_chosen_corner_and_grows_at_once() {
        let c = counter();
        let mut view = view();
        let (width, height) = (1280.0, 768.0);
        for corner in FpsCorner::ALL {
            view.set_corner(corner);
            view.update(width, height, &c);
            let ([x, y], [w, h]) = view.bounds();
            assert_eq!(h, COMPACT_HEIGHT);
            let expected_x = if corner.left() {
                MARGIN
            } else {
                width - MARGIN - w
            };
            let expected_y = if corner.top() {
                MARGIN
            } else {
                height - MARGIN - h
            };
            assert!((x - expected_x).abs() < 1e-3, "{corner:?}");
            assert!((y - expected_y).abs() < 1e-3, "{corner:?}");
        }
        let wide = view.bounds().1[0];
        view.update(width, height, &FpsCounter::new());
        assert_eq!(view.bounds().1[0], wide);
        // A new window size starts over, and the panel scales with it.
        view.update(width, 2.0 * height, &c);
        let (_, [w, h]) = view.bounds();
        assert_eq!(h, 2.0 * COMPACT_HEIGHT);
        assert!(w < 2.0 * wide * 1.01);
        assert_eq!(FpsCorner::default(), FpsCorner::BottomRight);
    }

    /// A counter that has run steadily at `hz` for `ms` milliseconds.
    fn steady(hz: u64, ms: u64) -> FpsCounter {
        let mut c = FpsCounter::new();
        let step = 1_000_000_000 / hz;
        for n in 1..=ms * 1_000_000 / step {
            c.frame(n * step, step, hz as f64, hz as f64);
        }
        c
    }

    #[test]
    fn the_compact_panel_narrows_a_second_after_its_texts_do() {
        let mut view = view();
        let (width, height) = (1280.0, 768.0);
        // Four-digit rates in the play...
        view.update(width, height, &steady(2000, 1000));
        let wide = view.bounds().1[0];
        // ...then three in the menu: the panel holds its width a while,
        let mut menu = steady(240, 300);
        view.update(width, height, &menu);
        assert_eq!(view.bounds().1[0], wide);
        // and fits the texts once they have needed less for a second.
        let start = menu.time_ms();
        let step = 1_000_000_000 / 240;
        let mut n = 300 * 1_000_000 / step;
        let mut run = |view: &mut FpsView, menu: &mut FpsCounter, until: f64| {
            while menu.time_ms() < until {
                n += 1;
                menu.frame(n * step, step, 240.0, 240.0);
                view.update(width, height, menu);
                if view.bounds().1[0] < wide {
                    return menu.time_ms();
                }
            }
            f64::INFINITY
        };
        // Wide again for a moment: the second starts over from there.
        let held = run(&mut view, &mut menu, start + 600.0);
        assert_eq!(held, f64::INFINITY);
        view.update(width, height, &steady(2000, 1000));
        let rewide = menu.time_ms();
        let narrowed = run(&mut view, &mut menu, rewide + SHRINK_AFTER_MS + 300.0);
        assert!(
            narrowed >= rewide + SHRINK_AFTER_MS,
            "{narrowed} vs {rewide}"
        );
        assert!(
            narrowed < rewide + SHRINK_AFTER_MS + 50.0,
            "{narrowed} vs {rewide}"
        );
        let narrow = view.bounds().1[0];
        assert!(narrow < wide - 1.0, "{narrow} vs {wide}");
        // It widens again at once.
        view.update(width, height, &steady(2000, 1000));
        assert_eq!(view.bounds().1[0], wide);
    }

    #[test]
    fn more_lines_make_a_wider_panel_and_full_is_a_fixed_block() {
        let c = counter();
        let mut view = view();
        let mut widths = Vec::new();
        for mode in [FpsMode::Fps, FpsMode::Draw] {
            view.set_mode(mode);
            view.update(1280.0, 768.0, &c);
            widths.push(view.bounds().1[0]);
        }
        assert!(widths[0] < widths[1]);
        view.set_mode(FpsMode::Full);
        view.update(1280.0, 768.0, &c);
        let (_, size) = view.bounds();
        assert_eq!(size, [FULL_WIDTH, FULL_HEIGHT]);
        assert!(size[1] > 5.0 * COMPACT_HEIGHT);
    }

    #[test]
    fn shapes_are_a_shadow_a_panel_and_an_outline_then_the_graphs() {
        let c = counter();
        let mut view = view();
        let theme = Theme::default();
        let mut batch = ShapeBatch::with_capacity(MAX_FPS_SHAPES);
        view.update(1280.0, 768.0, &c);
        view.push_shapes(&mut batch, &theme, white());
        assert_eq!(batch.len(), 3);
        batch.clear();
        view.set_mode(FpsMode::Full);
        view.update(1280.0, 768.0, &c);
        view.push_shapes(&mut batch, &theme, white());
        // Two floors and the bars of two seconds of 1 ms frames.
        assert!(batch.len() > 3 + 2, "{}", batch.len());
        assert!(batch.len() <= MAX_FPS_SHAPES);
        let with_shadow = batch.len();
        // No shadow without a shadow setting.
        batch.clear();
        view.push_shapes(&mut batch, &theme.with_shadow(0.0), white());
        assert_eq!(batch.len() + 1, with_shadow);
    }

    #[test]
    fn nothing_is_drawn_while_faded_out() {
        let mut view = view();
        view.update(1280.0, 768.0, &FpsCounter::new());
        let mut batch = ShapeBatch::with_capacity(MAX_FPS_SHAPES);
        view.push_shapes(&mut batch, &Theme::default(), white());
        assert!(batch.is_empty());
    }

    #[test]
    fn unknown_figures_show_a_dash_and_xruns_turn_red() {
        let mut c = FpsCounter::new();
        assert_eq!(c.texts().audio_period.as_str(), "-");
        assert_eq!(c.texts().input_polls.as_str(), "-");
        c.set_figures(ThreadFigures {
            update_hz: 2000.0,
            audio: Some(AudioFigures {
                callbacks_hz: 500.0,
                buffer_ms: 2.667,
                xruns: 0,
            }),
            input: Some(InputFigures {
                polls_hz: 1500.0,
                events_hz: 0.0,
            }),
        });
        let t = c.texts();
        assert_eq!(t.update_rate.as_str(), "2,000");
        assert_eq!(t.audio_period.as_str(), "2.0");
        assert_eq!(t.audio_buffer.as_str(), "2.7");
        assert_eq!(t.audio_xruns.as_str(), "0");
        assert_eq!(t.colour(FpsCell::AudioXruns), None);
        assert_eq!(t.input_polls.as_str(), "1,500");
        assert_eq!(t.input_events.as_str(), "0");
        c.set_figures(ThreadFigures {
            audio: Some(AudioFigures {
                callbacks_hz: 0.0,
                buffer_ms: 2.0,
                xruns: 1234,
            }),
            ..ThreadFigures::default()
        });
        let t = c.texts();
        assert_eq!(t.audio_xruns.as_str(), "1,234");
        assert_eq!(t.audio_period.as_str(), "-");
        assert_eq!(t.colour(FpsCell::AudioXruns), Some(colour(0.0)));
        assert_eq!(t.update_rate.as_str(), "-");
        assert_eq!(t.input_polls.as_str(), "-");
    }

    #[test]
    fn the_draw_time_and_the_graphs_follow_the_frames() {
        let mut c = counter();
        assert_eq!(c.texts().draw_time.as_str(), "1.0");
        assert_eq!(c.draw_graph().max(), 1.0);
        assert_eq!(c.update_graph().max(), 1.0);
        c.frame(2100 * MS, MS, 1000.0, 1000.0);
        assert!(c.draw_graph().max() > 99.0);
        assert_eq!(c.aim_draw(), 1000.0);
        assert_eq!(c.aim_update(), 1000.0);
    }
}
