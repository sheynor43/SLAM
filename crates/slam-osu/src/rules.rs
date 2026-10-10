//! The osu!standard object rules: hit circles and slider heads judged by position and time,
//! under a hit policy (note lock); slider ticks, repeats and tails judged by tracking.
//!
//! Lazer judges objects through drawables. Here every top-level object keeps the little state
//! lazer's drawables and lifetime entries hold: its lifetime, whether it is judged, and the
//! data the hit area and the hit policies read. Sliders keep theirs in [`slider`].
//!
//! Spinners are not judged yet (#51): they count as judged, without a judgement, once their end
//! time passes. Results on beatmaps with spinners are therefore not lazer's.

mod slider;

use slam_formats::osu::Vec2;

use crate::beatmap::Beatmap;
use crate::hit_windows::{HitWindows, MISS_WINDOW};
use crate::judgement::{HitResult, Judgement, JudgementLog, ObjectRef};
use crate::mods::{Mod, ModSet};
use crate::objects::{OBJECT_RADIUS, OsuHitObjectKind};
use crate::replay::{ActionEvent, Actions, Rules, StepInput};

use slider::SliderState;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (UpdateHitStateTransforms)
// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/Transformable.cs (LatestTransformEndTime)
/// How long a hit circle stays alive after a hit: its alpha fades out 800 ms after the judgement,
/// and the lifetime ends 1 ms after the last transform. Assumes lazer's hit animations setting
/// is on (the default); with it off, a 60 ms fade-out replaces the delayed one.
const HIT_LIFETIME: f64 = 800.0 + 1.0;
/// How long a circle stays alive after a miss: the 100 ms miss fade-out replaces the delayed one.
const MISS_LIFETIME: f64 = 100.0 + 1.0;

/// How long a hit circle or slider head stays alive after a judgement with `result`.
fn circle_lifetime(result: HitResult) -> f64 {
    if result.is_hit() {
        HIT_LIFETIME
    } else {
        MISS_LIFETIME
    }
}

/// Lazer's `ClickAction`: what a press on an object does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickAction {
    /// Nothing happens.
    Ignore,
    /// The object shakes and is not judged.
    Shake,
    /// The object is judged.
    Hit,
}

/// Which presses may judge an object while other objects are still pending (note lock).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HitPolicy {
    /// Lazer's default (`StartTimeOrderedHitPolicy`): an object cannot be hit before the start
    /// time of an earlier, unjudged circle or slider head; hitting one misses every earlier
    /// unjudged one.
    StartTimeOrdered,
    /// Classic's note lock (`LegacyHitPolicy`): an object cannot be hit while an earlier object
    /// that ends before it is unjudged, nor further than `hittable_range` from its start time.
    Legacy {
        /// The farthest a hit may be from the object's start time, exclusive.
        hittable_range: f64,
    },
}

impl HitPolicy {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModClassic.cs (ApplyToDrawableRuleset)
    /// The policy a set of mods selects: Classic's note lock if enabled, otherwise the default.
    pub fn for_mods(mods: &ModSet) -> HitPolicy {
        if !RulesOptions::classic(mods).is_some_and(|c| c.classic_note_lock) {
            return HitPolicy::StartTimeOrdered;
        }
        let autopilot = mods.get("AP").is_some();
        HitPolicy::Legacy {
            hittable_range: MISS_WINDOW - if autopilot { 200.0 } else { 0.0 },
        }
    }
}

/// The settings of the rules that mods change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RulesOptions {
    /// The hit policy.
    pub policy: HitPolicy,
    /// Lazer's `Slider.ClassicSliderBehaviour` (Classic's `NoSliderHeadAccuracy`): heads give
    /// a large tick, tails a small tick, and the slider itself a result in proportion to the
    /// nested objects hit.
    pub classic_slider_behaviour: bool,
    /// Classic's note lock on slider heads: a head that has been hit keeps taking presses away
    /// from objects under it until its slider is judged.
    pub block_input_under_slider_heads: bool,
}

impl RulesOptions {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModClassic.cs (ApplyToHitObject, ApplyToDrawableHitObject)
    /// The options a set of mods selects.
    pub fn for_mods(mods: &ModSet) -> RulesOptions {
        let classic = RulesOptions::classic(mods);
        RulesOptions {
            policy: HitPolicy::for_mods(mods),
            classic_slider_behaviour: classic.is_some_and(|c| c.no_slider_head_accuracy),
            block_input_under_slider_heads: classic.is_some_and(|c| c.classic_note_lock),
        }
    }

    /// Options with `policy` and lazer's default slider behaviour.
    pub fn with_policy(policy: HitPolicy) -> RulesOptions {
        RulesOptions {
            policy,
            classic_slider_behaviour: false,
            block_input_under_slider_heads: false,
        }
    }

    fn classic(mods: &ModSet) -> Option<&crate::mods::ClassicSettings> {
        mods.mods().iter().find_map(|m| match m {
            Mod::Classic(c) => Some(c),
            _ => None,
        })
    }
}

/// Whether a press at `position` lands on a hit circle drawn at `centre` with `scale`.
///
/// Lazer's `HitReceptor` is a 128×128 box with a corner radius of 64 inside the circle's scale
/// container, so the test is a distance check in its unscaled local space. The playfield to
/// screen space round trip is not reproduced (ADR-0028).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (HitReceptor)
// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Containers/CompositeDrawable.cs (Contains, effectiveCornerRadius)
fn hit_area_contains(centre: Vec2, scale: f32, position: Vec2) -> bool {
    const CORNER_EXPONENT: f32 = 2.0;
    let corner_radius = OBJECT_RADIUS * 0.8 * CORNER_EXPONENT / 2.0 + 0.2 * OBJECT_RADIUS;

    // The box shrunk by the corner radius is the single point at its centre.
    let local = (position - centre) / scale;
    // `MathF.Pow(d, 2)` is the correctly rounded square, so a product, not the platform's powf.
    let (dx, dy) = (local.x.abs(), local.y.abs());
    let distance = dx * dx + dy * dy;
    f64::from(distance) <= f64::from(corner_radius) * f64::from(corner_radius)
}

/// The type of a top-level object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Circle,
    /// A slider, with its index into `OsuRules::sliders`.
    Slider(usize),
    Spinner,
}

/// The state of one top-level object.
#[derive(Debug, Clone, PartialEq)]
struct ObjectState {
    start_time: f64,
    end_time: f64,
    /// Lifetime start: the start time minus the preempt.
    lifetime_start: f64,
    /// Stacked position: where the object is drawn.
    position: Vec2,
    scale: f32,
    stack_height: i32,
    kind: Kind,
    /// Whether the object itself is judged; a slider is judged after all its nested objects
    /// (a spinner: whether its end time has passed).
    judged: bool,
    /// Lifetime end. Lazer's lifetime entry starts with the end of the miss window, so an
    /// object skipped over never comes alive; once alive and idle, its drawable keeps it alive
    /// indefinitely; a judgement sets the end of the judged transforms.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuPlayfield.cs (OsuHitObjectLifetimeEntry)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Drawables/DrawableHitObject.cs (UpdateState)
    lifetime_end: f64,
}

impl ObjectState {
    /// Whether the object is alive at `time`, as osu-framework's lifetime manager decides.
    fn is_alive(&self, time: f64) -> bool {
        self.lifetime_start <= time && time < self.lifetime_end
    }
}

/// The osu!standard rules of a play.
///
/// Within an update, presses are handled first against the objects alive at the previous
/// update, as lazer handles input before its playfield updates object lifetimes; then objects
/// alive at the new time are updated (missed once their window has passed, sliders tracked),
/// latest first, as lazer's playfield draws earlier objects on top and updates its children back
/// to front.
#[derive(Debug, Clone, PartialEq)]
pub struct OsuRules {
    objects: Vec<ObjectState>,
    sliders: Vec<SliderState>,
    hit_windows: HitWindows,
    options: RulesOptions,
    /// Time of the last update.
    time: f64,
    /// Actions held after the last update.
    held: Actions,
    /// Objects before this index are dead for good: judged and expired, or never judged.
    head: usize,
    /// Objects not judged yet.
    remaining: usize,
    /// Objects stepped over before they came alive, never to be judged.
    skipped: usize,
}

impl OsuRules {
    /// Rules for a beatmap played with `mods` (the beatmap must already have them applied).
    /// Panics like [`OsuRules::with_options`].
    pub fn new(beatmap: &Beatmap, mods: &ModSet) -> OsuRules {
        OsuRules::with_options(beatmap, RulesOptions::for_mods(mods))
    }

    /// Rules with an explicit hit policy and lazer's default slider behaviour.
    pub fn with_policy(beatmap: &Beatmap, policy: HitPolicy) -> OsuRules {
        OsuRules::with_options(beatmap, RulesOptions::with_policy(policy))
    }

    /// Rules with explicit options.
    ///
    /// # Panics
    ///
    /// If a slider has no nested head or tail: the beatmap's objects must have their defaults
    /// applied ([`crate::objects::OsuHitObject::apply_defaults`]), as [`Beatmap::from_file`]
    /// does.
    pub fn with_options(beatmap: &Beatmap, options: RulesOptions) -> OsuRules {
        let mut sliders = Vec::new();
        let objects: Vec<ObjectState> = beatmap
            .hit_objects
            .iter()
            .map(|o| {
                let kind = match &o.kind {
                    OsuHitObjectKind::Circle => Kind::Circle,
                    OsuHitObjectKind::Slider(s) => {
                        sliders.push(SliderState::new(o, s, options.classic_slider_behaviour));
                        Kind::Slider(sliders.len() - 1)
                    }
                    OsuHitObjectKind::Spinner { .. } => Kind::Spinner,
                };
                // Sliders and spinners have empty hit windows: their miss window is zero.
                let miss_window = if kind == Kind::Circle {
                    MISS_WINDOW
                } else {
                    0.0
                };
                ObjectState {
                    start_time: o.start_time,
                    end_time: o.end_time(),
                    lifetime_start: o.start_time - o.defaults.time_preempt,
                    position: o.stacked_position(),
                    scale: o.defaults.scale,
                    stack_height: o.stack_height,
                    kind,
                    judged: false,
                    lifetime_end: o.end_time() + miss_window,
                }
            })
            .collect();
        // Every object has the same preempt, so lifetimes start in start time order, which
        // `live_range` relies on. Mods that change the preempt per object (Freeze Frame) are not
        // supported.
        debug_assert!(
            objects
                .windows(2)
                .all(|w| w[0].lifetime_start <= w[1].lifetime_start)
        );
        OsuRules {
            remaining: objects.len(),
            objects,
            sliders,
            hit_windows: HitWindows::new(beatmap.difficulty.overall_difficulty),
            options,
            time: f64::NEG_INFINITY,
            held: Actions::NONE,
            head: 0,
            skipped: 0,
        }
    }

    /// The hit policy in use.
    pub fn policy(&self) -> HitPolicy {
        self.options.policy
    }

    /// The options in use.
    pub fn options(&self) -> RulesOptions {
        self.options
    }

    /// The number of objects the play stepped over before they came alive (a replay starting
    /// after them). Lazer never judges these and never completes the play; here they count as
    /// done, without a judgement, so that the simulation ends.
    pub fn skipped_objects(&self) -> usize {
        self.skipped
    }

    /// The hit windows of circles and slider heads.
    pub fn hit_windows(&self) -> &HitWindows {
        &self.hit_windows
    }

    /// The live objects that may be alive at `time`, in start time order: lifetimes start in
    /// start time order, since every object has the same preempt.
    fn live_range(&self, time: f64) -> std::ops::Range<usize> {
        let end = self.head
            + self.objects[self.head..]
                .iter()
                .take_while(|o| o.lifetime_start <= time)
                .count();
        self.head..end
    }

    /// Whether object `i` blocks later hits under the default policy: a circle, or a slider
    /// through its head (lazer's `DrawableHitCircle`s).
    fn is_blocking(&self, i: usize) -> bool {
        matches!(self.objects[i].kind, Kind::Circle | Kind::Slider(_))
    }

    /// Whether the circle that object `i` blocks with is judged (see [`Self::is_blocking`]).
    fn blocking_judged(&self, i: usize) -> bool {
        match self.objects[i].kind {
            Kind::Slider(s) => self.sliders[s].head_judged(),
            _ => self.objects[i].judged,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (HitReceptor.OnPressed)
    /// Handles one press, with `held` the actions held after it. The earliest alive circle or
    /// slider head under the cursor that can still be hit takes it, whether it is then judged
    /// or not.
    fn press(&mut self, event: &ActionEvent, held: Actions, log: &mut JudgementLog) {
        if !(event.action == Actions::LEFT || event.action == Actions::RIGHT) {
            return;
        }
        let alive_time = self.time;
        for i in self.live_range(alive_time) {
            let o = &self.objects[i];
            if !o.is_alive(alive_time) {
                continue;
            }
            let can_be_hit = match o.kind {
                Kind::Circle => !o.judged,
                Kind::Slider(s) => {
                    let slider = &self.sliders[s];
                    slider.head_alive(alive_time)
                        && if self.options.block_input_under_slider_heads {
                            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModClassic.cs (blockInputToObjectsUnderSliderHead)
                            !o.judged
                        } else {
                            !slider.head_judged()
                        }
                }
                Kind::Spinner => false,
            };
            if !can_be_hit || !hit_area_contains(o.position, o.scale, event.position) {
                continue;
            }
            match o.kind {
                Kind::Slider(s) => self.press_head(i, s, event, held, alive_time, log),
                _ => self.try_hit(i, event.time, alive_time, log),
            }
            return;
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (CheckForResult, userTriggered)
    /// A press on circle `i` at `time`: judged if the time is within its windows and the policy
    /// allows it.
    fn try_hit(&mut self, i: usize, time: f64, alive_time: f64, log: &mut JudgementLog) {
        let result = self
            .hit_windows
            .result_for(time - self.objects[i].start_time);
        let action = self.check_hittable(i, false, time, result, alive_time);
        if result == HitResult::None || action != ClickAction::Hit {
            return;
        }
        self.judge(i, time, result, alive_time, log);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (CheckForResult, HitReceptor.OnPressed)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderHead.cs (CheckForResult, ResultFor)
    /// A press on the head of slider `i` (state `s`): judged like a circle, then the slider
    /// catches up with the nested objects already passed. The press sets the head's action
    /// only after that, so the tracking update of a first press accepts either action.
    fn press_head(
        &mut self,
        i: usize,
        s: usize,
        event: &ActionEvent,
        held: Actions,
        alive_time: f64,
        log: &mut JudgementLog,
    ) {
        // The cursor moves before the press reaches the slider.
        self.sliders[s].set_mouse(event.position);
        if !self.sliders[s].head_judged() {
            let time = event.time;
            let result = self.sliders[s].head_result_for(
                self.hit_windows
                    .result_for(time - self.objects[i].start_time),
            );
            let action = self.check_hittable(i, true, time, result, alive_time);
            if result != HitResult::None && action == ClickAction::Hit {
                self.judge_head(i, s, time, result, alive_time, log);
                self.sliders[s].post_process_head_judgement(i, time, held, log);
            }
        }
        self.sliders[s].set_head_hit_action(event.action);
    }

    /// Judges circle `i`: the policy reacts first, then the judgement is recorded. Lazer's
    /// playfield subscribes to new results before the ruleset that forwards them to scoring, so
    /// the misses `HandleHit` forces reach the score before the result that forced them.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuPlayfield.cs (constructor, onNewResult)
    fn judge(
        &mut self,
        i: usize,
        time: f64,
        result: HitResult,
        alive_time: f64,
        log: &mut JudgementLog,
    ) {
        self.mark_judged(i, time, circle_lifetime(result));
        self.handle_hit(i, time, alive_time, log);
        self.record(i, time, result, HitResult::Great, log);
    }

    /// Judges the head of slider `i` (state `s`), like [`Self::judge`] for a circle.
    fn judge_head(
        &mut self,
        i: usize,
        s: usize,
        time: f64,
        result: HitResult,
        alive_time: f64,
        log: &mut JudgementLog,
    ) {
        // The policy only looks at objects before this one, so it may react before the head is
        // marked; its misses are recorded first, as for circles.
        self.handle_hit(i, time, alive_time, log);
        self.sliders[s].judge_head(i, time, result, circle_lifetime(result), log);
    }

    /// Marks object `i` judged at `time`; it stays alive `lifetime` after its hit state time.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Judgements/JudgementResult.cs (TimeAbsolute)
    fn mark_judged(&mut self, i: usize, time: f64, lifetime: f64) {
        let o = &mut self.objects[i];
        debug_assert!(!o.judged, "object judged twice");
        o.judged = true;
        // The judged transforms start at the judgement time, clamped to the end time plus the
        // object's miss window (none for sliders and spinners).
        let hit_state_time = match o.kind {
            Kind::Circle => time.min(o.end_time + MISS_WINDOW),
            Kind::Slider(_) | Kind::Spinner => time.min(o.end_time),
        };
        o.lifetime_end = hit_state_time + lifetime;
        self.remaining -= 1;
    }

    fn record(
        &mut self,
        i: usize,
        time: f64,
        result: HitResult,
        max_result: HitResult,
        log: &mut JudgementLog,
    ) {
        log.apply(Judgement {
            object: ObjectRef {
                index: i as u32,
                nested: None,
            },
            time,
            result,
            max_result,
        });
    }

    /// Lazer's `IHitPolicy.CheckHittable`: whether a press at `time` with `result` may judge
    /// object `i` (its head if `head`), against the objects alive at `alive_time`.
    fn check_hittable(
        &self,
        i: usize,
        head: bool,
        time: f64,
        result: HitResult,
        alive_time: f64,
    ) -> ClickAction {
        match self.options.policy {
            HitPolicy::StartTimeOrdered => {
                self.check_start_time_ordered(i, time, result, alive_time)
            }
            HitPolicy::Legacy { hittable_range } => {
                self.check_legacy(i, head, time, result, alive_time, hittable_range)
            }
        }
    }

    /// The alive objects starting before object `i`, in start time order (lazer's
    /// `enumerateHitObjectsUpTo`, which also yields the heads of these objects' sliders).
    fn alive_before(&self, i: usize, alive_time: f64) -> impl Iterator<Item = usize> + '_ {
        let target = self.objects[i].start_time;
        self.live_range(alive_time)
            .take_while(move |&j| self.objects[j].start_time < target)
            .filter(move |&j| self.objects[j].is_alive(alive_time))
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/StartTimeOrderedHitPolicy.cs (CheckHittable)
    fn check_start_time_ordered(
        &self,
        i: usize,
        time: f64,
        result: HitResult,
        alive_time: f64,
    ) -> ClickAction {
        // Only circles and slider heads block later hits. A head blocks while its slider is
        // alive, whether the head itself still is or not.
        let blocking = self
            .alive_before(i, alive_time)
            .filter(|&j| self.is_blocking(j))
            .last();

        if let Some(b) = blocking {
            // Hits at exactly the blocking object's start time are allowed, for simultaneous
            // objects.
            if !self.blocking_judged(b) && time < self.objects[b].start_time {
                return ClickAction::Shake;
            }
        }

        if result == HitResult::None {
            return ClickAction::Shake;
        }

        ClickAction::Hit
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/LegacyHitPolicy.cs (CheckHittable)
    /// A slider head (`head`) is not among the alive top-level objects, so it skips the stacking
    /// check and is tested against every alive object.
    fn check_legacy(
        &self,
        i: usize,
        head: bool,
        time: f64,
        result: HitResult,
        alive_time: f64,
        hittable_range: f64,
    ) -> ClickAction {
        let target = &self.objects[i];
        let alive = || {
            self.live_range(alive_time)
                .filter(move |&j| self.objects[j].is_alive(alive_time))
        };

        // An unjudged stacked object directly before swallows the press.
        if !head && let Some(previous) = alive().take_while(|&j| j != i).last() {
            let previous = &self.objects[previous];
            if previous.stack_height > 0 && !previous.judged {
                return ClickAction::Ignore;
            }
        }

        if result == HitResult::None {
            return ClickAction::Shake;
        }

        for j in alive() {
            let other = &self.objects[j];
            if other.judged {
                continue;
            }
            if !head && j == i {
                break;
            }
            // 3 ms of leniency for slightly unsnapped objects.
            if other.end_time + 3.0 < target.start_time {
                return ClickAction::Shake;
            }
        }

        if (target.start_time - time).abs() < hittable_range {
            ClickAction::Hit
        } else {
            ClickAction::Shake
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/StartTimeOrderedHitPolicy.cs (HandleHit)
    /// Lazer's `IHitPolicy.HandleHit` after the circle or slider head of object `i` is judged.
    /// The default policy misses every earlier unjudged circle and head; their own `HandleHit`
    /// finds nothing left to miss.
    fn handle_hit(&mut self, i: usize, time: f64, alive_time: f64, log: &mut JudgementLog) {
        if self.options.policy != HitPolicy::StartTimeOrdered || !self.is_blocking(i) {
            return;
        }
        let target = self.objects[i].start_time;
        for j in self.live_range(alive_time) {
            let o = &self.objects[j];
            if o.start_time >= target {
                break;
            }
            if !o.is_alive(alive_time) || self.blocking_judged(j) {
                continue;
            }
            match o.kind {
                Kind::Circle => {
                    self.mark_judged(j, time, MISS_LIFETIME);
                    self.record(j, time, HitResult::Miss, HitResult::Great, log);
                }
                Kind::Slider(s) => {
                    let slider = &mut self.sliders[s];
                    let miss = slider.head_max_result().min_result();
                    slider.judge_head(j, time, miss, MISS_LIFETIME, log);
                }
                Kind::Spinner => {}
            }
        }
    }

    /// Updates the objects alive at `time` with `held` actions: circles and slider heads past
    /// their windows are missed, sliders track and judge their nested objects and then
    /// themselves, spinners past their end time count as judged.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (CheckForResult, not userTriggered)
    fn update_objects(&mut self, time: f64, held: Actions, log: &mut JudgementLog) {
        while let Some(o) = self.objects.get(self.head)
            && o.lifetime_end <= time
        {
            if !o.judged {
                // Stepped over before it came alive: lazer never judges it and never completes.
                // Count it out so that the simulation can end.
                self.skipped += 1;
                self.remaining -= 1;
            }
            self.head += 1;
        }

        for i in self.live_range(time).rev() {
            let o = &mut self.objects[i];
            if o.judged || !o.is_alive(time) {
                continue;
            }
            // An alive, idle drawable has no lifetime end.
            o.lifetime_end = f64::INFINITY;
            match o.kind {
                Kind::Circle => {
                    if !self.hit_windows.can_be_hit(time - o.start_time) {
                        self.judge(i, time, HitResult::Miss, time, log);
                    }
                }
                Kind::Slider(s) => self.update_slider(i, s, time, held, log),
                Kind::Spinner => {
                    if time >= o.end_time {
                        // Placeholder until spinners are judged.
                        self.mark_judged(i, time, HIT_LIFETIME);
                    }
                }
            }
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (load, CheckForResult)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderHead.cs (CheckForResult)
    /// Updates alive, unjudged slider `i` (state `s`) in its drawable order: the input manager
    /// and nested objects, then the head (missed once its window has passed), then the slider
    /// itself, judged once its tail is and its end time has passed.
    fn update_slider(
        &mut self,
        i: usize,
        s: usize,
        time: f64,
        held: Actions,
        log: &mut JudgementLog,
    ) {
        self.sliders[s].update_nested(i, time, held, false, log);

        let slider = &self.sliders[s];
        if slider.head_alive(time)
            && !slider.head_judged()
            && !self
                .hit_windows
                .can_be_hit(time - self.objects[i].start_time)
        {
            let miss = slider.head_max_result().min_result();
            self.judge_head(i, s, time, miss, time, log);
        }

        let slider = &self.sliders[s];
        if slider.tail_judged() && time >= slider.end_time() {
            let (result, max_result) = slider.result();
            self.mark_judged(i, time, slider::JUDGED_LIFETIME);
            self.record(i, time, result, max_result, log);
        }
    }
}

impl Rules for OsuRules {
    fn max_judgements(&self) -> usize {
        let circles = self
            .objects
            .iter()
            .filter(|o| o.kind == Kind::Circle)
            .count();
        circles
            + self
                .sliders
                .iter()
                .map(SliderState::judgement_count)
                .sum::<usize>()
    }

    fn update(&mut self, step: &StepInput<'_>, log: &mut JudgementLog) {
        let mut held = self.held;
        for event in step.events {
            // Lazer's key binding container updates the held actions before propagating.
            if event.pressed {
                held = held | event.action;
                self.press(event, held, log);
            } else {
                held = held.difference(event.action);
            }
        }
        self.held = step.held;

        // Sliders alive at the previous update receive the cursor position every update, as
        // osu-framework sends high-frequency mouse positions to its positional input queue.
        let alive_time = self.time;
        for i in self.live_range(alive_time) {
            let o = &self.objects[i];
            if let Kind::Slider(s) = o.kind
                && o.is_alive(alive_time)
            {
                self.sliders[s].set_mouse(step.position);
            }
        }

        self.time = step.time;
        self.update_objects(step.time, step.held, log);
    }

    fn is_complete(&self) -> bool {
        self.remaining == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::mods::ClassicSettings;
    use slam_formats::osu::decode_str;

    fn stacked_circles() -> Beatmap {
        // Three circles stacked at (80, 80), 100 ms apart; OD 0, AR 5 (preempt 1200).
        let text = "osu file format v14\n\n[Difficulty]\nCircleSize:5\nOverallDifficulty:0\n\
                    ApproachRate:5\n\n[TimingPoints]\n0,1000,4,2,0,100,1,0\n\n[HitObjects]\n\
                    80,80,1000,1,0\n80,80,1100,1,0\n80,80,1200,1,0\n";
        Beatmap::from_file(decode_str(text).unwrap().beatmap).unwrap()
    }

    #[test]
    fn policy_follows_classic_note_lock() {
        assert_eq!(
            HitPolicy::for_mods(&ModSet::default()),
            HitPolicy::StartTimeOrdered
        );
        let classic = ModSet::new(vec![Mod::Classic(ClassicSettings::default())]).unwrap();
        assert_eq!(
            HitPolicy::for_mods(&classic),
            HitPolicy::Legacy {
                hittable_range: 400.0
            }
        );
        let no_lock = ModSet::new(vec![Mod::Classic(ClassicSettings {
            classic_note_lock: false,
            ..ClassicSettings::default()
        })])
        .unwrap();
        assert_eq!(HitPolicy::for_mods(&no_lock), HitPolicy::StartTimeOrdered);
        let autopilot = ModSet::new(vec![
            Mod::Classic(ClassicSettings::default()),
            Mod::from_acronym("AP"),
        ])
        .unwrap();
        assert_eq!(
            HitPolicy::for_mods(&autopilot),
            HitPolicy::Legacy {
                hittable_range: 200.0
            }
        );
    }

    #[test]
    fn legacy_ignores_presses_under_an_unjudged_stacked_object() {
        let map = stacked_circles();
        assert!(map.hit_objects[0].stack_height > 0);
        let mut rules = OsuRules::with_policy(
            &map,
            HitPolicy::Legacy {
                hittable_range: 400.0,
            },
        );
        let alive = 500.0;
        assert_eq!(
            rules.check_hittable(1, false, 1050.0, HitResult::Ok, alive),
            ClickAction::Ignore
        );
        // Ignore comes before the early-press check.
        assert_eq!(
            rules.check_hittable(1, false, 1050.0, HitResult::None, alive),
            ClickAction::Ignore
        );
        assert_eq!(
            rules.check_hittable(0, false, 600.0, HitResult::None, alive),
            ClickAction::Shake
        );
        assert_eq!(
            rules.check_hittable(0, false, 1000.0, HitResult::Great, alive),
            ClickAction::Hit
        );
        // Once judged, the stacked object no longer blocks.
        rules.objects[0].judged = true;
        assert_eq!(
            rules.check_hittable(1, false, 1050.0, HitResult::Ok, alive),
            ClickAction::Hit
        );
    }

    #[test]
    fn start_time_ordered_blocks_before_the_last_earlier_circle() {
        let map = stacked_circles();
        let rules = OsuRules::with_policy(&map, HitPolicy::StartTimeOrdered);
        assert_eq!(
            rules.check_hittable(2, false, 1099.0, HitResult::Ok, 500.0),
            ClickAction::Shake
        );
        assert_eq!(
            rules.check_hittable(2, false, 1100.0, HitResult::Ok, 500.0),
            ClickAction::Hit
        );
        assert_eq!(
            rules.check_hittable(0, false, 1000.0, HitResult::None, 500.0),
            ClickAction::Shake
        );
    }

    #[test]
    fn corner_radius_is_the_object_radius() {
        assert_eq!(
            OBJECT_RADIUS * 0.8 * 2.0 / 2.0 + 0.2 * OBJECT_RADIUS,
            OBJECT_RADIUS
        );
    }

    #[test]
    fn hit_area_is_a_circle_of_the_scaled_radius() {
        let c = Vec2::new(100.0, 100.0);
        assert!(hit_area_contains(c, 0.5, Vec2::new(132.0, 100.0)));
        assert!(!hit_area_contains(c, 0.5, Vec2::new(132.01, 100.0)));
        assert!(hit_area_contains(c, 0.5, Vec2::new(122.0, 122.0)));
        assert!(!hit_area_contains(c, 0.5, Vec2::new(123.0, 123.0)));
        assert!(hit_area_contains(c, 1.0, Vec2::new(100.0, 36.0)));
    }
}
