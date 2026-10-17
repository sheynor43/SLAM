//! The hit error meter: lazer's `BarHitErrorMeter` with its default settings, without the
//! labels (hare and tortoise icons) and the appearing animations.
//!
//! Update side: [`HitErrorTargets`] picks the judgements the meter shows and their offsets;
//! [`HitErrorRing`] carries them in the snapshot. Draw side: [`HitErrorView`] keeps the alive
//! judgement lines and the average arrow, evaluated at the frame's game time as the popups are
//! (ADR-0043): transforms start at the judgement's time, not at the frame that took it.
//!
//! [`HitErrorMeter`] picks this meter or the legacy one ([`crate::legacy_hit_error`]) by
//! [`HitErrorStyle`] (ADR-0050).
//!
//! Lazer's default layouts put two vertical meters at the sides; this one is a single
//! horizontal bar at the bottom centre: the vertical meter turned so that early hits are on
//! the left and the average arrow is above the bar.

use slam_osu::Beatmap;
use slam_osu::hit_windows::{HitWindows, MISS_WINDOW};
use slam_osu::judgement::{HitResult, Judgement};
use slam_osu::objects::{NestedKind, OsuHitObjectKind};

use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::hal::Extent;
use slam_render::image::Image;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};

use crate::circles::{Easing, value_at};
use crate::legacy_hit_error::{LegacyHitErrorView, MAX_LEGACY_METER_SPRITES};
use crate::ring::EventRing;

/// Judgements a snapshot keeps for the meter between two frames.
pub const HIT_ERROR_RING_CAPACITY: usize = 64;

/// A judgement the meter shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitError {
    /// Gameplay time of the judgement.
    pub time: f64,
    /// How late the hit was, in milliseconds (negative when early): lazer's `TimeOffset`.
    pub offset: f64,
    pub result: HitResult,
}

impl Default for HitError {
    fn default() -> Self {
        HitError {
            time: 0.0,
            offset: 0.0,
            result: HitResult::None,
        }
    }
}

/// The judgements the meter shows, newest last.
pub type HitErrorRing = EventRing<HitError, HIT_ERROR_RING_CAPACITY>;

/// The judgement of an object that has hit windows: lazer gives them to hit circles and
/// slider heads only (the others' are `HitWindows.Empty`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Windowed {
    None,
    /// A hit circle at this time.
    Circle(f64),
    /// A slider whose head is this nested object at this time.
    Head(u32, f64),
}

/// Which judgements of a play the meter shows. Built at load.
#[derive(Debug, Clone)]
pub struct HitErrorTargets {
    objects: Vec<Windowed>,
}

impl HitErrorTargets {
    pub fn new(beatmap: &Beatmap) -> Self {
        let objects = beatmap
            .hit_objects
            .iter()
            .map(|o| match &o.kind {
                OsuHitObjectKind::Circle => Windowed::Circle(o.start_time),
                OsuHitObjectKind::Slider(slider) => slider
                    .nested
                    .iter()
                    .position(|n| n.kind == NestedKind::Head)
                    .map_or(Windowed::None, |i| {
                        Windowed::Head(i as u32, slider.nested[i].start_time)
                    }),
                OsuHitObjectKind::Spinner(_) => Windowed::None,
            })
            .collect();
        HitErrorTargets { objects }
    }

    /// What the meter shows of `judgement`, if anything.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/BarHitErrorMeter.cs (OnNewJudgement)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Judgements/JudgementResult.cs (TimeOffset)
    pub fn error(&self, judgement: &Judgement) -> Option<HitError> {
        // `IsScorable` holds for every result of an object with hit windows.
        if !judgement.result.is_hit() || judgement.result.is_bonus() {
            return None;
        }
        let time = match (
            self.objects.get(judgement.object.index as usize)?,
            judgement.object.nested,
        ) {
            (Windowed::Circle(time), None) => *time,
            (Windowed::Head(head, time), Some(n)) if n == *head => *time,
            _ => return None,
        };
        // Circles end where they start; their `MaximumJudgementOffset` is the miss window.
        Some(HitError {
            time: judgement.time,
            offset: (judgement.time - time).min(MISS_WINDOW),
            result: judgement.result,
        })
    }
}

// ---- Draw side ----

/// Which meter a play shows (ADR-0050).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HitErrorStyle {
    /// Stable's, lazer's `LegacyBarHitErrorMeter`: [`LegacyHitErrorView`].
    #[default]
    Legacy,
    /// Lazer's `BarHitErrorMeter`: [`HitErrorView`].
    Bar,
}

/// The meter of a play in its style.
#[derive(Debug, Clone)]
pub enum HitErrorMeter {
    Legacy(Box<LegacyHitErrorView>),
    Bar(Box<HitErrorView>),
}

impl HitErrorMeter {
    /// The meter in `style` of a play with the hit `windows` of its beatmap (mods applied),
    /// `size` times as large as lazer's.
    pub fn new(style: HitErrorStyle, windows: &HitWindows, size: f32) -> Self {
        match style {
            HitErrorStyle::Legacy => {
                HitErrorMeter::Legacy(Box::new(LegacyHitErrorView::new(windows).sized(size)))
            }
            HitErrorStyle::Bar => {
                HitErrorMeter::Bar(Box::new(HitErrorView::new(windows).sized(size)))
            }
        }
    }

    pub fn style(&self) -> HitErrorStyle {
        match self {
            HitErrorMeter::Legacy(_) => HitErrorStyle::Legacy,
            HitErrorMeter::Bar(_) => HitErrorStyle::Bar,
        }
    }

    /// Clears the judgements shown, as lazer on a seek.
    pub fn clear(&mut self) {
        match self {
            HitErrorMeter::Legacy(view) => view.clear(),
            HitErrorMeter::Bar(view) => view.clear(),
        }
    }

    /// Takes the new judgements of `ring` and drops what expired by game `time`.
    pub fn update(&mut self, ring: &HitErrorRing, restarts: u32, time: f64) {
        match self {
            HitErrorMeter::Legacy(view) => view.update(ring, restarts, time),
            HitErrorMeter::Bar(view) => view.update(ring, restarts, time),
        }
    }

    /// Writes the meter at game `time` on a window of `width` × `height` pixels.
    /// Allocation-free once `batch` has room for [`MAX_HIT_ERROR_SPRITES`].
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        width: f32,
        height: f32,
        time: f64,
        textures: &MeterTextures,
    ) {
        match self {
            HitErrorMeter::Legacy(view) => view.push(batch, width, height, time, textures),
            HitErrorMeter::Bar(view) => view.push(batch, width, height, time, textures),
        }
    }
}

/// The most sprites [`HitErrorMeter::push`] writes in any style.
pub const MAX_HIT_ERROR_SPRITES: usize = if MAX_METER_SPRITES > MAX_LEGACY_METER_SPRITES {
    MAX_METER_SPRITES
} else {
    MAX_LEGACY_METER_SPRITES
};

/// The most judgement lines alive at once; beyond it the oldest goes.
pub const MAX_LINES: usize = 128;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/BarHitErrorMeter.cs
const JUDGEMENT_LINE_WIDTH: f32 = 14.0;
const JUDGEMENT_LINE_THICKNESS: f32 = 4.0;
const MAX_CONCURRENT_JUDGEMENTS: usize = 50;
const CENTRE_MARKER_SIZE: f32 = 8.0;
const BAR_HEIGHT: f32 = 200.0;
const BAR_WIDTH: f32 = 2.0;
const CHEVRON_SIZE: f32 = 8.0;
const METER_MARGIN: f32 = 2.0;
const GRADIENT_START: f32 = 0.8;
const ARROW_MOVE_DURATION: f64 = 800.0;
const QUICK_FADE_TIME: f64 = 100.0;
const JUDGEMENT_FADE_IN_DURATION: f64 = 100.0;
const JUDGEMENT_FADE_OUT_DURATION: f64 = 5000.0;
const JUDGEMENT_LINE_ALPHA: f32 = 0.6;

/// Width of the meter across the bar, in lazer's units: the chevron, then the colour axis.
const METER_WIDTH: f32 = CHEVRON_SIZE + JUDGEMENT_LINE_WIDTH;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Containers/ScalingContainer.cs
/// The HUD's virtual size, fitted into the window by its smaller ratio.
pub(crate) const TARGET_SIZE: [f32; 2] = [1024.0, 768.0];
/// Space between the meter and the bottom of the window, in virtual units.
const BOTTOM_MARGIN: f32 = 10.0;

const fn hex(rgb: u32) -> Color {
    Color::rgb(
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
    )
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuColour.cs (ForHitResult)
/// The colour of `result` on the meter.
pub fn colour_for(result: HitResult) -> Color {
    match result {
        // osuTK's `Color4.Gray`.
        HitResult::IgnoreMiss | HitResult::SmallTickMiss => {
            Color::rgb(0.5019608, 0.5019608, 0.5019608)
        }
        HitResult::Miss | HitResult::LargeTickMiss | HitResult::ComboBreak => hex(0xed1121),
        HitResult::Meh => hex(0xffcc22),
        HitResult::Ok => hex(0x88b300),
        HitResult::Good => hex(0xb3d944),
        HitResult::SmallTickHit
        | HitResult::LargeTickHit
        | HitResult::SliderTailHit
        | HitResult::Great => hex(0x66ccff),
        _ => hex(0x99eeff),
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Extensions/Color4Extensions/Color4Extensions.cs (Darken)
fn darken(colour: Color, amount: f32) -> Color {
    let scalar = (1.0 + amount).max(1.0);
    Color {
        r: colour.r / scalar,
        g: colour.g / scalar,
        b: colour.b / scalar,
        a: colour.a,
    }
}

/// Where an offset lands along the bar: 0 at the early end, 1 at the late end.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/BarHitErrorMeter.cs (getRelativeJudgementPosition)
pub fn relative_position(offset: f64, max_window: f64) -> f32 {
    (((offset / max_window) + 1.0) as f32 / 2.0).clamp(0.0, 1.0)
}

/// A judgement line on the meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JudgementLine {
    /// Gameplay time it appeared at.
    pub time: f64,
    /// Along the bar, 0..=1.
    pub position: f32,
    pub result: HitResult,
    /// When it was told to fade out quickly (too many alive), if it was.
    pub cut: Option<f64>,
}

impl JudgementLine {
    const EMPTY: JudgementLine = JudgementLine {
        time: 0.0,
        position: 0.5,
        result: HitResult::None,
        cut: None,
    };

    /// When it expires.
    pub fn end(&self) -> f64 {
        match self.cut {
            Some(cut) => cut + QUICK_FADE_TIME,
            None => self.time + JUDGEMENT_FADE_IN_DURATION + JUDGEMENT_FADE_OUT_DURATION,
        }
    }

    /// Alpha and width (relative to the colour axis) at game `time`.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/BarHitErrorMeter.cs (JudgementLine.PrepareForUse, OnNewJudgement)
    pub fn pose(&self, time: f64) -> LinePose {
        match self.cut {
            // `ClearTransforms` keeps the values reached, then `FadeOut(quick_fade_time)`.
            Some(cut) if time >= cut => {
                let at_cut = natural_pose(cut - self.time);
                LinePose {
                    alpha: value_at(
                        time,
                        at_cut.alpha,
                        0.0,
                        cut,
                        cut + QUICK_FADE_TIME,
                        Easing::None,
                    )
                    .max(0.0),
                    width: at_cut.width,
                }
            }
            _ => natural_pose(time - self.time),
        }
    }
}

/// A judgement line at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinePose {
    pub alpha: f32,
    /// Relative to the colour axis' width.
    pub width: f32,
}

/// A line `elapsed` ms after it appeared, without cuts.
fn natural_pose(elapsed: f64) -> LinePose {
    let fade_in = JUDGEMENT_FADE_IN_DURATION;
    if elapsed < fade_in {
        let elapsed = elapsed.max(0.0);
        return LinePose {
            alpha: value_at(
                elapsed,
                0.0,
                JUDGEMENT_LINE_ALPHA,
                0.0,
                fade_in,
                Easing::OutQuint,
            ),
            width: value_at(elapsed, 0.0, 1.0, 0.0, fade_in, Easing::OutQuint),
        };
    }
    let end = fade_in + JUDGEMENT_FADE_OUT_DURATION;
    let elapsed = elapsed.min(end);
    LinePose {
        alpha: value_at(
            elapsed,
            JUDGEMENT_LINE_ALPHA,
            0.0,
            fade_in,
            end,
            Easing::None,
        ),
        width: value_at(elapsed, 1.0, 0.0, fade_in, end, Easing::InQuint),
    }
}

/// The average arrow's move along the bar.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ArrowMove {
    from: f32,
    to: f32,
    start: f64,
}

impl ArrowMove {
    const CENTRE: ArrowMove = ArrowMove {
        from: 0.5,
        to: 0.5,
        start: f64::NEG_INFINITY,
    };

    fn at(&self, time: f64) -> f32 {
        if time >= self.start + ARROW_MOVE_DURATION {
            self.to
        } else if time <= self.start {
            self.from
        } else {
            value_at(
                time,
                self.from,
                self.to,
                self.start,
                self.start + ARROW_MOVE_DURATION,
                Easing::OutQuint,
            )
        }
    }
}

/// The images the meter tints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterTextures {
    /// Opaque white.
    pub white: AtlasRegion,
    /// A white disc on transparent.
    pub disc: AtlasRegion,
    /// White, opaque at `u0` and transparent at `u1`.
    pub gradient: AtlasRegion,
    /// A white triangle pointing down, from
    /// [`triangle_image`](crate::legacy_hit_error::triangle_image).
    pub triangle: AtlasRegion,
}

/// The meter of a play on the draw side.
#[derive(Debug, Clone)]
pub struct HitErrorView {
    /// The colour zones from the outside in: Meh, Ok, Great with their windows.
    windows: [(HitResult, f64); 3],
    max_window: f64,
    /// Multiplier of the meter's size (ADR-0050).
    size: f32,
    /// Alive lines, oldest first.
    lines: [JudgementLine; MAX_LINES],
    len: usize,
    /// Judgements of the ring taken.
    seen: u64,
    restarts: u32,
    /// Lines that went early: the ring overflowed between frames or too many were alive.
    dropped: u64,
    floating_average: f64,
    arrow: ArrowMove,
}

impl HitErrorView {
    /// The meter of a play with the hit `windows` of its beatmap (mods applied).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/BarHitErrorMeter.cs (load, createColourBars)
    pub fn new(windows: &HitWindows) -> Self {
        // `GetAllAvailableWindows` of the hits, Meh first; max to avoid division by zero.
        let windows = [HitResult::Meh, HitResult::Ok, HitResult::Great]
            .map(|result| (result, windows.window_for(result)));
        HitErrorView {
            windows,
            max_window: windows[0].1.max(1.0),
            size: 1.0,
            lines: [JudgementLine::EMPTY; MAX_LINES],
            len: 0,
            seen: 0,
            restarts: 0,
            dropped: 0,
            floating_average: 0.0,
            arrow: ArrowMove::CENTRE,
        }
    }

    /// The meter `size` times as large (1 is lazer's).
    pub fn sized(self, size: f32) -> Self {
        debug_assert!(size.is_finite() && size > 0.0, "meter size {size}");
        HitErrorView { size, ..self }
    }

    /// Lazer's `Clear` on a seek: the lines go, the average and the arrow stay.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/BarHitErrorMeter.cs (Clear)
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Takes the new judgements of `ring` and drops the lines expired by game `time`. A
    /// restart clears the meter.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/BarHitErrorMeter.cs (OnNewJudgement)
    pub fn update(&mut self, ring: &HitErrorRing, restarts: u32, time: f64) {
        if restarts != self.restarts {
            self.restarts = restarts;
            self.len = 0;
            self.floating_average = 0.0;
            self.arrow = ArrowMove::CENTRE;
        }
        self.dropped += ring.skipped(self.seen);
        for error in ring.since(self.seen) {
            // Lazer cuts and expires by the frame's `Clock.CurrentTime`; the judgement's time
            // keeps the meter the same however frames fall (ADR-0043).
            let now = error.time;
            self.expire(now);
            if self.len > MAX_CONCURRENT_JUDGEMENTS {
                // With a bit of lenience against precision errors, as lazer.
                let lenient = now + QUICK_FADE_TIME * 1.1;
                if let Some(old) = self.lines[..self.len]
                    .iter_mut()
                    .find(|l| l.end() > lenient)
                {
                    old.cut = Some(now);
                }
            }
            if self.len == MAX_LINES {
                self.lines.copy_within(1..self.len, 0);
                self.len -= 1;
                self.dropped += 1;
            }
            self.lines[self.len] = JudgementLine {
                time: now,
                position: relative_position(error.offset, self.max_window),
                result: error.result,
                cut: None,
            };
            self.len += 1;
            self.floating_average = self.floating_average * 0.9 + error.offset * 0.1;
            self.arrow = ArrowMove {
                from: self.arrow.at(now),
                to: relative_position(self.floating_average, self.max_window),
                start: now,
            };
        }
        self.seen = ring.written();
        self.expire(time);
    }

    /// Drops the lines expired at `time`.
    fn expire(&mut self, time: f64) {
        let mut kept = 0;
        for i in 0..self.len {
            if time < self.lines[i].end() {
                self.lines[kept] = self.lines[i];
                kept += 1;
            }
        }
        self.len = kept;
    }

    /// The alive lines, oldest first.
    pub fn lines(&self) -> &[JudgementLine] {
        &self.lines[..self.len]
    }

    /// Lines that went before their end.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The moving average of the offsets, in milliseconds.
    pub fn floating_average(&self) -> f64 {
        self.floating_average
    }

    /// Where the average arrow is along the bar at game `time`, 0..=1.
    pub fn arrow_position(&self, time: f64) -> f32 {
        self.arrow.at(time)
    }

    /// Writes the meter at game `time` on a window of `width` × `height` pixels.
    /// Allocation-free once `batch` has room for [`MAX_METER_SPRITES`].
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HitErrorMeters/BarHitErrorMeter.cs (load, createColourBars, recreateCentreMarker)
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        width: f32,
        height: f32,
        time: f64,
        textures: &MeterTextures,
    ) {
        let place = Placement::new(width, height, self.size);
        let mut draw = Draw {
            batch,
            place,
            textures,
        };
        // The colour axis' centre line, in the meter's (vertical) units.
        let axis = CHEVRON_SIZE + JUDGEMENT_LINE_WIDTH / 2.0;
        let middle = BAR_HEIGHT / 2.0;
        let great = colour_for(self.windows[2].0);

        draw.disc([axis, middle], [CENTRE_MARKER_SIZE; 2], great, Blend::Alpha);
        // The early bars grow up from the middle, the late ones down.
        for direction in [-1.0, 1.0] {
            for (i, &(result, window)) in self.windows.iter().enumerate() {
                let colour = colour_for(result);
                let length = (window / self.max_window) as f32 * middle;
                if i == 0 {
                    let solid = length * GRADIENT_START;
                    draw.bar(axis, middle, direction, 0.0, solid, colour);
                    // The gradient box sits at 0.8 of the half bar, whatever its length.
                    draw.gradient(
                        axis,
                        middle,
                        direction,
                        middle * GRADIENT_START,
                        length * (1.0 - GRADIENT_START),
                        colour,
                    );
                } else {
                    draw.bar(axis, middle, direction, 0.0, length, colour);
                }
            }
        }
        for line in self.lines() {
            if time < line.time {
                continue;
            }
            let pose = line.pose(time);
            if pose.alpha <= 0.0 || pose.width <= 0.0 {
                continue;
            }
            let colour = colour_for(line.result);
            draw.capsule(
                [axis, line.position * BAR_HEIGHT],
                [JUDGEMENT_LINE_WIDTH * pose.width, JUDGEMENT_LINE_THICKNESS],
                Color {
                    a: colour.a * pose.alpha,
                    ..colour
                },
            );
        }
        draw.disc(
            [axis, middle],
            [CENTRE_MARKER_SIZE / 2.0; 2],
            darken(great, 0.3),
            Blend::Alpha,
        );
        draw.chevron([CHEVRON_SIZE / 2.0, self.arrow.at(time) * BAR_HEIGHT]);
    }
}

/// The most sprites [`HitErrorView::push`] writes.
pub const MAX_METER_SPRITES: usize = 2 + 2 * 4 + 3 * MAX_LINES + 2;

/// How the meter's own units (lazer's vertical meter: x across, y along the bar from the
/// early end) land in the window: turned so that y runs left to right and x top to bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    origin: [f32; 2],
    scale: f32,
}

impl Placement {
    /// The meter `size` times as large; the margin below it stays (a drawable's scale does
    /// not move it).
    fn new(width: f32, height: f32, size: f32) -> Self {
        let hud = (width / TARGET_SIZE[0]).min(height / TARGET_SIZE[1]);
        let scale = hud * size;
        let across = METER_WIDTH + METER_MARGIN;
        Placement {
            origin: [
                width / 2.0 - BAR_HEIGHT / 2.0 * scale,
                height - BOTTOM_MARGIN * hud - across * scale,
            ],
            scale,
        }
    }

    fn point(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [
            self.origin[0] + y * self.scale,
            self.origin[1] + x * self.scale,
        ]
    }

    fn size(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [y * self.scale, x * self.scale]
    }
}

struct Draw<'a> {
    batch: &'a mut SpriteBatch,
    place: Placement,
    textures: &'a MeterTextures,
}

impl Draw<'_> {
    /// A sprite of `region`'s `uv` centred on `centre` with `size`, both in window pixels.
    fn sprite(
        &mut self,
        region: &AtlasRegion,
        uv: UvRect,
        centre: [f32; 2],
        size: [f32; 2],
        colour: Color,
        blend: Blend,
    ) {
        let mut sprite = Sprite::new(
            region.texture,
            uv,
            [centre[0] - size[0] / 2.0, centre[1] - size[1] / 2.0],
            size,
        );
        sprite.colour = colour;
        sprite.blend = blend;
        self.batch.push(&sprite);
    }

    fn disc(&mut self, centre: [f32; 2], size: [f32; 2], colour: Color, blend: Blend) {
        let disc = self.textures.disc;
        let (centre, size) = (self.place.point(centre), self.place.size(size));
        self.sprite(&disc, disc.uv, centre, size, colour, blend);
    }

    /// A colour bar `BAR_WIDTH` wide on the axis at `x`, from `start` to `start + length` away
    /// from the `middle` in `direction` (-1 early, 1 late).
    fn bar(&mut self, x: f32, middle: f32, direction: f32, start: f32, length: f32, colour: Color) {
        let white = self.textures.white;
        let centre = self
            .place
            .point([x, middle + direction * (start + length / 2.0)]);
        let size = self.place.size([BAR_WIDTH, length]);
        self.sprite(&white, white.uv, centre, size, colour, Blend::Alpha);
    }

    /// As [`Draw::bar`], fading from `colour` to transparent away from the middle.
    fn gradient(
        &mut self,
        x: f32,
        middle: f32,
        direction: f32,
        start: f32,
        length: f32,
        colour: Color,
    ) {
        let gradient = self.textures.gradient;
        let centre = self
            .place
            .point([x, middle + direction * (start + length / 2.0)]);
        let size = self.place.size([BAR_WIDTH, length]);
        // Along the bar is left to right on screen: the early bar fades towards the left.
        let uv = if direction < 0.0 {
            UvRect {
                u0: gradient.uv.u1,
                u1: gradient.uv.u0,
                ..gradient.uv
            }
        } else {
            gradient.uv
        };
        self.sprite(&gradient, uv, centre, size, colour, Blend::Alpha);
    }

    /// A rectangle with fully rounded ends (osu-framework's `Circle`), drawn additively as
    /// the judgement lines are: two half discs and the straight part between them.
    fn capsule(&mut self, centre: [f32; 2], size: [f32; 2], colour: Color) {
        let disc = self.textures.disc;
        let white = self.textures.white;
        let (centre, size) = (self.place.point(centre), self.place.size(size));
        let blend = Blend::Additive;
        let uv = disc.uv;
        // Along the long side: `along` is 0 for x, 1 for y.
        let along = usize::from(size[1] > size[0]);
        let radius = size[1 - along] / 2.0;
        let straight = size[along] - 2.0 * radius;
        let mut offset = [0.0; 2];
        let mut cap = [0.0; 2];
        cap[along] = radius;
        cap[1 - along] = size[1 - along];
        offset[along] = straight / 2.0 + radius / 2.0;
        let (first, second) = if along == 0 {
            let mid = (uv.u0 + uv.u1) / 2.0;
            (UvRect { u1: mid, ..uv }, UvRect { u0: mid, ..uv })
        } else {
            let mid = (uv.v0 + uv.v1) / 2.0;
            (UvRect { v1: mid, ..uv }, UvRect { v0: mid, ..uv })
        };
        self.sprite(
            &disc,
            first,
            [centre[0] - offset[0], centre[1] - offset[1]],
            cap,
            colour,
            blend,
        );
        if straight > 0.0 {
            let mut middle = size;
            middle[along] = straight;
            self.sprite(&white, white.uv, centre, middle, colour, blend);
        }
        self.sprite(
            &disc,
            second,
            [centre[0] + offset[0], centre[1] + offset[1]],
            cap,
            colour,
            blend,
        );
    }

    /// The average arrow: a chevron `CHEVRON_SIZE` across centred on `centre`, pointing at the
    /// bar (Font Awesome's `ChevronRight` in the vertical meter), as two strokes. The
    /// proportions approximate the glyph; they are not lazer's.
    fn chevron(&mut self, centre: [f32; 2]) {
        let white = self.textures.white;
        let half = CHEVRON_SIZE / 2.0;
        let thickness = CHEVRON_SIZE * 0.2 * self.place.scale;
        let tip = self.place.point([centre[0] + half * 0.4, centre[1]]);
        for side in [-1.0, 1.0] {
            let end = self
                .place
                .point([centre[0] - half * 0.4, centre[1] + side * half * 0.75]);
            let (dx, dy) = (tip[0] - end[0], tip[1] - end[1]);
            let length = (dx * dx + dy * dy).sqrt() + thickness / 2.0;
            let mid = [(tip[0] + end[0]) / 2.0, (tip[1] + end[1]) / 2.0];
            let mut sprite = Sprite::new(white.texture, white.uv, mid, [length, thickness]);
            sprite.origin = [length / 2.0, thickness / 2.0];
            sprite.rotation = dy.atan2(dx);
            self.batch.push(&sprite);
        }
    }
}

/// Side of the disc image: large enough for the centre marker's edge to stay smooth.
const DISC_SIDE: u32 = 64;
/// Length of the gradient image.
const GRADIENT_LENGTH: u32 = 64;

/// A white disc on transparent, anti-aliased over a pixel, for [`MeterTextures::disc`].
pub fn disc_image() -> Image {
    let side = DISC_SIDE as usize;
    let radius = DISC_SIDE as f32 / 2.0;
    let mut pixels = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            let (dx, dy) = (x as f32 + 0.5 - radius, y as f32 + 0.5 - radius);
            let coverage = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            pixels.extend_from_slice(&[
                u8::MAX,
                u8::MAX,
                u8::MAX,
                (coverage * 255.0).round() as u8,
            ]);
        }
    }
    Image {
        size: Extent::new(DISC_SIDE, DISC_SIDE),
        pixels,
    }
}

/// White fading from opaque on the left to transparent on the right, for
/// [`MeterTextures::gradient`]: its first and last texels are the ends, so that the
/// interpolation between texel centres spans the image.
pub fn gradient_image() -> Image {
    let length = GRADIENT_LENGTH as usize;
    let mut pixels = Vec::with_capacity(length * 4);
    for x in 0..length {
        let alpha = 1.0 - x as f32 / (length - 1) as f32;
        pixels.extend_from_slice(&[u8::MAX, u8::MAX, u8::MAX, (alpha * 255.0).round() as u8]);
    }
    Image {
        size: Extent::new(GRADIENT_LENGTH, 1),
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_scale_by_the_meh_window_and_clamp() {
        assert_eq!(relative_position(0.0, 100.0), 0.5);
        assert_eq!(relative_position(-100.0, 100.0), 0.0);
        assert_eq!(relative_position(50.0, 100.0), 0.75);
        assert_eq!(relative_position(-400.0, 100.0), 0.0);
        assert_eq!(relative_position(400.0, 100.0), 1.0);
    }

    #[test]
    fn darken_divides() {
        let c = darken(Color::rgb(0.65, 1.3, 0.0), 0.3);
        assert!((c.r - 0.5).abs() < 1e-6 && (c.g - 1.0).abs() < 1e-6);
    }
}
