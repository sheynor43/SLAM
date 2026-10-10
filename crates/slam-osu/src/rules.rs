//! The osu!standard object rules: hit circles judged by position and time, under a hit policy
//! (note lock).
//!
//! Lazer judges objects through drawables. Here every top-level object keeps the little state
//! lazer's drawables and lifetime entries hold: its lifetime, whether it is judged, and the
//! data the hit area and the hit policies read.
//!
//! Sliders and spinners are not judged yet (#50, #51): they count as judged, without a
//! judgement, once their end time passes. Results on beatmaps with sliders or spinners are
//! therefore not lazer's.

use slam_formats::osu::Vec2;

use crate::beatmap::Beatmap;
use crate::hit_windows::{HitWindows, MISS_WINDOW};
use crate::judgement::{HitResult, Judgement, JudgementLog, ObjectRef};
use crate::mods::{Mod, ModSet};
use crate::objects::{OBJECT_RADIUS, OsuHitObjectKind};
use crate::replay::{ActionEvent, Actions, Rules, StepInput};

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (UpdateHitStateTransforms)
// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/Transformable.cs (LatestTransformEndTime)
/// How long a hit circle stays alive after a hit: its alpha fades out 800 ms after the judgement,
/// and the lifetime ends 1 ms after the last transform. Assumes lazer's hit animations setting
/// is on (the default); with it off, a 60 ms fade-out replaces the delayed one.
const HIT_LIFETIME: f64 = 800.0 + 1.0;
/// How long a circle stays alive after a miss: the 100 ms miss fade-out replaces the delayed one.
const MISS_LIFETIME: f64 = 100.0 + 1.0;

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
    /// time of an earlier, unjudged circle; hitting a circle misses every earlier unjudged one.
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
        let classic_note_lock = mods
            .mods()
            .iter()
            .any(|m| matches!(m, Mod::Classic(c) if c.classic_note_lock));
        if !classic_note_lock {
            return HitPolicy::StartTimeOrdered;
        }
        let autopilot = mods.get("AP").is_some();
        HitPolicy::Legacy {
            hittable_range: MISS_WINDOW - if autopilot { 200.0 } else { 0.0 },
        }
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
    is_circle: bool,
    /// Whether the object is judged (a slider or spinner: whether its end time has passed).
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
/// alive at the new time are updated (missed once their window has passed), latest first, as
/// lazer's playfield draws earlier objects on top and updates its children back to front.
#[derive(Debug, Clone, PartialEq)]
pub struct OsuRules {
    objects: Vec<ObjectState>,
    hit_windows: HitWindows,
    policy: HitPolicy,
    /// Time of the last update.
    time: f64,
    /// Objects before this index are dead for good: judged and expired, or never judged.
    head: usize,
    /// Objects not judged yet.
    remaining: usize,
    /// Objects stepped over before they came alive, never to be judged.
    skipped: usize,
}

impl OsuRules {
    /// Rules for a beatmap played with `mods` (the beatmap must already have them applied).
    pub fn new(beatmap: &Beatmap, mods: &ModSet) -> OsuRules {
        OsuRules::with_policy(beatmap, HitPolicy::for_mods(mods))
    }

    /// Rules with an explicit hit policy.
    pub fn with_policy(beatmap: &Beatmap, policy: HitPolicy) -> OsuRules {
        let objects: Vec<ObjectState> = beatmap
            .hit_objects
            .iter()
            .map(|o| ObjectState {
                start_time: o.start_time,
                end_time: o.end_time(),
                lifetime_start: o.start_time - o.defaults.time_preempt,
                position: o.stacked_position(),
                scale: o.defaults.scale,
                stack_height: o.stack_height,
                is_circle: matches!(o.kind, OsuHitObjectKind::Circle),
                judged: false,
                lifetime_end: o.end_time() + MISS_WINDOW,
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
            hit_windows: HitWindows::new(beatmap.difficulty.overall_difficulty),
            policy,
            time: f64::NEG_INFINITY,
            head: 0,
            skipped: 0,
        }
    }

    /// The hit policy in use.
    pub fn policy(&self) -> HitPolicy {
        self.policy
    }

    /// The number of objects the play stepped over before they came alive (a replay starting
    /// after them). Lazer never judges these and never completes the play; here they count as
    /// done, without a judgement, so that the simulation ends.
    pub fn skipped_objects(&self) -> usize {
        self.skipped
    }

    /// The hit windows of circles.
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

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (HitReceptor.OnPressed)
    /// Handles one press. The earliest alive, unjudged circle under the cursor takes it, whether
    /// it is then judged or not.
    fn press(&mut self, event: &ActionEvent, log: &mut JudgementLog) {
        if !(event.action == Actions::LEFT || event.action == Actions::RIGHT) {
            return;
        }
        let alive_time = self.time;
        for i in self.live_range(alive_time) {
            let o = &self.objects[i];
            if !o.is_circle || o.judged || !o.is_alive(alive_time) {
                continue;
            }
            if hit_area_contains(o.position, o.scale, event.position) {
                self.try_hit(i, event.time, alive_time, log);
                return;
            }
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (CheckForResult, userTriggered)
    /// A press on circle `i` at `time`: judged if the time is within its windows and the policy
    /// allows it.
    fn try_hit(&mut self, i: usize, time: f64, alive_time: f64, log: &mut JudgementLog) {
        let result = self
            .hit_windows
            .result_for(time - self.objects[i].start_time);
        let action = self.check_hittable(i, time, result, alive_time);
        if result == HitResult::None || action != ClickAction::Hit {
            return;
        }
        self.judge(i, time, result, alive_time, log);
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
        self.mark_judged(i, time, result);
        self.handle_hit(i, time, alive_time, log);
        self.record(i, time, result, log);
    }

    fn mark_judged(&mut self, i: usize, time: f64, result: HitResult) {
        let o = &mut self.objects[i];
        debug_assert!(!o.judged, "object judged twice");
        o.judged = true;
        o.lifetime_end = time
            + if result.is_hit() {
                HIT_LIFETIME
            } else {
                MISS_LIFETIME
            };
        self.remaining -= 1;
    }

    fn record(&mut self, i: usize, time: f64, result: HitResult, log: &mut JudgementLog) {
        log.apply(Judgement {
            object: ObjectRef {
                index: i as u32,
                nested: None,
            },
            time,
            result,
            max_result: HitResult::Great,
        });
    }

    /// Lazer's `IHitPolicy.CheckHittable`: whether a press at `time` with `result` may judge
    /// object `i`, against the objects alive at `alive_time`.
    fn check_hittable(
        &self,
        i: usize,
        time: f64,
        result: HitResult,
        alive_time: f64,
    ) -> ClickAction {
        match self.policy {
            HitPolicy::StartTimeOrdered => {
                self.check_start_time_ordered(i, time, result, alive_time)
            }
            HitPolicy::Legacy { hittable_range } => {
                self.check_legacy(i, time, result, alive_time, hittable_range)
            }
        }
    }

    /// The alive objects starting before object `i`, in start time order (lazer's
    /// `enumerateHitObjectsUpTo`). Slider heads will join them with #50.
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
        // Only circles block later hits.
        let blocking = self
            .alive_before(i, alive_time)
            .filter(|&j| self.objects[j].is_circle)
            .last();

        if let Some(b) = blocking {
            let b = &self.objects[b];
            // Hits at exactly the blocking object's start time are allowed, for simultaneous
            // objects.
            if !b.judged && time < b.start_time {
                return ClickAction::Shake;
            }
        }

        if result == HitResult::None {
            return ClickAction::Shake;
        }

        ClickAction::Hit
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/LegacyHitPolicy.cs (CheckHittable)
    fn check_legacy(
        &self,
        i: usize,
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
        if let Some(previous) = alive().take_while(|&j| j != i).last() {
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
            if j == i {
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
    /// Lazer's `IHitPolicy.HandleHit` after object `i` is judged. The default policy misses
    /// every earlier unjudged circle; their own `HandleHit` finds nothing left to miss.
    fn handle_hit(&mut self, i: usize, time: f64, alive_time: f64, log: &mut JudgementLog) {
        if self.policy != HitPolicy::StartTimeOrdered || !self.objects[i].is_circle {
            return;
        }
        let target = self.objects[i].start_time;
        for j in self.live_range(alive_time) {
            let o = &self.objects[j];
            if o.start_time >= target {
                break;
            }
            if o.is_alive(alive_time) && o.is_circle && !o.judged {
                self.mark_judged(j, time, HitResult::Miss);
                self.record(j, time, HitResult::Miss, log);
            }
        }
    }

    /// Updates the objects alive at `time`: circles past their windows are missed, sliders and
    /// spinners past their end time count as judged.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableHitCircle.cs (CheckForResult, not userTriggered)
    fn update_objects(&mut self, time: f64, log: &mut JudgementLog) {
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
            if o.is_circle {
                if !self.hit_windows.can_be_hit(time - o.start_time) {
                    self.judge(i, time, HitResult::Miss, time, log);
                }
            } else if time >= o.end_time {
                // Placeholder until sliders and spinners are judged.
                o.judged = true;
                o.lifetime_end = time + HIT_LIFETIME;
                self.remaining -= 1;
            }
        }
    }
}

impl Rules for OsuRules {
    fn max_judgements(&self) -> usize {
        self.objects.iter().filter(|o| o.is_circle).count()
    }

    fn update(&mut self, step: &StepInput<'_>, log: &mut JudgementLog) {
        for event in step.events {
            if event.pressed {
                self.press(event, log);
            }
        }
        self.time = step.time;
        self.update_objects(step.time, log);
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
            rules.check_hittable(1, 1050.0, HitResult::Ok, alive),
            ClickAction::Ignore
        );
        // Ignore comes before the early-press check.
        assert_eq!(
            rules.check_hittable(1, 1050.0, HitResult::None, alive),
            ClickAction::Ignore
        );
        assert_eq!(
            rules.check_hittable(0, 600.0, HitResult::None, alive),
            ClickAction::Shake
        );
        assert_eq!(
            rules.check_hittable(0, 1000.0, HitResult::Great, alive),
            ClickAction::Hit
        );
        // Once judged, the stacked object no longer blocks.
        rules.objects[0].judged = true;
        assert_eq!(
            rules.check_hittable(1, 1050.0, HitResult::Ok, alive),
            ClickAction::Hit
        );
    }

    #[test]
    fn start_time_ordered_blocks_before_the_last_earlier_circle() {
        let map = stacked_circles();
        let rules = OsuRules::with_policy(&map, HitPolicy::StartTimeOrdered);
        assert_eq!(
            rules.check_hittable(2, 1099.0, HitResult::Ok, 500.0),
            ClickAction::Shake
        );
        assert_eq!(
            rules.check_hittable(2, 1100.0, HitResult::Ok, 500.0),
            ClickAction::Hit
        );
        assert_eq!(
            rules.check_hittable(0, 1000.0, HitResult::None, 500.0),
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
