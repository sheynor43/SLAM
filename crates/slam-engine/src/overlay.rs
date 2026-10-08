//! Frame-time overlay: a graph of the latest frame times, their percentiles and the
//! rates of the draw, update and input threads, drawn through the sprite batcher.
//!
//! Text uses a built-in 3×5 bitmap font of digits and the few capital letters the
//! labels need. The font sheet is rasterised at an integer scale and drawn 1:1 at
//! whole pixels, so linear filtering samples texel centres and glyphs stay sharp.

use std::fmt::Write as _;
use std::sync::Arc;

use slam_input::InputCounters;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::textures::TextureId;
use slam_render::{Color, Extent};

use crate::draw::FrameInfo;
use crate::stats::{EngineCounters, FrameSummary, FrameTimes, RateMeter};

/// Characters of the font, in sheet order. Lowercase letters are drawn as capitals;
/// anything else is drawn as a space.
pub const GLYPHS: &[u8; 30] = b" 0123456789.-ADEGHIMNPRSTUVWXZ";

/// Rows of each glyph, top first; bit 2 is the left column.
const BITMAPS: [[u8; 5]; 30] = [
    [0b000, 0b000, 0b000, 0b000, 0b000], // space
    [0b111, 0b101, 0b101, 0b101, 0b111], // 0
    [0b010, 0b110, 0b010, 0b010, 0b111], // 1
    [0b111, 0b001, 0b111, 0b100, 0b111], // 2
    [0b111, 0b001, 0b111, 0b001, 0b111], // 3
    [0b101, 0b101, 0b111, 0b001, 0b001], // 4
    [0b111, 0b100, 0b111, 0b001, 0b111], // 5
    [0b111, 0b100, 0b111, 0b101, 0b111], // 6
    [0b111, 0b001, 0b001, 0b001, 0b001], // 7
    [0b111, 0b101, 0b111, 0b101, 0b111], // 8
    [0b111, 0b101, 0b111, 0b001, 0b111], // 9
    [0b000, 0b000, 0b000, 0b000, 0b010], // .
    [0b000, 0b000, 0b111, 0b000, 0b000], // -
    [0b010, 0b101, 0b111, 0b101, 0b101], // A
    [0b110, 0b101, 0b101, 0b101, 0b110], // D
    [0b111, 0b100, 0b110, 0b100, 0b111], // E
    [0b111, 0b100, 0b101, 0b101, 0b111], // G
    [0b101, 0b101, 0b111, 0b101, 0b101], // H
    [0b111, 0b010, 0b010, 0b010, 0b111], // I
    [0b101, 0b111, 0b111, 0b101, 0b101], // M
    [0b110, 0b101, 0b101, 0b101, 0b101], // N
    [0b111, 0b101, 0b111, 0b100, 0b100], // P
    [0b110, 0b101, 0b110, 0b101, 0b101], // R
    [0b111, 0b100, 0b111, 0b001, 0b111], // S
    [0b111, 0b010, 0b010, 0b010, 0b010], // T
    [0b101, 0b101, 0b101, 0b101, 0b111], // U
    [0b101, 0b101, 0b101, 0b101, 0b010], // V
    [0b101, 0b101, 0b111, 0b111, 0b101], // W
    [0b101, 0b101, 0b010, 0b101, 0b101], // X
    [0b111, 0b001, 0b010, 0b100, 0b111], // Z
];

const GLYPH_W: u32 = 3;
const GLYPH_H: u32 = 5;
/// Transparent texels between cells of the sheet.
const CELL_GAP: u32 = 1;
/// Cells in the sheet: the glyphs, then one solid cell for rectangles.
const CELLS: u32 = GLYPHS.len() as u32 + 1;

/// Sheet cell of each ASCII character; 0 (space) for characters without a glyph.
const GLYPH_OF: [u8; 128] = {
    let mut table = [0u8; 128];
    let mut i = 0;
    while i < GLYPHS.len() {
        let g = GLYPHS[i];
        table[g as usize] = i as u8;
        table[g.to_ascii_lowercase() as usize] = i as u8;
        i += 1;
    }
    table
};

fn glyph_index(c: u8) -> usize {
    GLYPH_OF.get(c as usize).map_or(0, |&i| i as usize)
}

/// The font sheet at `scale` texels per font pixel: tightly packed RGBA8 rows, white
/// glyphs on transparent black, one cell per glyph left to right and a final solid
/// white cell. Add it to an atlas and pass the region to [`FrameOverlay::new`].
///
/// # Panics
/// If `scale` is zero.
pub fn font_sheet(scale: u32) -> (Extent, Vec<u8>) {
    assert!(scale > 0, "font scale must be positive");
    let size = sheet_size(scale);
    let mut pixels = vec![0u8; (size.width * size.height * 4) as usize];
    let mut set = |x: u32, y: u32| {
        let i = ((y * size.width + x) * 4) as usize;
        pixels[i..i + 4].fill(255);
    };
    for cell in 0..CELLS {
        let x0 = cell * (GLYPH_W * scale + CELL_GAP);
        for py in 0..GLYPH_H {
            for px in 0..GLYPH_W {
                let on = match BITMAPS.get(cell as usize) {
                    Some(rows) => rows[py as usize] >> (GLYPH_W - 1 - px) & 1 == 1,
                    None => true,
                };
                if on {
                    for y in py * scale..(py + 1) * scale {
                        for x in px * scale..(px + 1) * scale {
                            set(x0 + x, y);
                        }
                    }
                }
            }
        }
    }
    (size, pixels)
}

fn sheet_size(scale: u32) -> Extent {
    Extent::new(
        CELLS * GLYPH_W * scale + (CELLS - 1) * CELL_GAP,
        GLYPH_H * scale,
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayConfig {
    /// Texels per font pixel; must match the [`font_sheet`] in the atlas.
    pub scale: u32,
    /// Frame times shown in the graph, newest on the right.
    pub graph_bars: usize,
    pub bar_width: f32,
    pub graph_height: f32,
    /// How often the numbers and the graph scale are recomputed.
    pub refresh_ns: u64,
    /// Frame times the percentiles are taken over. A count, not a duration: the
    /// default 4096 spans 0.5 s at 8000 FPS but over a minute at 60 FPS.
    pub history: usize,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            scale: 2,
            graph_bars: 240,
            bar_width: 1.0,
            graph_height: 64.0,
            refresh_ns: 250_000_000,
            history: FrameTimes::DEFAULT_CAPACITY,
        }
    }
}

/// Longest text line, in characters.
const LINE_CAP: usize = 64;
/// Characters the background is sized for, whatever the current text: the usual
/// lines fit, and the box does not change width between refreshes.
const TEXT_COLUMNS: usize = 52;

/// A text line formatted without allocating; text past the capacity is cut.
#[derive(Clone, Copy, Debug)]
struct Line {
    buf: [u8; LINE_CAP],
    len: usize,
}

impl Line {
    const EMPTY: Self = Self {
        buf: [0; LINE_CAP],
        len: 0,
    };

    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl std::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let n = s.len().min(LINE_CAP - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Writes a rate in whole Hz, or `-` when not measured yet.
fn write_hz(line: &mut Line, label: &str, hz: Option<f64>) {
    let _ = match hz {
        Some(hz) => write!(line, "{label} {hz:.0} HZ"),
        None => write!(line, "{label} - HZ"),
    };
}

pub const BACKGROUND: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.6,
};
pub const TEXT: Color = Color::WHITE;
/// Frame time within 1.5× the median.
pub const BAR_OK: Color = Color::rgb(0.3, 0.85, 0.3);
/// Within 3× the median.
pub const BAR_SLOW: Color = Color::rgb(0.95, 0.8, 0.2);
pub const BAR_SPIKE: Color = Color::rgb(0.95, 0.25, 0.2);
/// The median line across the graph.
pub const MEDIAN_LINE: Color = Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.5,
};

/// Collects frame times and thread rates on the draw thread and draws them. Call
/// [`FrameOverlay::record`] once per frame and [`FrameOverlay::push`] when building
/// the sprite batch. Allocation-free after construction.
pub struct FrameOverlay {
    config: OverlayConfig,
    texture: TextureId,
    glyph_uv: [UvRect; GLYPHS.len()],
    /// A single texel inside the solid cell, stretched over rectangles.
    solid_uv: UvRect,
    times: FrameTimes,
    engine: Arc<EngineCounters>,
    input: Option<Arc<InputCounters>>,
    draw_hz: RateMeter,
    update_hz: RateMeter,
    input_hz: RateMeter,
    last_refresh_ns: Option<u64>,
    summary: Option<FrameSummary>,
    /// Frame time at the top of the graph, in ns; 0 before the first summary.
    graph_top_ns: f32,
    lines: [Line; 2],
}

impl FrameOverlay {
    /// `font` is where the [`font_sheet`] at `config.scale` was put in an atlas.
    /// Rates come from `engine` (pass the counters given to
    /// [`Engine::spawn_with_counters`](crate::Engine::spawn_with_counters)) and, if
    /// given, `input` ([`slam_input::InputWindow::counters`]).
    ///
    /// # Panics
    /// If `font` is not the size of the sheet at `config.scale`, or `config.history`
    /// is zero.
    pub fn new(
        config: OverlayConfig,
        font: AtlasRegion,
        engine: Arc<EngineCounters>,
        input: Option<Arc<InputCounters>>,
    ) -> Self {
        assert_eq!(
            font.size,
            sheet_size(config.scale),
            "font region does not hold the sheet at this scale"
        );
        let texel = |x: f32, y: f32| {
            let uv = font.uv;
            [
                uv.u0 + (uv.u1 - uv.u0) * x / font.size.width as f32,
                uv.v0 + (uv.v1 - uv.v0) * y / font.size.height as f32,
            ]
        };
        let cell = |i: u32| {
            let x = (i * (GLYPH_W * config.scale + CELL_GAP)) as f32;
            let [u0, v0] = texel(x, 0.0);
            let [u1, v1] = texel(
                x + (GLYPH_W * config.scale) as f32,
                (GLYPH_H * config.scale) as f32,
            );
            UvRect { u0, v0, u1, v1 }
        };
        let solid = cell(CELLS - 1);
        let centre = [(solid.u0 + solid.u1) / 2.0, (solid.v0 + solid.v1) / 2.0];
        let mut overlay = Self {
            config,
            texture: font.texture,
            glyph_uv: std::array::from_fn(|i| cell(i as u32)),
            solid_uv: UvRect {
                u0: centre[0],
                v0: centre[1],
                u1: centre[0],
                v1: centre[1],
            },
            times: FrameTimes::new(config.history),
            engine,
            input,
            draw_hz: RateMeter::new(config.refresh_ns),
            update_hz: RateMeter::new(config.refresh_ns),
            input_hz: RateMeter::new(config.refresh_ns),
            last_refresh_ns: None,
            summary: None,
            graph_top_ns: 0.0,
            lines: [Line::EMPTY; 2],
        };
        overlay.format();
        overlay
    }

    /// Records this frame's start. Every `config.refresh_ns` it recomputes the
    /// percentiles (linear in `config.history`, a few µs at the default) and the
    /// rates; that frame is slower, which the graph shows too.
    pub fn record(&mut self, info: &FrameInfo) {
        let now = info.wait.woke_ns;
        self.times.record_start(now);
        let due = match self.last_refresh_ns {
            Some(last) => now.saturating_sub(last) >= self.config.refresh_ns,
            None => true,
        };
        if due {
            self.last_refresh_ns = Some(now);
            // The rate windows match the refresh period, so the shared counters are
            // read only here, not every frame.
            self.draw_hz.sample(self.engine.draw_frames(), now);
            self.update_hz.sample(self.engine.update_ticks(), now);
            if let Some(input) = &self.input {
                self.input_hz.sample(input.polls(), now);
            }
            self.refresh();
        }
    }

    /// Latest percentiles, `None` until two frames were recorded.
    pub fn summary(&self) -> Option<FrameSummary> {
        self.summary
    }

    pub fn frame_times(&self) -> &FrameTimes {
        &self.times
    }

    /// Size of the overlay in pixels.
    pub fn size(&self) -> [f32; 2] {
        let s = self.config.scale as f32;
        let pad = 2.0 * s;
        let advance = (GLYPH_W + 1) as f32 * s;
        let text = TEXT_COLUMNS as f32 * advance;
        let graph = self.config.graph_bars as f32 * self.config.bar_width;
        [
            text.max(graph) + 2.0 * pad,
            self.lines.len() as f32 * self.line_height() + self.config.graph_height + 2.0 * pad,
        ]
    }

    /// Appends the overlay with its top-left corner at `position` (rounded to whole
    /// pixels).
    pub fn push(&self, batch: &mut SpriteBatch, position: [f32; 2]) {
        let s = self.config.scale as f32;
        let pad = 2.0 * s;
        let [x0, y0] = [position[0].round(), position[1].round()];
        self.rect(batch, [x0, y0], self.size(), BACKGROUND);

        let mut y = y0 + pad;
        for line in &self.lines {
            self.text(batch, [x0 + pad, y], line.as_bytes(), TEXT);
            y += self.line_height();
        }

        let height = self.config.graph_height;
        let bottom = y + height;
        if self.graph_top_ns > 0.0 {
            let median = self.summary.map_or(0.0, |s| s.p50_ns as f32);
            let w = self.config.bar_width;
            // Newest on the right, also before the graph is full.
            let shown = self.config.graph_bars.min(self.times.len());
            let first = (self.config.graph_bars - shown) as f32;
            for (i, ns) in self.times.latest(shown).enumerate() {
                let i = first + i as f32;
                let ns = ns as f32;
                let h = (ns / self.graph_top_ns).min(1.0) * height;
                let colour = if ns <= 1.5 * median {
                    BAR_OK
                } else if ns <= 3.0 * median {
                    BAR_SLOW
                } else {
                    BAR_SPIKE
                };
                self.rect(batch, [x0 + pad + i * w, bottom - h], [w, h], colour);
            }
            let line_y = (bottom - median / self.graph_top_ns * height).round();
            let graph_w = self.config.graph_bars as f32 * w;
            self.rect(batch, [x0 + pad, line_y], [graph_w, 1.0], MEDIAN_LINE);
        }
    }

    fn line_height(&self) -> f32 {
        (GLYPH_H + 2) as f32 * self.config.scale as f32
    }

    fn rect(&self, batch: &mut SpriteBatch, position: [f32; 2], size: [f32; 2], colour: Color) {
        let mut sprite = Sprite::new(self.texture, self.solid_uv, position, size);
        sprite.colour = colour;
        batch.push(&sprite);
    }

    fn text(&self, batch: &mut SpriteBatch, [x, y]: [f32; 2], text: &[u8], colour: Color) {
        let s = self.config.scale as f32;
        let size = [GLYPH_W as f32 * s, GLYPH_H as f32 * s];
        let advance = (GLYPH_W + 1) as f32 * s;
        for (i, &c) in text.iter().enumerate() {
            let glyph = glyph_index(c);
            if glyph == 0 {
                continue;
            }
            let mut sprite = Sprite::new(
                self.texture,
                self.glyph_uv[glyph],
                [x + i as f32 * advance, y],
                size,
            );
            sprite.colour = colour;
            batch.push(&sprite);
        }
    }

    fn refresh(&mut self) {
        self.summary = self.times.summary();
        self.graph_top_ns = self
            .summary
            .map_or(0.0, |s| (2.0 * s.p50_ns as f32).max(s.p999_ns as f32) * 1.1);
        self.format();
    }

    fn format(&mut self) {
        let [rates, times] = &mut self.lines;
        *rates = Line::EMPTY;
        write_hz(rates, "DRAW", self.draw_hz.hz());
        write_hz(rates, "  UPDATE", self.update_hz.hz());
        if self.input.is_some() {
            write_hz(rates, "  INPUT", self.input_hz.hz());
        }
        *times = Line::EMPTY;
        let ms = |ns: u32| ns as f64 / 1e6;
        let _ = match self.summary {
            Some(s) => write!(
                times,
                "P50 {:.3}  P99 {:.3}  P99.9 {:.3}  MAX {:.3} MS",
                ms(s.p50_ns),
                ms(s.p99_ns),
                ms(s.p999_ns),
                ms(s.max_ns)
            ),
            None => write!(times, "P50 -  P99 -  P99.9 -  MAX - MS"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limiter::FrameWait;
    use slam_render::Origin;

    fn region(scale: u32) -> AtlasRegion {
        AtlasRegion {
            texture: TextureId(3),
            origin: Origin::new(0, 0),
            size: sheet_size(scale),
            uv: UvRect::FULL,
        }
    }

    fn frame(woke_ns: u64) -> FrameInfo {
        FrameInfo {
            wait: FrameWait {
                deadline_ns: woke_ns,
                woke_ns,
                resynced: false,
            },
            fresh: true,
        }
    }

    fn overlay(config: OverlayConfig, input: bool) -> FrameOverlay {
        FrameOverlay::new(
            config,
            region(config.scale),
            Arc::default(),
            input.then(Arc::default),
        )
    }

    fn texel(size: Extent, pixels: &[u8], x: u32, y: u32) -> bool {
        pixels[((y * size.width + x) * 4) as usize] == 255
    }

    #[test]
    fn every_glyph_has_a_bitmap() {
        assert_eq!(GLYPHS.len(), BITMAPS.len());
        for (i, &g) in GLYPHS.iter().enumerate() {
            assert_eq!(glyph_index(g), i, "duplicate glyph {}", g as char);
        }
        assert_eq!(glyph_index(b'p'), glyph_index(b'P'));
        assert_eq!(glyph_index(b'?'), 0);
    }

    #[test]
    fn sheet_is_scaled_glyphs_and_a_solid_cell() {
        let (size, pixels) = font_sheet(2);
        assert_eq!(size, Extent::new(31 * 6 + 30, 10));
        assert_eq!(pixels.len(), (size.width * size.height * 4) as usize);
        // "1" is cell 2: its top row is .x. → only texels 2..4 of 0..6 are set.
        let x0 = 2 * 7;
        let top: Vec<bool> = (0..6).map(|x| texel(size, &pixels, x0 + x, 0)).collect();
        assert_eq!(top, [false, false, true, true, false, false]);
        assert!(texel(size, &pixels, x0 + 2, 1), "scaled vertically too");
        // Gap after the cell stays clear.
        assert!((0..10).all(|y| !texel(size, &pixels, x0 + 6, y)));
        // Solid cell fully set.
        let solid = 30 * 7;
        assert!((0..10).all(|y| (0..6).all(|x| texel(size, &pixels, solid + x, y))));
        // Space is empty.
        assert!((0..10).all(|y| (0..6).all(|x| !texel(size, &pixels, x, y))));
    }

    #[test]
    #[should_panic(expected = "font region")]
    fn rejects_a_region_of_another_scale() {
        FrameOverlay::new(OverlayConfig::default(), region(3), Arc::default(), None);
    }

    #[test]
    fn uv_of_a_glyph_inside_an_atlas_page() {
        // The sheet at (10, 20) of a 1000×100 page.
        let size = sheet_size(1);
        let font = AtlasRegion {
            texture: TextureId(0),
            origin: Origin::new(10, 20),
            size,
            uv: UvRect::of(Origin::new(10, 20), size, Extent::new(1000, 100)),
        };
        let config = OverlayConfig {
            scale: 1,
            ..OverlayConfig::default()
        };
        let o = FrameOverlay::new(config, font, Arc::default(), None);
        let one = o.glyph_uv[glyph_index(b'1')];
        let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(close(one.u0, (10.0 + 8.0) / 1000.0), "{one:?}");
        assert!(close(one.u1, (10.0 + 11.0) / 1000.0), "{one:?}");
        assert!(close(one.v0, 0.2) && close(one.v1, 0.25), "{one:?}");
        let solid_x = 10.0 + 30.0 * 4.0 + 1.5;
        assert!(close(o.solid_uv.u0, solid_x / 1000.0) && o.solid_uv.u0 == o.solid_uv.u1);
    }

    #[test]
    fn text_before_any_frame() {
        let o = overlay(OverlayConfig::default(), true);
        assert_eq!(o.lines[0].as_bytes(), b"DRAW - HZ  UPDATE - HZ  INPUT - HZ");
        assert_eq!(o.lines[1].as_bytes(), b"P50 -  P99 -  P99.9 -  MAX - MS");
        assert_eq!(o.summary(), None);
    }

    #[test]
    fn refreshes_numbers_and_rates() {
        let config = OverlayConfig::default();
        let engine = Arc::new(EngineCounters::default());
        let input = Arc::new(InputCounters::default());
        let mut o = FrameOverlay::new(
            config,
            region(config.scale),
            Arc::clone(&engine),
            Some(Arc::clone(&input)),
        );
        // 1 ms frames for 300 ms; update ticks twice per frame, input every 8 frames.
        for i in 0..=300u64 {
            engine.set_draw_frames(i);
            engine.set_update_ticks(2 * i);
            if i % 8 == 0 {
                input.record_poll();
            }
            o.record(&frame(i * 1_000_000));
        }
        let s = o.summary().unwrap();
        assert_eq!((s.samples, s.p50_ns, s.max_ns), (250, 1_000_000, 1_000_000));
        assert_eq!(
            o.lines[0].as_bytes(),
            b"DRAW 1000 HZ  UPDATE 2000 HZ  INPUT 124 HZ"
        );
        assert_eq!(
            o.lines[1].as_bytes(),
            b"P50 1.000  P99 1.000  P99.9 1.000  MAX 1.000 MS"
        );
    }

    #[test]
    fn without_input_counters_omits_input() {
        let o = overlay(OverlayConfig::default(), false);
        assert_eq!(o.lines[0].as_bytes(), b"DRAW - HZ  UPDATE - HZ");
    }

    #[test]
    fn long_text_is_cut() {
        let mut line = Line::EMPTY;
        let _ = write!(line, "{}", "9".repeat(100));
        assert_eq!(line.len, LINE_CAP);
    }

    #[test]
    fn draws_background_text_bars_and_median() {
        let config = OverlayConfig {
            graph_bars: 4,
            graph_height: 100.0,
            ..OverlayConfig::default()
        };
        let mut o = overlay(config, false);
        // Frame times 1, 1, 2, 10 ms, then one refresh.
        let mut t = 0;
        for ms in [0, 1, 1, 2, 10] {
            t += ms * 1_000_000;
            o.record(&frame(t));
        }
        o.refresh();
        let s = o.summary().unwrap();
        assert_eq!((s.p50_ns, s.p999_ns), (1_000_000, 10_000_000));

        let mut batch = SpriteBatch::new();
        o.push(&mut batch, [10.4, 20.6]);
        let glyphs: usize = o
            .lines
            .iter()
            .flat_map(|l| l.as_bytes())
            .filter(|&&c| c != b' ')
            .count();
        // Background, glyphs, 4 bars, median line.
        assert_eq!(batch.len(), 1 + glyphs + 4 + 1);
        assert!(batch.batches().iter().all(|b| b.texture == TextureId(3)));

        // Quads in order: background first, at the rounded position.
        let quad = |i: usize| {
            let v = &batch.vertices()[i * 4..i * 4 + 4];
            let xs = v.iter().map(|v| v.position[0]);
            let ys = v.iter().map(|v| v.position[1]);
            (
                xs.clone().fold(f32::MAX, f32::min),
                ys.clone().fold(f32::MAX, f32::min),
                xs.fold(f32::MIN, f32::max),
                ys.fold(f32::MIN, f32::max),
            )
        };
        let [w, h] = o.size();
        assert_eq!(quad(0), (10.0, 21.0, 10.0 + w, 21.0 + h));

        // Top of the graph is max(2 × p50, p99.9) × 1.1 = 11 ms; bars stand on the
        // graph bottom, the 10 ms spike is 10/11 of the height.
        let bottom = 21.0 + h - 4.0;
        let bars = 1 + glyphs;
        let heights: Vec<f32> = (bars..bars + 4)
            .map(|i| {
                let (_, top, _, bot) = quad(i);
                assert!((bot - bottom).abs() < 1e-3);
                bot - top
            })
            .collect();
        let expected = [1.0, 1.0, 2.0, 10.0].map(|ms| ms / 11.0 * 100.0);
        for (h, e) in heights.iter().zip(expected) {
            assert!((h - e).abs() < 1e-3, "{heights:?} vs {expected:?}");
        }
    }

    #[test]
    fn bars_start_on_the_right_before_the_graph_fills() {
        let config = OverlayConfig {
            graph_bars: 10,
            bar_width: 2.0,
            ..OverlayConfig::default()
        };
        let mut o = overlay(config, false);
        for ms in [0, 1, 2, 3] {
            o.record(&frame(ms * 1_000_000));
        }
        o.refresh();
        let mut batch = SpriteBatch::new();
        o.push(&mut batch, [0.0, 0.0]);
        let n = batch.len();
        // Three bars, then the median line; the newest bar ends at the graph's right
        // edge.
        let pad = 2.0 * config.scale as f32;
        let left = |quad: usize| batch.vertices()[quad * 4].position[0];
        assert_eq!(left(n - 4), pad + 7.0 * 2.0);
        assert_eq!(left(n - 2), pad + 9.0 * 2.0);
    }

    #[test]
    fn bar_colours_follow_the_median() {
        let config = OverlayConfig {
            graph_bars: 3,
            ..OverlayConfig::default()
        };
        let mut o = overlay(config, false);
        // Median 2 ms; then 2 (ok), 5 (slow), 7 (spike) are the newest three.
        for ms in [0, 2, 4, 6, 8, 10, 12, 17, 24] {
            o.record(&frame(ms * 1_000_000));
        }
        o.refresh();
        assert_eq!(o.summary().unwrap().p50_ns, 2_000_000);
        let mut batch = SpriteBatch::new();
        o.push(&mut batch, [0.0, 0.0]);
        let n = batch.len();
        let colour = |quad: usize| batch.vertices()[quad * 4].colour;
        let pack = |c: Color| [c.r, c.g, c.b, c.a];
        assert_eq!(colour(n - 4), pack(BAR_OK));
        assert_eq!(colour(n - 3), pack(BAR_SLOW));
        assert_eq!(colour(n - 2), pack(BAR_SPIKE));
        assert_eq!(colour(n - 1), pack(MEDIAN_LINE));
    }
}
