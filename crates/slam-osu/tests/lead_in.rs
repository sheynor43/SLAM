//! Start time of a play and the intro skip.

use slam_osu::Beatmap;
use slam_osu::lead_in::{IntroSkip, MINIMUM_SKIP_TIME, earliest_start_time, gameplay_start_time};

/// A beatmap with `AudioLeadIn` and approach rate `ar` and one circle at `first` ms, or no
/// objects.
fn beatmap(audio_lead_in: i32, ar: f32, first: Option<i32>) -> Beatmap {
    let object = first
        .map(|t| format!("256,192,{t},1,0,0:0:0:0:\n"))
        .unwrap_or_default();
    let text = format!(
        "osu file format v14\n\n[General]\nAudioLeadIn: {audio_lead_in}\nMode: 0\n\n\
         [Difficulty]\nApproachRate:{ar}\n\n[TimingPoints]\n0,1000,4,2,0,100,1,0\n\n\
         [HitObjects]\n{object}"
    );
    Beatmap::decode(text.as_bytes()).unwrap()
}

/// Lazer's `TestSceneLeadIn.first_hit_object`.
const FIRST_HIT_OBJECT: i32 = 2170;

#[test]
fn gameplay_starts_two_seconds_or_one_preempt_before_the_first_object() {
    assert_eq!(
        gameplay_start_time(&beatmap(0, 5.0, Some(FIRST_HIT_OBJECT))),
        170.0
    );
    // AR 0: a 1800 ms preempt is still under two seconds.
    assert_eq!(
        gameplay_start_time(&beatmap(0, 0.0, Some(FIRST_HIT_OBJECT))),
        170.0
    );
    assert_eq!(gameplay_start_time(&beatmap(0, 5.0, None)), 0.0);
}

#[test]
fn gameplay_start_follows_a_preempt_longer_than_two_seconds() {
    let mut map = beatmap(0, 5.0, Some(FIRST_HIT_OBJECT));
    map.hit_objects[0].defaults.time_preempt = 2500.0;
    assert_eq!(gameplay_start_time(&map), -330.0);
    assert_eq!(earliest_start_time(&map, None), -330.0);
}

// Values from lazer's `TestSceneLeadIn.TestLeadInProducesCorrectStartTime`.
#[test]
fn audio_lead_in_moves_the_start_before_the_first_object() {
    for (lead_in, expected) in [
        (1000, 0.0),
        (2000, 0.0),
        (3000, f64::from(FIRST_HIT_OBJECT - 3000)),
        (10000, f64::from(FIRST_HIT_OBJECT - 10000)),
    ] {
        let map = beatmap(lead_in, 5.0, Some(FIRST_HIT_OBJECT));
        assert_eq!(
            earliest_start_time(&map, None),
            expected,
            "lead-in {lead_in}"
        );
    }
}

// Values from lazer's `TestSceneLeadIn.TestStoryboardProducesCorrectStartTimeSimpleAlpha`.
#[test]
fn an_earlier_storyboard_event_moves_the_start() {
    let map = beatmap(0, 5.0, Some(FIRST_HIT_OBJECT));
    for (event, expected) in [
        (1000.0, 0.0),
        (0.0, 0.0),
        (-1000.0, -1000.0),
        (-10000.0, -10000.0),
    ] {
        assert_eq!(
            earliest_start_time(&map, Some(event)),
            expected,
            "event {event}"
        );
    }
}

#[test]
fn a_late_first_object_starts_at_zero() {
    let map = beatmap(0, 5.0, Some(30000));
    assert_eq!(gameplay_start_time(&map), 28000.0);
    assert_eq!(earliest_start_time(&map, None), 0.0);
}

#[test]
fn an_empty_beatmap_starts_at_zero_and_ignores_the_lead_in() {
    assert_eq!(earliest_start_time(&beatmap(5000, 5.0, None), None), 0.0);
    assert_eq!(
        earliest_start_time(&beatmap(5000, 5.0, None), Some(-500.0)),
        -500.0
    );
}

#[test]
fn the_skip_is_offered_only_with_time_to_skip() {
    // Gameplay starts at 28 s: the intro is skippable until 27 s.
    let skip = IntroSkip::new(28000.0, 0.0, 0.0);
    assert!(skip.is_offered());
    assert!(skip.is_available(0.0));
    assert!(skip.is_available(26999.0));
    assert!(!skip.is_available(27000.0));
    assert!(!skip.is_available(30000.0));

    // Gameplay starts at 170 ms: nothing to skip from 0.
    let skip = IntroSkip::new(170.0, 0.0, 0.0);
    assert!(!skip.is_offered());
    assert!(!skip.is_available(-500.0));
    assert_eq!(skip.target(-500.0, false), None);

    // The skip period ends exactly where the overlay appeared.
    let skip = IntroSkip::new(1000.0, 0.0, 0.0);
    assert!(!skip.is_offered());
}

#[test]
fn a_long_audio_lead_in_can_be_skipped() {
    let map = beatmap(10000, 5.0, Some(FIRST_HIT_OBJECT));
    let start = earliest_start_time(&map, None);
    let skip = IntroSkip::new(gameplay_start_time(&map), start, start);
    assert_eq!(skip.target(start, false), Some(170.0 - MINIMUM_SKIP_TIME));
    assert_eq!(skip.target(-831.0, false), Some(-830.0));
    assert_eq!(skip.target(-830.0, false), None);
}

#[test]
fn the_skip_goes_one_second_before_gameplay_start() {
    let skip = IntroSkip::new(28000.0, 0.0, 0.0);
    assert_eq!(skip.target(0.0, false), Some(27000.0));
    assert_eq!(skip.target(12345.0, true), Some(27000.0));
    assert_eq!(skip.target(27000.0, false), None);
}

#[test]
fn a_very_long_intro_is_skipped_to_zero_first() {
    // Starts before −10 s, skip target past 6 s.
    let skip = IntroSkip::new(30000.0, -15000.0, -15000.0);
    assert_eq!(skip.target(-15000.0, false), Some(0.0));
    assert_eq!(skip.target(-15000.0, true), Some(29000.0));
    // The second skip, from 0, goes the full length.
    assert_eq!(skip.target(0.0, false), Some(29000.0));

    // No exception for a start at −10 s exactly or a target at 6 s exactly.
    assert_eq!(
        IntroSkip::new(30000.0, -10000.0, -10000.0).target(-10000.0, false),
        Some(29000.0)
    );
    assert_eq!(
        IntroSkip::new(7000.0, -15000.0, -15000.0).target(-15000.0, false),
        Some(6000.0)
    );
}

/// Skips from the start of a play of `map` with the earliest storyboard event at
/// `storyboard`, until no skip is available.
fn skips(map: &Beatmap, storyboard: f64) -> Vec<f64> {
    let start = earliest_start_time(map, Some(storyboard));
    let skip = IntroSkip::new(gameplay_start_time(map), start, start);
    let mut current = start;
    let mut targets = Vec::new();
    while let Some(target) = skip.target(current, false) {
        targets.push(target);
        current = target;
    }
    targets
}

// Cases from lazer's `TestSceneStoryboardWithIntro` (`TestStoryboardSingleSkip`,
// `TestStoryboardDoubleSkip`): a storyboard intro before a first object at 0, 30 s and 11 s.
#[test]
fn storyboard_intros_are_skipped_once_or_twice() {
    assert_eq!(skips(&beatmap(0, 5.0, Some(0)), -5000.0), [-3000.0]);
    assert_eq!(skips(&beatmap(0, 5.0, Some(30000)), -5000.0), [27000.0]);
    assert_eq!(
        skips(&beatmap(0, 5.0, Some(11000)), -11000.0),
        [0.0, 8000.0]
    );
}
