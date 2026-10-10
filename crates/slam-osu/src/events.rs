//! Slider events: the times and path positions of a slider's head, ticks, repeats and tail.

use crate::dotnet;

/// Kind of a [`SliderEvent`] (lazer's `SliderEventType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliderEventKind {
    /// A tick inside a span.
    Tick,
    /// osu!stable's tick shortly before the tail (see [`TAIL_LENIENCY`]). osu!standard objects
    /// ignore it; it exists for difficulty compatibility and mods that need it.
    LegacyLastTick,
    /// The slider head.
    Head,
    /// The slider tail.
    Tail,
    /// A repeat at the end of a span.
    Repeat,
}

/// One slider event (lazer's `SliderEventDescriptor`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderEvent {
    /// Kind of the event.
    pub kind: SliderEventKind,
    /// Time in ms.
    pub time: f64,
    /// Zero-based index of the span; increases after each repeat.
    pub span_index: i32,
    /// Start time of the span in ms.
    pub span_start_time: f64,
    /// Progress along the path, 0..=1.
    pub path_progress: f64,
}

/// Lazer's `SliderEventGenerator.TAIL_LENIENCY`: osu!stable's legacy last tick sits this many
/// ms before the end of the slider (or at its middle for sliders shorter than 72 ms).
pub const TAIL_LENIENCY: f64 = -36.0;

/// A very lenient maximum length of a slider for ticks to be generated. Lazer keeps it for
/// hand-edited maps such as /b/1573664; normal maps never reach it.
const MAX_LENGTH: f64 = 100000.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/SliderEventGenerator.cs (Generate, generateTicks)
/// Appends the events of a slider to `events` in lazer's order: head, then for each span its
/// ticks in time order and its repeat, then the legacy last tick and the tail.
///
/// No ticks are generated when `tick_distance` is 0. An infinite `tick_distance` is clamped to
/// the path length and normally gives none either; as in lazer, a NaN velocity still lets one
/// tick through at the end of the path.
pub fn generate(
    events: &mut Vec<SliderEvent>,
    start_time: f64,
    span_duration: f64,
    velocity: f64,
    tick_distance: f64,
    total_distance: f64,
    span_count: i32,
) {
    let length = dotnet::min(MAX_LENGTH, total_distance);
    let tick_distance = dotnet::clamp(tick_distance, 0.0, length);

    let min_distance_from_end = velocity * 10.0;

    events.push(SliderEvent {
        kind: SliderEventKind::Head,
        time: start_time,
        span_index: 0,
        span_start_time: start_time,
        path_progress: 0.0,
    });

    for span in 0..span_count {
        let span_start_time = start_time + f64::from(span) * span_duration;
        let reversed = span % 2 == 1;

        if tick_distance != 0.0 {
            let first_tick = events.len();
            generate_ticks(
                events,
                span,
                span_start_time,
                span_duration,
                reversed,
                length,
                tick_distance,
                min_distance_from_end,
            );

            // Ticks of a reversed span come out in reverse time order.
            if reversed {
                events[first_tick..].reverse();
            }
        }

        if span < span_count - 1 {
            events.push(SliderEvent {
                kind: SliderEventKind::Repeat,
                time: span_start_time + span_duration,
                span_index: span,
                span_start_time: start_time + f64::from(span) * span_duration,
                path_progress: f64::from((span + 1) % 2),
            });
        }
    }

    let total_duration = f64::from(span_count) * span_duration;

    // The legacy last tick is at `start_time + max(duration / 2, duration - 36)`, written as
    // lazer does to keep osu!stable's floating point precision.
    let final_span_index = span_count.wrapping_sub(1);
    let final_span_start_time = start_time + f64::from(final_span_index) * span_duration;

    let legacy_last_tick_time = dotnet::max(
        start_time + total_duration / 2.0,
        (final_span_start_time + span_duration) + TAIL_LENIENCY,
    );
    let mut legacy_last_tick_progress =
        (legacy_last_tick_time - final_span_start_time) / span_duration;

    if span_count % 2 == 0 {
        legacy_last_tick_progress = 1.0 - legacy_last_tick_progress;
    }

    events.push(SliderEvent {
        kind: SliderEventKind::LegacyLastTick,
        time: legacy_last_tick_time,
        span_index: final_span_index,
        span_start_time: final_span_start_time,
        path_progress: legacy_last_tick_progress,
    });

    events.push(SliderEvent {
        kind: SliderEventKind::Tail,
        time: start_time + total_duration,
        span_index: final_span_index,
        span_start_time: start_time + f64::from(span_count.wrapping_sub(1)) * span_duration,
        path_progress: f64::from(span_count % 2),
    });
}

#[allow(clippy::too_many_arguments)] // Mirrors lazer's signature.
fn generate_ticks(
    events: &mut Vec<SliderEvent>,
    span_index: i32,
    span_start_time: f64,
    span_duration: f64,
    reversed: bool,
    length: f64,
    tick_distance: f64,
    min_distance_from_end: f64,
) {
    let mut d = tick_distance;
    while d <= length {
        if d >= length - min_distance_from_end {
            break;
        }

        // Progress is measured from the start of the path, so that ticks of repeat spans sit
        // exactly where those of the first span do.
        let path_progress = d / length;
        let time_progress = if reversed {
            1.0 - path_progress
        } else {
            path_progress
        };

        events.push(SliderEvent {
            kind: SliderEventKind::Tick,
            time: span_start_time + time_progress * span_duration,
            span_index,
            span_start_time,
            path_progress,
        });

        d += tick_distance;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Beatmaps/SliderEventGenerationTest.cs
    const START_TIME: f64 = 0.0;
    const SPAN_DURATION: f64 = 1000.0;

    fn events(
        velocity: f64,
        tick_distance: f64,
        total_distance: f64,
        span_count: i32,
    ) -> Vec<SliderEvent> {
        let mut e = Vec::new();
        generate(
            &mut e,
            START_TIME,
            SPAN_DURATION,
            velocity,
            tick_distance,
            total_distance,
            span_count,
        );
        e
    }

    fn kinds_times(e: &[SliderEvent]) -> Vec<(SliderEventKind, f64)> {
        e.iter().map(|e| (e.kind, e.time)).collect()
    }

    use SliderEventKind::*;

    #[test]
    fn single_span() {
        let e = events(1.0, SPAN_DURATION / 2.0, SPAN_DURATION, 1);
        assert_eq!(
            kinds_times(&e),
            [
                (Head, 0.0),
                (Tick, 500.0),
                (LegacyLastTick, 964.0),
                (Tail, 1000.0)
            ]
        );
    }

    #[test]
    fn repeat() {
        let e = events(1.0, SPAN_DURATION / 2.0, SPAN_DURATION, 2);
        assert_eq!(
            kinds_times(&e),
            [
                (Head, 0.0),
                (Tick, 500.0),
                (Repeat, 1000.0),
                (Tick, 1500.0),
                (LegacyLastTick, 1964.0),
                (Tail, 2000.0)
            ]
        );
    }

    #[test]
    fn non_even_ticks() {
        let e = events(1.0, 300.0, SPAN_DURATION, 2);
        assert_eq!(
            kinds_times(&e),
            [
                (Head, 0.0),
                (Tick, 300.0),
                (Tick, 600.0),
                (Tick, 900.0),
                (Repeat, 1000.0),
                (Tick, 1100.0),
                (Tick, 1400.0),
                (Tick, 1700.0),
                (LegacyLastTick, 1964.0),
                (Tail, 2000.0)
            ]
        );
    }

    #[test]
    fn last_tick_offset() {
        let e = events(1.0, SPAN_DURATION / 2.0, SPAN_DURATION, 1);
        assert_eq!(e[2].kind, LegacyLastTick);
        assert_eq!(e[2].time, SPAN_DURATION + TAIL_LENIENCY);
    }

    #[test]
    fn minimum_tick_distance() {
        const VELOCITY: f64 = 5.0;
        const MIN_DISTANCE: f64 = VELOCITY * 10.0;

        let e = events(VELOCITY, VELOCITY, SPAN_DURATION, 2);
        for event in e.iter().filter(|e| e.kind == Tick) {
            assert!(
                event.time < SPAN_DURATION - MIN_DISTANCE
                    || event.time > SPAN_DURATION + MIN_DISTANCE,
                "tick at {}",
                event.time
            );
        }
    }

    #[test]
    fn repeats_generated_even_for_zero_length_slider() {
        let e = events(1.0, SPAN_DURATION / 2.0, 0.0, 2);
        assert_eq!(
            kinds_times(&e),
            [
                (Head, 0.0),
                (Repeat, 1000.0),
                (LegacyLastTick, 1964.0),
                (Tail, 2000.0)
            ]
        );
    }

    #[test]
    fn infinite_tick_distance_gives_no_ticks() {
        let e = events(1.0, f64::INFINITY, SPAN_DURATION, 3);
        assert!(e.iter().all(|e| e.kind != Tick));
    }

    #[test]
    fn nan_distance_does_not_panic() {
        let e = events(1.0, 100.0, f64::NAN, 1);
        assert_eq!(e.len(), 3);
    }
}
