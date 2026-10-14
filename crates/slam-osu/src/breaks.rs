//! Whether gameplay is in a break: lazer's `BreakTracker`.

pub use slam_formats::osu::Break;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Timing/BreakPeriod.cs (MIN_BREAK_DURATION)
/// The shortest break, in ms, that has an effect; shorter breaks are ignored.
pub const MIN_BREAK_DURATION: f64 = 650.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/BreakOverlay.cs (BREAK_FADE_DURATION)
/// How long the break overlay fades, in ms. A break stops counting as one this much before
/// its end, so the fade out finishes at the end.
pub const BREAK_FADE_DURATION: f64 = MIN_BREAK_DURATION / 2.0;

/// A break as the tracker sees it: the beatmap's break with its end moved
/// [`BREAK_FADE_DURATION`] earlier. Both ends are inclusive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Period {
    /// Start time, in ms.
    pub start: f64,
    /// End time, in ms.
    pub end: f64,
}

impl Period {
    /// `end - start`.
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/BreakTracker.cs
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/PeriodTracker.cs
/// Tracks whether the gameplay time is in a break, as lazer's `BreakTracker` does: inside a
/// break with an effect (at least [`MIN_BREAK_DURATION`] long, ended
/// [`BREAK_FADE_DURATION`] early), and also before the gameplay start and once every object
/// is judged.
///
/// Remembers the nearest break between queries, like lazer's `PeriodTracker`, so querying
/// times close to each other is cheap. Construction allocates; [`BreakTracker::update`] does
/// not.
#[derive(Debug, Clone)]
pub struct BreakTracker {
    periods: Vec<Period>,
    nearest: usize,
    gameplay_start_time: f64,
}

/// The tracker's state at a time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BreakState {
    /// Whether gameplay counts as a break: lazer's `IsBreakTime`.
    pub is_break_time: bool,
    /// The break the time is in, if any: lazer's `CurrentPeriod`.
    pub period: Option<Period>,
}

impl BreakTracker {
    /// A tracker for `breaks` (any order) with gameplay starting at `gameplay_start_time`
    /// ([`crate::lead_in::gameplay_start_time`]).
    pub fn new(breaks: &[Break], gameplay_start_time: f64) -> BreakTracker {
        let mut periods: Vec<Period> = breaks
            .iter()
            .filter(|b| b.end - b.start >= MIN_BREAK_DURATION)
            .map(|b| Period {
                start: b.start,
                end: b.end - BREAK_FADE_DURATION,
            })
            .collect();
        // `OrderBy` is a stable sort by start.
        periods.sort_by(|a, b| a.start.total_cmp(&b.start));
        BreakTracker {
            periods,
            nearest: 0,
            gameplay_start_time,
        }
    }

    /// The breaks with an effect, sorted by start.
    pub fn periods(&self) -> &[Period] {
        &self.periods
    }

    /// The state at `time`. `completed` is whether every object is judged (lazer's
    /// `ScoreProcessor.HasCompleted`).
    pub fn update(&mut self, time: f64, completed: bool) -> BreakState {
        match self.find(time) {
            Some(period) => BreakState {
                is_break_time: true,
                period: Some(period),
            },
            None => BreakState {
                is_break_time: time < self.gameplay_start_time || completed,
                period: None,
            },
        }
    }

    /// Lazer's `PeriodTracker.IsInAny`: walks from the nearest period found last time. With
    /// overlapping breaks (broken files) the answer depends on the times asked before, and
    /// those differ from lazer's (every frame-stable step); with breaks that do not overlap
    /// it does not.
    fn find(&mut self, time: f64) -> Option<Period> {
        if self.periods.is_empty() {
            return None;
        }

        let last = self.periods.len() - 1;
        if time > self.periods[self.nearest].end {
            while time > self.periods[self.nearest].end && self.nearest < last {
                self.nearest += 1;
            }
        } else {
            while time < self.periods[self.nearest].start && self.nearest > 0 {
                self.nearest -= 1;
            }
        }

        let nearest = self.periods[self.nearest];
        (time >= nearest.start && time <= nearest.end).then_some(nearest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(breaks: &[(f64, f64)], gameplay_start: f64) -> BreakTracker {
        let breaks: Vec<Break> = breaks
            .iter()
            .map(|&(start, end)| Break { start, end })
            .collect();
        BreakTracker::new(&breaks, gameplay_start)
    }

    fn is_break(tracker: &mut BreakTracker, time: f64) -> bool {
        tracker.update(time, false).is_break_time
    }

    #[test]
    fn a_break_ends_by_the_fade_duration_early_with_inclusive_bounds() {
        let mut t = tracker(&[(10_000.0, 12_000.0)], 0.0);
        assert!(!is_break(&mut t, 9_999.9));
        assert!(is_break(&mut t, 10_000.0));
        assert!(is_break(&mut t, 11_675.0));
        assert!(!is_break(&mut t, 11_675.1));
        assert_eq!(
            t.update(11_000.0, false).period,
            Some(Period {
                start: 10_000.0,
                end: 11_675.0
            })
        );
        assert_eq!(t.update(12_000.0, false).period, None);
    }

    #[test]
    fn breaks_shorter_than_the_minimum_have_no_effect() {
        let mut t = tracker(&[(1_000.0, 1_649.0), (5_000.0, 5_650.0)], 0.0);
        assert_eq!(t.periods().len(), 1);
        assert!(!is_break(&mut t, 1_200.0));
        assert!(is_break(&mut t, 5_000.0));
        assert!(is_break(&mut t, 5_325.0));
        assert!(!is_break(&mut t, 5_325.5));
    }

    #[test]
    fn before_the_gameplay_start_and_after_completion_count_as_breaks() {
        let mut t = tracker(&[], 1_500.0);
        assert!(is_break(&mut t, -2_000.0));
        assert!(is_break(&mut t, 1_499.9));
        assert!(!is_break(&mut t, 1_500.0));
        assert!(t.update(90_000.0, true).is_break_time);
        assert_eq!(t.update(90_000.0, true).period, None);
    }

    #[test]
    fn walks_forward_and_back_through_several_breaks() {
        let mut t = tracker(
            &[
                (30_000.0, 32_000.0),
                (10_000.0, 12_000.0),
                (20_000.0, 22_000.0),
            ],
            0.0,
        );
        let starts: Vec<f64> = t.periods().iter().map(|p| p.start).collect();
        assert_eq!(starts, [10_000.0, 20_000.0, 30_000.0]);

        assert!(is_break(&mut t, 31_000.0));
        assert!(!is_break(&mut t, 25_000.0));
        assert!(is_break(&mut t, 11_000.0));
        assert!(is_break(&mut t, 21_000.0));
        assert!(!is_break(&mut t, 40_000.0));
        assert!(!is_break(&mut t, 5_000.0));
        assert!(is_break(&mut t, 10_500.0));
    }
}
