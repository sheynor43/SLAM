//! The FPS counter over every screen (ADR-0053, ADR-0060): where its inputs come from (the
//! engine's counters and rate controls, read on the draw thread once per frame), and when
//! and where it shows.

use std::sync::Arc;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slam_audio::{MusicStats, StreamCounters};
use slam_engine::limiter::LimiterMode;
use slam_engine::{DrawRateControl, EngineCounters, UpdateRateControl};
use slam_input::InputCounters;
use slam_play::{
    AudioFigures, FpsCorner, FpsCounter, FpsLabels, FpsMode, FpsView, InputFigures, ThreadFigures,
};
use slam_render::atlas::AtlasRegion;
use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_text::{FontRegistry, GlyphCache};
use slam_ui::theme::Theme;

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
    /// What the other threads have done lately (ADR-0138).
    pub figures: ThreadFigures,
}

/// How long the other threads' counters are watched before their rates are worked out.
const MEASURE_NS: u64 = 250_000_000;

/// The audio thread's counters and the stream's shape.
#[derive(Clone, Debug)]
enum AudioSources {
    /// One stream for the engine's whole life (the `play` command).
    Stream {
        stream: Arc<StreamCounters>,
        music: Arc<MusicStats>,
        buffer_ms: f64,
    },
    /// The shell's outputs, which come and go with the device (ADR-0089).
    Meter(Arc<AudioMeter>),
}

impl AudioSources {
    fn callbacks(&self) -> u64 {
        match self {
            AudioSources::Stream { stream, .. } => stream.callbacks.load(Ordering::Relaxed),
            AudioSources::Meter(meter) => meter.callbacks.load(Ordering::Relaxed),
        }
    }

    /// The buffer's length and the gaps so far; `None` without an output.
    fn shape(&self) -> Option<(f64, u64)> {
        match self {
            AudioSources::Stream {
                stream,
                music,
                buffer_ms,
            } => Some((
                *buffer_ms,
                stream.missed_buffers.load(Ordering::Relaxed)
                    + music.underruns.load(Ordering::Relaxed),
            )),
            AudioSources::Meter(meter) => meter.live.load(Ordering::Acquire).then(|| {
                (
                    f64::from_bits(meter.buffer_ms.load(Ordering::Relaxed)),
                    meter.xruns.load(Ordering::Relaxed),
                )
            }),
        }
    }
}

/// The shell's audio counters for the counter on the draw thread: running totals over
/// every output the window has had, kept by the update thread (see [`AudioFeed`]), as
/// the output thread reopens the stream with each device switch (ADR-0089).
#[derive(Debug, Default)]
pub struct AudioMeter {
    callbacks: AtomicU64,
    xruns: AtomicU64,
    /// `f64` bits.
    buffer_ms: AtomicU64,
    live: AtomicBool,
}

/// One output's counters as the update thread reads them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioReading {
    /// Which output: its counters' address, as each opened output has its own.
    pub output: usize,
    /// Callbacks so far.
    pub callbacks: u64,
    /// Missed buffers and music underruns so far.
    pub xruns: u64,
    /// The stream's buffer, in milliseconds.
    pub buffer_ms: f64,
}

impl AudioReading {
    /// The readings of an output of `buffer_frames` at `sample_rate`, with its tracks.
    pub fn of(
        stream: &Arc<StreamCounters>,
        music: &[&MusicStats],
        buffer_frames: u32,
        sample_rate: u32,
    ) -> AudioReading {
        AudioReading {
            output: Arc::as_ptr(stream) as usize,
            callbacks: stream.callbacks.load(Ordering::Relaxed),
            xruns: stream.missed_buffers.load(Ordering::Relaxed)
                + music
                    .iter()
                    .map(|m| m.underruns.load(Ordering::Relaxed))
                    .sum::<u64>(),
            buffer_ms: buffer_ms(buffer_frames, sample_rate),
        }
    }
}

/// The update thread's side of an [`AudioMeter`]: adds each output's counts since the
/// last tick to the totals. Atomics only.
#[derive(Debug)]
pub struct AudioFeed {
    meter: Arc<AudioMeter>,
    /// The output last seen, its callbacks and its gaps.
    last: Option<(usize, u64, u64)>,
}

impl AudioFeed {
    pub fn new(meter: Arc<AudioMeter>) -> AudioFeed {
        AudioFeed { meter, last: None }
    }

    /// Follows the output now; `None` while there is none (opening or lost).
    pub fn follow(&mut self, now: Option<AudioReading>) {
        let meter = &self.meter;
        let Some(now) = now else {
            meter.live.store(false, Ordering::Relaxed);
            self.last = None;
            return;
        };
        // An output first seen (also one swapped in between two ticks) counts from here.
        let (callbacks, xruns) = match self.last {
            Some((output, callbacks, xruns)) if output == now.output => (callbacks, xruns),
            _ => (now.callbacks, now.xruns),
        };
        meter
            .callbacks
            .fetch_add(now.callbacks.saturating_sub(callbacks), Ordering::Relaxed);
        meter
            .xruns
            .fetch_add(now.xruns.saturating_sub(xruns), Ordering::Relaxed);
        meter
            .buffer_ms
            .store(now.buffer_ms.to_bits(), Ordering::Relaxed);
        // Release: the draw thread that sees the output live sees its buffer, too.
        meter.live.store(true, Ordering::Release);
        self.last = Some((now.output, now.callbacks, now.xruns));
    }
}

fn buffer_ms(buffer_frames: u32, sample_rate: u32) -> f64 {
    f64::from(buffer_frames) * 1000.0 / f64::from(sample_rate.max(1))
}

/// Counter readings at one moment.
#[derive(Clone, Copy, Debug, Default)]
struct Reading {
    ticks: u64,
    polls: u64,
    queued: u64,
    callbacks: u64,
}

/// Handles of a running (or about to run) engine, made before its threads start.
#[derive(Clone, Debug)]
pub struct FpsSource {
    counters: Arc<EngineCounters>,
    update_rate: UpdateRateControl,
    draw_rate: DrawRateControl,
    /// The last interval taken, for frames between two update ticks.
    update_interval_ns: u64,
    input: Option<Arc<InputCounters>>,
    audio: Option<AudioSources>,
    /// The readings and the time of the last measure; the rates since.
    measured: Option<(u64, Reading)>,
    figures: ThreadFigures,
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
            input: None,
            audio: None,
            measured: None,
            figures: ThreadFigures::default(),
        }
    }

    /// Adds the input thread's counters.
    pub fn with_input(mut self, counters: Arc<InputCounters>) -> FpsSource {
        self.input = Some(counters);
        self
    }

    /// Adds the audio thread's counters, and the stream's buffer of `buffer_frames` at
    /// `sample_rate`.
    pub fn with_audio(
        mut self,
        stream: Arc<StreamCounters>,
        music: Arc<MusicStats>,
        buffer_frames: u32,
        sample_rate: u32,
    ) -> FpsSource {
        self.audio = Some(AudioSources::Stream {
            stream,
            music,
            buffer_ms: buffer_ms(buffer_frames, sample_rate),
        });
        self
    }

    /// Adds the shell's audio counters, which outlive each output.
    pub fn with_audio_meter(mut self, meter: Arc<AudioMeter>) -> FpsSource {
        self.audio = Some(AudioSources::Meter(meter));
        self
    }

    fn read(&self) -> Reading {
        Reading {
            ticks: self.counters.update_ticks(),
            polls: self.input.as_ref().map_or(0, |i| i.polls()),
            queued: self.input.as_ref().map_or(0, |i| i.queued()),
            callbacks: self.audio.as_ref().map_or(0, AudioSources::callbacks),
        }
    }

    /// Works the other threads' rates out from the counters since the last measure, every
    /// [`MEASURE_NS`]. Relaxed loads only.
    fn measure(&mut self, now_ns: u64) {
        let now = self.read();
        let Some((before_ns, before)) = self.measured else {
            self.measured = Some((now_ns, now));
            return;
        };
        let elapsed = now_ns.saturating_sub(before_ns);
        if elapsed < MEASURE_NS {
            return;
        }
        let rate = |now: u64, before: u64| now.saturating_sub(before) as f64 * 1e9 / elapsed as f64;
        self.figures =
            ThreadFigures {
                update_hz: rate(now.ticks, before.ticks),
                audio: self.audio.as_ref().and_then(AudioSources::shape).map(
                    |(buffer_ms, xruns)| AudioFigures {
                        callbacks_hz: rate(now.callbacks, before.callbacks),
                        buffer_ms,
                        xruns,
                    },
                ),
                input: self.input.as_ref().map(|_| InputFigures {
                    polls_hz: rate(now.polls, before.polls),
                    events_hz: rate(now.queued, before.queued),
                }),
            };
        self.measured = Some((now_ns, now));
    }

    /// The inputs now, a few relaxed atomic operations; the other threads' rates are
    /// worked out four times a second. Without a draw limit the update rate is the draw
    /// aim, too.
    pub fn sample(&mut self, now_ns: u64) -> FpsInputs {
        self.measure(now_ns);
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
            figures: self.figures,
        }
    }
}

/// Room in a text batch for the counter's numbers and, in its thread panel, the threads'
/// names.
pub const MAX_FPS_GLYPHS: usize = 160;
/// Room in a shape batch for the counter.
pub use slam_play::fps_view::MAX_FPS_SHAPES;

/// What the update thread says of the counter each tick: the settings in force (or being
/// edited), so that a change shows at once.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FpsDisplay {
    pub show: bool,
    pub corner: FpsCorner,
    pub screens: FpsScreens,
    pub mode: FpsMode,
}

impl FpsDisplay {
    pub fn of(frame: &Frame) -> FpsDisplay {
        FpsDisplay {
            show: frame.show_fps,
            corner: frame.fps_corner,
            screens: frame.fps_screens,
            mode: frame.fps_mode,
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
        FpsOverlay::with_view(source, FpsView::new(registry, labels))
    }

    /// The counter over `view`, laid out already.
    pub fn with_view(source: FpsSource, view: FpsView) -> FpsOverlay {
        FpsOverlay {
            source,
            counter: FpsCounter::new(),
            view,
            shown: false,
        }
    }

    /// Exchanges the counter's texts with `view`, laid out in another language (ADR-0091).
    /// The caller frees the old one off the hot paths. Does not allocate.
    pub fn swap_view(&mut self, view: &mut FpsView) {
        std::mem::swap(&mut self.view, view);
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
        let inputs = self.source.sample(now_ns);
        let shows = display.shows(gameplay);
        if shows && !self.shown {
            self.counter = FpsCounter::new();
            self.view.restart();
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
        self.counter.set_figures(inputs.figures);
        self.view.set_corner(display.corner);
        self.view.set_mode(display.mode);
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
        shapes: &mut ShapeBatch,
        theme: &Theme,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        white: AtlasRegion,
    ) {
        if let Some(view) = self.view() {
            view.push_shapes(shapes, theme, white);
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
            source.sample(0),
            FpsInputs {
                update_interval_ns: 0,
                aim_draw: 240.0,
                aim_update: 4000.0,
                figures: ThreadFigures::default(),
            }
        );
        // Unlimited drawing aims at the update rate.
        draw_rate.set_mode(LimiterMode::Unlimited).unwrap();
        assert_eq!(source.sample(1).aim_draw, 4000.0);
    }

    fn labels() -> FpsLabels {
        FpsLabels::english()
    }

    #[test]
    fn the_other_threads_rates_come_from_counter_deltas() {
        let counters = Arc::<EngineCounters>::default();
        let input = Arc::<InputCounters>::default();
        let stream = Arc::<StreamCounters>::default();
        let music = Arc::<MusicStats>::default();
        let mut source = FpsSource::new(
            Arc::clone(&counters),
            UpdateRateControl::new(),
            DrawRateControl::new(),
        )
        .with_input(Arc::clone(&input))
        .with_audio(Arc::clone(&stream), Arc::clone(&music), 128, 48_000);
        // The first sample only starts the clock.
        assert_eq!(source.sample(0).figures, ThreadFigures::default());
        for _ in 0..50 {
            input.record_poll();
        }
        for _ in 0..10 {
            input.record_queued();
        }
        stream.callbacks.fetch_add(94, Ordering::Relaxed);
        stream.missed_buffers.fetch_add(2, Ordering::Relaxed);
        music.underruns.fetch_add(1, Ordering::Relaxed);
        // Too soon to measure.
        assert_eq!(
            source.sample(MEASURE_NS / 2).figures,
            ThreadFigures::default()
        );
        // Half a second: rates are twice the deltas.
        let figures = source.sample(MEASURE_NS * 2).figures;
        // The engine's tick counter is the engine's to write.
        assert_eq!(figures.update_hz, 0.0);
        let audio = figures.audio.unwrap();
        assert_eq!(audio.callbacks_hz, 188.0);
        assert!((audio.buffer_ms - 128.0 * 1000.0 / 48_000.0).abs() < 1e-9);
        assert_eq!(audio.xruns, 3);
        let input = figures.input.unwrap();
        assert_eq!((input.polls_hz, input.events_hz), (100.0, 20.0));
    }

    #[test]
    fn the_meter_sums_the_outputs_and_shows_none_without_one() {
        let meter = Arc::<AudioMeter>::default();
        let mut feed = AudioFeed::new(Arc::clone(&meter));
        let mut source = FpsSource::new(
            Arc::default(),
            UpdateRateControl::new(),
            DrawRateControl::new(),
        )
        .with_audio_meter(Arc::clone(&meter));
        let reading = |output, callbacks, xruns, buffer_ms| {
            Some(AudioReading {
                output,
                callbacks,
                xruns,
                buffer_ms,
            })
        };
        // An output that ran before the meter saw it: only what comes after counts.
        feed.follow(reading(1, 1000, 5, 2.5));
        source.sample(0);
        feed.follow(reading(1, 1040, 6, 2.5));
        // The device switches: no figures while it opens,
        feed.follow(None);
        let figures = source.sample(MEASURE_NS).figures;
        assert_eq!(figures.audio, None);
        // and the new output counts from when it is seen.
        feed.follow(reading(2, 10, 0, 5.0));
        feed.follow(reading(2, 30, 2, 5.0));
        // An output swapped in between two ticks counts from when it is seen, too, even
        // with more counts than the old one had.
        feed.follow(reading(3, 2000, 9, 5.0));
        feed.follow(reading(3, 2005, 9, 5.0));
        let audio = source.sample(2 * MEASURE_NS).figures.audio.unwrap();
        // 20 + 5 callbacks in a quarter of a second.
        assert_eq!(audio.callbacks_hz, 25.0 * 4.0);
        assert_eq!(audio.buffer_ms, 5.0);
        assert_eq!(audio.xruns, 1 + 2);
        assert_eq!(meter.callbacks.load(Ordering::Relaxed), 40 + 20 + 5);
        let stream = Arc::<StreamCounters>::default();
        stream.callbacks.store(7, Ordering::Relaxed);
        stream.missed_buffers.store(1, Ordering::Relaxed);
        let music = MusicStats::default();
        music.underruns.store(2, Ordering::Relaxed);
        assert_eq!(
            AudioReading::of(&stream, &[&music, &music], 480, 48_000),
            AudioReading {
                output: Arc::as_ptr(&stream) as usize,
                callbacks: 7,
                xruns: 5,
                buffer_ms: 10.0,
            }
        );
    }

    #[test]
    fn missing_threads_have_no_figures() {
        let mut source = FpsSource::new(
            Arc::default(),
            UpdateRateControl::new(),
            DrawRateControl::new(),
        );
        source.sample(0);
        let figures = source.sample(MEASURE_NS).figures;
        assert_eq!((figures.audio, figures.input), (None, None));
    }

    #[test]
    fn the_screens_setting_picks_where_it_shows() {
        let display = |show, screens| FpsDisplay {
            show,
            corner: FpsCorner::default(),
            screens,
            mode: FpsMode::default(),
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
            mode: FpsMode::Full,
        };
        overlay.frame(1_000_000, 1280.0, 768.0, display, false);
        assert!(overlay.view().is_none());
        overlay.frame(2_000_000, 1280.0, 768.0, display, true);
        assert!(overlay.view().is_some());
        overlay.frame(3_000_000, 1280.0, 768.0, FpsDisplay::default(), true);
        assert!(overlay.view().is_none());
    }
}
