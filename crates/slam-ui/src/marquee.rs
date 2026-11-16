//! Marquee for texts that do not fit their room (ADR-0148): the text rides to its end
//! and back with a pause at each end, its cut edges fade out, and it can be dragged or
//! moved with the horizontal wheel (Shift + wheel).
//!
//! The width of a text is known where it is laid out, on the draw side, so a [`Marquee`]
//! lives in the view. The update side only reports the pointer, the wheel and drags in
//! a [`TextScroll`] in its snapshot; a [`ScrollTracker`] in the view turns those into
//! moves of the marquee under the pointer. Positions are functions of time, not sums
//! over frames, so they do not depend on the frame rate.

use std::sync::atomic::{AtomicU32, Ordering};

use slam_render::Color;
use slam_render::sprite::SpriteBatch;
use slam_text::{Clip, GlyphCache, push_layout_clipped};

use crate::menu::Rect;
use crate::menu::view::Line;
use crate::theme::Theme;

/// Below this many pixels of overflow a text counts as fitting: no ride for a rounding
/// error.
const MIN_OVERFLOW: f32 = 0.5;

/// The marquee tokens at one scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarqueeParams {
    /// Pixels per second.
    pub speed: f32,
    pub pause_ns: u64,
    /// Width of a faded edge, pixels.
    pub fade: f32,
    /// Pixels per wheel notch.
    pub wheel_step: f32,
}

impl MarqueeParams {
    /// `scale` is pixels per reference unit (see [`Theme::scale`]).
    pub fn new(theme: &Theme, scale: f32) -> MarqueeParams {
        MarqueeParams {
            speed: (theme.marquee_speed * scale).max(1.0),
            pause_ns: u64::from(theme.marquee_pause_ms) * 1_000_000,
            fade: theme.marquee_fade * scale,
            wheel_step: theme.marquee_wheel_step * scale,
        }
    }
}

/// What the text is doing since `since_ns`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Phase {
    /// Standing at `at` until the pause ends; then it rides towards the end it faces.
    #[default]
    Hold,
    /// Riding from `at` to `to`.
    Ride { to: f32 },
}

/// One text's ride. Offsets run from 0 (start shown) to the overflow (end shown); the
/// text is drawn that far left of its place.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Marquee {
    overflow: f32,
    phase: Phase,
    since_ns: u64,
    at: f32,
    /// Heading for the end (towards `overflow`) rather than the start.
    forward: bool,
    /// Where the text was last drawn: what the pointer is tested against.
    window: Rect,
}

impl Marquee {
    /// How far the text overflows its room: 0 when it fits.
    pub fn overflow(&self) -> f32 {
        self.overflow
    }

    /// Whether the text rides.
    pub fn rides(&self) -> bool {
        self.overflow > 0.0
    }

    /// Where the text was last drawn.
    pub fn window(&self) -> Rect {
        self.window
    }

    /// Records where the text is drawn (what [`push_marquee`] does).
    pub fn place(&mut self, window: Rect) {
        self.window = window;
    }

    /// Sets how far the text overflows its room. A change restarts the ride from the
    /// start (the text or its room changed); the same overflow keeps it.
    pub fn set_overflow(&mut self, overflow: f32, now_ns: u64) {
        let overflow = if overflow > MIN_OVERFLOW {
            overflow
        } else {
            0.0
        };
        if (overflow - self.overflow).abs() > MIN_OVERFLOW / 2.0 {
            self.overflow = overflow;
            self.restart(now_ns);
        }
    }

    /// Back to the start, pausing from `now_ns`.
    pub fn restart(&mut self, now_ns: u64) {
        self.phase = Phase::Hold;
        self.since_ns = now_ns;
        self.at = 0.0;
        self.forward = true;
    }

    /// Puts the text at `offset` (clamped) by hand: it stands there for a pause from
    /// `now_ns`, then rides on towards the end it was heading for.
    pub fn set_by_hand(&mut self, offset: f32, now_ns: u64) {
        if !self.rides() {
            return;
        }
        if let Phase::Ride { to } = self.phase {
            self.forward = to > self.at;
        }
        self.phase = Phase::Hold;
        self.since_ns = now_ns;
        self.at = offset.clamp(0.0, self.overflow);
    }

    /// Moves the text `by` pixels further towards its end (negative: its start), as by
    /// hand.
    pub fn nudge(&mut self, by: f32, now_ns: u64, params: &MarqueeParams) {
        let at = self.offset(now_ns, params);
        self.set_by_hand(at + by, now_ns);
    }

    /// The offset at `now_ns`, moving the ride on to the phase `now_ns` falls in. Times
    /// before the current phase read as its start.
    pub fn offset(&mut self, now_ns: u64, params: &MarqueeParams) -> f32 {
        if !self.rides() {
            return 0.0;
        }
        // Each pass ends a phase (a hold or a ride): eight passes cover two whole cycles
        // between two calls, which no frame gap reaches but a stall or a view not drawn
        // for a while; those restart.
        for _ in 0..8 {
            let elapsed = now_ns.saturating_sub(self.since_ns);
            match self.phase {
                Phase::Hold => {
                    if elapsed < params.pause_ns {
                        return self.at;
                    }
                    let mut to = if self.forward { self.overflow } else { 0.0 };
                    if (to - self.at).abs() < MIN_OVERFLOW {
                        // Already there (put by hand at the end it headed for): turn.
                        self.forward = !self.forward;
                        to = if self.forward { self.overflow } else { 0.0 };
                    }
                    self.since_ns += params.pause_ns;
                    self.phase = Phase::Ride { to };
                }
                Phase::Ride { to } => {
                    let duration = ride_ns((to - self.at).abs(), params.speed);
                    if elapsed < duration {
                        let t = elapsed as f64 / duration as f64;
                        let eased = in_out_sine(t) as f32;
                        return self.at + (to - self.at) * eased;
                    }
                    self.since_ns += duration;
                    self.at = to;
                    self.forward = to < self.overflow / 2.0;
                    self.phase = Phase::Hold;
                }
            }
        }
        // Many cycles behind (the view was not drawn for a while): start over.
        self.restart(now_ns);
        self.at
    }
}

/// How long riding `distance` pixels takes at `speed` pixels per second.
fn ride_ns(distance: f32, speed: f32) -> u64 {
    ((f64::from(distance) / f64::from(speed)) * 1e9).max(1.0) as u64
}

/// Eases in and out along a half cosine: no jolt where the text starts and stops.
fn in_out_sine(t: f64) -> f64 {
    0.5 - 0.5 * (std::f64::consts::PI * t.clamp(0.0, 1.0)).cos()
}

/// Draws `line` inside `rect`, vertically centred, `pad` from the left edge; when it is
/// wider than the room between the pads it rides as `marquee` says at `now_ns`, cut to
/// that room with faded edges. Keeps `marquee` up to date with the text's overflow and
/// where it was drawn. Allocation-free once the batch has room.
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_marquee(
    batch: &mut SpriteBatch,
    line: &Line,
    rect: &Rect,
    pad: f32,
    colour: Color,
    glyphs: &mut GlyphCache,
    marquee: &mut Marquee,
    params: &MarqueeParams,
    now_ns: u64,
) {
    let left = rect.x + pad;
    let room = (rect.width - 2.0 * pad).max(0.0);
    marquee.set_overflow(line.layout.width() - room, now_ns);
    marquee.window = *rect;
    let offset = marquee.offset(now_ns, params);
    let overflow = marquee.overflow();
    // A fade only where text is cut, growing in as the text leaves the edge.
    let fade = |hidden: f32| params.fade * (hidden / params.fade.max(1e-3)).clamp(0.0, 1.0);
    let clip = Clip {
        left,
        right: left + room,
        fade_left: fade(offset),
        fade_right: fade(overflow - offset),
    };
    let position = [
        left - offset,
        rect.y + (rect.height - line.layout.height()) / 2.0,
    ];
    if overflow > 0.0 {
        push_layout_clipped(batch, &line.layout, position, colour, &clip, glyphs);
    } else {
        slam_text::push_layout(batch, &line.layout, position, colour, glyphs);
    }
}

/// A drag of a text the update side saw, in the snapshot: a press that moved sideways
/// past the drag threshold before anything else took it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextDrag {
    /// Tells drags apart: a new press, a new id.
    pub id: u32,
    /// Where the press was, target pixels.
    pub press: [f32; 2],
    /// How far the pointer moved sideways since the press, pixels.
    pub dx: f32,
}

/// Tells apart the [`TextScroll`]s made: a screen opened again starts its sums over, and
/// a view that outlives it must not read the new sums against the old ones.
static EPOCHS: AtomicU32 = AtomicU32::new(0);

/// What the update side reports for marquee texts, in its snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextScroll {
    /// The pointer, target pixels.
    pub pointer: [f32; 2],
    /// Wheel notches that move texts sideways (horizontal wheel, Shift + wheel), summed
    /// since the screen opened; positive moves a text towards its end.
    pub wheel: f32,
    /// The drag going on, if any.
    pub drag: Option<TextDrag>,
    /// The id of the latest drag.
    last_id: u32,
    /// Which screen's sums these are.
    epoch: u32,
}

impl Default for TextScroll {
    /// Fresh sums, told apart from every other `TextScroll` made.
    fn default() -> TextScroll {
        TextScroll {
            pointer: [0.0; 2],
            wheel: 0.0,
            drag: None,
            last_id: 0,
            epoch: EPOCHS.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl TextScroll {
    /// Adds sideways wheel notches (positive: towards a text's end).
    pub fn wheel(&mut self, notches: f32) {
        self.wheel += notches;
    }

    /// The pointer moved to `pointer`; a drag follows it.
    pub fn pointer(&mut self, pointer: [f32; 2]) {
        self.pointer = pointer;
        if let Some(drag) = &mut self.drag {
            drag.dx = pointer[0] - drag.press[0];
        }
    }

    /// A drag starts from `press` (the pointer is at `self.pointer`).
    pub fn start_drag(&mut self, press: [f32; 2]) {
        self.last_id = self.last_id.wrapping_add(1);
        self.drag = Some(TextDrag {
            id: self.last_id,
            press,
            dx: self.pointer[0] - press[0],
        });
    }

    /// The drag ends (the button went up).
    pub fn end_drag(&mut self) {
        self.drag = None;
    }
}

/// The view's side of [`TextScroll`]: applies new wheel notches and drags to the
/// marquee under the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollTracker {
    /// The epoch of the sums seen: other sums start the tracker over.
    epoch: Option<u32>,
    wheel_seen: Option<f32>,
    /// The drag being applied: its id, the index of its marquee and the offset it
    /// started from.
    drag: Option<(u32, usize, f32)>,
}

impl ScrollTracker {
    /// Applies what `input` holds that this tracker has not applied yet to `marquees`
    /// (the view's marquees, in the same order every call). Allocation-free.
    pub fn apply<'a>(
        &mut self,
        input: &TextScroll,
        now_ns: u64,
        params: &MarqueeParams,
        marquees: impl IntoIterator<Item = &'a mut Marquee>,
    ) {
        if self.epoch != Some(input.epoch) {
            *self = ScrollTracker {
                epoch: Some(input.epoch),
                ..ScrollTracker::default()
            };
        }
        let wheel = match self.wheel_seen {
            // The first look only learns where the sum is.
            None => 0.0,
            Some(seen) => input.wheel - seen,
        };
        self.wheel_seen = Some(input.wheel);
        let started = match (input.drag, self.drag) {
            (Some(d), Some((id, ..))) if d.id == id => None,
            (Some(d), _) => Some(d),
            (None, _) => {
                self.drag = None;
                None
            }
        };
        if started.is_some() {
            self.drag = None;
        }
        if wheel == 0.0 && input.drag.is_none() {
            return;
        }
        let under = |m: &Marquee, [x, y]: [f32; 2]| m.rides() && m.window.contains(x, y);
        for (i, m) in marquees.into_iter().enumerate() {
            if wheel != 0.0 && under(m, input.pointer) {
                m.nudge(wheel * params.wheel_step, now_ns, params);
            }
            if let Some(d) = started
                && self.drag.is_none()
                && under(m, d.press)
            {
                let base = m.offset(now_ns, params);
                self.drag = Some((d.id, i, base));
            }
            if let (Some(d), Some((_, at, base))) = (input.drag, self.drag)
                && at == i
            {
                m.set_by_hand(base - d.dx, now_ns);
            }
        }
    }
}

#[cfg(test)]
#[path = "marquee_tests.rs"]
mod tests;
