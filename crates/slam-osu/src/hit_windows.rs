//! Hit windows: how far from an object's time a press may land for each result.

use crate::difficulty::DifficultyRange;
use crate::judgement::HitResult;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuHitWindows.cs
/// Great window by overall difficulty.
pub const GREAT_WINDOW_RANGE: DifficultyRange = DifficultyRange::new(80.0, 50.0, 20.0);
/// Ok window by overall difficulty.
pub const OK_WINDOW_RANGE: DifficultyRange = DifficultyRange::new(140.0, 100.0, 60.0);
/// Meh window by overall difficulty.
pub const MEH_WINDOW_RANGE: DifficultyRange = DifficultyRange::new(200.0, 150.0, 100.0);
/// The miss window: a press this close to the object, but outside the meh window, misses it.
pub const MISS_WINDOW: f64 = 400.0;

/// The hit windows of a hit circle or slider head (lazer's `OsuHitWindows`), as half-widths in
/// milliseconds around the object's time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitWindows {
    great: f64,
    ok: f64,
    meh: f64,
}

impl HitWindows {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuHitWindows.cs (SetDifficulty)
    /// The windows for an overall difficulty (after mods). The windows end half a millisecond
    /// before a whole number, so that integer offsets fall unambiguously inside or outside.
    pub fn new(overall_difficulty: f32) -> HitWindows {
        let difficulty = f64::from(overall_difficulty);
        HitWindows {
            great: GREAT_WINDOW_RANGE.value(difficulty).floor() - 0.5,
            ok: OK_WINDOW_RANGE.value(difficulty).floor() - 0.5,
            meh: MEH_WINDOW_RANGE.value(difficulty).floor() - 0.5,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuHitWindows.cs (WindowFor)
    /// The window of `result`: one of Great, Ok, Meh and Miss.
    ///
    /// # Panics
    /// For any other result, as lazer throws.
    pub fn window_for(&self, result: HitResult) -> f64 {
        match result {
            HitResult::Great => self.great,
            HitResult::Ok => self.ok,
            HitResult::Meh => self.meh,
            HitResult::Miss => MISS_WINDOW,
            _ => panic!("osu! hit windows have no {result:?} window"),
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitWindows.cs (ResultFor)
    /// The result of a press `time_offset` ms after the object's time (negative when early):
    /// the best window containing it, [`HitResult::None`] outside the miss window.
    pub fn result_for(&self, time_offset: f64) -> HitResult {
        let time_offset = time_offset.abs();
        for result in [
            HitResult::Great,
            HitResult::Ok,
            HitResult::Meh,
            HitResult::Miss,
        ] {
            if time_offset <= self.window_for(result) {
                return result;
            }
        }
        HitResult::None
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitWindows.cs (CanBeHit)
    /// Whether the object can still be hit `time_offset` ms after its time. Not symmetric: any
    /// time before the object qualifies, so this only tells when the object has been missed.
    pub fn can_be_hit(&self, time_offset: f64) -> bool {
        // The lowest successful result of the osu! windows is Meh.
        time_offset <= self.meh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_by_od() {
        let w = HitWindows::new(0.0);
        assert_eq!(
            [w.great, w.ok, w.meh, w.window_for(HitResult::Miss)],
            [79.5, 139.5, 199.5, 400.0]
        );
        let w = HitWindows::new(5.0);
        assert_eq!([w.great, w.ok, w.meh], [49.5, 99.5, 149.5]);
        let w = HitWindows::new(10.0);
        assert_eq!([w.great, w.ok, w.meh], [19.5, 59.5, 99.5]);
        // OD 7: great 50 - 30 * 0.4 = 38 (exactly), ok 100 - 40 * 0.4 = 84, meh 150 - 50 * 0.4 = 130.
        let w = HitWindows::new(7.0);
        assert_eq!([w.great, w.ok, w.meh], [37.5, 83.5, 129.5]);
    }

    #[test]
    fn fractional_od_floors_before_the_half() {
        // OD 8.3 as a float is 8.30000019..., so the meh window 150 - 50 * 0.66000004 is just
        // below 117 and floors to 116, not 117 as with an exact 8.3.
        let w = HitWindows::new(8.3);
        assert_eq!([w.great, w.ok, w.meh], [29.5, 72.5, 115.5]);
    }

    #[test]
    fn result_for_is_symmetric_and_inclusive() {
        let w = HitWindows::new(5.0);
        assert_eq!(w.result_for(0.0), HitResult::Great);
        assert_eq!(w.result_for(49.5), HitResult::Great);
        assert_eq!(w.result_for(-49.5), HitResult::Great);
        assert_eq!(w.result_for(50.0), HitResult::Ok);
        assert_eq!(w.result_for(-99.5), HitResult::Ok);
        assert_eq!(w.result_for(149.5), HitResult::Meh);
        assert_eq!(w.result_for(-150.0), HitResult::Miss);
        assert_eq!(w.result_for(400.0), HitResult::Miss);
        assert_eq!(w.result_for(-400.5), HitResult::None);
    }

    #[test]
    fn can_be_hit_ends_after_meh() {
        let w = HitWindows::new(5.0);
        assert!(w.can_be_hit(-10_000.0));
        assert!(w.can_be_hit(149.5));
        assert!(!w.can_be_hit(150.0));
    }

    #[test]
    #[should_panic(expected = "no Perfect window")]
    fn window_for_other_results_panics() {
        HitWindows::new(5.0).window_for(HitResult::Perfect);
    }
}
