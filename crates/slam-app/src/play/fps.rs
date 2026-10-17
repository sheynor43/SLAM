//! Where the FPS counter's inputs come from (ADR-0053): the engine's counters and rate
//! controls, read on the draw thread once per frame.

use std::sync::Arc;

use slam_engine::limiter::LimiterMode;
use slam_engine::{DrawRateControl, EngineCounters, UpdateRateControl};

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
}
