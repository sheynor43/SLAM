//! The legacy combo counter's animation (`LegacyDefaultComboCounter`): an increase pops the
//! new value out big and faint at once and shows it in the counter 160 ms later with a small
//! pop; a combo break rolls the shown value down to zero and fades it out.
//!
//! Draw side: [`ComboCounter::update`] takes the combo once a frame, [`ComboCounter::pose`]
//! says what to draw. Times are the HUD's clock in milliseconds (lazer's HUD is not on the
//! gameplay clock). Allocation-free.

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyDefaultComboCounter.cs
const BIG_POP_OUT_DURATION: f64 = 300.0;
const SMALL_POP_OUT_DURATION: f64 = 100.0;
const FADE_OUT_DURATION: f64 = 100.0;
/// Per unit rolled.
const ROLLING_DURATION: f64 = 20.0;
/// When the counter takes the popped-out value, after the pop out starts.
const SMALL_POP_OUT_DELAY: f64 = BIG_POP_OUT_DURATION - 140.0;
const BIG_POP_OUT_SCALE: f32 = 1.56;
const BIG_POP_OUT_ALPHA: f32 = 0.6;
const SMALL_POP_OUT_SCALE: f32 = 1.1;

/// An alpha moving linearly from `from` to `to`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fade {
    start: f64,
    duration: f64,
    from: f32,
    to: f32,
}

impl Fade {
    fn instant(alpha: f32) -> Fade {
        Fade {
            start: f64::NEG_INFINITY,
            duration: 0.0,
            from: alpha,
            to: alpha,
        }
    }

    fn at(&self, time: f64) -> f32 {
        if self.duration <= 0.0 || time >= self.start + self.duration {
            return self.to;
        }
        let t = ((time - self.start) / self.duration).max(0.0) as f32;
        self.from + (self.to - self.from) * t
    }
}

/// The shown value rolling to `to` (`TransformTo(DisplayedCount)`, no easing).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Roll {
    start: f64,
    duration: f64,
    from: u32,
    to: u32,
}

impl Roll {
    fn at(&self, time: f64) -> u32 {
        if self.duration <= 0.0 || time >= self.start + self.duration {
            return self.to;
        }
        let t = ((time - self.start) / self.duration).max(0.0);
        let value = f64::from(self.from) + t * (f64::from(self.to) - f64::from(self.from));
        // .NET's `Math.Round`: halves to even.
        value.round_ties_even() as u32
    }
}

/// What the counter draws at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComboPose {
    /// The value of the counter's own text.
    pub shown: u32,
    pub shown_alpha: f32,
    pub shown_scale: f32,
    /// The popped-out value, its scale and alpha (drawn additive), while it shows.
    pub pop_out: Option<(u32, f32, f32)>,
}

/// The state of a legacy combo counter.
#[derive(Debug, Clone, PartialEq)]
pub struct ComboCounter {
    /// The combo last taken.
    previous: u32,
    /// `DisplayedCount`: the value of the counter's own text.
    displayed: u32,
    rolling: bool,
    /// Invalidates the scheduled small pop out (`scheduledPopOutCurrentId`).
    pop_out_id: u32,
    /// When the scheduled small pop out runs, and its id.
    scheduled: Option<(f64, u32)>,
    roll: Option<Roll>,
    alpha: Fade,
    /// When the shown text's small pop out started.
    small_pop: Option<f64>,
    /// When the big pop out started, and its value.
    pop_out: Option<(f64, u32)>,
}

impl ComboCounter {
    /// A counter loaded at `time` showing `combo` (`LoadComplete`).
    pub fn new(combo: u32, time: f64) -> ComboCounter {
        let mut counter = ComboCounter {
            previous: 0,
            displayed: 0,
            rolling: false,
            pop_out_id: 0,
            scheduled: None,
            roll: None,
            alpha: Fade::instant(0.0),
            small_pop: None,
            pop_out: None,
        };
        // `Current.BindValueChanged(combo => updateCount(combo.NewValue == 0), true)`
        counter.update_count(combo, combo == 0, time);
        counter
    }

    /// Takes the `combo` at `time`. Call once a frame, with times that do not go back.
    pub fn update(&mut self, combo: u32, time: f64) {
        // The judgements change the combo before the HUD updates; the HUD then applies its
        // transforms and runs its scheduler.
        if combo != self.previous {
            self.update_count(combo, combo == 0, time);
        }
        if let Some(roll) = self.roll {
            self.set_displayed(roll.at(time), time);
            if time >= roll.start + roll.duration {
                self.roll = None;
            }
        }
        if let Some((due, id)) = self.scheduled
            && time >= due
        {
            self.scheduled = None;
            // Too late: a later change invalidated it.
            if id == self.pop_out_id {
                self.set_displayed(self.displayed + 1, time);
            }
        }
    }

    /// What to draw at `time`.
    pub fn pose(&self, time: f64) -> ComboPose {
        let shown_scale = self
            .small_pop
            .map_or(1.0, |start| small_pop_scale(time - start));
        let pop_out = self.pop_out.and_then(|(start, value)| {
            let t = (time - start) / BIG_POP_OUT_DURATION;
            (t < 1.0).then(|| {
                let t = t.max(0.0) as f32;
                (
                    value,
                    BIG_POP_OUT_SCALE + (1.0 - BIG_POP_OUT_SCALE) * t,
                    BIG_POP_OUT_ALPHA * (1.0 - t),
                )
            })
        });
        ComboPose {
            shown: self.displayed,
            shown_alpha: self.alpha.at(time),
            shown_scale,
            pop_out,
        }
    }

    fn update_count(&mut self, combo: u32, rolling: bool, time: f64) {
        let prev = self.previous;
        self.previous = combo;
        if !rolling {
            // `FinishTransforms(false, nameof(DisplayedCount))`
            if let Some(roll) = self.roll.take() {
                self.set_displayed(roll.to, time);
            }
            self.rolling = false;
            self.set_displayed(prev, time);
            if prev + 1 == combo {
                self.on_count_increment(prev, combo, time);
            } else {
                self.on_count_change(combo, time);
            }
        } else {
            self.on_count_rolling(self.displayed, combo, time);
            self.rolling = true;
        }
    }

    /// The `DisplayedCount` setter.
    fn set_displayed(&mut self, value: u32, time: f64) {
        if self.displayed == value {
            return;
        }
        if self.rolling {
            self.on_displayed_count_rolling(value, time);
        } else if self.displayed + 1 == value {
            self.on_displayed_count_increment(time);
        } else {
            self.on_displayed_count_change(value);
        }
        self.displayed = value;
    }

    fn fade_out(&mut self, duration: f64, time: f64) {
        self.alpha = Fade {
            start: time,
            duration,
            from: self.alpha.at(time),
            to: 0.0,
        };
    }

    fn on_count_increment(&mut self, current: u32, new: u32, time: f64) {
        self.pop_out_id = self.pop_out_id.wrapping_add(1);
        if self.displayed < current {
            self.set_displayed(self.displayed + 1, time);
        }
        self.alpha = Fade::instant(1.0);
        self.pop_out = Some((time, new));
        self.scheduled = Some((time + SMALL_POP_OUT_DELAY, self.pop_out_id));
    }

    fn on_count_rolling(&mut self, current: u32, new: u32, time: f64) {
        self.pop_out_id = self.pop_out_id.wrapping_add(1);
        // Hides the counter if it was going from 0 to 1 but did not finish.
        if current == 0 && new == 0 {
            self.fade_out(FADE_OUT_DURATION, time);
        }
        self.roll = Some(Roll {
            start: time,
            duration: f64::from(current.abs_diff(new)) * ROLLING_DURATION,
            from: current,
            to: new,
        });
    }

    fn on_count_change(&mut self, new: u32, time: f64) {
        self.pop_out_id = self.pop_out_id.wrapping_add(1);
        if new == 0 {
            self.alpha = Fade::instant(0.0);
        }
        self.set_displayed(new, time);
    }

    fn on_displayed_count_rolling(&mut self, value: u32, time: f64) {
        if value == 0 {
            self.fade_out(FADE_OUT_DURATION, time);
        } else {
            self.alpha = Fade::instant(1.0);
        }
        self.small_pop = None;
    }

    fn on_displayed_count_change(&mut self, value: u32) {
        self.alpha = Fade::instant(if value == 0 { 0.0 } else { 1.0 });
        self.small_pop = None;
    }

    fn on_displayed_count_increment(&mut self, time: f64) {
        self.alpha = Fade::instant(1.0);
        self.small_pop = Some(time);
    }
}

/// The shown text's scale `elapsed` ms into a small pop out: up with `Easing.In`, back with
/// `Easing.Out`, half the duration each.
fn small_pop_scale(elapsed: f64) -> f32 {
    let half = SMALL_POP_OUT_DURATION / 2.0;
    let grow = SMALL_POP_OUT_SCALE - 1.0;
    if elapsed <= 0.0 || elapsed >= SMALL_POP_OUT_DURATION {
        1.0
    } else if elapsed < half {
        let t = (elapsed / half) as f32;
        1.0 + grow * t * t
    } else {
        let t = ((elapsed - half) / half) as f32;
        SMALL_POP_OUT_SCALE - grow * t * (2.0 - t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn starts_hidden_at_zero() {
        let counter = ComboCounter::new(0, 0.0);
        let pose = counter.pose(0.0);
        assert_eq!(pose.shown, 0);
        assert_eq!(pose.shown_alpha, 0.0);
        assert_eq!(pose.pop_out, None);
    }

    #[test]
    fn an_increase_pops_out_and_reaches_the_counter_160_ms_later() {
        let mut counter = ComboCounter::new(0, 0.0);
        counter.update(1, 1000.0);
        // The counter shows (still "0x"), the pop out starts big and faint.
        let pose = counter.pose(1000.0);
        assert_eq!(pose.shown, 0);
        assert_eq!(pose.shown_alpha, 1.0);
        let (value, scale, alpha) = pose.pop_out.unwrap();
        assert_eq!(value, 1);
        assert!(close(scale, 1.56) && close(alpha, 0.6));
        // Half way it is half way.
        let (_, scale, alpha) = counter.pose(1150.0).pop_out.unwrap();
        assert!(close(scale, 1.28) && close(alpha, 0.3));
        counter.update(1, 1159.0);
        assert_eq!(counter.pose(1159.0).shown, 0);
        counter.update(1, 1160.0);
        let pose = counter.pose(1160.0);
        assert_eq!(pose.shown, 1);
        assert_eq!(pose.shown_scale, 1.0);
        // The small pop: 1.1 at 50 ms, back to 1 at 100 ms; quadratic in, then out.
        assert!(close(counter.pose(1185.0).shown_scale, 1.025));
        assert!(close(counter.pose(1210.0).shown_scale, 1.1));
        assert!(close(counter.pose(1235.0).shown_scale, 1.025));
        assert_eq!(counter.pose(1260.0).shown_scale, 1.0);
        // The pop out is over at 300 ms.
        assert!(counter.pose(1299.0).pop_out.is_some());
        assert_eq!(counter.pose(1300.0).pop_out, None);
    }

    #[test]
    fn quick_increases_take_the_previous_value_at_once() {
        let mut counter = ComboCounter::new(0, 0.0);
        counter.update(1, 0.0);
        counter.update(2, 50.0);
        // The first small pop is cancelled; the counter jumps to 1 with a small pop.
        let pose = counter.pose(50.0);
        assert_eq!(pose.shown, 1);
        assert_eq!(pose.pop_out.unwrap().0, 2);
        counter.update(2, 160.0);
        assert_eq!(counter.pose(160.0).shown, 1);
        counter.update(2, 210.0);
        assert_eq!(counter.pose(210.0).shown, 2);
    }

    #[test]
    fn a_break_rolls_down_and_fades_out() {
        let mut counter = ComboCounter::new(0, 0.0);
        for (i, combo) in (1..=10).enumerate() {
            counter.update(combo, i as f64 * 1000.0);
        }
        counter.update(10, 9500.0);
        assert_eq!(counter.pose(9500.0).shown, 10);
        // 10 units at 20 ms each.
        counter.update(0, 10_000.0);
        counter.update(0, 10_100.0);
        let pose = counter.pose(10_100.0);
        assert_eq!(pose.shown, 5);
        assert_eq!(pose.shown_alpha, 1.0);
        counter.update(0, 10_200.0);
        let pose = counter.pose(10_200.0);
        assert_eq!(pose.shown, 0);
        assert_eq!(pose.shown_alpha, 1.0);
        assert!(close(counter.pose(10_250.0).shown_alpha, 0.5));
        assert_eq!(counter.pose(10_300.0).shown_alpha, 0.0);
    }

    #[test]
    fn rolling_rounds_halves_to_even() {
        let roll = Roll {
            start: 0.0,
            duration: 200.0,
            from: 10,
            to: 0,
        };
        // 8.5 and 7.5.
        assert_eq!(roll.at(30.0), 8);
        assert_eq!(roll.at(50.0), 8);
    }

    #[test]
    fn a_break_as_the_small_pop_is_due_cancels_it() {
        let mut counter = ComboCounter::new(0, 0.0);
        counter.update(1, 0.0);
        counter.update(0, 160.0);
        let pose = counter.pose(160.0);
        assert_eq!(pose.shown, 0);
        assert_eq!(pose.shown_scale, 1.0);
        // From 0 to 0: it fades out.
        assert!(close(pose.shown_alpha, 1.0));
        counter.update(0, 210.0);
        assert!(close(counter.pose(210.0).shown_alpha, 0.5));
        counter.update(0, 400.0);
        assert_eq!(counter.pose(400.0).shown, 0);
    }

    #[test]
    fn a_hit_during_the_roll_finishes_it() {
        let mut counter = ComboCounter::new(0, 0.0);
        for (i, combo) in (1..=10).enumerate() {
            counter.update(combo, i as f64 * 1000.0);
        }
        counter.update(0, 10_000.0);
        counter.update(0, 10_100.0);
        counter.update(1, 10_110.0);
        let pose = counter.pose(10_110.0);
        assert_eq!(pose.shown, 0);
        assert_eq!(pose.shown_alpha, 1.0);
        assert_eq!(pose.pop_out.unwrap().0, 1);
        counter.update(1, 10_270.0);
        assert_eq!(counter.pose(10_270.0).shown, 1);
    }

    #[test]
    fn other_changes_show_at_once() {
        let mut counter = ComboCounter::new(0, 0.0);
        counter.update(42, 0.0);
        let pose = counter.pose(0.0);
        assert_eq!(pose.shown, 42);
        assert_eq!(pose.shown_alpha, 1.0);
        assert_eq!(pose.pop_out, None);
        let loaded = ComboCounter::new(7, 0.0);
        assert_eq!(loaded.pose(0.0).shown, 7);
        assert_eq!(loaded.pose(0.0).shown_alpha, 1.0);
    }
}
