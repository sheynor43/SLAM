//! The FPS counter over every screen (ADR-0053, ADR-0060): where its inputs come from (the
//! engine's counters and rate controls, read on the draw thread once per frame), and when
//! and where it shows.

use std::sync::Arc;

use slam_engine::limiter::LimiterMode;
use slam_engine::{DrawRateControl, EngineCounters, UpdateRateControl};
use slam_play::{FpsCorner, FpsCounter, FpsLabels, FpsView};
use slam_render::atlas::AtlasRegion;
use slam_render::sprite::SpriteBatch;
use slam_text::{FontRegistry, GlyphCache};

use crate::settings::{FpsScreens, Frame};

/// What [`slam_play::FpsCounter::frame`] takes besides the frame's time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FpsInputs {
    /// The update thread's longest scheduled tick interval since the previous frame, or
    /// the last one when it ran no scheduled tick since; 0 before it has one.
    pub update_interval_ns: u64,
    /// The draw rate aimed for, in Hz.
    pub aim_draw: f64,
    /// The update rate aimed for, in Hz.
    pub aim_update: f64,
}

/// Handles of a running (or about to run) engine, made before its threads start.
#[derive(Clone, Debug)]
pub struct FpsSource {
    counters: Arc<EngineCounters>,
    update_rate: UpdateRateControl,
    draw_rate: DrawRateControl,
    /// The last interval taken, for frames between two update ticks.
    update_interval_ns: u64,
}

impl FpsSource {
    pub fn new(
        counters: Arc<EngineCounters>,
        update_rate: UpdateRateControl,
        draw_rate: DrawRateControl,
    ) -> FpsSource {
        FpsSource {
            counters,
            update_rate,
            draw_rate,
            update_interval_ns: 0,
        }
    }

    /// The inputs now: three relaxed atomic operations. Without a draw limit the update
    /// rate is the draw aim, too.
    pub fn sample(&mut self) -> FpsInputs {
        match self.counters.take_update_interval_ns() {
            0 => {}
            ns => self.update_interval_ns = ns,
        }
        let aim_update = self.update_rate.hz();
        let aim_draw = match self.draw_rate.mode() {
            LimiterMode::Hz(hz) => hz,
            LimiterMode::Unlimited => aim_update,
        };
        FpsInputs {
            update_interval_ns: self.update_interval_ns,
            aim_draw,
            aim_update,
        }
    }
}

/// Room in a text batch for the counter's two numbers.
pub const MAX_FPS_GLYPHS: usize = 48;

/// What the update thread says of the counter each tick: the settings in force (or being
/// edited), so that a change shows at once.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FpsDisplay {
    pub show: bool,
    pub corner: FpsCorner,
    pub screens: FpsScreens,
}

impl FpsDisplay {
    pub fn of(frame: &Frame) -> FpsDisplay {
        FpsDisplay {
            show: frame.show_fps,
            corner: frame.fps_corner,
            screens: frame.fps_screens,
        }
    }

    /// Whether the counter shows while the play is (`true`) or is not on top.
    pub fn shows(self, gameplay: bool) -> bool {
        self.show && self.screens.shows(gameplay)
    }
}

/// The counter on the draw thread: drawn last, over any screen.
#[derive(Debug)]
pub struct FpsOverlay {
    source: FpsSource,
    counter: FpsCounter,
    view: FpsView,
    shown: bool,
}

impl FpsOverlay {
    /// Lays out every character the counter shows: call it off the hot paths.
    pub fn new(source: FpsSource, registry: Arc<FontRegistry>, labels: &FpsLabels) -> FpsOverlay {
        FpsOverlay {
            source,
            counter: FpsCounter::new(),
            view: FpsView::new(registry, labels),
            shown: false,
        }
    }

    /// See [`FpsView::request_glyphs`].
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        self.view.request_glyphs(glyphs)
    }

    /// One drawn frame starting at `now_ns` on a window of `width` × `height` pixels, with
    /// the play on top (`gameplay`) or not. The engine's inputs are taken whether it shows
    /// or not, so that a spike while hidden does not show when it comes back. A counter
    /// shown again starts afresh, as lazer's `requestDisplay`. Allocation-free.
    pub fn frame(
        &mut self,
        now_ns: u64,
        width: f32,
        height: f32,
        display: FpsDisplay,
        gameplay: bool,
    ) {
        let inputs = self.source.sample();
        let shows = display.shows(gameplay);
        if shows && !self.shown {
            self.counter = FpsCounter::new();
        }
        self.shown = shows;
        if !shows {
            return;
        }
        self.counter.frame(
            now_ns,
            inputs.update_interval_ns,
            inputs.aim_draw,
            inputs.aim_update,
        );
        self.view.set_corner(display.corner);
        self.view.update(width, height, &self.counter);
    }

    /// The view to draw this frame, `None` while hidden.
    pub fn view(&self) -> Option<&FpsView> {
        self.shown.then_some(&self.view)
    }

    /// Writes the counter's box and text, if it shows. Allocation-free once the batches
    /// have room.
    pub fn push(
        &self,
        shapes: &mut SpriteBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        white: AtlasRegion,
    ) {
        if let Some(view) = self.view() {
            view.push_shapes(shapes, white);
            view.push_text(text, glyphs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_aims_are_the_requested_rates() {
        let update_rate = UpdateRateControl::new();
        let draw_rate = DrawRateControl::new();
        let mut source = FpsSource::new(Arc::default(), update_rate.clone(), draw_rate.clone());
        update_rate.set_hz(4000.0).unwrap();
        draw_rate.set_mode(LimiterMode::Hz(240.0)).unwrap();
        assert_eq!(
            source.sample(),
            FpsInputs {
                update_interval_ns: 0,
                aim_draw: 240.0,
                aim_update: 4000.0,
            }
        );
        // Unlimited drawing aims at the update rate.
        draw_rate.set_mode(LimiterMode::Unlimited).unwrap();
        assert_eq!(source.sample().aim_draw, 4000.0);
    }

    fn labels() -> FpsLabels {
        let placeholder = slam_play::hud::OFFSET_PLACEHOLDER;
        FpsLabels {
            frame_time: format!("{placeholder} ms"),
            fps: format!("{placeholder} fps"),
        }
    }

    #[test]
    fn the_screens_setting_picks_where_it_shows() {
        let display = |show, screens| FpsDisplay {
            show,
            corner: FpsCorner::default(),
            screens,
        };
        for (screens, gameplay, menus) in [
            (FpsScreens::Everywhere, true, true),
            (FpsScreens::Gameplay, true, false),
            (FpsScreens::Menus, false, true),
        ] {
            assert_eq!(display(true, screens).shows(true), gameplay, "{screens:?}");
            assert_eq!(display(true, screens).shows(false), menus, "{screens:?}");
            assert!(!display(false, screens).shows(true));
            assert!(!display(false, screens).shows(false));
        }
        assert_eq!(FpsDisplay::of(&Frame::default()), FpsDisplay::default());
    }

    #[test]
    fn the_overlay_shows_only_where_asked() {
        let source = FpsSource::new(
            Arc::default(),
            UpdateRateControl::new(),
            DrawRateControl::new(),
        );
        let registry = Arc::new(FontRegistry::with_builtin());
        let mut overlay = FpsOverlay::new(source, registry, &labels());
        let display = FpsDisplay {
            show: true,
            corner: FpsCorner::TopLeft,
            screens: FpsScreens::Gameplay,
        };
        overlay.frame(1_000_000, 1280.0, 768.0, display, false);
        assert!(overlay.view().is_none());
        overlay.frame(2_000_000, 1280.0, 768.0, display, true);
        assert!(overlay.view().is_some());
        overlay.frame(3_000_000, 1280.0, 768.0, FpsDisplay::default(), true);
        assert!(overlay.view().is_none());
    }
}
