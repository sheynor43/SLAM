//! Dragging the song list with the primary button, and the fling after it: lazer's
//! `ScrollContainer` drag, with a hard edge instead of its elastic overscroll (ADR-0093).
//!
//! Distances are in theme reference pixels (lazer's carousel space at 768 px of height);
//! the screen turns them into rows. Time is the update clock in nanoseconds: drag events
//! carry none, so their deltas are gathered and averaged on the next tick, as lazer
//! averages its per-frame drag events.

// Ported from osu-framework 2026.921.1: osu.Framework/Input/MouseButtonEventManager.cs (ClickDragDistance)
/// How far, in physical pixels, the pointer moves from the press before it drags.
pub const CLICK_DRAG_DISTANCE: f32 = 10.0;

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Containers/ScrollContainer.cs
/// The decay of the fling per millisecond (`DistanceDecayDrag`).
const DISTANCE_DECAY_DRAG: f64 = 0.0035;
/// A release slower than this, in px/ms, does not fling (`velocity_cutoff`).
const VELOCITY_CUTOFF: f64 = 0.1;
/// The weight of the drag average decays by this per millisecond.
const AVERAGE_DECAY: f64 = 0.95;
/// The fling stops this close to its end, in reference pixels (`Precision.FLOAT_EPSILON`).
const SETTLE_DISTANCE: f64 = 1e-3;

/// What a press does on release.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Release {
    /// A click at the press point.
    Click { x: f32, y: f32 },
    /// The press dragged: no click.
    Dragged,
}

#[derive(Clone, Copy, Debug)]
struct Press {
    x: f32,
    y: f32,
    last_y: f32,
    dragging: bool,
    /// The pointer went past the click distance along the list: the release is no click.
    blocks_click: bool,
    /// Reference pixels dragged since the last tick, signed as the scroll.
    pending: f64,
    last_drag_ns: u64,
    average_time: f64,
    average_delta: f64,
}

/// The drag and fling state of a scrolled list.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListDrag {
    press: Option<Press>,
    /// A drag ended and flings on the next tick.
    released: Option<Press>,
    /// Where the fling goes and how fast it gets there, in reference pixels.
    fling: Option<(f64, f64)>,
    now_ns: u64,
}

impl ListDrag {
    /// The primary button went down at `(x, y)` (physical pixels): a running fling stops
    /// where it is, as lazer's `Target = Current`.
    pub fn press(&mut self, x: f32, y: f32) {
        self.fling = None;
        self.released = None;
        self.press = Some(Press {
            x,
            y,
            last_y: y,
            dragging: false,
            blocks_click: false,
            pending: 0.0,
            last_drag_ns: self.now_ns,
            average_time: 0.0,
            average_delta: 0.0,
        });
    }

    /// The pointer moved to `(x, y)`; returns the reference pixels to scroll by (positive
    /// towards the list's end). `scale` is physical pixels per reference pixel.
    pub fn moved(&mut self, x: f32, y: f32, scale: f32) -> f32 {
        let Some(press) = &mut self.press else {
            return 0.0;
        };
        if !press.dragging {
            if (x - press.x).hypot(y - press.y) <= CLICK_DRAG_DISTANCE {
                return 0.0;
            }
            // The drag starts here: what moved up to the threshold does not scroll.
            press.dragging = true;
            press.last_y = y;
            press.last_drag_ns = self.now_ns;
        }
        press.blocks_click |= (y - press.y).abs() > CLICK_DRAG_DISTANCE;
        let offset = -(y - press.last_y) / scale;
        press.last_y = y;
        press.pending += f64::from(offset);
        offset
    }

    /// The primary button went up: a click, a drag (which flings on the next tick), or
    /// nothing when there was no press.
    pub fn release(&mut self) -> Option<Release> {
        let press = self.press.take()?;
        if press.dragging {
            self.released = Some(press);
        }
        Some(if press.blocks_click {
            Release::Dragged
        } else {
            Release::Click {
                x: press.x,
                y: press.y,
            }
        })
    }

    /// Forgets the press and stops the fling (the list was moved another way).
    pub fn cancel(&mut self) {
        *self = ListDrag {
            now_ns: self.now_ns,
            ..ListDrag::default()
        };
    }

    /// Whether the button is held over the list.
    pub fn is_pressed(&self) -> bool {
        self.press.is_some()
    }

    /// Stops the fling only, keeping a press.
    pub fn stop(&mut self) {
        self.fling = None;
    }

    /// Advances to `now_ns`. `current` is the scroll in reference pixels, `max` its end;
    /// returns where the fling moved it, `None` when it does not move.
    pub fn tick(&mut self, now_ns: u64, current: f32, max: f32) -> Option<f32> {
        let elapsed = now_ns.saturating_sub(self.now_ns) as f64 / 1e6;
        self.now_ns = now_ns;
        if let Some(press) = &mut self.press {
            average(press, now_ns);
        }
        if let Some(mut press) = self.released.take() {
            average(&mut press, now_ns);
            self.fling = fling(&press, now_ns, f64::from(current), f64::from(max));
        }
        let (target, decay) = self.fling?;
        // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Containers/ScrollContainer.cs (updatePosition)
        let current = f64::from(current);
        let mut next = target + (current - target) * (-decay * elapsed).exp();
        if (next - target).abs() <= SETTLE_DISTANCE {
            next = target;
            self.fling = None;
        }
        Some(next as f32)
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Containers/ScrollContainer.cs (OnDrag)
/// Folds the deltas gathered since the last tick into the sliding average, as one drag
/// event at `now_ns`.
fn average(press: &mut Press, now_ns: u64) {
    if press.pending == 0.0 || !press.dragging {
        return;
    }
    let time_delta = now_ns.saturating_sub(press.last_drag_ns) as f64 / 1e6;
    let decay = AVERAGE_DECAY.powf(time_delta);
    press.average_time = press.average_time * decay + time_delta;
    press.average_delta = press.average_delta * decay + press.pending;
    press.last_drag_ns = now_ns;
    press.pending = 0.0;
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Containers/ScrollContainer.cs (OnDragEnd)
/// Where a drag released at `now_ns` flings to, and its decay; `None` when too slow.
fn fling(press: &Press, now_ns: u64, current: f64, max: f64) -> Option<(f64, f64)> {
    if press.average_time <= 0.0 {
        return None;
    }
    let velocity = press.average_delta / press.average_time;
    let since = now_ns.saturating_sub(press.last_drag_ns) as f64 / 1e6;
    if (AVERAGE_DECAY.powf(since) * velocity).abs() < VELOCITY_CUTOFF {
        return None;
    }
    let distance = velocity / (1.0 - (-DISTANCE_DECAY_DRAG).exp());
    // The hard edge: the fling ends at the list's ends instead of overshooting them.
    let target = (current + distance).clamp(0.0, max.max(0.0));
    (target != current).then_some((target, DISTANCE_DECAY_DRAG))
}

#[cfg(test)]
mod tests;
