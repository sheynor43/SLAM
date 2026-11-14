//! The player's FPS counter (ADR-0053): the update thread's frame time and the draw
//! rate, smoothed, with spikes shown at once, coloured against the target rates and
//! dimmed while steady.
//!
//! Draw side. [`FpsCounter::frame`] runs once per drawn frame with the frame's start and
//! the update thread's last scheduled tick interval; it steps on the draw thread's
//! clock, where lazer steps on the update thread's. Allocation-free.

use slam_render::Color;

use crate::fps_graph::FrameGraph;
use crate::hud::{NumberText, number_text};

pub use crate::fps_view::{FpsCell, FpsCorner, FpsLabels, FpsMode, FpsRow, FpsView};

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/FPSCounter.cs
const MIN_TIME_BETWEEN_UPDATES: f64 = 10.0;
const SPIKE_TIME_MS: f64 = 20.0;
const DAMP_TIME: f64 = 100.0;
/// Without a significant change for this long, the counter dims.
const DISPLAY_TIME: f64 = 2000.0;
const FADE_DURATION: f64 = 300.0;
const DIMMED_ALPHA: f32 = 0.7;

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

/// What the other threads have been doing, as rates over the last second or so (ADR-0138).
/// Measured by the caller from counters that already exist; `None` where it has none.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ThreadFigures {
    /// Update ticks per second; 0 before the first measure.
    pub update_hz: f64,
    pub audio: Option<AudioFigures>,
    pub input: Option<InputFigures>,
}

/// The audio thread's figures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioFigures {
    /// Callbacks per second.
    pub callbacks_hz: f64,
    /// The buffer's length in milliseconds.
    pub buffer_ms: f64,
    /// Buffers missed plus music underruns since the stream opened.
    pub xruns: u64,
}

/// The input thread's figures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputFigures {
    /// Returns of the event wait per second.
    pub polls_hz: f64,
    /// Input events queued per second.
    pub events_hz: f64,
}

/// The counter's texts as last updated: each number and, where it is judged against a
/// target, its colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FpsTexts {
    /// The update thread's frame time, e.g. `0.9` or `12`.
    pub frame_time: NumberText,
    pub frame_time_colour: Color,
    /// The draw rate, e.g. `1,000`.
    pub fps: NumberText,
    pub fps_colour: Color,
    /// The draw thread's frame time, as the update thread's.
    pub draw_time: NumberText,
    pub draw_time_colour: Color,
    /// The other threads' figures; `-` where unknown.
    pub update_rate: NumberText,
    pub audio_period: NumberText,
    pub audio_buffer: NumberText,
    pub audio_xruns: NumberText,
    pub input_polls: NumberText,
    pub input_events: NumberText,
    /// Set while the audio thread has missed anything.
    pub audio_xruns_colour: Option<Color>,
}

impl FpsTexts {
    /// The text of `cell`.
    pub fn number(&self, cell: FpsCell) -> NumberText {
        match cell {
            FpsCell::Fps => self.fps,
            FpsCell::DrawTime => self.draw_time,
            FpsCell::UpdateTime => self.frame_time,
            FpsCell::UpdateRate => self.update_rate,
            FpsCell::AudioPeriod => self.audio_period,
            FpsCell::AudioBuffer => self.audio_buffer,
            FpsCell::AudioXruns => self.audio_xruns,
            FpsCell::InputPolls => self.input_polls,
            FpsCell::InputEvents => self.input_events,
        }
    }

    /// The colour of `cell` against its target; `None` for a plain figure.
    pub fn colour(&self, cell: FpsCell) -> Option<Color> {
        match cell {
            FpsCell::Fps => Some(self.fps_colour),
            FpsCell::DrawTime => Some(self.draw_time_colour),
            FpsCell::UpdateTime => Some(self.frame_time_colour),
            FpsCell::AudioXruns => self.audio_xruns_colour,
            _ => None,
        }
    }
}

/// The text of a figure that is not known.
fn unknown() -> NumberText {
    number_text(format_args!("-"))
}

/// A rate in Hz as a whole number.
fn format_hz(hz: f64) -> NumberText {
    grouped(round_half_away(hz))
}

/// A period in milliseconds from a rate in Hz; unknown for none.
fn format_period(hz: f64) -> NumberText {
    if hz > 0.0 && hz.is_finite() {
        format_frame_time(1000.0 / hz)
    } else {
        unknown()
    }
}

/// A buffer length in milliseconds, to a tenth.
fn format_buffer(ms: f64) -> NumberText {
    number_text(format_args!("{:.1}", ms.max(0.0)))
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
    displayed_draw_time: f64,
    aim_draw_fps: f64,
    aim_update_fps: f64,
    last_update: f64,
    last_display_required_time: f64,
    is_displayed: bool,
    fade: Fade,
    texts: FpsTexts,
    figures: ThreadFigures,
    draw_graph: FrameGraph,
    update_graph: FrameGraph,
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
            displayed_draw_time: 0.0,
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
                draw_time: NumberText::default(),
                draw_time_colour: Color::WHITE,
                update_rate: unknown(),
                audio_period: unknown(),
                audio_buffer: unknown(),
                audio_xruns: unknown(),
                input_polls: unknown(),
                input_events: unknown(),
                audio_xruns_colour: None,
            },
            figures: ThreadFigures::default(),
            draw_graph: FrameGraph::new(),
            update_graph: FrameGraph::new(),
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

        if elapsed_draw_frame_time > 0.0 {
            self.draw_graph
                .record(self.time, elapsed_draw_frame_time as f32);
        }
        if update_interval_ns > 0 {
            self.update_graph
                .record(self.time, elapsed_update_frame_time as f32);
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

        self.displayed_draw_time = if has_draw_spike {
            elapsed_draw_frame_time
        } else {
            damp_continuously(
                self.displayed_draw_time,
                elapsed_draw_frame_time,
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
        self.texts.draw_time = format_frame_time(self.displayed_draw_time);
        self.texts.draw_time_colour =
            colour((1000.0 / self.displayed_draw_time) / self.aim_draw_fps);
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

    /// Takes the other threads' figures; formats them when they changed. Allocation-free.
    pub fn set_figures(&mut self, figures: ThreadFigures) {
        if figures == self.figures {
            return;
        }
        self.figures = figures;
        let texts = &mut self.texts;
        texts.update_rate = if figures.update_hz > 0.0 {
            format_hz(figures.update_hz)
        } else {
            unknown()
        };
        match figures.audio {
            Some(audio) => {
                texts.audio_period = format_period(audio.callbacks_hz);
                texts.audio_buffer = format_buffer(audio.buffer_ms);
                texts.audio_xruns = grouped(audio.xruns);
                texts.audio_xruns_colour = (audio.xruns > 0).then(|| colour(0.0));
            }
            None => {
                texts.audio_period = unknown();
                texts.audio_buffer = unknown();
                texts.audio_xruns = unknown();
                texts.audio_xruns_colour = None;
            }
        }
        match figures.input {
            Some(input) => {
                texts.input_polls = format_hz(input.polls_hz);
                texts.input_events = format_hz(input.events_hz);
            }
            None => {
                texts.input_polls = unknown();
                texts.input_events = unknown();
            }
        }
    }

    /// The draw frame times of the last few seconds.
    pub fn draw_graph(&self) -> &FrameGraph {
        &self.draw_graph
    }

    /// The update tick intervals of the last few seconds.
    pub fn update_graph(&self) -> &FrameGraph {
        &self.update_graph
    }

    /// The draw rate aimed for, in Hz.
    pub fn aim_draw(&self) -> f64 {
        self.aim_draw_fps
    }

    /// The update rate aimed for, in Hz.
    pub fn aim_update(&self) -> f64 {
        self.aim_update_fps
    }

    /// The texts as last updated.
    pub fn texts(&self) -> &FpsTexts {
        &self.texts
    }

    /// The counter's alpha now (lazer's `mainContent`); the panel's own
    /// alpha is a fraction of it.
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    pub(crate) fn labels() -> FpsLabels {
        FpsLabels::english()
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
}
