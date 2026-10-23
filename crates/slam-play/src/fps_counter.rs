//! The player's FPS counter (ADR-0053): the update thread's frame time and the draw
//! rate, smoothed, with spikes shown at once, coloured against the target rates and
//! dimmed while steady.
//!
//! Draw side. [`FpsCounter::frame`] runs once per drawn frame with the frame's start and
//! the update thread's last scheduled tick interval; it steps on the draw thread's
//! clock, where lazer steps on the update thread's. Allocation-free.

use std::fmt;
use std::sync::Arc;

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_text::{
    FamilyId, FontRegistry, GlyphCache, GlyphKey, Layout, LayoutContext, TextStats, TextStyle,
    UI_FAMILY, push_layout,
};

use crate::hud::{NUMBER_TEXT_LEN, NumberText, Template, number_text};

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/FPSCounter.cs
const MIN_TIME_BETWEEN_UPDATES: f64 = 10.0;
const SPIKE_TIME_MS: f64 = 20.0;
const DAMP_TIME: f64 = 100.0;
/// Without a significant change for this long, the counter dims.
const DISPLAY_TIME: f64 = 2000.0;
const FADE_DURATION: f64 = 300.0;
const DIMMED_ALPHA: f32 = 0.7;
/// The background's alpha (lazer's `idle_background_alpha`; no hover here).
pub const BACKGROUND_ALPHA: f32 = 0.4;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuColour.cs (Red, Orange2, Lime0)
const RED: [f32; 3] = hex(0xed, 0x11, 0x21);
const ORANGE2: [f32; 3] = hex(0xeb, 0xc2, 0x47);
const LIME0: [f32; 3] = hex(0xcc, 0xff, 0x99);

/// `Color4Extensions.FromHex`: bytes over `255f`.
const fn hex(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

// Ported from osu-framework 2026.921.1: osu.Framework/Timing/FramedClock.cs (ProcessFrame)
/// lazer's draw `FramesPerSecond`: frames counted over windows of 250 ms. The framework
/// adds the previous frame's elapsed time (read before the clock moves); this adds the
/// current one, which gives the same rate once steady.
#[derive(Debug, Clone, Default)]
struct FrameRate {
    time_until_next_calculation: f64,
    time_since_last_calculation: f64,
    frames_since_last_calculation: i32,
    frames_per_second: f64,
}

const FPS_CALCULATION_INTERVAL: f64 = 250.0;

impl FrameRate {
    fn process(&mut self, elapsed: f64) {
        if self.time_until_next_calculation <= 0.0 {
            self.time_until_next_calculation += FPS_CALCULATION_INTERVAL;
            self.frames_per_second = if self.frames_since_last_calculation == 0 {
                0.0
            } else {
                // `int * 1000f` is a float in C#, divided as a double.
                (f64::from(self.frames_since_last_calculation as f32 * 1000.0f32)
                    / self.time_since_last_calculation)
                    .ceil()
            };
            self.time_since_last_calculation = 0.0;
            self.frames_since_last_calculation = 0;
        }
        self.frames_since_last_calculation += 1;
        self.time_until_next_calculation -= elapsed;
        self.time_since_last_calculation += elapsed;
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Utils/Interpolation.cs (DampContinuously)
fn damp_continuously(current: f64, target: f64, half_time: f64, elapsed: f64) -> f64 {
    let exponent = elapsed / half_time;
    current + (target - current) * (1.0 - 0.5f64.powf(exponent))
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/DefaultEasingFunction.cs (OutQuint)
fn out_quint(time: f64) -> f64 {
    let t = time - 1.0;
    t * t * t * t * t + 1.0
}

/// A `FadeTo(to, 300, Easing.OutQuint)` from the alpha at its start.
#[derive(Debug, Clone, Copy)]
struct Fade {
    from: f32,
    to: f32,
    start: f64,
}

impl Fade {
    fn alpha(&self, time: f64) -> f32 {
        let current = time - self.start;
        if current <= 0.0 {
            return self.from;
        }
        if current >= FADE_DURATION {
            return self.to;
        }
        // `Interpolation.ValueAt` for floats lerps in double.
        let t = out_quint(current / FADE_DURATION);
        (f64::from(self.from) + f64::from(self.to - self.from) * t) as f32
    }
}

/// The counter's two texts as last updated: the number and its colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FpsTexts {
    /// The update thread's frame time, e.g. `0.9` or `12`.
    pub frame_time: NumberText,
    pub frame_time_colour: Color,
    /// The draw rate, e.g. `1,000`.
    pub fps: NumberText,
    pub fps_colour: Color,
}

/// The counter's state on the draw thread.
#[derive(Debug, Clone)]
pub struct FpsCounter {
    /// Engine time of the first frame; `None` before it.
    start_ns: Option<u64>,
    last_frame_ns: u64,
    /// Milliseconds since the first frame (lazer's `Time.Current`).
    time: f64,
    draw_rate: FrameRate,
    displayed_fps_count: f64,
    displayed_frame_time: f64,
    aim_draw_fps: f64,
    aim_update_fps: f64,
    last_update: f64,
    last_display_required_time: f64,
    is_displayed: bool,
    fade: Fade,
    texts: FpsTexts,
}

impl Default for FpsCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl FpsCounter {
    pub fn new() -> FpsCounter {
        let mut counter = FpsCounter {
            start_ns: None,
            last_frame_ns: 0,
            time: 0.0,
            draw_rate: FrameRate::default(),
            displayed_fps_count: 0.0,
            displayed_frame_time: 0.0,
            aim_draw_fps: 0.0,
            aim_update_fps: 0.0,
            last_update: 0.0,
            last_display_required_time: 0.0,
            is_displayed: false,
            // `mainContent` starts transparent.
            fade: Fade {
                from: 0.0,
                to: 0.0,
                start: 0.0,
            },
            texts: FpsTexts {
                frame_time: NumberText::default(),
                frame_time_colour: Color::WHITE,
                fps: NumberText::default(),
                fps_colour: Color::WHITE,
            },
        };
        // `LoadComplete`.
        counter.request_display();
        counter
    }

    /// One drawn frame starting at engine time `now_ns`. `update_interval_ns` is the
    /// update thread's last scheduled tick interval (0 before it has one); the aims are
    /// the target update and draw rates in Hz.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/FPSCounter.cs (Update)
    pub fn frame(&mut self, now_ns: u64, update_interval_ns: u64, aim_draw: f64, aim_update: f64) {
        let start = *self.start_ns.get_or_insert(now_ns);
        let elapsed_draw_frame_time = if now_ns > self.last_frame_ns && self.last_frame_ns != 0 {
            (now_ns - self.last_frame_ns) as f64 / 1e6
        } else {
            0.0
        };
        self.last_frame_ns = now_ns;
        self.time = now_ns.saturating_sub(start) as f64 / 1e6;
        self.draw_rate.process(elapsed_draw_frame_time);
        let elapsed_update_frame_time = update_interval_ns as f64 / 1e6;

        // Long suspensions (debugger, sleep) are skipped.
        if elapsed_update_frame_time > 10000.0 {
            return;
        }

        let aim_rates_changed = self.update_aim_fps(aim_draw, aim_update);

        let has_update_spike =
            self.displayed_frame_time < SPIKE_TIME_MS && elapsed_update_frame_time > SPIKE_TIME_MS;
        let has_draw_spike = self.displayed_fps_count > 1000.0 / SPIKE_TIME_MS
            && elapsed_draw_frame_time > SPIKE_TIME_MS;

        // lazer damps the frame time by the update thread's elapsed time, as it runs
        // there; this runs per drawn frame, so it steps by the draw thread's.
        self.displayed_frame_time = if has_update_spike {
            elapsed_update_frame_time
        } else {
            damp_continuously(
                self.displayed_frame_time,
                elapsed_update_frame_time,
                DAMP_TIME,
                elapsed_draw_frame_time,
            )
        };

        self.displayed_fps_count = if has_draw_spike {
            1000.0 / elapsed_draw_frame_time
        } else {
            damp_continuously(
                self.displayed_fps_count,
                self.draw_rate.frames_per_second,
                DAMP_TIME,
                elapsed_draw_frame_time,
            )
        };

        if self.time - self.last_update > MIN_TIME_BETWEEN_UPDATES {
            self.update_fps_display();
            self.update_frame_time_display();
            self.last_update = self.time;
        }

        let has_significant_changes = aim_rates_changed
            || has_draw_spike
            || has_update_spike
            || self.displayed_fps_count < self.aim_draw_fps * 0.8
            || 1000.0 / self.displayed_frame_time < self.aim_update_fps * 0.8;

        if has_significant_changes {
            self.request_display();
        } else if self.is_displayed && self.time - self.last_display_required_time > DISPLAY_TIME {
            self.fade_to(DIMMED_ALPHA);
            self.is_displayed = false;
        }
    }

    fn request_display(&mut self) {
        self.last_display_required_time = self.time;
        if !self.is_displayed {
            self.fade_to(1.0);
            self.is_displayed = true;
        }
    }

    fn fade_to(&mut self, to: f32) {
        self.fade = Fade {
            from: self.fade.alpha(self.time),
            to,
            start: self.time,
        };
    }

    fn update_fps_display(&mut self) {
        self.texts.fps_colour = colour(self.displayed_fps_count / self.aim_draw_fps);
        self.texts.fps = format_fps(self.displayed_fps_count);
    }

    fn update_frame_time_display(&mut self) {
        self.texts.frame_time = format_frame_time(self.displayed_frame_time);
        self.texts.frame_time_colour =
            colour((1000.0 / self.displayed_frame_time) / self.aim_update_fps);
    }

    fn update_aim_fps(&mut self, aim_draw: f64, aim_update: f64) -> bool {
        if self.aim_draw_fps != aim_draw || self.aim_update_fps != aim_update {
            self.aim_draw_fps = aim_draw;
            self.aim_update_fps = aim_update;
            return true;
        }
        false
    }

    /// The texts as last updated.
    pub fn texts(&self) -> &FpsTexts {
        &self.texts
    }

    /// The counter's alpha now (lazer's `mainContent`); the background is
    /// [`BACKGROUND_ALPHA`] of it.
    pub fn alpha(&self) -> f32 {
        self.fade.alpha(self.time)
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/FPSCounter.cs (getColour)
/// The colour for a rate against its target: red to orange up to half, orange to lime
/// up to 90 %.
pub fn colour(performance_ratio: f64) -> Color {
    if performance_ratio < 0.5 {
        value_at(performance_ratio, RED, ORANGE2, 0.0, 0.5)
    } else {
        value_at(performance_ratio, ORANGE2, LIME0, 0.5, 0.9)
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Utils/Interpolation.cs (ValueAt, Color4)
/// Interpolates sRGB colours in linear light, time clamped to the range.
fn value_at(time: f64, start: [f32; 3], end: [f32; 3], start_time: f64, end_time: f64) -> Color {
    let current = time - start_time;
    let duration = end_time - start_time;
    let rgb = if current == 0.0 {
        start
    } else {
        // `Math.Max(0, Math.Min(1, NaN))` is NaN in C#; a missing target shows the start.
        let t = (current / duration) as f32;
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let mut rgb = [0.0; 3];
        for i in 0..3 {
            let (s, e) = (to_linear(start[i]), to_linear(end[i]));
            rgb[i] = to_srgb(s + t * (e - s));
        }
        rgb
    };
    Color::rgb(rgb[0], rgb[1], rgb[2])
}

// Ported from osu-framework 2026.921.1: osu.Framework/Extensions/Color4Extensions/Color4Extensions.cs (ToLinear, ToSRGB)
/// The float overloads, as `Color4` channels are floats.
const GAMMA: f32 = 2.4;

fn to_linear(colour: f32) -> f32 {
    if colour == 1.0 {
        return 1.0;
    }
    if colour <= 0.04045 {
        colour / 12.92
    } else {
        ((colour + 0.055) / 1.055).powf(GAMMA)
    }
}

fn to_srgb(colour: f32) -> f32 {
    if colour == 1.0 {
        return 1.0;
    }
    if colour < 0.0031308 {
        12.92 * colour
    } else {
        1.055 * colour.powf(1.0 / GAMMA) - 0.055
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/FPSCounter.cs (updateFpsDisplay)
/// The draw rate as `#,0`: a whole number with thousands separators. lazer formats in
/// the UI culture; this always uses commas.
pub fn format_fps(fps: f64) -> NumberText {
    grouped(round_half_away(fps))
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/FPSCounter.cs (updateFrameTimeDisplay)
/// The frame time as `N1` below 5 ms, `N0` from there.
pub fn format_frame_time(ms: f64) -> NumberText {
    if ms < 5.0 {
        // .NET rounds the exact value half away from zero; Rust rounds the exact value
        // too, but ties to even. Only a product that is exact can be a tie.
        let tenths = ms * 10.0;
        if tenths.fract() == 0.5 && ms.mul_add(10.0, -tenths) == 0.0 {
            let tenths = round_half_away(tenths);
            number_text(format_args!("{}.{}", tenths / 10, tenths % 10))
        } else {
            number_text(format_args!("{:.1}", ms.max(0.0)))
        }
    } else {
        grouped(round_half_away(ms))
    }
}

/// .NET rounds formatted numbers half away from zero. NaN and below 0 give 0, too large
/// values saturate.
fn round_half_away(value: f64) -> u64 {
    value.round() as u64
}

/// `value` with commas between groups of three digits.
fn grouped(value: u64) -> NumberText {
    let mut digits = NumberText::default();
    let _ = std::fmt::Write::write_fmt(&mut digits, format_args!("{value}"));
    let digits = digits.as_str();
    let mut text = NumberText::default();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            let _ = std::fmt::Write::write_char(&mut text, ',');
        }
        let _ = std::fmt::Write::write_char(&mut text, digit);
    }
    text
}

/// The counter's localised texts: the unit around each number, with
/// [`crate::hud::OFFSET_PLACEHOLDER`] standing for it.
#[derive(Debug, Clone)]
pub struct FpsLabels {
    /// The frame time, e.g. `{} ms`.
    pub frame_time: String,
    /// The draw rate, e.g. `{} fps`.
    pub fps: String,
}

/// Window height the sizes are given at; they scale with the window.
const REFERENCE_HEIGHT: f32 = 768.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/FPSCounter.cs (load)
// The box sits at the bottom right with this margin and is this tall.
const BOX_MARGIN: f32 = 5.0;
const BOX_HEIGHT: f32 = 26.0;
const FRAME_TIME_SIZE: f32 = 16.0;
const FPS_SIZE: f32 = 13.0;
const WEIGHT: u16 = 600;
// Each text's distance from the box's right edge (its margin) and its top from the box's
// top (its `Y` plus margin). lazer also tightens the letters (-1 and -2) and sets the
// digits fixed-width; text here has neither.
const FRAME_TIME_RIGHT: f32 = 1.0;
const FRAME_TIME_TOP: f32 = -1.0;
const FPS_RIGHT: f32 = 2.0;
const FPS_TOP: f32 = 12.0;

/// What a number is laid out with to settle the buffers and warm the glyphs.
const WARM_NUMBER: &str = "0123456789.,0123456789";
const BACKGROUND: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

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

/// Text sizes and positions of one window size.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Frame {
    width: f32,
    height: f32,
    scale: f32,
}

/// The draw side of the FPS counter: lays out its two texts and the box behind them.
pub struct FpsView {
    registry: Arc<FontRegistry>,
    family: FamilyId,
    context: LayoutContext,
    frame: Frame,
    corner: FpsCorner,
    /// Frame time, then fps.
    templates: [Template; 2],
    text: String,
    layouts: [Layout; 2],
    /// The numbers the layouts show; `None` lays them out on the next update.
    shown: Option<[NumberText; 2]>,
    /// The box's width in pixels: it only grows, until the window changes size.
    box_width: f32,
    colours: [Color; 2],
    alpha: f32,
    /// The templates and the number alone laid out with [`WARM_NUMBER`]: the glyphs to
    /// warm.
    warm: [Layout; 3],
}

impl fmt::Debug for FpsView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FpsView")
            .field("frame", &self.frame)
            .field("corner", &self.corner)
            .field("box_width", &self.box_width)
            .field("shown", &self.shown)
            .finish_non_exhaustive()
    }
}

impl FpsView {
    /// A view with fonts from `registry` and the localised `labels`. Lays out every
    /// character it will show, so that later layouts do not allocate: call it off the hot
    /// paths.
    pub fn new(registry: Arc<FontRegistry>, labels: &FpsLabels) -> FpsView {
        let templates = [
            Template::new(&labels.frame_time),
            Template::new(&labels.fps),
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
            templates,
            text: String::with_capacity(longest + NUMBER_TEXT_LEN),
            layouts: [Layout::new(), Layout::new()],
            shown: None,
            box_width: 0.0,
            colours: [Color::WHITE; 2],
            alpha: 0.0,
            warm: [Layout::new(), Layout::new(), Layout::new()],
        };
        let style = TextStyle::new(view.family, WEIGHT, 1.0);
        for i in 0..2 {
            view.templates[i].write(WARM_NUMBER, &mut view.text);
            view.context
                .layout_into(&view.registry, &view.text, &style, &mut view.warm[i]);
            view.context
                .layout_into(&view.registry, &view.text, &style, &mut view.layouts[i]);
        }
        view.context
            .layout_into(&view.registry, WARM_NUMBER, &style, &mut view.warm[2]);
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

    /// Puts the box in `corner` from the next frame on.
    pub fn set_corner(&mut self, corner: FpsCorner) {
        self.corner = corner;
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
        }
        let texts = counter.texts();
        self.colours = [texts.frame_time_colour, texts.fps_colour];
        self.alpha = counter.alpha();
        let numbers = [texts.frame_time, texts.fps];
        if self.shown == Some(numbers) {
            return;
        }
        self.shown = Some(numbers);
        let scale = frame.scale;
        for (i, size) in [FRAME_TIME_SIZE, FPS_SIZE].into_iter().enumerate() {
            // lazer's texts start empty; no lone unit before the first numbers.
            if numbers[i].as_str().is_empty() {
                self.text.clear();
            } else {
                self.templates[i].write(numbers[i].as_str(), &mut self.text);
            }
            let style = TextStyle::new(self.family, WEIGHT, size * scale);
            self.context
                .layout_into(&self.registry, &self.text, &style, &mut self.layouts[i]);
        }
        let [frame_time, fps] = &self.layouts;
        let counters = (frame_time.width() + 2.0 * FRAME_TIME_RIGHT * scale)
            .max(fps.width() + 2.0 * FPS_RIGHT * scale);
        self.box_width = self.box_width.max(counters);
    }

    /// The box's position and size.
    fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let Frame {
            width,
            height,
            scale,
        } = self.frame;
        let size = [self.box_width, BOX_HEIGHT * scale];
        let margin = BOX_MARGIN * scale;
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

    /// Writes the counter's background, tinting `white` (an opaque white image).
    /// Allocation-free once `batch` has room.
    pub fn push_shapes(&self, batch: &mut SpriteBatch, white: AtlasRegion) {
        if self.alpha <= 0.0 {
            return;
        }
        let (position, size) = self.bounds();
        let mut sprite = Sprite::new(white.texture, white.uv, position, size);
        sprite.colour = Color {
            a: BACKGROUND_ALPHA * self.alpha,
            ..BACKGROUND
        };
        batch.push(&sprite);
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
        let right = x + size[0];
        for (i, (from_right, top)) in [(FRAME_TIME_RIGHT, FRAME_TIME_TOP), (FPS_RIGHT, FPS_TOP)]
            .into_iter()
            .enumerate()
        {
            let layout = &self.layouts[i];
            let colour = Color {
                a: self.alpha,
                ..self.colours[i]
            };
            let position = [right - from_right * scale - layout.width(), y + top * scale];
            let s = push_layout(batch, layout, position, colour, glyphs);
            stats.drawn += s.drawn;
            stats.pending += s.pending;
        }
        stats
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::hud::OFFSET_PLACEHOLDER;

    const MS: u64 = 1_000_000;

    pub(crate) fn labels() -> FpsLabels {
        FpsLabels {
            frame_time: format!("{OFFSET_PLACEHOLDER} ms"),
            fps: format!("{OFFSET_PLACEHOLDER} fps"),
        }
    }

    /// Runs frames `period_ms` apart for `duration_ms` with a steady update interval.
    fn run(
        counter: &mut FpsCounter,
        from_ms: u64,
        duration_ms: u64,
        period_ms: u64,
        update_ms: f64,
    ) {
        let mut t = from_ms;
        while t < from_ms + duration_ms {
            counter.frame(t * MS, (update_ms * 1e6) as u64, 1000.0, 1000.0);
            t += period_ms;
        }
    }

    #[test]
    fn numbers_are_formatted_like_lazer() {
        assert_eq!(format_fps(0.0).as_str(), "0");
        assert_eq!(format_fps(999.5).as_str(), "1,000");
        assert_eq!(format_fps(12_345.0).as_str(), "12,345");
        assert_eq!(format_fps(f64::NAN).as_str(), "0");
        assert_eq!(format_frame_time(0.94).as_str(), "0.9");
        // 0.95 is a little below in binary.
        assert_eq!(format_frame_time(0.95).as_str(), "0.9");
        assert_eq!(format_frame_time(0.25).as_str(), "0.3");
        // `NaN < 5` is false, as in C#: the whole-number branch.
        assert_eq!(format_frame_time(f64::NAN).as_str(), "0");
        assert_eq!(format_frame_time(4.96).as_str(), "5.0");
        assert_eq!(format_frame_time(5.0).as_str(), "5");
        assert_eq!(format_frame_time(1234.4).as_str(), "1,234");
    }

    #[test]
    fn colours_follow_the_ratio() {
        let rgb = |c: Color| [c.r, c.g, c.b].map(|v| (v * 255.0).round() as u8);
        assert_eq!(rgb(colour(0.0)), [0xed, 0x11, 0x21]);
        assert_eq!(rgb(colour(0.5)), [0xeb, 0xc2, 0x47]);
        assert_eq!(rgb(colour(0.9)), [0xcc, 0xff, 0x99]);
        assert_eq!(rgb(colour(2.0)), [0xcc, 0xff, 0x99]);
        // Halfway in linear light is brighter than halfway in sRGB.
        let mid = colour(0.7);
        assert!(mid.g > (0xc2 as f32 + 0xff as f32) / 2.0 / 255.0, "{mid:?}");
    }

    #[test]
    fn the_draw_rate_settles_on_the_frame_rate() {
        let mut c = FpsCounter::new();
        run(&mut c, 1, 2000, 1, 1.0);
        assert_eq!(c.texts().fps.as_str(), "1,000");
        assert_eq!(c.texts().frame_time.as_str(), "1.0");
        assert_eq!(c.texts().fps_colour, colour(1.0));
    }

    #[test]
    fn an_update_spike_shows_at_once() {
        let mut c = FpsCounter::new();
        run(&mut c, 1, 1000, 1, 1.0);
        // One frame with a stalled update tick, then past the text throttle.
        c.frame(1001 * MS, 30 * MS, 1000.0, 1000.0);
        run(&mut c, 1002, 15, 1, 1.0);
        let shown: f64 = c.texts().frame_time.as_str().parse().unwrap();
        assert!(shown > 20.0 * 0.5f64.powf(15.0 / 100.0) - 1.0, "{shown}");
    }

    #[test]
    fn a_draw_spike_shows_at_once() {
        let mut c = FpsCounter::new();
        run(&mut c, 1, 1000, 1, 1.0);
        c.frame(1041 * MS, MS, 1000.0, 1000.0);
        c.frame(1052 * MS, MS, 1000.0, 1000.0);
        // 40 ms frame: 25 fps shown, then damped back up from there.
        let fps: f64 = c.texts().fps.as_str().replace(',', "").parse().unwrap();
        assert!(fps < 100.0, "{fps}");
    }

    #[test]
    fn it_fades_in_then_dims_when_steady() {
        let mut c = FpsCounter::new();
        c.frame(1000 * MS, MS, 1000.0, 1000.0);
        assert_eq!(c.alpha(), 0.0);
        run(&mut c, 1001, 400, 1, 1.0);
        assert_eq!(c.alpha(), 1.0);
        // Steady from ~0.5 s (smoothing reaches 80 % of the aims); dims 2 s later.
        run(&mut c, 1401, 3000, 1, 1.0);
        assert_eq!(c.alpha(), DIMMED_ALPHA);
        // A slow frame rate is significant again.
        run(&mut c, 4401, 1000, 4, 1.0);
        assert_eq!(c.alpha(), 1.0);
    }

    #[test]
    fn a_suspension_is_skipped() {
        let mut c = FpsCounter::new();
        run(&mut c, 1, 1000, 1, 1.0);
        let before = *c.texts();
        c.frame(20_000 * MS, 15_000 * MS, 1000.0, 1000.0);
        c.frame(20_020 * MS, 15_000 * MS, 1000.0, 1000.0);
        assert_eq!(c.texts().frame_time, before.frame_time);
    }

    #[test]
    fn the_box_sits_at_the_bottom_right_and_only_grows() {
        let mut c = FpsCounter::new();
        run(&mut c, 1, 2000, 1, 1.0);
        let mut view = FpsView::new(Arc::new(FontRegistry::with_builtin()), &labels());
        view.update(1280.0, 768.0, &c);
        let (position, size) = view.bounds();
        assert_eq!(size[1], BOX_HEIGHT);
        assert!((position[0] + size[0] - (1280.0 - BOX_MARGIN)).abs() < 1e-3);
        assert!((position[1] + size[1] - (768.0 - BOX_MARGIN)).abs() < 1e-3);
        // "1.0 ms" over "1,000 fps": the box holds the wider with its margins.
        let [frame_time, fps] = &view.layouts;
        let wider =
            (frame_time.width() + 2.0 * FRAME_TIME_RIGHT).max(fps.width() + 2.0 * FPS_RIGHT);
        assert!((size[0] - wider).abs() < 1e-3);
        assert!((fps.font_size() - FPS_SIZE).abs() < 1e-4);
        assert_eq!(view.alpha, 1.0);

        // Narrower text keeps the box; a new window size starts it again.
        view.update(1280.0, 768.0, &FpsCounter::new());
        assert_eq!(view.bounds().1[0], size[0]);
        view.update(1280.0, 1536.0, &FpsCounter::new());
        let (position, size) = view.bounds();
        assert_eq!(size[1], 2.0 * BOX_HEIGHT);
        assert!(size[0] < 2.0 * wider);
        assert!((position[1] + size[1] - (1536.0 - 2.0 * BOX_MARGIN)).abs() < 1e-3);
    }

    #[test]
    fn the_box_sits_in_the_chosen_corner() {
        let mut c = FpsCounter::new();
        run(&mut c, 1, 2000, 1, 1.0);
        let mut view = FpsView::new(Arc::new(FontRegistry::with_builtin()), &labels());
        let (width, height) = (1280.0, 768.0);
        for corner in FpsCorner::ALL {
            view.set_corner(corner);
            view.update(width, height, &c);
            let ([x, y], [w, h]) = view.bounds();
            let expected_x = if corner.left() {
                BOX_MARGIN
            } else {
                width - BOX_MARGIN - w
            };
            let expected_y = if corner.top() {
                BOX_MARGIN
            } else {
                height - BOX_MARGIN - h
            };
            assert!((x - expected_x).abs() < 1e-3, "{corner:?}");
            assert!((y - expected_y).abs() < 1e-3, "{corner:?}");
        }
        assert_eq!(FpsCorner::default(), FpsCorner::BottomRight);
    }
}
