//! When gameplay starts: the lead-in before the first object and skipping the intro.

use crate::beatmap::Beatmap;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/MasterGameplayClockContainer.cs (MINIMUM_SKIP_TIME)
/// How long before [`gameplay_start_time`] a skip ends: the intro is skipped to this much
/// before it, and only while the clock is earlier than that.
pub const MINIMUM_SKIP_TIME: f64 = 1000.0;

/// The first object's start minus `max(2000, preempt)`, or 0 for an empty beatmap: the
/// time the gameplay is considered to begin. Frame stability starts here, and the intro can
/// be skipped up to [`MINIMUM_SKIP_TIME`] before it.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/DrawableOsuRuleset.cs (GameplayStartTime)
pub fn gameplay_start_time(beatmap: &Beatmap) -> f64 {
    match beatmap.hit_objects.first() {
        Some(first) => first.start_time - first.defaults.time_preempt.max(2000.0),
        None => 0.0,
    }
}

/// The time the gameplay clock starts from: 0, or earlier when [`gameplay_start_time`] is
/// negative, when the storyboard has an event before that (`storyboard_earliest`, the
/// earliest start of a non-video element) or when the beatmap's `AudioLeadIn` asks to
/// start earlier before the first object.
///
/// Lazer requires at least one object here; an empty beatmap ignores `AudioLeadIn`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/MasterGameplayClockContainer.cs (findEarliestStartTime)
pub fn earliest_start_time(beatmap: &Beatmap, storyboard_earliest: Option<f64>) -> f64 {
    let mut time = gameplay_start_time(beatmap).min(0.0);
    if let Some(event) = storyboard_earliest {
        time = time.min(event);
    }
    if let Some(first) = beatmap.hit_objects.first()
        && beatmap.audio_lead_in > 0.0
    {
        time = time.min(first.start_time - beatmap.audio_lead_in);
    }
    time
}

/// Skipping the intro: lazer's `SkipOverlay` (when the skip is offered) together with
/// `MasterGameplayClockContainer.Skip` (where it goes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntroSkip {
    /// [`gameplay_start_time`].
    gameplay_start_time: f64,
    /// The time the clock starts from ([`earliest_start_time`]).
    start_time: f64,
    /// The clock time when the skip overlay appeared.
    display_time: f64,
}

impl IntroSkip {
    /// The skip for a play that starts from `start_time` and shows the skip overlay at
    /// `display_time`.
    pub fn new(gameplay_start_time: f64, start_time: f64, display_time: f64) -> IntroSkip {
        IntroSkip {
            gameplay_start_time,
            start_time,
            display_time,
        }
    }

    /// The time the overlay hides and the skip stops being offered.
    fn fade_out_begin_time(&self) -> f64 {
        self.gameplay_start_time - MINIMUM_SKIP_TIME
    }

    /// Whether the skip is offered at all: lazer expires the overlay when there is no time
    /// to skip from where it appeared.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/SkipOverlay.cs (LoadComplete)
    pub fn is_offered(&self) -> bool {
        self.fade_out_begin_time() > self.display_time
    }

    /// Whether the skip can be used at `current`: the overlay's skip period.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/SkipOverlay.cs (Update)
    pub fn is_available(&self, current: f64) -> bool {
        if !self.is_offered() {
            return false;
        }
        let progress = (1.0
            - (current - self.display_time) / (self.fade_out_begin_time() - self.display_time))
            .max(0.0);
        progress > 0.0
    }

    /// Where a skip at `current` seeks to, or `None` when the skip is not available. A very
    /// long intro (the play starts before −10 s and the skip would go past 6 s) is skipped in
    /// two steps, first to 0, unless `full_length` is set.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/MasterGameplayClockContainer.cs (Skip)
    pub fn target(&self, current: f64, full_length: bool) -> Option<f64> {
        if !self.is_available(current) || current > self.gameplay_start_time - MINIMUM_SKIP_TIME {
            return None;
        }

        let mut target = self.gameplay_start_time - MINIMUM_SKIP_TIME;
        if !full_length && self.start_time < -10000.0 && current < 0.0 && target > 6000.0 {
            // double skip exception for storyboards with very long intros
            target = 0.0;
        }
        Some(target)
    }
}
