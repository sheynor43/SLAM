//! Relative ("high precision") mouse mode and cursor sensitivity (ADR-0069).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Lowest cursor sensitivity, as `MouseHandler.Sensitivity` in osu-framework.
pub const MIN_SENSITIVITY: f64 = 0.1;
/// Highest cursor sensitivity.
pub const MAX_SENSITIVITY: f64 = 10.0;

/// Turns relative mouse mode on or off and sets the cursor sensitivity, from any thread,
/// without locks. Like [`crate::CursorVisibility`], the window applies the mode before
/// waiting for its next event; the sensitivity is read on every mouse motion. Off with
/// sensitivity 1 at start. The default handle belongs to no window (for tests).
#[derive(Clone)]
pub struct MouseSettings {
    shared: Arc<Shared>,
}

struct Shared {
    relative: AtomicBool,
    /// `f64` bits.
    sensitivity: AtomicU64,
}

impl Default for MouseSettings {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared {
                relative: AtomicBool::new(false),
                sensitivity: AtomicU64::new(1.0f64.to_bits()),
            }),
        }
    }
}

impl MouseSettings {
    /// Asks for relative mode where feasible ("high precision mouse").
    pub fn set_relative(&self, relative: bool) {
        self.shared.relative.store(relative, Ordering::Relaxed);
    }

    /// The last wish.
    pub fn is_relative(&self) -> bool {
        self.shared.relative.load(Ordering::Relaxed)
    }

    /// Sets the multiplier of relative motion, clamped to
    /// [`MIN_SENSITIVITY`]..=[`MAX_SENSITIVITY`]; NaN is ignored. Absolute motion is
    /// never scaled.
    pub fn set_sensitivity(&self, sensitivity: f64) {
        if sensitivity.is_nan() {
            return;
        }
        let value = sensitivity.clamp(MIN_SENSITIVITY, MAX_SENSITIVITY);
        self.shared
            .sensitivity
            .store(value.to_bits(), Ordering::Relaxed);
    }

    /// The current multiplier.
    pub fn sensitivity(&self) -> f64 {
        f64::from_bits(self.shared.sensitivity.load(Ordering::Relaxed))
    }
}

/// What the window must do after a [`RelativeMouse`] call, in this order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Change {
    /// Turn SDL relative mode on or off.
    pub relative: Option<bool>,
    /// Then move the OS cursor here, in window points.
    pub warp: Option<[f32; 2]>,
}

impl Change {
    fn merge(&mut self, other: Change) {
        if other.relative.is_some() {
            self.relative = other.relative;
        }
        if other.warp.is_some() {
            self.warp = other.warp;
        }
    }
}

/// Decides when relative mode is on and turns mouse motion into window positions, as
/// osu-framework's `MouseHandler`. The cursor is confined only while the window is
/// fullscreen (osu-framework's default `ConfineMouseMode.Fullscreen`), where the OS
/// cursor cannot leave it anyway.
// Ported from osu-framework 2026.921.1: osu.Framework/Input/Handlers/Mouse/MouseHandler.cs
#[derive(Debug)]
pub(crate) struct RelativeMouse {
    /// The setting (`UseRelativeMode`).
    enabled: bool,
    focused: bool,
    cursor_in_window: bool,
    cursor_hidden: bool,
    /// Positions stay inside the window and the cursor is never handed over.
    confined: bool,
    /// Relative motion needs an absolute position to start from.
    absolute_received: bool,
    /// Whether relative mode is on (as last asked of SDL).
    active: bool,
    /// Re-check the conditions on the next motion (the cursor has just entered).
    pending_update: bool,
    /// SDL refused relative mode and the retry on the next motion was spent; reset
    /// when a condition changes, so a lasting refusal costs no call per motion.
    retry_spent: bool,
    /// The position last reported.
    position: [f32; 2],
    /// The position to hand to the OS cursor when relative mode turns off.
    last_position: Option<[f32; 2]>,
    previous_outside: bool,
    /// Window size in points.
    size: [f32; 2],
}

impl RelativeMouse {
    /// The state when the window starts. The conditions are checked on the first motion,
    /// since SDL may not report the cursor entering a window it was created under.
    pub fn new(focused: bool, cursor_in_window: bool, size: [f32; 2]) -> Self {
        Self {
            enabled: false,
            focused,
            cursor_in_window,
            cursor_hidden: false,
            confined: false,
            absolute_received: false,
            active: false,
            pending_update: true,
            retry_spent: false,
            position: [0.0; 2],
            last_position: None,
            previous_outside: true,
            size,
        }
    }

    #[cfg(test)]
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn set_enabled(&mut self, enabled: bool) -> Change {
        if self.enabled == enabled {
            return Change::default();
        }
        self.enabled = enabled;
        self.changed()
    }

    pub fn set_focused(&mut self, focused: bool) -> Change {
        if self.focused == focused {
            return Change::default();
        }
        self.focused = focused;
        self.changed()
    }

    pub fn set_cursor_hidden(&mut self, hidden: bool) -> Change {
        if self.cursor_hidden == hidden {
            return Change::default();
        }
        self.cursor_hidden = hidden;
        self.changed()
    }

    /// The window entered or left fullscreen, which confines the cursor.
    pub fn set_confined(&mut self, confined: bool) -> Change {
        if self.confined == confined {
            return Change::default();
        }
        self.confined = confined;
        self.changed()
    }

    /// The cursor entered or left the window.
    pub fn set_cursor_in_window(&mut self, inside: bool) -> Change {
        self.cursor_in_window = inside;
        self.retry_spent = false;
        if inside {
            // Don't switch the moment the cursor enters: wait for its next motion.
            self.pending_update = true;
            Change::default()
        } else {
            self.update()
        }
    }

    pub fn set_size(&mut self, width: f32, height: f32) {
        self.size = [width, height];
    }

    /// SDL turning relative mode on failed: the next motion tries again, once per change
    /// of conditions.
    pub fn rejected(&mut self) {
        self.active = false;
        self.pending_update |= !self.retry_spent;
        self.retry_spent = true;
    }

    /// One SDL motion event: the OS cursor at `absolute`, moved by `delta` (both in
    /// window points). Returns the position to report and what the window must do.
    pub fn motion(
        &mut self,
        absolute: [f32; 2],
        delta: [f32; 2],
        sensitivity: f64,
    ) -> ([f32; 2], Change) {
        if self.active {
            let scale = sensitivity as f32;
            self.position = [
                self.position[0] + delta[0] * scale,
                self.position[1] + delta[1] * scale,
            ];
        } else {
            self.absolute_received = true;
            self.position = absolute;
        }
        if self.confined {
            // As osu-framework's `UserInputManager`: inside, one point short of the edge.
            self.position = [
                self.position[0].clamp(0.0, (self.size[0] - 1.0).max(0.0)),
                self.position[1].clamp(0.0, (self.size[1] - 1.0).max(0.0)),
            ];
        }
        let position = self.position;
        let mut change = Change::default();
        if self.pending_update {
            change.merge(self.update());
            self.pending_update = false;
        }
        let [x, y] = position;
        let outside = x < 0.0 || y < 0.0 || x >= self.size[0] || y >= self.size[1];
        if self.active {
            change.merge(self.update());
            self.last_position = Some(position);
            if !self.confined && outside && !self.previous_outside {
                // Let the OS take the cursor until it comes back.
                self.active = false;
                change.relative = Some(false);
                change.warp = self.last_position.take();
            }
        }
        self.previous_outside = outside;
        (position, change)
    }

    /// Re-checks the conditions after one of them changed.
    fn changed(&mut self) -> Change {
        self.retry_spent = false;
        self.update()
    }

    fn update(&mut self) -> Change {
        let wanted = self.enabled
            && self.absolute_received
            && self.focused
            && (self.cursor_in_window || self.confined)
            && self.cursor_hidden;
        let mut change = Change::default();
        if wanted != self.active {
            self.active = wanted;
            change.relative = Some(wanted);
        }
        if !wanted && let Some(position) = self.last_position.take() {
            change.warp = Some(position);
        }
        change
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: [f32; 2] = [1280.0, 720.0];

    /// A window with the cursor over it, OS cursor hidden, high precision on, and one
    /// absolute motion to `start`, which turns relative mode on.
    fn relative_at(start: [f32; 2]) -> RelativeMouse {
        let mut mouse = RelativeMouse::new(true, true, SIZE);
        mouse.set_cursor_hidden(true);
        mouse.set_enabled(true);
        let (_, change) = mouse.motion(start, [0.0, 0.0], 1.0);
        assert_eq!(change.relative, Some(true));
        assert!(mouse.is_active());
        mouse
    }

    #[test]
    fn absolute_motion_reports_the_os_position_unscaled() {
        let mut mouse = RelativeMouse::new(true, true, SIZE);
        let (position, change) = mouse.motion([100.0, 200.0], [5.0, 5.0], 2.0);
        assert_eq!(position, [100.0, 200.0]);
        assert_eq!(change, Change::default());
        assert!(!mouse.is_active());
    }

    #[test]
    fn sensitivity_one_adds_the_delta() {
        let mut mouse = relative_at([100.0, 100.0]);
        let (position, change) = mouse.motion([0.0, 0.0], [10.0, -4.0], 1.0);
        assert_eq!(position, [110.0, 96.0]);
        assert_eq!(change, Change::default());
    }

    #[test]
    fn sensitivity_two_doubles_the_path() {
        let mut mouse = relative_at([100.0, 100.0]);
        let (position, _) = mouse.motion([0.0, 0.0], [10.0, -4.0], 2.0);
        assert_eq!(position, [120.0, 92.0]);
    }

    #[test]
    fn delta_is_scaled_in_f32() {
        let mut mouse = relative_at([0.5, 0.5]);
        let (position, _) = mouse.motion([0.0, 0.0], [0.3, 0.7], 1.37);
        assert_eq!(position, [0.5 + 0.3 * 1.37f32, 0.5 + 0.7 * 1.37f32]);
    }

    #[test]
    fn needs_every_condition() {
        // Setting off.
        let mut mouse = RelativeMouse::new(true, true, SIZE);
        mouse.set_cursor_hidden(true);
        mouse.motion([10.0, 10.0], [0.0, 0.0], 1.0);
        assert!(!mouse.is_active());
        // OS cursor shown.
        let mut mouse = RelativeMouse::new(true, true, SIZE);
        mouse.set_enabled(true);
        mouse.motion([10.0, 10.0], [0.0, 0.0], 1.0);
        assert!(!mouse.is_active());
        // Not focused.
        let mut mouse = RelativeMouse::new(false, true, SIZE);
        mouse.set_enabled(true);
        mouse.set_cursor_hidden(true);
        mouse.motion([10.0, 10.0], [0.0, 0.0], 1.0);
        assert!(!mouse.is_active());
        // Cursor outside the window.
        let mut mouse = RelativeMouse::new(true, false, SIZE);
        mouse.set_enabled(true);
        mouse.set_cursor_hidden(true);
        mouse.motion([10.0, 10.0], [0.0, 0.0], 1.0);
        assert!(!mouse.is_active());
    }

    #[test]
    fn waits_for_an_absolute_position() {
        let mut mouse = RelativeMouse::new(true, true, SIZE);
        mouse.set_cursor_hidden(true);
        assert_eq!(mouse.set_enabled(true), Change::default());
        assert!(!mouse.is_active());
    }

    #[test]
    fn enabling_after_motion_switches_at_once() {
        let mut mouse = RelativeMouse::new(true, true, SIZE);
        mouse.set_cursor_hidden(true);
        mouse.motion([10.0, 10.0], [0.0, 0.0], 1.0);
        assert_eq!(mouse.set_enabled(true).relative, Some(true));
    }

    #[test]
    fn focus_loss_hands_the_position_to_the_os_cursor() {
        let mut mouse = relative_at([100.0, 100.0]);
        mouse.motion([0.0, 0.0], [20.0, 30.0], 1.0);
        let change = mouse.set_focused(false);
        assert_eq!(
            change,
            Change {
                relative: Some(false),
                warp: Some([120.0, 130.0]),
            }
        );
        assert!(!mouse.is_active());
        // Handed over once.
        assert_eq!(mouse.set_cursor_hidden(false), Change::default());
    }

    #[test]
    fn showing_the_os_cursor_turns_relative_mode_off() {
        let mut mouse = relative_at([100.0, 100.0]);
        mouse.motion([0.0, 0.0], [1.0, 1.0], 1.0);
        let change = mouse.set_cursor_hidden(false);
        assert_eq!(change.relative, Some(false));
        assert_eq!(change.warp, Some([101.0, 101.0]));
    }

    #[test]
    fn leaving_the_window_hands_over_to_the_os() {
        let mut mouse = relative_at([1270.0, 100.0]);
        mouse.motion([0.0, 0.0], [5.0, 0.0], 1.0);
        let (position, change) = mouse.motion([0.0, 0.0], [10.0, 0.0], 1.0);
        assert_eq!(position, [1285.0, 100.0]);
        assert_eq!(
            change,
            Change {
                relative: Some(false),
                warp: Some([1285.0, 100.0]),
            }
        );
        assert!(!mouse.is_active());
        // The OS cursor moves outside: positions are absolute again.
        let (position, change) = mouse.motion([1300.0, 100.0], [15.0, 0.0], 2.0);
        assert_eq!(position, [1300.0, 100.0]);
        assert_eq!(change, Change::default());
    }

    #[test]
    fn coming_back_turns_relative_mode_on_at_the_next_motion() {
        let mut mouse = relative_at([1270.0, 100.0]);
        mouse.motion([0.0, 0.0], [20.0, 0.0], 1.0);
        assert!(!mouse.is_active());
        mouse.motion([1300.0, 100.0], [0.0, 0.0], 1.0);
        assert_eq!(mouse.set_cursor_in_window(false), Change::default());
        assert_eq!(mouse.set_cursor_in_window(true), Change::default());
        assert!(!mouse.is_active());
        let (position, change) = mouse.motion([1200.0, 100.0], [-100.0, 0.0], 1.0);
        assert_eq!(position, [1200.0, 100.0]);
        assert_eq!(change.relative, Some(true));
        // From here on the delta counts.
        let (position, _) = mouse.motion([0.0, 0.0], [-10.0, 0.0], 1.0);
        assert_eq!(position, [1190.0, 100.0]);
    }

    #[test]
    fn rejected_mode_is_retried_once_per_change() {
        let mut mouse = relative_at([100.0, 100.0]);
        mouse.rejected();
        assert!(!mouse.is_active());
        let (_, change) = mouse.motion([105.0, 100.0], [5.0, 0.0], 1.0);
        assert_eq!(change.relative, Some(true));
        // Refused again: no more calls on motion.
        mouse.rejected();
        let (_, change) = mouse.motion([106.0, 100.0], [1.0, 0.0], 1.0);
        assert_eq!(change, Change::default());
        // A new condition change tries again.
        mouse.set_focused(false);
        assert_eq!(mouse.set_focused(true).relative, Some(true));
        mouse.rejected();
        let (_, change) = mouse.motion([107.0, 100.0], [1.0, 0.0], 1.0);
        assert_eq!(change.relative, Some(true));
    }

    #[test]
    fn disabling_hands_the_position_over() {
        let mut mouse = relative_at([100.0, 100.0]);
        mouse.motion([0.0, 0.0], [3.0, 4.0], 2.0);
        assert_eq!(
            mouse.set_enabled(false),
            Change {
                relative: Some(false),
                warp: Some([106.0, 108.0]),
            }
        );
    }

    #[test]
    fn leave_event_hands_over_at_once() {
        let mut mouse = relative_at([100.0, 100.0]);
        mouse.motion([0.0, 0.0], [1.0, 0.0], 1.0);
        assert_eq!(
            mouse.set_cursor_in_window(false),
            Change {
                relative: Some(false),
                warp: Some([101.0, 100.0]),
            }
        );
    }

    #[test]
    fn window_edges() {
        // Left edge: 0 is inside, below 0 is outside.
        let mut mouse = relative_at([1.0, 100.0]);
        let (_, change) = mouse.motion([0.0, 0.0], [-1.0, 0.0], 1.0);
        assert_eq!(change, Change::default());
        let (_, change) = mouse.motion([0.0, 0.0], [-0.5, 0.0], 1.0);
        assert_eq!(change.warp, Some([-0.5, 100.0]));
        // Top edge.
        let mut mouse = relative_at([100.0, 0.0]);
        let (_, change) = mouse.motion([0.0, 0.0], [0.0, -1.0], 1.0);
        assert_eq!(change.relative, Some(false));
        // Right edge: the width itself is outside.
        let mut mouse = relative_at([1279.0, 100.0]);
        let (_, change) = mouse.motion([0.0, 0.0], [1.0, 0.0], 1.0);
        assert_eq!(change.warp, Some([1280.0, 100.0]));
    }

    #[test]
    fn first_relative_motion_out_of_the_window_hands_over() {
        let mut mouse = relative_at([1279.0, 100.0]);
        let (_, change) = mouse.motion([0.0, 0.0], [50.0, 0.0], 1.0);
        assert_eq!(change.relative, Some(false));
    }

    #[test]
    fn shrinking_the_window_moves_its_edge() {
        let mut mouse = relative_at([900.0, 100.0]);
        mouse.set_size(800.0, 600.0);
        // Outside already: no new crossing.
        let (_, change) = mouse.motion([0.0, 0.0], [1.0, 0.0], 1.0);
        assert_eq!(change.relative, Some(false));
    }

    #[test]
    fn entering_while_active_merges_both_checks() {
        let mut mouse = relative_at([100.0, 100.0]);
        mouse.set_cursor_in_window(true);
        let (position, change) = mouse.motion([0.0, 0.0], [2.0, 0.0], 1.0);
        assert_eq!(position, [102.0, 100.0]);
        assert_eq!(change, Change::default());
    }

    #[test]
    fn fullscreen_confines_instead_of_handing_over() {
        let mut mouse = relative_at([1270.0, 700.0]);
        assert_eq!(mouse.set_confined(true), Change::default());
        let (position, change) = mouse.motion([0.0, 0.0], [50.0, 50.0], 1.0);
        assert_eq!(position, [1279.0, 719.0]);
        assert_eq!(change, Change::default());
        assert!(mouse.is_active());
        // Back from the edge at once, without the overshoot.
        let (position, _) = mouse.motion([0.0, 0.0], [-9.0, -19.0], 1.0);
        assert_eq!(position, [1270.0, 700.0]);
        let (position, _) = mouse.motion([0.0, 0.0], [-2000.0, -2000.0], 1.0);
        assert_eq!(position, [0.0, 0.0]);
    }

    #[test]
    fn fullscreen_counts_as_cursor_in_window() {
        let mut mouse = RelativeMouse::new(true, false, SIZE);
        mouse.set_enabled(true);
        mouse.set_cursor_hidden(true);
        mouse.set_confined(true);
        let (_, change) = mouse.motion([10.0, 10.0], [0.0, 0.0], 1.0);
        assert_eq!(change.relative, Some(true));
        // Leaving fullscreen without the cursor having entered the window hands it over.
        let change = mouse.set_confined(false);
        assert_eq!(change.relative, Some(false));
    }

    #[test]
    fn settings_clamp_the_sensitivity() {
        let settings = MouseSettings::default();
        assert_eq!(settings.sensitivity(), 1.0);
        assert!(!settings.is_relative());
        settings.set_sensitivity(25.0);
        assert_eq!(settings.sensitivity(), MAX_SENSITIVITY);
        settings.set_sensitivity(0.0);
        assert_eq!(settings.sensitivity(), MIN_SENSITIVITY);
        settings.set_sensitivity(f64::NAN);
        assert_eq!(settings.sensitivity(), MIN_SENSITIVITY);
        let other = settings.clone();
        other.set_relative(true);
        other.set_sensitivity(2.5);
        assert!(settings.is_relative());
        assert_eq!(settings.sensitivity(), 2.5);
    }
}
