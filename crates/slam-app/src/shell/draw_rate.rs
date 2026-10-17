//! Which draw rate applies (ADR-0047): the gameplay rate while a play is on a visible
//! layer of the stack, the menu rate otherwise.

use slam_engine::limiter::LimiterMode;

use crate::settings::Frame;

/// Remembers the rate last requested, so that the engine is only written on a change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawRateSwitch {
    last: LimiterMode,
}

impl DrawRateSwitch {
    /// A switch for an engine that starts at `initial`.
    pub fn new(initial: LimiterMode) -> DrawRateSwitch {
        DrawRateSwitch { last: initial }
    }

    /// The rate to request now, `None` while it is the one already requested. Does not
    /// allocate.
    pub fn next(&mut self, gameplay_visible: bool, frame: &Frame) -> Option<LimiterMode> {
        let wanted = if gameplay_visible {
            frame.draw
        } else {
            frame.menu_draw
        }
        .to_limiter();
        if wanted == self.last {
            return None;
        }
        self.last = wanted;
        Some(wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DrawRate;

    fn frame(draw: f64, menu: f64) -> Frame {
        Frame {
            draw: DrawRate::Hz(draw),
            menu_draw: DrawRate::Hz(menu),
            ..Frame::default()
        }
    }

    #[test]
    fn it_follows_the_play_and_writes_only_on_a_change() {
        let f = frame(1000.0, 60.0);
        let mut switch = DrawRateSwitch::new(LimiterMode::Hz(60.0));
        // On the menu at the start: nothing to do, however often it is asked.
        assert_eq!(switch.next(false, &f), None);
        assert_eq!(switch.next(false, &f), None);
        // A play comes in.
        assert_eq!(switch.next(true, &f), Some(LimiterMode::Hz(1000.0)));
        assert_eq!(switch.next(true, &f), None);
        // And goes.
        assert_eq!(switch.next(false, &f), Some(LimiterMode::Hz(60.0)));
        assert_eq!(switch.next(false, &f), None);
    }

    #[test]
    fn new_settings_apply_to_the_mode_in_force() {
        let mut switch = DrawRateSwitch::new(LimiterMode::Hz(60.0));
        assert_eq!(
            switch.next(false, &frame(1000.0, 144.0)),
            Some(LimiterMode::Hz(144.0))
        );
        // The gameplay rate alone changing is not seen from the menu.
        assert_eq!(switch.next(false, &frame(240.0, 144.0)), None);
        assert_eq!(
            switch.next(true, &frame(240.0, 144.0)),
            Some(LimiterMode::Hz(240.0))
        );
    }

    #[test]
    fn unlimited_is_a_rate_too() {
        let mut f = frame(1000.0, 60.0);
        f.menu_draw = DrawRate::Unlimited;
        let mut switch = DrawRateSwitch::new(LimiterMode::Hz(60.0));
        assert_eq!(switch.next(false, &f), Some(LimiterMode::Unlimited));
    }
}
