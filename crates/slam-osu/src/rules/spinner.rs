//! Spinner judgement: the state lazer keeps in `DrawableSpinner`, its `SpinnerRotationTracker`
//! and the `SpinnerSpinHistory` of its result.
//!
//! While the spinner can be spun and a button is held, the cursor's angle around the spinner's
//! centre is accumulated, scaled by the gameplay rate. Each whole spin judges the next tick (a
//! bonus once past the required spins plus a gap); once the end time passes, the remaining
//! ticks are missed and the spinner is judged by the fraction of the required spins done.

use slam_formats::osu::Vec2;

use crate::dotnet;
use crate::judgement::{HitResult, Judgement, JudgementLog, ObjectRef};
use crate::math;
use crate::objects::{OsuHitObject, Spinner};
use crate::replay::Actions;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs (fade_out_duration, UpdateHitStateTransforms)
// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/Transformable.cs (LatestTransformEndTime)
/// How long a spinner stays alive after its judgement: it fades out over 240 ms and expires 1 ms
/// after the last transform, as a slider does.
pub(super) const JUDGED_LIFETIME: f64 = 240.0 + 1.0;

/// .NET's `float.RadiansToDegrees`.
fn radians_to_degrees(radians: f32) -> f32 {
    (radians * 180.0) / std::f32::consts::PI
}

/// .NET's `Math.Sign(float)` for a non-NaN value.
fn sign(value: f32) -> i32 {
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/SpinnerSpinHistory.cs
/// Lazer's `SpinnerSpinHistory`: the scoring rotation of a spinner. A spin completes once the
/// rotation in one direction since the last completed spin reaches 360°; spinning back within a
/// spin does not lower the total, and changing direction does not add to it.
///
/// Time only moves forward here, so lazer's rewinding branch, which pops spins completed after
/// the reported time, is not ported, and the completed spins are a count, not a stack.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SpinHistory {
    completed_spins: i32,
    /// The total accumulated (signed) rotation.
    total_accumulated_rotation: f32,
    total_accumulated_rotation_at_last_completion: f32,
    /// The largest absolute rotation of the current spin.
    current_spin_max_rotation: f32,
    last_report_time: f64,
}

impl SpinHistory {
    /// A history with no rotation.
    pub(super) fn new() -> SpinHistory {
        SpinHistory {
            completed_spins: 0,
            total_accumulated_rotation: 0.0,
            total_accumulated_rotation_at_last_completion: 0.0,
            current_spin_max_rotation: 0.0,
            last_report_time: f64::NEG_INFINITY,
        }
    }

    /// The sum of the completed spins and the current partial spin, in degrees.
    pub(super) fn total_rotation(&self) -> f32 {
        // `360 * Count` is an unchecked int product, converted to float for the sum.
        360i32.wrapping_mul(self.completed_spins) as f32 + self.current_spin_max_rotation
    }

    /// The current spin, from -360 to 360.
    fn current_spin_rotation(&self) -> f32 {
        self.total_accumulated_rotation - self.total_accumulated_rotation_at_last_completion
    }

    /// Reports a rotation of `delta` degrees at `current_time`, which must not be before the
    /// previous report.
    pub(super) fn report_delta(&mut self, current_time: f64, delta: f32) {
        if delta == 0.0 {
            return;
        }

        debug_assert!(
            current_time.partial_cmp(&self.last_report_time) != Some(std::cmp::Ordering::Less),
            "spin history rewound"
        );

        self.total_accumulated_rotation += delta;

        self.current_spin_max_rotation = dotnet::max_f32(
            self.current_spin_max_rotation,
            self.current_spin_rotation().abs(),
        );

        while self.current_spin_max_rotation >= 360.0 {
            let direction = sign(self.current_spin_rotation());
            self.completed_spins += 1;
            // `direction * 360` is an int, converted to float for the sum.
            self.total_accumulated_rotation_at_last_completion += (direction * 360) as f32;
            self.current_spin_max_rotation = self.current_spin_rotation().abs();
        }

        self.last_report_time = current_time;
    }
}

/// The state of one spinner.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SpinnerState {
    start_time: f64,
    end_time: f64,
    /// Where the spinner is drawn: the centre of its rotation.
    position: Vec2,
    spins_required: i32,
    /// The number of ticks; the ticks from `spins_required_for_bonus` on are bonus ticks.
    tick_count: usize,
    spins_required_for_bonus: i32,
    /// Lazer's `MaximumBonusSpins`: the number of bonus ticks.
    maximum_bonus_spins: i32,
    /// Ticks before this index are judged.
    ticks_judged: usize,
    /// When each tick judged as a hit was judged, in order: the first ones, as ticks are hit in
    /// order and only missed once the end time passes. Reserved for every tick up front.
    tick_hits: Vec<f64>,
    /// Lazer's `Result.TimeCompleted`: the first update from the start time on with the
    /// required spins done.
    time_completed: Option<f64>,
    /// Lazer's gameplay rate (`GetTrueGameplayRate`): the rotation is scaled by it, so that the
    /// spins needed do not depend on the speed.
    rate: f64,
    history: SpinHistory,
    /// `completedFullSpins`: the whole spins already turned into ticks.
    completed_full_spins: i32,
    /// When `completed_full_spins` last changed.
    full_spins_changed: Option<f64>,
    /// Whole spins completed after the last tick was judged, each of which plays
    /// `spinnerbonus-max` in lazer.
    bonus_max_spins: u32,
    /// The tracker's `currentRotation`: the rotation added so far, signed.
    current_rotation: f32,
    /// The tracker's `Rotation`: the drawn rotation, damped towards `current_rotation`.
    rotation: f32,
    /// The tracker's `IsSpinning`, as of the last update.
    is_spinning: bool,
    tracking: bool,
    /// The last cursor position the tracker received; none before it has seen input.
    mouse: Option<Vec2>,
    last_angle: Option<f32>,
}

impl SpinnerState {
    /// The state of `object`, which is `spinner`, played at `rate`.
    pub(super) fn new(object: &OsuHitObject, spinner: &Spinner, rate: f64) -> SpinnerState {
        let tick_count = usize::try_from(spinner.tick_count()).unwrap_or(0);
        SpinnerState {
            start_time: object.start_time,
            end_time: spinner.end_time,
            position: object.stacked_position(),
            spins_required: spinner.spins_required,
            tick_count,
            spins_required_for_bonus: spinner.spins_required_for_bonus(),
            maximum_bonus_spins: spinner.maximum_bonus_spins,
            ticks_judged: 0,
            tick_hits: Vec::with_capacity(tick_count),
            time_completed: None,
            rate,
            history: SpinHistory::new(),
            completed_full_spins: 0,
            full_spins_changed: None,
            bonus_max_spins: 0,
            current_rotation: 0.0,
            rotation: 0.0,
            is_spinning: false,
            tracking: false,
            mouse: None,
            last_angle: None,
        }
    }

    /// The number of judgements the spinner produces: its ticks and itself.
    pub(super) fn judgement_count(&self) -> usize {
        self.tick_count + 1
    }

    /// The tracker receives the cursor position.
    pub(super) fn set_mouse(&mut self, position: Vec2) {
        self.mouse = Some(position);
    }

    /// The scoring rotation so far, in degrees.
    pub(super) fn total_rotation(&self) -> f32 {
        self.history.total_rotation()
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Default/SpinnerRotationTracker.cs (IsSpinnableTime)
    fn is_spinnable_time(&self, time: f64) -> bool {
        self.start_time <= time && self.end_time > time
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs (progressUnclamped)
    /// The fraction of the required spins done, past 1 once spun beyond them.
    pub(super) fn progress_unclamped(&self) -> f32 {
        if self.spins_required == 0 {
            // Spinners too short to require a whole spin are implicitly hit.
            1.0
        } else {
            self.total_rotation() / 360.0 / self.spins_required as f32
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs (Progress)
    /// The fraction of the required spins done, from 0 to 1.
    pub(super) fn progress(&self) -> f32 {
        dotnet::clamp_f32(self.progress_unclamped(), 0.0, 1.0)
    }

    /// Whether the tracker considers the spinner spun (lazer's `IsSpinning`): the drawn
    /// rotation lags more than 10° behind the rotation added.
    pub(super) fn is_spinning(&self) -> bool {
        self.is_spinning
    }

    /// The tracker's drawn rotation in degrees, clockwise.
    pub(super) fn rotation(&self) -> f32 {
        self.rotation
    }

    /// The tracker's rotation spun in degrees, clockwise, which the drawn one follows.
    pub(super) fn spun_rotation(&self) -> f32 {
        self.current_rotation
    }

    /// When the required spins were done, if they were.
    pub(super) fn time_completed(&self) -> Option<f64> {
        self.time_completed
    }

    /// When each tick hit so far was judged, in order.
    pub(super) fn tick_hits(&self) -> &[f64] {
        &self.tick_hits
    }

    /// When the last bonus tick hit so far was judged.
    pub(super) fn last_bonus_hit(&self) -> Option<f64> {
        let first_bonus = usize::try_from(self.spins_required_for_bonus).unwrap_or(0);
        self.tick_hits.get(first_bonus..)?.last().copied()
    }

    /// Lazer's `CompletedFullSpins`: the whole spins already turned into ticks or bonus-max
    /// samples; always 0 for a spinner without ticks.
    pub(super) fn completed_full_spins(&self) -> i32 {
        self.completed_full_spins
    }

    /// When [`Self::completed_full_spins`] last changed: lazer's bonus counter animates then.
    pub(super) fn full_spins_changed(&self) -> Option<f64> {
        self.full_spins_changed
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs (CurrentBonusScore, MaximumBonusScore, score_per_tick)
    /// Lazer's `CurrentBonusScore` and `MaximumBonusScore`: the score of the bonus ticks hit so
    /// far, and of all of them.
    pub(super) fn bonus_score(&self) -> (f64, f64) {
        let score_per_tick = HitResult::LargeBonus.base_score();
        let spins = (self.completed_full_spins - self.spins_required_for_bonus)
            .clamp(0, self.maximum_bonus_spins.max(0));
        // C#'s unchecked `int` products.
        (
            f64::from(score_per_tick.wrapping_mul(spins)),
            f64::from(score_per_tick.wrapping_mul(self.maximum_bonus_spins)),
        )
    }

    /// The whole spins completed after the last tick, which play `spinnerbonus-max`; always 0
    /// for a spinner without ticks.
    pub(super) fn bonus_max_spins(&self) -> u32 {
        self.bonus_max_spins
    }

    /// Updates the alive, unjudged spinner (object `index`) at `time`, `elapsed` after the
    /// previous update, with `held` actions, in its drawable order: tracking, then the rotation
    /// tracker, then the judgement and the ticks of the whole spins. Returns the spinner's
    /// result once it is judged.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs (Update, correctButtonPressed, UpdateAfterChildren)
    pub(super) fn update(
        &mut self,
        index: usize,
        time: f64,
        elapsed: f64,
        held: Actions,
        log: &mut JudgementLog,
    ) -> Option<HitResult> {
        // The spinner has no result yet, or it would not be updated.
        self.tracking = self.is_spinnable_time(time)
            && (held.contains(Actions::LEFT) || held.contains(Actions::RIGHT));

        self.update_tracker(time, elapsed);

        // `DrawableHitObject.UpdateAfterChildren` checks for a result before the bonus.
        let result = self.check_for_result(index, time, log);
        self.update_bonus_score(index, time, log);
        result
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Default/SpinnerRotationTracker.cs (Update)
    /// The tracker's update: the angle the cursor moved through around the centre since the
    /// last update, taken the short way round, is added while tracking; then the drawn rotation
    /// follows it, and the spinner counts as spinning while it lags far enough behind.
    ///
    /// Lazer takes the cursor in the local space of a square container centred on the spinner;
    /// here the angle is taken from playfield positions directly, without the round trip through
    /// screen space (ADR-0028).
    fn update_tracker(&mut self, time: f64, elapsed: f64) {
        self.track_cursor(time);

        self.is_spinning =
            self.is_spinnable_time(time) && (self.current_rotation - self.rotation).abs() > 10.0;
        // Ported from osu-framework 2026.921.1: osu.Framework/Utils/Interpolation.cs (Damp, Lerp)
        // In double precision, stored back as float.
        let amount = 1.0 - math::pow(0.99, elapsed.abs());
        let start = f64::from(self.rotation);
        self.rotation = (start + (f64::from(self.current_rotation) - start) * amount) as f32;
    }

    /// The tracker's handling of the cursor position, if it has received one.
    fn track_cursor(&mut self, time: f64) {
        let Some(mouse) = self.mouse else {
            return;
        };
        let offset = mouse - self.position;
        // `MathF.Atan2(x, y)`: the angle from the downward axis.
        let this_angle = -radians_to_degrees(math::atan2f(offset.x, offset.y));

        let mut delta = match self.last_angle {
            None => 0.0,
            Some(last_angle) => this_angle - last_angle,
        };

        // Take the shortest path round, as a full turn between two updates cannot be told
        // apart.
        if delta > 180.0 {
            delta -= 360.0;
        }
        if delta < -180.0 {
            delta += 360.0;
        }

        if self.tracking {
            self.add_rotation(time, delta);
        }

        self.last_angle = Some(this_angle);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Default/SpinnerRotationTracker.cs (AddRotation)
    fn add_rotation(&mut self, time: f64, delta: f32) {
        if !self.is_spinnable_time(time) {
            return;
        }
        // Lazer first moves `currentRotation` to the drawn `Rotation`, which has followed it
        // and so is still 0 here.
        // Spinning at a faster rate covers less gameplay time, so the rotation is scaled up.
        let delta = (f64::from(delta) * self.rate.abs()) as f32;
        self.current_rotation += delta;
        self.history.report_delta(time, delta);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs (CheckForResult)
    /// Once the end time has passed, misses the remaining ticks and judges the spinner by its
    /// progress.
    fn check_for_result(
        &mut self,
        index: usize,
        time: f64,
        log: &mut JudgementLog,
    ) -> Option<HitResult> {
        if time < self.start_time {
            return None;
        }

        // Not used for judgement: the classic spinner shows "clear" from then.
        if self.progress() >= 1.0 && self.time_completed.is_none() {
            self.time_completed = Some(time);
        }

        if time < self.end_time {
            return None;
        }

        while self.ticks_judged < self.tick_count {
            self.judge_tick(index, time, false, log);
        }

        let progress = self.progress();
        let result = if progress >= 1.0 {
            HitResult::Great
        } else if f64::from(progress) > 0.9 {
            HitResult::Ok
        } else if f64::from(progress) > 0.75 {
            HitResult::Meh
        } else {
            // Lazer's last branch, `Time.Current >= EndTime`, always holds here.
            HitResult::Miss
        };
        Some(result)
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs (updateBonusScore)
    /// Judges a tick hit for every whole spin since the last update. Spins past the last tick
    /// only play a sample: they are counted in `bonus_max_spins`.
    fn update_bonus_score(&mut self, index: usize, time: f64, log: &mut JudgementLog) {
        if self.tick_count == 0 {
            return;
        }

        // A C# `(int)` cast, like `as`.
        let spins = (self.total_rotation() / 360.0) as i32;

        if spins < self.completed_full_spins {
            // Rewinding in lazer; the total rotation never drops here.
            self.completed_full_spins = spins;
            self.full_spins_changed = Some(time);
            return;
        }
        if spins != self.completed_full_spins {
            self.full_spins_changed = Some(time);
        }

        while self.completed_full_spins != spins {
            if self.ticks_judged < self.tick_count {
                self.judge_tick(index, time, true, log);
            } else {
                self.bonus_max_spins += 1;
            }
            self.completed_full_spins += 1;
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinnerTick.cs (TriggerResult)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SpinnerTick.cs (OsuSpinnerTickJudgement)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SpinnerBonusTick.cs (OsuSpinnerBonusTickJudgement)
    /// Judges the next tick, with its best result if `hit` and its worst otherwise.
    fn judge_tick(&mut self, index: usize, time: f64, hit: bool, log: &mut JudgementLog) {
        let k = self.ticks_judged;
        // `k < tick_count`, which came from an i32.
        let max_result = if k as i32 >= self.spins_required_for_bonus {
            HitResult::LargeBonus
        } else {
            HitResult::SmallBonus
        };
        let result = if hit {
            self.tick_hits.push(time);
            max_result
        } else {
            max_result.min_result()
        };
        log.apply(Judgement {
            object: ObjectRef {
                index: index as u32,
                nested: Some(k as u32),
            },
            time,
            result,
            max_result,
        });
        self.ticks_judged += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/SpinnerSpinHistoryTest.cs (TestSpinOneDirection)
    #[test]
    fn spin_one_direction() {
        let cases: [(f32, f32); 16] = [
            (0.0, 0.0),
            (10.0, 10.0),
            (180.0, 180.0),
            (350.0, 350.0),
            (360.0, 360.0),
            (370.0, 370.0),
            (540.0, 540.0),
            (720.0, 720.0),
            (-0.0, 0.0),
            (-10.0, 10.0),
            (-180.0, 180.0),
            (-350.0, 350.0),
            (-360.0, 360.0),
            (-370.0, 370.0),
            (-540.0, 540.0),
            (-720.0, 720.0),
        ];
        for (spin, expected) in cases {
            let mut history = SpinHistory::new();
            history.report_delta(500.0, spin);
            assert_eq!(history.total_rotation(), expected, "spin {spin}");
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/SpinnerSpinHistoryTest.cs (TestSpinMultipleDirections)
    #[test]
    fn spin_multiple_directions() {
        let cases: [(f32, f32, f32, f32); 23] = [
            (0.0, 0.0, 0.0, 0.0),
            (10.0, -10.0, 0.0, 10.0),
            (-10.0, 10.0, 0.0, 10.0),
            (10.0, -20.0, 0.0, 10.0),
            (-10.0, 20.0, 0.0, 10.0),
            (20.0, -10.0, 0.0, 20.0),
            (-20.0, 10.0, 0.0, 20.0),
            (10.0, -360.0, 0.0, 350.0),
            (-10.0, 360.0, 0.0, 350.0),
            (360.0, -10.0, 0.0, 370.0),
            (360.0, 10.0, 0.0, 370.0),
            (-360.0, 10.0, 0.0, 370.0),
            (-360.0, -10.0, 0.0, 370.0),
            (10.0, 10.0, 10.0, 30.0),
            (10.0, 10.0, -10.0, 20.0),
            (10.0, -10.0, 10.0, 10.0),
            (-10.0, -10.0, -10.0, 30.0),
            (-10.0, -10.0, 10.0, 20.0),
            (-10.0, 10.0, 10.0, 10.0),
            (10.0, -20.0, -350.0, 360.0),
            (10.0, -20.0, 350.0, 340.0),
            (-10.0, 20.0, 350.0, 360.0),
            (-10.0, 20.0, -350.0, 340.0),
        ];
        for (spin1, spin2, spin3, expected) in cases {
            let mut history = SpinHistory::new();
            history.report_delta(500.0, spin1);
            history.report_delta(1000.0, spin2);
            history.report_delta(1500.0, spin3);
            assert_eq!(
                history.total_rotation(),
                expected,
                "spins {spin1}, {spin2}, {spin3}"
            );
        }
    }

    #[test]
    fn radians_to_degrees_matches_dotnet() {
        assert_eq!(radians_to_degrees(std::f32::consts::PI), 180.0);
        assert_eq!(radians_to_degrees(-std::f32::consts::FRAC_PI_2), -90.0);
    }
}
