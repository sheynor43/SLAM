//! Health: drain over time, the change each judgement makes, and failing.
//!
//! Lazer's `OsuHealthProcessor`, or `OsuLegacyHealthProcessor` when Classic's classic health is
//! on. The drain rate is computed once from the beatmap; [`HealthProcessor::update`] drains and
//! [`HealthProcessor::apply`] applies judgements without allocating.

use std::ops::Range;

use crate::autoplay::{self, Target};
use crate::beatmap::Beatmap;
use crate::difficulty::DifficultyRange;
use crate::dotnet;
use crate::judgement::{HitResult, Judgement};
use crate::mods::{Mod, ModSet};
use crate::objects::{OsuHitObject, OsuHitObjectKind};
use crate::precision::DOUBLE_EPSILON;
use crate::rules::RulesOptions;

use slam_formats::osu::Break;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/DrainingHealthProcessor.cs
/// Allowed error of the lowest health of a perfect play from its target.
const MINIMUM_HEALTH_ERROR: f64 = 0.01;
/// Lowest health of a perfect play by HP drain rate.
const TARGET_MINIMUM_HEALTH: DifficultyRange = DifficultyRange::new(0.99, 0.9, 0.4);

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/LegacyDrainingHealthProcessor.cs (ApplyBeatmap, ComputeDrainRate)
/// Health a legacy perfect play may never reach.
const LOWEST_HP_EVER: DifficultyRange = DifficultyRange::new(0.975, 0.8, 0.3);
/// Lowest health of a legacy perfect play at its end.
const LOWEST_HP_END: DifficultyRange = DifficultyRange::new(0.99, 0.9, 0.4);
/// Health a legacy perfect play must recover per object.
const HP_RECOVERY_AVAILABLE: DifficultyRange = DifficultyRange::new(0.04, 0.02, 0.0);
/// The first drain rate the legacy search tries.
const LEGACY_INITIAL_DROP: f64 = 0.00025;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuHealthProcessor.cs (getHealthIncreaseFor)
/// Health lost on a missed tick (small or large).
const TICK_MISS: DifficultyRange = DifficultyRange::new(-0.02, -0.075, -0.14);
/// Health lost on a miss.
const MISS: DifficultyRange = DifficultyRange::new(-0.03, -0.125, -0.2);

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Judgements/Judgement.cs
/// `DEFAULT_MAX_HEALTH_INCREASE`.
const DEFAULT_MAX_HEALTH_INCREASE: f64 = 0.05;

/// Which of lazer's health processors to follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthKind {
    /// `OsuHealthProcessor`: the drain rate is searched so that a perfect play bottoms out at a
    /// target health, and the last object of a combo gives a bonus.
    Standard,
    /// `OsuLegacyHealthProcessor` (Classic's classic health): stable's drain rate search and
    /// health multiplier.
    Legacy,
}

/// How to build a [`HealthProcessor`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HealthOptions {
    /// The processor.
    pub kind: HealthKind,
    /// Lazer's `Slider.ClassicSliderBehaviour`: changes the best results of slider heads,
    /// tails and the slider itself, which the drain rate is computed from.
    pub classic_slider_behaviour: bool,
    /// When health starts draining; lazer's player uses the start of the first object.
    pub drain_start_time: f64,
}

impl HealthOptions {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (LoadComponents)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModClassic.cs (CreateHealthProcessor)
    /// The options a play of `beatmap` with `mods` uses.
    pub fn for_play(beatmap: &Beatmap, mods: &ModSet) -> HealthOptions {
        let classic_health = mods.mods().iter().any(|m| match m {
            Mod::Classic(c) => c.classic_health,
            _ => false,
        });
        HealthOptions {
            kind: if classic_health {
                HealthKind::Legacy
            } else {
                HealthKind::Standard
            },
            classic_slider_behaviour: RulesOptions::for_mods(mods).classic_slider_behaviour,
            // Lazer indexes the first object and throws on an empty beatmap; an empty beatmap
            // never drains, so any time does.
            drain_start_time: beatmap.hit_objects.first().map_or(0.0, |o| o.start_time),
        }
    }
}

/// A top-level object, as far as health cares.
#[derive(Debug, Clone, PartialEq)]
struct ObjectInfo {
    new_combo: bool,
    last_in_combo: bool,
    kind: ObjectKind,
}

#[derive(Debug, Clone, PartialEq)]
enum ObjectKind {
    Circle,
    /// The slider's nested objects in [`HealthProcessor::nested`].
    Slider(Range<u32>),
    /// Ticks from this index on are bonus ticks.
    Spinner {
        bonus_from: i32,
    },
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Judgements/ComboResult.cs
/// How well the current combo went; ordered from worst to best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ComboResult {
    None,
    Good,
    Perfect,
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/PeriodTracker.cs
/// Whether a time is in one of a set of periods, remembering where the last query landed.
#[derive(Debug, Clone, PartialEq)]
struct PeriodTracker {
    /// `(start, end)` sorted by start.
    periods: Vec<(f64, f64)>,
    nearest_index: usize,
}

impl PeriodTracker {
    fn new(mut periods: Vec<(f64, f64)>) -> PeriodTracker {
        // `OrderBy` is a stable sort.
        periods.sort_by(|a, b| dotnet::compare_f64(a.0, b.0));
        PeriodTracker {
            periods,
            nearest_index: 0,
        }
    }

    fn is_in_any(&mut self, time: f64) -> bool {
        if self.periods.is_empty() {
            return false;
        }

        if time > self.periods[self.nearest_index].1 {
            while time > self.periods[self.nearest_index].1
                && self.nearest_index < self.periods.len() - 1
            {
                self.nearest_index += 1;
            }
        } else {
            while time < self.periods[self.nearest_index].0 && self.nearest_index > 0 {
                self.nearest_index -= 1;
            }
        }

        let (start, end) = self.periods[self.nearest_index];
        time >= start && time <= end
    }
}

/// The health of a play (lazer's `HealthProcessor` for osu!standard).
#[derive(Debug, Clone, PartialEq)]
pub struct HealthProcessor {
    kind: HealthKind,
    drain_rate_setting: f64,
    drain_start_time: f64,
    gameplay_end_time: f64,
    drain_rate: f64,
    hp_multiplier_normal: f64,
    no_drain: PeriodTracker,
    objects: Vec<ObjectInfo>,
    nested: Vec<Target>,
    health: f64,
    has_failed: bool,
    combo_result: ComboResult,
    last_update_time: Option<f64>,
}

impl HealthProcessor {
    /// The health processor of a play of `beatmap` with `mods`.
    pub fn for_play(beatmap: &Beatmap, mods: &ModSet) -> HealthProcessor {
        HealthProcessor::new(beatmap, HealthOptions::for_play(beatmap, mods))
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/DrainingHealthProcessor.cs (ApplyBeatmap, Reset)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/LegacyDrainingHealthProcessor.cs (ApplyBeatmap, Reset)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/JudgementProcessor.cs (ApplyBeatmap)
    /// A processor for `beatmap` at full health, with its drain rate computed.
    ///
    /// Lazer's drain lenience is always 0 for osu!standard and is left out.
    pub fn new(beatmap: &Beatmap, options: HealthOptions) -> HealthProcessor {
        let objects = &beatmap.hit_objects;

        let mut nested = Vec::new();
        let infos = objects
            .iter()
            .map(|o| ObjectInfo {
                new_combo: o.new_combo,
                last_in_combo: o.combo.last_in_combo,
                kind: match &o.kind {
                    OsuHitObjectKind::Circle => ObjectKind::Circle,
                    OsuHitObjectKind::Slider(slider) => {
                        let start = nested.len() as u32;
                        nested.extend(slider.nested.iter().map(|n| Target::of_nested(&n.kind)));
                        ObjectKind::Slider(start..nested.len() as u32)
                    }
                    OsuHitObjectKind::Spinner(spinner) => ObjectKind::Spinner {
                        bonus_from: spinner.spins_required_for_bonus(),
                    },
                },
            })
            .collect();

        let gameplay_end_time = objects.last().map_or(0.0, OsuHitObject::end_time);

        let no_drain = PeriodTracker::new(
            beatmap
                .breaks
                .iter()
                .map(|b| {
                    let start = objects
                        .iter()
                        .map(OsuHitObject::end_time)
                        .rev()
                        .find(|&end| end <= b.start)
                        .unwrap_or(f64::MIN);
                    let end = objects
                        .iter()
                        .map(|o| o.start_time)
                        .find(|&start| start >= b.end)
                        .unwrap_or(f64::MAX);
                    // Lazer's `Period` throws when `start >= end`: a zero-length break with one
                    // object ending and the next starting at its time. Such a period only holds
                    // that instant here.
                    (start, end)
                })
                .collect(),
        );

        let mut processor = HealthProcessor {
            kind: options.kind,
            drain_rate_setting: f64::from(beatmap.difficulty.drain_rate),
            drain_start_time: options.drain_start_time,
            gameplay_end_time,
            drain_rate: 0.0,
            hp_multiplier_normal: 1.0,
            no_drain,
            objects: infos,
            nested,
            health: 1.0,
            has_failed: false,
            combo_result: ComboResult::Perfect,
            last_update_time: None,
        };

        processor.drain_rate = match options.kind {
            HealthKind::Standard => {
                let increases = processor.simulate_autoplay(beatmap, options);
                let target = TARGET_MINIMUM_HEALTH.value(processor.drain_rate_setting);
                // Drain lenience 0 adds nothing back.
                let target = dotnet::clamp(target, 0.0, 1.0);
                compute_drain_rate(
                    &increases,
                    &beatmap.breaks,
                    options.drain_start_time,
                    target,
                )
            }
            HealthKind::Legacy => processor.compute_legacy_drain_rate(beatmap, options),
        };
        processor
    }

    /// The current health, 0..=1.
    pub fn health(&self) -> f64 {
        self.health
    }

    /// Sets the health, clamped to 0..=1 (lazer's `Health.Value`).
    pub fn set_health(&mut self, health: f64) {
        self.health = dotnet::clamp(health, 0.0, 1.0);
    }

    /// The drain rate: health lost per millisecond.
    pub fn drain_rate(&self) -> f64 {
        self.drain_rate
    }

    /// The legacy processor's multiplier of health gains (`HpMultiplierNormal`); 1 for the
    /// standard processor.
    pub fn hp_multiplier_normal(&self) -> f64 {
        self.hp_multiplier_normal
    }

    /// Which processor this is.
    pub fn kind(&self) -> HealthKind {
        self.kind
    }

    /// Whether the player has failed.
    pub fn has_failed(&self) -> bool {
        self.has_failed
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/DrainingHealthProcessor.cs (Update)
    /// Drains health up to `time`, the time of an update. Lazer updates the health processor
    /// before the playfield, so this comes before the update's judgements.
    ///
    /// The first update does not drain, as in lazer, whose `FrameStabilityContainer` processes
    /// an extra frame on its first update to zero the elapsed time (`applyFrameStability`).
    pub fn update(&mut self, time: f64) {
        let elapsed = self.last_update_time.map_or(0.0, |last| time - last);
        self.last_update_time = Some(time);

        if self.no_drain.is_in_any(time) {
            return;
        }

        // When jumping in and out of gameplay time within a single frame, health should only be
        // drained for the period within the gameplay time.
        let last_gameplay_time = dotnet::clamp(
            time - elapsed,
            self.drain_start_time,
            self.gameplay_end_time,
        );
        let current_gameplay_time =
            dotnet::clamp(time, self.drain_start_time, self.gameplay_end_time);

        self.set_health(
            self.health - self.drain_rate * (current_gameplay_time - last_gameplay_time),
        );
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HealthProcessor.cs (ApplyResultInternal, TriggerFailure)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (onFail)
    /// Applies a judgement; if it meets a fail condition, asks the mods whether the player fails
    /// ([`ModSet::perform_fail`]). Once failed, judgements change nothing.
    pub fn apply(&mut self, judgement: &Judgement, mods: &mut ModSet) {
        if self.has_failed {
            return;
        }

        let increase = self.health_increase_for(judgement);
        self.set_health(self.health + increase);

        if self.check_default_fail_condition(judgement) {
            self.trigger_failure(mods);
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HealthProcessor.cs (TriggerFailure)
    /// Fails the player unless a mod prevents it.
    fn trigger_failure(&mut self, mods: &mut ModSet) {
        if self.has_failed {
            return;
        }

        let mut health = self.health;
        let fail = mods.perform_fail(&mut health);
        self.set_health(health);
        if fail {
            self.has_failed = true;
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/DrainingHealthProcessor.cs (CheckDefaultFailCondition)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HealthProcessor.cs (CheckDefaultFailCondition)
    fn check_default_fail_condition(&self, judgement: &Judgement) -> bool {
        if judgement.max_result.is_bonus() || judgement.result == HitResult::IgnoreHit {
            return false;
        }

        // `Precision.AlmostBigger(Health.MinValue, Health.Value)`.
        0.0 > self.health - DOUBLE_EPSILON
    }

    /// What `judgement` judged, and whether it is a top-level object.
    fn target(&self, judgement: &Judgement) -> (Target, Option<&ObjectInfo>) {
        let info = &self.objects[judgement.object.index as usize];
        match (judgement.object.nested, &info.kind) {
            (None, ObjectKind::Circle) => (Target::Circle, Some(info)),
            (None, ObjectKind::Slider(_)) => (Target::Slider, Some(info)),
            (None, ObjectKind::Spinner { .. }) => (Target::Spinner, Some(info)),
            (Some(k), ObjectKind::Slider(range)) => (self.nested[(range.start + k) as usize], None),
            (Some(k), ObjectKind::Spinner { bonus_from }) => {
                // `k` counts ticks of one spinner, which came from an i32.
                if k as i32 >= *bonus_from {
                    (Target::SpinnerBonusTick, None)
                } else {
                    (Target::SpinnerTick, None)
                }
            }
            (Some(_), ObjectKind::Circle) => {
                debug_assert!(false, "judgement of a nested object of a circle");
                (Target::Circle, None)
            }
        }
    }

    /// The health change of a judgement in play.
    fn health_increase_for(&mut self, judgement: &Judgement) -> f64 {
        let (target, info) = self.target(judgement);
        match self.kind {
            HealthKind::Standard => {
                // Nested objects have the combo information interface too, but never start or
                // end a combo.
                let (new_combo, last_in_combo) =
                    info.map_or((false, false), |i| (i.new_combo, i.last_in_combo));
                self.standard_increase_with_combo(
                    target,
                    judgement.result,
                    new_combo,
                    last_in_combo,
                )
            }
            HealthKind::Legacy => self.legacy_increase(target, judgement.result),
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuHealthProcessor.cs (GetHealthIncreaseFor)
    fn standard_increase_with_combo(
        &mut self,
        target: Target,
        result: HitResult,
        new_combo: bool,
        last_in_combo: bool,
    ) -> f64 {
        if new_combo {
            self.combo_result = ComboResult::Perfect;
        }

        match result {
            HitResult::LargeTickMiss | HitResult::Ok => self.set_combo_result(ComboResult::Good),
            HitResult::Meh | HitResult::Miss => self.set_combo_result(ComboResult::None),
            _ => {}
        }

        // The slider tail has a special judgement that can't accurately be described above.
        if target == Target::SliderTail && !result.is_hit() {
            self.set_combo_result(ComboResult::Good);
        }

        if last_in_combo && result.is_hit() {
            return match self.combo_result {
                ComboResult::Perfect => self.standard_increase(target, result) + 0.07,
                ComboResult::Good => self.standard_increase(target, result) + 0.05,
                ComboResult::None => self.standard_increase(target, result) + 0.03,
            };
        }

        self.standard_increase(target, result)
    }

    fn set_combo_result(&mut self, result: ComboResult) {
        self.combo_result = self.combo_result.min(result);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuHealthProcessor.cs (getHealthIncreaseFor)
    fn standard_increase(&self, target: Target, result: HitResult) -> f64 {
        match result {
            HitResult::SmallTickMiss | HitResult::LargeTickMiss => {
                TICK_MISS.value(self.drain_rate_setting)
            }
            HitResult::Miss => MISS.value(self.drain_rate_setting),
            // When classic slider mechanics are enabled, this result comes from the tail.
            HitResult::SmallTickHit => 0.02,
            HitResult::SliderTailHit | HitResult::LargeTickHit => match target {
                Target::SliderTick => 0.015,
                Target::SliderHead | Target::SliderTail | Target::SliderRepeat => 0.02,
                _ => base_health_increase(result),
            },
            HitResult::Meh => 0.002,
            HitResult::Ok => 0.011,
            HitResult::Great => 0.03,
            HitResult::SmallBonus => 0.0085,
            HitResult::LargeBonus => 0.01,
            _ => base_health_increase(result),
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuLegacyHealthProcessor.cs (GetHealthIncreaseFor)
    fn legacy_increase(&self, target: Target, result: HitResult) -> f64 {
        legacy_increase(
            self.drain_rate_setting,
            self.hp_multiplier_normal,
            target,
            result,
        )
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/JudgementProcessor.cs (SimulateAutoplay, EnumerateHitObjects, GetSimulatedHitResult)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/DrainingHealthProcessor.cs (ApplyResultInternal)
    /// The health increases of a perfect play, `(time, amount)` in judgement order: each
    /// object's nested objects in start time order, then the object. Bonus results are left
    /// out.
    fn simulate_autoplay(&self, beatmap: &Beatmap, options: HealthOptions) -> Vec<(f64, f64)> {
        let classic = options.classic_slider_behaviour;
        let mut increases = Vec::new();
        autoplay::for_each_judgement(beatmap, |time, target| {
            let result = target.max_result(classic);
            if !result.is_bonus() {
                // While simulating, the combo is not tracked.
                increases.push((time, self.standard_increase(target, result)));
            }
        });
        increases
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/LegacyDrainingHealthProcessor.cs (ComputeDrainRate)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuLegacyHealthProcessor.cs (EnumerateTopLevelHitObjects, EnumerateNestedHitObjects)
    /// Stable's drain rate search; also settles `hp_multiplier_normal`.
    fn compute_legacy_drain_rate(&mut self, beatmap: &Beatmap, options: HealthOptions) -> f64 {
        let classic = options.classic_slider_behaviour;
        let hp_setting = self.drain_rate_setting;
        let lowest_hp_ever = LOWEST_HP_EVER.value(hp_setting);
        let lowest_hp_end = LOWEST_HP_END.value(hp_setting);
        let hp_recovery_available = HP_RECOVERY_AVAILABLE.value(hp_setting);

        let breaks = &beatmap.breaks;
        let mut test_drop = LEGACY_INITIAL_DROP;
        self.hp_multiplier_normal = 1.0;

        loop {
            let mut current_hp = 1.0;
            let mut current_hp_uncapped = 1.0;

            let mut last_time = options.drain_start_time;
            let mut current_break = 0;
            let mut fail = false;
            let mut top_level_object_count: i32 = 0;

            let multiplier = self.hp_multiplier_normal;
            let reduce_hp = |amount: f64, hp: &mut f64, uncapped: &mut f64| {
                *uncapped = dotnet::max(0.0, *uncapped - amount);
                *hp = dotnet::max(0.0, *hp - amount);
            };
            let increase_hp = |target: Target, hp: &mut f64, uncapped: &mut f64| {
                let amount =
                    legacy_increase(hp_setting, multiplier, target, target.max_result(classic));
                *uncapped += amount;
                *hp = dotnet::max(0.0, dotnet::min(1.0, *hp + amount));
            };

            for (h, info) in beatmap.hit_objects.iter().zip(&self.objects) {
                top_level_object_count = top_level_object_count.wrapping_add(1);

                while current_break < breaks.len() && breaks[current_break].end <= h.start_time {
                    // If two hitobjects are separated by a break period, there is no drain for the
                    // full duration between the hitobjects. This differs from legacy (version < 8)
                    // beatmaps which continue draining until the break section is entered, but
                    // this shouldn't have a noticeable impact in practice.
                    last_time = h.start_time;
                    current_break += 1;
                }

                reduce_hp(
                    test_drop * (h.start_time - last_time),
                    &mut current_hp,
                    &mut current_hp_uncapped,
                );

                let end_time = h.end_time();
                last_time = end_time;

                // Lazer also tracks the lowest health here, but never reads it.
                if current_hp <= lowest_hp_ever {
                    fail = true;
                    test_drop *= 0.96;
                    break;
                }

                let hp_reduction = test_drop * (end_time - h.start_time);
                let hp_overkill = dotnet::max(0.0, hp_reduction - current_hp);
                reduce_hp(hp_reduction, &mut current_hp, &mut current_hp_uncapped);

                match (&h.kind, &info.kind) {
                    (OsuHitObjectKind::Slider(_), ObjectKind::Slider(range)) => {
                        for &target in &self.nested[range.start as usize..range.end as usize] {
                            increase_hp(target, &mut current_hp, &mut current_hp_uncapped);
                        }
                    }
                    (OsuHitObjectKind::Spinner(spinner), _) => {
                        // Bonus ticks are left out.
                        for i in 0..spinner.tick_count().max(0) {
                            if !spinner.is_bonus_tick(i) {
                                increase_hp(
                                    Target::SpinnerTick,
                                    &mut current_hp,
                                    &mut current_hp_uncapped,
                                );
                            }
                        }
                    }
                    _ => {}
                }

                // Note: Because HP is capped during the above increases, long sliders (with many
                // ticks) or spinners will appear to overkill at lower drain levels than they
                // should. However, it is also not correct to simply use the uncapped version.
                if hp_overkill > 0.0 && current_hp - hp_overkill <= lowest_hp_ever {
                    fail = true;
                    test_drop *= 0.96;
                    break;
                }

                let target = match info.kind {
                    ObjectKind::Circle => Target::Circle,
                    ObjectKind::Slider(_) => Target::Slider,
                    ObjectKind::Spinner { .. } => Target::Spinner,
                };
                increase_hp(target, &mut current_hp, &mut current_hp_uncapped);
            }

            if top_level_object_count == 0 {
                return test_drop;
            }

            if !fail && current_hp < lowest_hp_end {
                fail = true;
                test_drop *= 0.94;
                self.hp_multiplier_normal *= 1.01;
            }

            let recovery = (current_hp_uncapped - 1.0) / f64::from(top_level_object_count.max(1));

            if !fail && recovery < hp_recovery_available {
                fail = true;
                test_drop *= 0.96;
                self.hp_multiplier_normal *= 1.01;
            }

            if !fail && self.hp_multiplier_normal.is_infinite() {
                // Drain computation algorithm diverged to infinity: pass with zero drop and reset
                // the HP multiplier to 1.
                self.hp_multiplier_normal = 1.0;
                return 0.0;
            }

            if !fail {
                return test_drop;
            }
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/DrainingHealthProcessor.cs (ComputeDrainRate)
/// The drain rate at which a play with these health `increases` (`(time, amount)` in judgement
/// order) bottoms out within [`MINIMUM_HEALTH_ERROR`] of `target_minimum_health`: a binary
/// search that ends after 30 halvings at the latest.
fn compute_drain_rate(
    increases: &[(f64, f64)],
    breaks: &[Break],
    drain_start_time: f64,
    target_minimum_health: f64,
) -> f64 {
    if increases.len() <= 1 {
        return 0.0;
    }

    let mut adjustment: i32 = 1;
    let mut result = 1.0;

    // Although we expect the following loop to converge within 30 iterations (health within
    // 1/2^31 accuracy of the target), we'll still keep a safety measure to avoid infinite loops
    // by detecting overflows.
    while adjustment > 0 {
        let mut current_health = 1.0;
        let mut lowest_health = 1.0;
        let mut current_break = 0;

        for (i, &(current_time, amount)) in increases.iter().enumerate() {
            let mut last_time = if i > 0 {
                increases[i - 1].0
            } else {
                drain_start_time
            };

            while current_break < breaks.len() && breaks[current_break].end <= current_time {
                // If two hitobjects are separated by a break period, there is no drain for the
                // full duration between the hitobjects.
                last_time = current_time;
                current_break += 1;
            }

            // Apply health adjustments
            current_health -= (current_time - last_time) * result;
            lowest_health = dotnet::min(lowest_health, current_health);
            current_health = dotnet::min(1.0, current_health + amount);

            // Common scenario for when the drain rate is definitely too harsh
            if lowest_health < 0.0 {
                break;
            }
        }

        // Stop if the resulting health is within a reasonable offset from the target
        if (lowest_health - target_minimum_health).abs() <= MINIMUM_HEALTH_ERROR {
            break;
        }

        // This effectively works like a binary search - each iteration the search space moves
        // closer to the target, but may exceed it.
        adjustment = adjustment.wrapping_mul(2);
        result += 1.0 / f64::from(adjustment) * sign(lowest_health - target_minimum_health);
    }

    result
}

/// .NET's `Math.Sign(double)` as a double; it throws on NaN, which gives 0 here.
fn sign(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuLegacyHealthProcessor.cs (GetHealthIncreaseFor)
/// Stable's health change: misses as in lazer, gains times `hp_multiplier_normal`.
fn legacy_increase(
    drain_rate: f64,
    hp_multiplier_normal: f64,
    target: Target,
    result: HitResult,
) -> f64 {
    let increase = match result {
        HitResult::SmallTickMiss | HitResult::LargeTickMiss => return TICK_MISS.value(drain_rate),
        HitResult::Miss => return MISS.value(drain_rate),
        // This result always comes from the slider tail, which is judged the same as a repeat.
        HitResult::SmallTickHit => 0.02,
        // This result comes from either a slider tick or repeat.
        HitResult::SliderTailHit | HitResult::LargeTickHit => {
            if target == Target::SliderTick {
                0.015
            } else {
                0.02
            }
        }
        HitResult::Meh => 0.002,
        HitResult::Ok => 0.011,
        HitResult::Great => 0.03,
        HitResult::SmallBonus => 0.0085,
        HitResult::LargeBonus => 0.01,
        _ => 0.0,
    };
    hp_multiplier_normal * increase
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Judgements/Judgement.cs (HealthIncreaseFor)
/// The default health change of a result.
fn base_health_increase(result: HitResult) -> f64 {
    match result {
        HitResult::SmallTickHit => DEFAULT_MAX_HEALTH_INCREASE * 0.5,
        HitResult::SmallTickMiss => -DEFAULT_MAX_HEALTH_INCREASE * 0.5,
        HitResult::SliderTailHit | HitResult::LargeTickHit => DEFAULT_MAX_HEALTH_INCREASE,
        HitResult::LargeTickMiss => -DEFAULT_MAX_HEALTH_INCREASE,
        HitResult::Miss => -DEFAULT_MAX_HEALTH_INCREASE * 2.0,
        HitResult::Meh => DEFAULT_MAX_HEALTH_INCREASE * 0.05,
        HitResult::Ok => DEFAULT_MAX_HEALTH_INCREASE * 0.5,
        HitResult::Good => DEFAULT_MAX_HEALTH_INCREASE * 0.75,
        HitResult::Great => DEFAULT_MAX_HEALTH_INCREASE,
        HitResult::Perfect => DEFAULT_MAX_HEALTH_INCREASE * 1.05,
        HitResult::SmallBonus => DEFAULT_MAX_HEALTH_INCREASE * 0.5,
        HitResult::LargeBonus => DEFAULT_MAX_HEALTH_INCREASE,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breaks(periods: &[(f64, f64)]) -> Vec<Break> {
        periods
            .iter()
            .map(|&(start, end)| Break { start, end })
            .collect()
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Gameplay/TestSceneDrainingHealthProcessor.cs (TestNoBreakDrainRate, TestSingleBreakDrainRate, TestOverlappingBreakDrainRate, TestSequentialBreakDrainRate)
    /// Lazer's tests of the generic `DrainingHealthProcessor`: two objects judged `Perfect` at
    /// 0 and 2000, drain from -1000, HP 5.
    #[test]
    fn drain_rate_matches_lazer_tests() {
        let perfect = base_health_increase(HitResult::Perfect);
        let increases = [(0.0, perfect), (2000.0, perfect)];
        let target = TARGET_MINIMUM_HEALTH.value(5.0);
        let rate = |b: &[(f64, f64)]| compute_drain_rate(&increases, &breaks(b), -1000.0, target);

        assert!((rate(&[]) - 4.5e-5).abs() <= 0.1e-5, "{}", rate(&[]));
        for b in [
            &[(500.0, 1500.0)][..],
            &[(500.0, 1400.0), (750.0, 1500.0)],
            &[(500.0, 1000.0), (1000.0, 1500.0)],
        ] {
            assert!((rate(b) - 9.1e-5).abs() <= 0.1e-5, "{b:?}: {}", rate(b));
        }
    }

    #[test]
    fn single_increase_does_not_drain() {
        assert_eq!(compute_drain_rate(&[(0.0, 0.03)], &[], 0.0, 0.9), 0.0);
    }

    #[test]
    fn period_tracker_walks_both_ways() {
        let mut t = PeriodTracker::new(vec![(300.0, 400.0), (100.0, 200.0)]);
        assert!(!t.is_in_any(50.0));
        assert!(t.is_in_any(100.0));
        assert!(t.is_in_any(200.0));
        assert!(!t.is_in_any(250.0));
        assert!(t.is_in_any(350.0));
        assert!(!t.is_in_any(450.0));
        assert!(t.is_in_any(150.0));
    }

    #[test]
    fn sign_matches_dotnet() {
        assert_eq!(sign(2.0), 1.0);
        assert_eq!(sign(-0.5), -1.0);
        assert_eq!(sign(0.0), 0.0);
        assert_eq!(sign(-0.0), 0.0);
    }
}
