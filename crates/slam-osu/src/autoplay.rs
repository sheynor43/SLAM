//! The judgements of a perfect play, which lazer's judgement processors simulate to settle
//! their maxima (`JudgementProcessor.SimulateAutoplay`).

use crate::beatmap::Beatmap;
use crate::judgement::HitResult;
use crate::objects::{NestedKind, OsuHitObjectKind};

/// What a judgement judged, as far as the judgement processors care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Circle,
    Slider,
    Spinner,
    SliderHead,
    SliderTick,
    SliderRepeat,
    SliderTail,
    SpinnerTick,
    SpinnerBonusTick,
}

impl Target {
    /// The target of a slider's nested object.
    pub(crate) fn of_nested(kind: &NestedKind) -> Target {
        match kind {
            NestedKind::Head => Target::SliderHead,
            NestedKind::Tick { .. } => Target::SliderTick,
            NestedKind::Repeat { .. } => Target::SliderRepeat,
            NestedKind::Tail { .. } => Target::SliderTail,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Slider.cs (CreateJudgement)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SliderHeadCircle.cs (CreateJudgement)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SliderTailCircle.cs (CreateJudgement)
    /// The best result of the target, which a perfect play gets.
    pub(crate) fn max_result(self, classic_slider_behaviour: bool) -> HitResult {
        match self {
            Target::Circle | Target::Spinner => HitResult::Great,
            Target::Slider if classic_slider_behaviour => HitResult::Great,
            Target::Slider => HitResult::IgnoreHit,
            Target::SliderHead if classic_slider_behaviour => HitResult::LargeTickHit,
            Target::SliderHead => HitResult::Great,
            Target::SliderTick | Target::SliderRepeat => HitResult::LargeTickHit,
            Target::SliderTail if classic_slider_behaviour => HitResult::SmallTickHit,
            Target::SliderTail => HitResult::SliderTailHit,
            Target::SpinnerTick => HitResult::SmallBonus,
            Target::SpinnerBonusTick => HitResult::LargeBonus,
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/JudgementProcessor.cs (SimulateAutoplay, EnumerateHitObjects)
/// Calls `f(time, target)` for every judgement of a perfect play, in lazer's simulation order:
/// each object's nested objects in start time order, then the object. `time` is the judged
/// object's end time.
pub(crate) fn for_each_judgement(beatmap: &Beatmap, mut f: impl FnMut(f64, Target)) {
    for object in &beatmap.hit_objects {
        match &object.kind {
            OsuHitObjectKind::Slider(slider) => {
                for n in &slider.nested {
                    f(n.start_time, Target::of_nested(&n.kind));
                }
                f(object.end_time(), Target::Slider);
            }
            OsuHitObjectKind::Spinner(spinner) => {
                for i in 0..spinner.tick_count().max(0) {
                    let target = if spinner.is_bonus_tick(i) {
                        Target::SpinnerBonusTick
                    } else {
                        Target::SpinnerTick
                    };
                    f(spinner.tick_time(object.start_time, i), target);
                }
                f(object.end_time(), Target::Spinner);
            }
            OsuHitObjectKind::Circle => f(object.end_time(), Target::Circle),
        }
    }
}
