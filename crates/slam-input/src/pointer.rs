//! The pointer as the window thread last saw it, for a thread that draws a cursor of its own
//! (ADR-0124): the freshest position and the mouse buttons held, readable from any thread
//! without locks.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::MouseButton;

/// Shared state behind a [`PointerWatch`]; written by the window thread only.
#[derive(Debug, Default)]
struct Shared {
    /// `x` in the high half and `y` in the low half, as `f32` bits.
    position: AtomicU64,
    /// Whether any position came yet.
    moved: AtomicU32,
    /// Bits of the buttons held, see [`bit`].
    buttons: AtomicU32,
    /// Non-zero while the pointer is outside the window (the OS cursor shows there).
    outside: AtomicU32,
}

/// A read handle on the pointer: the position in window points of the last motion the
/// window got, and whether a mouse button is down. Clones share the state. The default
/// handle belongs to no window: it never moves (for tests).
#[derive(Clone, Debug, Default)]
pub struct PointerWatch {
    shared: Arc<Shared>,
}

fn bit(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 4,
        MouseButton::X1 => 8,
        MouseButton::X2 => 16,
    }
}

impl PointerWatch {
    /// The last position in window points, or `None` before the pointer first moved and
    /// while it is outside the window.
    pub fn position(&self) -> Option<[f32; 2]> {
        if self.shared.moved.load(Ordering::Acquire) == 0
            || self.shared.outside.load(Ordering::Relaxed) != 0
        {
            return None;
        }
        let bits = self.shared.position.load(Ordering::Relaxed);
        Some([
            f32::from_bits((bits >> 32) as u32),
            f32::from_bits(bits as u32),
        ])
    }

    /// Whether any mouse button is held.
    pub fn pressed(&self) -> bool {
        self.shared.buttons.load(Ordering::Relaxed) != 0
    }

    /// Records a position; for the window thread.
    pub fn record_move(&self, x: f32, y: f32) {
        let bits = (u64::from(x.to_bits()) << 32) | u64::from(y.to_bits());
        self.shared.position.store(bits, Ordering::Relaxed);
        self.shared.outside.store(0, Ordering::Relaxed);
        self.shared.moved.store(1, Ordering::Release);
    }

    /// Records the pointer leaving (`true`) or entering (`false`) the window; for the window
    /// thread. The next motion also counts as entering.
    pub fn record_outside(&self, outside: bool) {
        self.shared
            .outside
            .store(u32::from(outside), Ordering::Relaxed);
    }

    /// Releases every button, for when the window loses focus and the releases will not
    /// come; for the window thread.
    pub fn release_all(&self) {
        self.shared.buttons.store(0, Ordering::Relaxed);
    }

    /// Records a button going down or up; for the window thread.
    pub fn record_button(&self, button: MouseButton, down: bool) {
        let bit = bit(button);
        if down {
            self.shared.buttons.fetch_or(bit, Ordering::Relaxed);
        } else {
            self.shared.buttons.fetch_and(!bit, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_position_before_the_first_move() {
        let pointer = PointerWatch::default();
        assert_eq!(pointer.position(), None);
        assert!(!pointer.pressed());
    }

    #[test]
    fn a_clone_sees_the_latest_position() {
        let pointer = PointerWatch::default();
        let reader = pointer.clone();
        pointer.record_move(12.5, -3.0);
        pointer.record_move(100.25, 40.0);
        assert_eq!(reader.position(), Some([100.25, 40.0]));
    }

    #[test]
    fn pressed_while_any_button_is_down() {
        let pointer = PointerWatch::default();
        pointer.record_button(MouseButton::Left, true);
        pointer.record_button(MouseButton::Right, true);
        assert!(pointer.pressed());
        pointer.record_button(MouseButton::Left, false);
        assert!(pointer.pressed());
        pointer.record_button(MouseButton::Right, false);
        assert!(!pointer.pressed());
    }

    #[test]
    fn outside_hides_the_position_until_enter_or_motion() {
        let pointer = PointerWatch::default();
        pointer.record_move(5.0, 6.0);
        pointer.record_outside(true);
        assert_eq!(pointer.position(), None);
        pointer.record_outside(false);
        assert_eq!(pointer.position(), Some([5.0, 6.0]));
        pointer.record_outside(true);
        pointer.record_move(7.0, 8.0);
        assert_eq!(pointer.position(), Some([7.0, 8.0]));
    }

    #[test]
    fn release_all_clears_held_buttons() {
        let pointer = PointerWatch::default();
        pointer.record_button(MouseButton::Left, true);
        pointer.record_button(MouseButton::X2, true);
        pointer.release_all();
        assert!(!pointer.pressed());
    }
}
