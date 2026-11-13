//! The curtain that covers the window while the interface switches language (ADR-0091):
//! vertical slats slide down from the top, the centre ones first, hold while the screens are
//! swapped and their glyphs made, then slide on down and out the same way.

use slam_render::atlas::AtlasRegion;
use slam_render::shape::ShapeBatch;
use slam_render::sprite::Sprite;

use crate::anim::{ease_out_cubic, progress};
use crate::theme::Theme;

/// How many slats cover the window.
pub const SLATS: usize = 10;
/// The most sprites [`Curtain::push`] writes: a body and an edge per slat.
pub const MAX_SPRITES: usize = 2 * SLATS;
/// How long one slat takes to slide.
pub const SLAT_NS: u64 = 180_000_000;
/// How much later the outermost slats start than the centre ones: closing or opening takes
/// `SLAT_NS + STAGGER_NS` (250 ms, ARCHITECTURE §12).
pub const STAGGER_NS: u64 = 70_000_000;
/// How long one phase, closing or opening, takes.
pub const PHASE_NS: u64 = SLAT_NS + STAGGER_NS;
/// The longest the curtain stays closed waiting: it opens then whatever is ready.
pub const HOLD_LIMIT_NS: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Open,
    Closing { from_ns: u64 },
    Closed { since_ns: u64 },
    Opening { from_ns: u64 },
}

/// Where the curtain is. Time-driven; does not allocate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Curtain {
    phase: Phase,
    /// Asked to close while opening: closes again once open.
    close_again: bool,
}

impl Default for Curtain {
    fn default() -> Curtain {
        Curtain::new()
    }
}

impl Curtain {
    /// An open curtain.
    pub const fn new() -> Curtain {
        Curtain {
            phase: Phase::Open,
            close_again: false,
        }
    }

    /// Starts closing at `now_ns`. Closing or closed already, nothing changes; while opening,
    /// it closes again once open.
    pub fn close(&mut self, now_ns: u64) {
        match self.phase {
            Phase::Open => self.phase = Phase::Closing { from_ns: now_ns },
            Phase::Opening { .. } => self.close_again = true,
            Phase::Closing { .. } | Phase::Closed { .. } => {}
        }
    }

    /// Moves on to `now_ns`. A closed curtain opens once `ready` (what it hid is ready to
    /// show) or after [`HOLD_LIMIT_NS`].
    pub fn advance(&mut self, now_ns: u64, ready: bool) {
        match self.phase {
            Phase::Open => {}
            Phase::Closing { from_ns } => {
                if now_ns.saturating_sub(from_ns) >= PHASE_NS {
                    self.phase = Phase::Closed { since_ns: now_ns };
                }
            }
            Phase::Closed { since_ns } => {
                if ready || now_ns.saturating_sub(since_ns) >= HOLD_LIMIT_NS {
                    self.phase = Phase::Opening { from_ns: now_ns };
                }
            }
            Phase::Opening { from_ns } => {
                if now_ns.saturating_sub(from_ns) >= PHASE_NS {
                    self.phase = Phase::Open;
                    if std::mem::take(&mut self.close_again) {
                        self.phase = Phase::Closing { from_ns: now_ns };
                    }
                }
            }
        }
    }

    /// Whether the window is fully covered: what is under it may change unseen.
    pub fn is_closed(&self) -> bool {
        matches!(self.phase, Phase::Closed { .. })
    }

    /// Whether any of it shows.
    pub fn is_shown(&self) -> bool {
        self.phase != Phase::Open
    }

    /// Writes the slats at `now_ns` on a `width` × `height` window into `shapes`, tinting
    /// `white` (an opaque white image). Allocation-free once the batch has room.
    pub fn push(
        &self,
        shapes: &mut ShapeBatch,
        theme: &Theme,
        width: f32,
        height: f32,
        now_ns: u64,
        white: AtlasRegion,
    ) {
        let edge = theme.curtain_edge * theme.scale(height);
        let mut rect = |x: f32, y: f32, w: f32, h: f32, colour| {
            if w > 0.0 && h > 0.0 {
                let mut sprite = Sprite::new(white.texture, white.uv, [x, y], [w, h]);
                sprite.colour = colour;
                shapes.push(&sprite);
            }
        };
        for i in 0..SLATS {
            let x = width * i as f32 / SLATS as f32;
            let w = width * (i + 1) as f32 / SLATS as f32 - x;
            match self.phase {
                Phase::Open => return,
                Phase::Closed { .. } => rect(x, 0.0, w, height, theme.curtain_fill),
                Phase::Closing { from_ns } => {
                    // Down from the top: the leading edge at the bottom.
                    let bottom = height * slat(i, from_ns, now_ns);
                    rect(x, 0.0, w, bottom, theme.curtain_fill);
                    rect(
                        x,
                        bottom - edge,
                        w,
                        edge.min(bottom),
                        theme.widget_accent_fill,
                    );
                }
                Phase::Opening { from_ns } => {
                    // On down and out: the leading edge at the top.
                    let top = height * slat(i, from_ns, now_ns);
                    rect(x, top, w, height - top, theme.curtain_fill);
                    rect(x, top, w, edge.min(height - top), theme.widget_accent_fill);
                }
            }
        }
    }
}

/// How far slat `i` of a phase that started at `from_ns` has slid at `now_ns`, 0 to 1,
/// eased: the centre slats start first, the outer ones up to [`STAGGER_NS`] later.
fn slat(i: usize, from_ns: u64, now_ns: u64) -> f32 {
    let half = (SLATS - 1) as f32 / 2.0;
    let off_centre = (i as f32 - half).abs() / half;
    let delay = (STAGGER_NS as f32 * off_centre) as u64;
    ease_out_cubic(progress(from_ns + delay, now_ns, SLAT_NS))
}

#[cfg(test)]
mod tests;
