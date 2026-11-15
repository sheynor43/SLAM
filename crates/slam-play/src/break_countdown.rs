//! The time left in a break (ADR-0142): lazer's break overlay (the seconds over a bar
//! shrinking to the break's end) or the same on a panel of SLAM's theme.

use std::sync::Arc;

use slam_osu::breaks::{BREAK_FADE_DURATION, Period};
use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::shape::{Shape, ShapeBatch};
use slam_render::sprite::SpriteBatch;
use slam_text::{
    FamilyId, FontRegistry, GlyphCache, GlyphKey, Layout, LayoutContext, TextStats, TextStyle,
    UI_FAMILY, push_layout,
};
use slam_ui::hud_layout::unit_scale;
use slam_ui::menu::Rect;
use slam_ui::theme::{ColourScheme, Theme};
use slam_ui::widgets::{Look, draw};

use crate::hit_error::LAZER_PIXELS_PER_UNIT;
use crate::service::{BreakDisplay, ServiceState};

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/BreakOverlay.cs
/// The bar's full width, a part of the window's.
const REMAINING_TIME_MAX_SIZE: f32 = 0.3;
/// The counter sits this far above the centre, in lazer pixels.
const VERTICAL_MARGIN: f32 = 15.0;
/// The bar's height, in lazer pixels.
const BAR_HEIGHT: f32 = 8.0;
/// The counter slides in from this far left, in lazer pixels.
const COUNTER_SLIDE: f32 = 50.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Break/RemainingTimeCounter.cs
/// The counter's size, in lazer pixels.
const COUNTER_SIZE: f32 = 33.0;
/// The weight of lazer's numeric font (Venera Bold).
const WEIGHT: u16 = 700;

/// SLAM's panel, in lazer pixels.
const PANEL_SIZE: [f32; 2] = [200.0, 76.0];
const PANEL_PADDING: f32 = 12.0;
const PANEL_BAR_HEIGHT: f32 = 6.0;
const PANEL_TEXT_SIZE: f32 = 32.0;

/// The most shapes [`BreakCountdownView::push_shapes`] writes: the panel with its shadow
/// and pattern, the track and the bar.
pub const MAX_BREAK_SHAPES: usize = 16;
/// The most glyphs [`BreakCountdownView::push_text`] writes.
pub const MAX_BREAK_GLYPHS: usize = 10;

/// The digits laid out once, so that their glyphs are ready before the first break.
const WARM_DIGITS: &str = "0123456789";

/// `Easing.OutQuint`.
fn out_quint(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(5)
}

/// What lazer's break overlay shows of `period` at `time`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LazerPose {
    /// The whole overlay's alpha.
    pub alpha: f32,
    /// The bar's width, a part of the window's.
    pub bar: f32,
    /// The time the counter shows, in ms; `None` before it starts.
    pub counter_ms: Option<f64>,
    /// The counter's offset from its place, in lazer pixels.
    pub counter_x: f32,
}

impl LazerPose {
    /// The overlay of `period` (lazer's: the break with its end
    /// [`BREAK_FADE_DURATION`] early) at `time`. The bar follows the time exactly rather
    /// than lazer's smoothing (`DampContinuously`, 40 ms half time).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/BreakOverlay.cs (updateDisplay, remainingTimeForCurrentPeriod, Update)
    pub fn of(period: Period, time: f64) -> LazerPose {
        let fade = BREAK_FADE_DURATION;
        let since = time - period.start;
        if since < 0.0 {
            return LazerPose {
                alpha: 0.0,
                bar: 0.0,
                counter_ms: None,
                counter_x: -COUNTER_SLIDE,
            };
        }
        let fade_in = (since / fade).clamp(0.0, 1.0);
        let fade_out = ((time - period.end) / fade).clamp(0.0, 1.0);
        let alpha = (fade_in.min(1.0 - fade_out)) as f32;
        // The bar's box opens over the fade and closes at once at the period's end
        // (osu-framework's `Delay` counts from the start of the transform before it).
        let duration = period.duration();
        let opened = if since >= duration {
            0.0
        } else {
            f64::from(REMAINING_TIME_MAX_SIZE) * out_quint(fade_in)
        };
        // Lazer takes the fade off the period's end a second time here.
        let remaining = if time <= period.end && duration > 0.0 {
            ((period.end - time - fade) / duration).max(0.0)
        } else {
            0.0
        };
        // From the period's start the counter counts down from its duration and the fade
        // to 0, linearly.
        let counter = ((duration + fade) - since).max(0.0);
        LazerPose {
            alpha,
            bar: (opened * remaining) as f32,
            counter_ms: Some(counter),
            counter_x: -COUNTER_SLIDE * (1.0 - out_quint(fade_in)) as f32,
        }
    }

    /// The counter's text: whole seconds, rounded up (`RemainingTimeCounter`).
    pub fn seconds(&self) -> Option<u32> {
        self.counter_ms.map(|ms| (ms / 1000.0).ceil() as u32)
    }
}

/// The draw side of the time left in a break.
pub struct BreakCountdownView {
    registry: Arc<FontRegistry>,
    family: FamilyId,
    context: LayoutContext,
    /// The seconds as laid out, and for which size.
    layout: Layout,
    shown: Option<(u32, u32)>,
    text: String,
    width: f32,
    height: f32,
    /// Window pixels per lazer pixel.
    unit: f32,
}

impl BreakCountdownView {
    /// The view, its digits laid out. Allocates.
    pub fn new(registry: Arc<FontRegistry>) -> BreakCountdownView {
        let family = registry.family(UI_FAMILY).unwrap_or(FamilyId(u32::MAX));
        let mut view = BreakCountdownView {
            registry,
            family,
            context: LayoutContext::new(),
            layout: Layout::new(),
            shown: None,
            text: String::with_capacity(16),
            width: 0.0,
            height: 0.0,
            unit: 0.0,
        };
        let style = TextStyle::new(view.family, WEIGHT, 1.0);
        view.context
            .layout_into(&view.registry, WARM_DIGITS, &style, &mut view.layout);
        view
    }

    /// Asks `glyphs` for the digits at the sizes the view draws them on a window of
    /// `width` × `height` pixels. Returns how many are not ready yet.
    pub fn request_glyphs(&mut self, glyphs: &mut GlyphCache, width: f32, height: f32) -> usize {
        let unit = unit_scale(width, height) / LAZER_PIXELS_PER_UNIT;
        let mut pending = 0;
        for size in [COUNTER_SIZE, PANEL_TEXT_SIZE] {
            let style = TextStyle::new(self.family, WEIGHT, size * unit);
            self.context
                .layout_into(&self.registry, WARM_DIGITS, &style, &mut self.layout);
            pending += self
                .layout
                .glyphs()
                .iter()
                .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), slam_text::Glyph::Pending))
                .count();
        }
        self.shown = None;
        pending
    }

    /// Lays out the seconds `state` shows at game time `time` on a window of `width` ×
    /// `height` pixels. Call once a frame before pushing. Allocation-free once warm.
    pub fn update(&mut self, width: f32, height: f32, state: &ServiceState, time: f64) {
        self.width = width;
        self.height = height;
        self.unit = unit_scale(width, height) / LAZER_PIXELS_PER_UNIT;
        let Some(period) = state.break_period else {
            return;
        };
        let size = match state.break_display {
            BreakDisplay::Lazer => COUNTER_SIZE,
            BreakDisplay::Slam => PANEL_TEXT_SIZE,
            _ => return,
        };
        let Some(seconds) = LazerPose::of(period, time).seconds() else {
            return;
        };
        let key = (seconds, (size * self.unit * 64.0) as u32);
        if self.shown == Some(key) {
            return;
        }
        self.shown = Some(key);
        self.text.clear();
        // At most ten digits: no allocation past the capacity.
        let _ = std::fmt::Write::write_fmt(&mut self.text, format_args!("{seconds}"));
        let style = TextStyle::new(self.family, WEIGHT, size * self.unit);
        self.context
            .layout_into(&self.registry, &self.text, &style, &mut self.layout);
    }

    /// Writes the bar or the panel for `state` at game time `time`. Allocation-free once
    /// `batch` has room for a panel.
    pub fn push_shapes(
        &self,
        batch: &mut ShapeBatch,
        state: &ServiceState,
        time: f64,
        white: AtlasRegion,
        scheme: ColourScheme,
        effects: bool,
    ) {
        let Some(period) = state.break_period else {
            return;
        };
        let pose = LazerPose::of(period, time);
        if pose.alpha <= 0.0 {
            return;
        }
        match state.break_display {
            BreakDisplay::Lazer => {
                let [cx, cy] = [self.width / 2.0, self.height / 2.0];
                let width = pose.bar * self.width;
                // A circle: the height never more than the width.
                let height = (BAR_HEIGHT * self.unit).min(width);
                if width <= 0.0 {
                    return;
                }
                let shape = Shape {
                    radius: height / 2.0,
                    ..Shape::new(
                        white.texture,
                        white.uv,
                        [cx - width / 2.0, cy - height / 2.0],
                        [width, height],
                        Color {
                            a: pose.alpha,
                            ..Color::WHITE
                        },
                    )
                };
                batch.push_shape(&shape);
            }
            BreakDisplay::Slam => {
                let theme = Theme::new(scheme, 1.0).with_effects(effects);
                let unit = self.unit;
                let panel = self.panel();
                let fill = Color {
                    a: theme.menu_panel_fill.a * pose.alpha,
                    ..theme.menu_panel_fill
                };
                draw(batch, white, &panel, fill, Look::Panel, &theme, unit);
                // The bar under the seconds, full at the period's start.
                let pad = PANEL_PADDING * unit;
                let track = Rect {
                    x: panel.x + pad,
                    y: panel.y + panel.height - pad - PANEL_BAR_HEIGHT * unit,
                    width: panel.width - 2.0 * pad,
                    height: PANEL_BAR_HEIGHT * unit,
                };
                let track_fill = Color {
                    a: theme.song_bar_track_fill.a * pose.alpha,
                    ..theme.song_bar_track_fill
                };
                draw(batch, white, &track, track_fill, Look::Plain, &theme, unit);
                let left = slam_left(period, time);
                if left > 0.0 {
                    let bar = Rect {
                        width: track.width * left,
                        ..track
                    };
                    let accent = Color {
                        a: theme.accent.a * pose.alpha,
                        ..theme.accent
                    };
                    draw(batch, white, &bar, accent, Look::Plain, &theme, unit);
                }
            }
            _ => {}
        }
    }

    /// Writes the seconds for `state` at game time `time`, as laid out by
    /// [`BreakCountdownView::update`].
    pub fn push_text(
        &self,
        batch: &mut SpriteBatch,
        state: &ServiceState,
        time: f64,
        glyphs: &mut GlyphCache,
        scheme: ColourScheme,
    ) -> TextStats {
        let Some(period) = state.break_period else {
            return TextStats::default();
        };
        let pose = LazerPose::of(period, time);
        if pose.alpha <= 0.0 || pose.counter_ms.is_none() || self.shown.is_none() {
            return TextStats::default();
        }
        let (w, h) = (self.layout.width(), self.layout.height());
        let unit = self.unit;
        match state.break_display {
            BreakDisplay::Lazer => {
                // Bottom centre `VERTICAL_MARGIN` over the window's centre.
                let x = self.width / 2.0 + pose.counter_x * unit - w / 2.0;
                let y = self.height / 2.0 - VERTICAL_MARGIN * unit - h;
                let colour = Color {
                    a: pose.alpha,
                    ..Color::WHITE
                };
                push_layout(batch, &self.layout, [x, y], colour, glyphs)
            }
            BreakDisplay::Slam => {
                let theme = Theme::new(scheme, 1.0);
                let panel = self.panel();
                let pad = PANEL_PADDING * unit;
                let room = panel.height - 3.0 * pad - PANEL_BAR_HEIGHT * unit;
                let x = panel.x + (panel.width - w) / 2.0;
                let y = panel.y + pad + (room - h) / 2.0;
                let colour = Color {
                    a: theme.text.a * pose.alpha,
                    ..theme.text
                };
                push_layout(batch, &self.layout, [x, y], colour, glyphs)
            }
            _ => TextStats::default(),
        }
    }

    /// SLAM's panel, centred in the window.
    fn panel(&self) -> Rect {
        let [w, h] = PANEL_SIZE.map(|s| s * self.unit);
        Rect {
            x: (self.width - w) / 2.0,
            y: (self.height - h) / 2.0,
            width: w,
            height: h,
        }
    }
}

/// The part of `period` left at `time` for SLAM's bar: 1 at its start, 0 at its end.
fn slam_left(period: Period, time: f64) -> f32 {
    if period.duration() <= 0.0 {
        return 0.0;
    }
    ((period.end - time) / period.duration()).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests;
