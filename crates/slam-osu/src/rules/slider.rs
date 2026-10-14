//! Slider judgement: the state lazer keeps in `DrawableSlider`, its nested drawables and its
//! `SliderInputManager`.
//!
//! The head is judged like a hit circle by [`super::OsuRules`], under the hit policy. Ticks,
//! repeats and the tail are judged by tracking: the cursor in the follow area with a valid
//! action held. The slider itself is judged once its tail is and its end time has passed.

use std::cmp::Ordering;

use slam_formats::osu::Vec2;

use crate::events::TAIL_LENIENCY;
use crate::hit_windows::MISS_WINDOW;
use crate::judgement::{HitResult, Judgement, JudgementLog, ObjectRef};
use crate::objects::{NestedKind, OsuHitObject, Slider, curve_position_at};
use crate::path::SliderPath;
use crate::replay::Actions;

/// How far the expanded follow area reaches, in radii.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderBall.cs (FOLLOW_AREA)
const FOLLOW_AREA: f32 = 2.4;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (UpdateHitStateTransforms)
// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/Transformable.cs (LatestTransformEndTime)
/// How long a slider stays alive after its judgement: it fades out over 240 ms and expires 1 ms
/// after the last transform.
pub(super) const JUDGED_LIFETIME: f64 = 240.0 + 1.0;

/// What a nested object is, for judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Head,
    Tick,
    Repeat,
    Tail,
}

/// The judgement state of one nested object.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Nested {
    role: Role,
    start_time: f64,
    max_result: HitResult,
    /// [`HitResult::None`] until judged.
    result: HitResult,
    /// When it was judged; NaN until then.
    time: f64,
}

impl Nested {
    fn judged(&self) -> bool {
        self.result != HitResult::None
    }
}

/// The state of one slider.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SliderState {
    path: SliderPath,
    span_count: i32,
    start_time: f64,
    end_time: f64,
    duration: f64,
    /// Stacked position: where the slider is drawn.
    position: Vec2,
    /// `(float)Slider.Radius`.
    radius: f32,
    /// Lazer's `Slider.ClassicSliderBehaviour` (Classic's `NoSliderHeadAccuracy`).
    classic: bool,
    /// Head, ticks, repeats and tail, in the beatmap's order (by start time).
    nested: Vec<Nested>,
    head: usize,
    tail: usize,
    /// The last tick or repeat.
    last_tick: Option<usize>,
    /// Every nested object before this index is judged.
    first_unjudged: usize,
    /// Lifetime end of the head drawable; it comes alive with the slider.
    head_lifetime_end: f64,
    /// `HitReceptor.HitAction`: the first action that pressed on the head while it could be
    /// hit, whether or not the press judged it.
    head_hit_action: Option<Actions>,
    tracking: bool,
    /// Tracking as lazer's `DrawableSlider.Update` reads it: the parent updates before its
    /// `SliderInputManager` child, so it sees tracking from the previous update, changed only
    /// by presses since.
    drawable_tracking: bool,
    time_to_accept_any_key_after: Option<f64>,
    /// The actions held at the previous tracking update.
    last_pressed: Actions,
    /// The last cursor position the slider received; none before it has seen input.
    mouse: Option<Vec2>,
}

/// `Vector2.LengthSquared`.
fn length_squared(v: Vec2) -> f32 {
    v.x * v.x + v.y * v.y
}

impl SliderState {
    /// The state of `object`, which is `slider`, before anything is judged.
    pub(super) fn new(object: &OsuHitObject, slider: &Slider, classic: bool) -> SliderState {
        let nested: Vec<Nested> = slider
            .nested
            .iter()
            .map(|n| {
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SliderHeadCircle.cs (CreateJudgement)
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SliderTailCircle.cs (CreateJudgement)
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SliderEndCircle.cs (SliderEndJudgement)
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Judgements/SliderTickJudgement.cs
                let (role, max_result) = match n.kind {
                    NestedKind::Head if classic => (Role::Head, HitResult::LargeTickHit),
                    NestedKind::Head => (Role::Head, HitResult::Great),
                    NestedKind::Tick { .. } => (Role::Tick, HitResult::LargeTickHit),
                    NestedKind::Repeat { .. } => (Role::Repeat, HitResult::LargeTickHit),
                    NestedKind::Tail { .. } if classic => (Role::Tail, HitResult::SmallTickHit),
                    NestedKind::Tail { .. } => (Role::Tail, HitResult::SliderTailHit),
                };
                Nested {
                    role,
                    start_time: n.start_time,
                    max_result,
                    result: HitResult::None,
                    time: f64::NAN,
                }
            })
            .collect();
        let find = |role| nested.iter().position(|n| n.role == role);
        SliderState {
            path: slider.path.clone(),
            span_count: slider.span_count(),
            start_time: object.start_time,
            end_time: slider.end_time(object.start_time),
            duration: slider.duration(object.start_time),
            position: object.stacked_position(),
            radius: object.defaults.radius() as f32,
            classic,
            // Every slider is given a head and a tail when its nested objects are created
            // (`OsuHitObject::apply_defaults`); `OsuRules` documents the requirement.
            head: find(Role::Head).expect("a slider has a head"),
            tail: find(Role::Tail).expect("a slider has a tail"),
            last_tick: nested
                .iter()
                .rposition(|n| matches!(n.role, Role::Tick | Role::Repeat)),
            first_unjudged: 0,
            nested,
            head_lifetime_end: f64::INFINITY,
            head_hit_action: None,
            tracking: false,
            drawable_tracking: false,
            time_to_accept_any_key_after: None,
            last_pressed: Actions::NONE,
            mouse: None,
        }
    }

    /// The number of judgements the slider produces: its nested objects and itself.
    pub(super) fn judgement_count(&self) -> usize {
        self.nested.len() + 1
    }

    pub(super) fn end_time(&self) -> f64 {
        self.end_time
    }

    pub(super) fn head_judged(&self) -> bool {
        self.nested[self.head].judged()
    }

    pub(super) fn tail_judged(&self) -> bool {
        self.nested[self.tail].judged()
    }

    /// The head's hit state time and result, once judged: the judgement time clamped to the
    /// end of its miss window (`JudgementResult.TimeAbsolute`).
    pub(super) fn head_judgement(&self) -> Option<(f64, HitResult)> {
        let n = &self.nested[self.head];
        n.judged()
            .then(|| (n.time.min(n.start_time + MISS_WINDOW), n.result))
    }

    /// The tail's hit state time and result, once judged. A tail has no hit windows, so its
    /// judgement time is clamped to its start time.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SliderEndCircle.cs (CreateHitWindows)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Judgements/JudgementResult.cs (TimeAbsolute)
    pub(super) fn tail_judgement(&self) -> Option<(f64, HitResult)> {
        self.nested_judgement(self.tail)
    }

    /// The hit state time and result of nested object `k`, once judged. Ticks, repeats and the
    /// tail have no hit windows: their judgement time is clamped to their start time.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SliderTick.cs (CreateHitWindows)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SliderEndCircle.cs (CreateHitWindows)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Judgements/JudgementResult.cs (TimeAbsolute)
    fn nested_judgement(&self, k: usize) -> Option<(f64, HitResult)> {
        if k == self.head {
            return self.head_judgement();
        }
        let n = &self.nested[k];
        n.judged().then(|| (n.time.min(n.start_time), n.result))
    }

    /// The hit state times and results of the nested objects, in the beatmap's order
    /// (`Slider::nested`); `None` for those not judged yet.
    pub(super) fn nested_judgements(&self) -> impl Iterator<Item = Option<(f64, HitResult)>> + '_ {
        (0..self.nested.len()).map(|k| self.nested_judgement(k))
    }

    /// Whether the tail is judged as a hit.
    pub(super) fn tail_hit(&self) -> bool {
        self.nested[self.tail].result.is_hit()
    }

    /// Tracking as the slider drawable saw it at its last update (see `drawable_tracking`).
    pub(super) fn drawable_tracking(&self) -> bool {
        self.drawable_tracking
    }

    /// Records the tracking the drawable sees in this update, before the input manager
    /// updates; call it first in every update of a live slider.
    pub(super) fn begin_update(&mut self) {
        self.drawable_tracking = self.tracking;
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/SliderInputManager.cs (updateTracking)
    /// An update at `time` of a slider already judged, which is otherwise not updated: its
    /// input manager still is, and tracking needs `!Judged || Time <= EndTime`. A slider is
    /// judged at its end time or later, so the cursor and actions of its judgement update
    /// stand for those of this one. They differ only for an update at exactly the end time
    /// after a judgement at that same time, where lazer would check the cursor and actions
    /// again.
    pub(super) fn update_judged(&mut self, time: f64) {
        self.begin_update();
        self.tracking = self.tracking && time <= self.end_time;
    }

    /// Whether the head drawable is alive at `time`, given that the slider is.
    pub(super) fn head_alive(&self, time: f64) -> bool {
        time < self.head_lifetime_end
    }

    /// The best result of the head.
    pub(super) fn head_max_result(&self) -> HitResult {
        self.nested[self.head].max_result
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderHead.cs (ResultFor)
    /// The head's result for a press with the hit circle `result`. With classic behaviour, a
    /// head is fully hit within any window and missed outside, so a press never goes unjudged
    /// for being too early.
    pub(super) fn head_result_for(&self, result: HitResult) -> HitResult {
        if !self.classic {
            return result;
        }
        if result.is_hit() {
            HitResult::LargeTickHit
        } else {
            HitResult::LargeTickMiss
        }
    }

    /// Records the first action that pressed on the head (`HitAction ??= e.Action`).
    pub(super) fn set_head_hit_action(&mut self, action: Actions) {
        self.head_hit_action.get_or_insert(action);
    }

    /// The slider receives the cursor position.
    pub(super) fn set_mouse(&mut self, position: Vec2) {
        self.mouse = Some(position);
    }

    /// Judges the head and sets its lifetime: `lifetime` after its hit state time, the
    /// judgement time clamped to the end of its miss window (`JudgementResult.TimeAbsolute`).
    pub(super) fn judge_head(
        &mut self,
        index: usize,
        time: f64,
        result: HitResult,
        lifetime: f64,
        log: &mut JudgementLog,
    ) {
        self.head_lifetime_end = time.min(self.start_time + MISS_WINDOW) + lifetime;
        self.judge_nested(index, self.head, time, result, log);
    }

    fn judge_nested(
        &mut self,
        index: usize,
        k: usize,
        time: f64,
        result: HitResult,
        log: &mut JudgementLog,
    ) {
        let n = &mut self.nested[k];
        debug_assert!(!n.judged(), "nested object judged twice");
        debug_assert!(
            result == n.max_result || result == n.max_result.min_result() || n.role == Role::Head,
            "invalid result for a nested object"
        );
        n.result = result;
        n.time = time;
        log.apply(Judgement {
            object: ObjectRef {
                index: index as u32,
                nested: Some(k as u32),
            },
            time,
            result,
            max_result: n.max_result,
        });
    }

    /// `HitForcefully` (`hit`) or `MissForcefully` on nested object `k`.
    fn judge_nested_forcefully(
        &mut self,
        index: usize,
        k: usize,
        time: f64,
        hit: bool,
        log: &mut JudgementLog,
    ) {
        let max = self.nested[k].max_result;
        let result = if hit { max } else { max.min_result() };
        self.judge_nested(index, k, time, result, log);
    }

    /// The result and best result of the slider itself, once its tail is judged.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (CheckForResult)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Slider.cs (CreateJudgement)
    pub(super) fn result(&self) -> (HitResult, HitResult) {
        if self.classic {
            // Judged proportionally to the number of nested objects hit, as osu!stable does.
            let total_ticks = self.nested.len();
            let hit_ticks = self.nested.iter().filter(|n| n.result.is_hit()).count();
            let result = if hit_ticks == total_ticks {
                HitResult::Great
            } else if hit_ticks == 0 {
                HitResult::Miss
            } else {
                // Counts fit in an i32 in lazer, so the conversion is exact.
                let hit_fraction = hit_ticks as f64 / total_ticks as f64;
                if hit_fraction >= 0.5 {
                    HitResult::Ok
                } else {
                    HitResult::Meh
                }
            };
            (result, HitResult::Great)
        } else {
            // Only the nested objects count; the slider is judged for its hit or miss state.
            let result = if self.nested.iter().any(|n| n.result.is_hit()) {
                HitResult::IgnoreHit
            } else {
                HitResult::IgnoreMiss
            };
            (result, HitResult::IgnoreHit)
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/SliderInputManager.cs (PostProcessHeadJudgement)
    /// After the head is judged at `time` with `held` actions: when hit late, judges the nested
    /// objects already passed, all hit if the cursor is within the expanded follow area of every
    /// one of them, all missed otherwise; then updates tracking.
    pub(super) fn post_process_head_judgement(
        &mut self,
        index: usize,
        time: f64,
        held: Actions,
        log: &mut JudgementLog,
    ) {
        if !self.nested[self.head].result.is_hit() {
            return;
        }
        if !self.is_mouse_in_follow_area(true, time) {
            return;
        }
        // The cursor is known, as it is in the follow area.
        let Some(mouse) = self.mouse else {
            return;
        };
        let mouse_position_in_slider = mouse - self.position;

        let mut all_ticks_in_range = true;
        for n in &self.nested {
            if n.judged() {
                continue;
            }
            if n.start_time > time {
                break;
            }
            let radius = self.follow_radius(true);
            let object_progress =
                ((n.start_time - self.start_time) / self.duration).clamp(0.0, 1.0);
            let object_position = curve_position_at(&self.path, self.span_count, object_progress);
            // Tracking to the nested object farthest out may have required leaving the area.
            if length_squared(object_position - mouse_position_in_slider) > radius * radius {
                all_ticks_in_range = false;
                break;
            }
        }

        for k in 0..self.nested.len() {
            let n = &self.nested[k];
            if n.judged() {
                continue;
            }
            if n.start_time > time {
                break;
            }
            self.judge_nested_forcefully(index, k, time, all_ticks_in_range, log);
        }

        // With every nested object hit, track the full extent; otherwise tracking would have
        // broken at some point, so it needs the cursor within the ball.
        let valid_position = all_ticks_in_range || self.is_mouse_in_follow_area(false, time);
        // The slider cannot be judged before its head.
        self.update_tracking(valid_position, time, held, false);
    }

    /// The slider's `SliderInputManager` and nested objects update at `time`, in drawable order:
    /// tracking first, then ticks, repeats and the tail. `judged` is whether the slider is.
    pub(super) fn update_nested(
        &mut self,
        index: usize,
        time: f64,
        held: Actions,
        judged: bool,
        log: &mut JudgementLog,
    ) {
        let valid_position = self.is_mouse_in_follow_area(self.tracking, time);
        self.update_tracking(valid_position, time, held, judged);

        // Only the objects from the first unjudged one up to those the tail leniency reaches
        // can be judged now; nested objects are sorted by start time.
        while self
            .nested
            .get(self.first_unjudged)
            .is_some_and(Nested::judged)
        {
            self.first_unjudged += 1;
        }
        let window_end = self.first_unjudged
            + self.nested[self.first_unjudged..]
                .iter()
                // Not "less than", like lazer's checks, so that a NaN time stays in the window.
                .take_while(|n| {
                    (time - n.start_time).partial_cmp(&TAIL_LENIENCY) != Some(Ordering::Less)
                })
                .count();

        // The tail container comes after ticks and repeats so that the tail is judged last.
        for role in [Role::Tick, Role::Repeat, Role::Tail] {
            for k in self.first_unjudged..window_end {
                if self.nested[k].role == role && !self.nested[k].judged() {
                    self.try_judge_nested_object(index, k, time, log);
                }
            }
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/SliderInputManager.cs (TryJudgeNestedObject)
    fn try_judge_nested_object(
        &mut self,
        index: usize,
        k: usize,
        time: f64,
        log: &mut JudgementLog,
    ) {
        let time_offset = time - self.nested[k].start_time;
        match self.nested[k].role {
            Role::Repeat | Role::Tick => {
                if time_offset < 0.0 {
                    return;
                }
            }
            Role::Tail => {
                if time_offset < TAIL_LENIENCY {
                    return;
                }
                // The leniency must not let the tail be judged before the last tick or repeat.
                if let Some(last) = self.last_tick
                    && !self.nested[last].judged()
                {
                    return;
                }
            }
            Role::Head => return,
        }

        if !self.head_judged() {
            return;
        }

        if self.tracking {
            self.judge_nested_forcefully(index, k, time, true, log);
        } else if time_offset >= 0.0 {
            self.judge_nested_forcefully(index, k, time, false, log);
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/SliderInputManager.cs (IsMouseInFollowArea)
    /// Whether the cursor is in the follow area at `time`; `expanded` tests against the largest
    /// follow circle.
    fn is_mouse_in_follow_area(&self, expanded: bool, time: f64) -> bool {
        let Some(mouse) = self.mouse else {
            return false;
        };
        let radius = self.follow_radius(expanded);
        let follow_progress = ((time - self.start_time) / self.duration).clamp(0.0, 1.0);
        let follow_circle_position =
            curve_position_at(&self.path, self.span_count, follow_progress);
        let mouse_position_in_slider = mouse - self.position;
        length_squared(mouse_position_in_slider - follow_circle_position) <= radius * radius
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/SliderInputManager.cs (getFollowRadius)
    fn follow_radius(&self, expanded: bool) -> f32 {
        let mut radius = self.radius;
        if expanded {
            radius *= FOLLOW_AREA;
        }
        radius
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/SliderInputManager.cs (updateTracking)
    /// Updates tracking at `time` with `held` actions. `judged` is whether the slider is.
    fn update_tracking(
        &mut self,
        is_valid_tracking_position: bool,
        time: f64,
        held: Actions,
        judged: bool,
    ) {
        // From the moment the head is pressed on, there is an action to restrict tracking to.
        let head_circle_hit_action = self.head_hit_action;

        if head_circle_hit_action.is_none() {
            self.time_to_accept_any_key_after = None;
        }

        // Tracking needs the head's action until the other action has been released; holding
        // one action from before the slider and tapping the other must not track the slider.
        if let Some(hit_action) = head_circle_hit_action
            && self.time_to_accept_any_key_after.is_none()
        {
            let other_key = if hit_action == Actions::RIGHT {
                Actions::LEFT
            } else {
                Actions::RIGHT
            };
            if !self.last_pressed.contains(other_key) {
                self.time_to_accept_any_key_after = Some(time);
            }
        }

        self.last_pressed = held;
        let valid_tracking_action = Actions::SINGLE
            .iter()
            .any(|&action| held.contains(action) && self.is_valid_tracking_action(action, time));

        // A slider past its end time may still be waiting for judgements, so tracking only
        // stops for good once it is judged.
        self.tracking = (!judged || time <= self.end_time)
            && is_valid_tracking_position
            && valid_tracking_action;
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/SliderInputManager.cs (isValidTrackingAction)
    fn is_valid_tracking_action(&self, action: Actions, time: f64) -> bool {
        if let Some(hit_action) = self.head_hit_action
            && self
                .time_to_accept_any_key_after
                .is_none_or(|after| time <= after)
        {
            return action == hit_action;
        }
        action == Actions::LEFT || action == Actions::RIGHT
    }
}
