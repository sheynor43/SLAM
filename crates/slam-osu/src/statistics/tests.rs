use super::*;
use crate::Mod;
use crate::mods::RateAdjust;

fn beatmap(timing: &str, events: &str, objects: &str) -> Beatmap {
    let text = format!(
        "osu file format v14\n[General]\nMode: 0\n[Difficulty]\nSliderMultiplier:1\n[Events]\n{events}[TimingPoints]\n{timing}[HitObjects]\n{objects}"
    );
    Beatmap::decode(text.as_bytes()).unwrap()
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Beatmaps/BeatmapExtensionsTest.cs (TestLengthCalculations)
#[test]
fn length_spans_the_objects_and_drain_drops_the_breaks() {
    let b = beatmap(
        "0,500,4,2,0,70,1,0\n",
        "2,50000,75000\n2,100000,150000\n",
        "256,192,5000,1,0\n256,192,300000,1,0\n256,192,280000,12,0,320000\n",
    );
    assert_eq!(playable_bounds(&b.hit_objects), (5000.0, 320000.0));
    let s = BeatmapStatistics::of(&b);
    assert_eq!(s.length, 315000.0);
    assert_eq!(s.drain_length, 240000.0);
    assert_eq!((s.circles, s.sliders, s.spinners), (2, 0, 1));
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Beatmaps/BeatmapExtensionsTest.cs (TestDrainLengthCannotGoNegative)
#[test]
fn drain_length_is_not_negative() {
    let b = beatmap(
        "0,500,4,2,0,70,1,0\n",
        "2,0,350000\n",
        "256,192,5000,1,0\n256,192,300000,1,0\n256,192,280000,12,0,320000\n",
    );
    assert_eq!(BeatmapStatistics::of(&b).drain_length, 0.0);
}

#[test]
fn no_objects_and_no_timing_points() {
    let b = beatmap("", "", "");
    let s = BeatmapStatistics::of(&b);
    assert_eq!((s.length, s.drain_length), (0.0, 0.0));
    assert_eq!(
        (s.bpm_minimum, s.bpm_maximum, s.bpm_most_common),
        (60.0, 60.0, 60.0)
    );
}

#[test]
fn bpm_range_and_the_longest_lasting_bpm() {
    // 120 BPM from 0 (the first point counts from 0) to 10 s, 200 BPM to 12 s, 150 BPM to
    // the last object at 30 s; a point after the last object counts nothing.
    let b = beatmap(
        "1000,500,4,2,0,70,1,0\n10000,300,4,2,0,70,1,0\n12000,400,4,2,0,70,1,0\n40000,100,4,2,0,70,1,0\n",
        "",
        "256,192,1000,1,0\n256,192,30000,1,0\n",
    );
    let s = BeatmapStatistics::of(&b);
    assert_eq!(s.bpm_minimum, 120.0);
    assert_eq!(s.bpm_maximum, 600.0);
    assert_eq!(s.bpm_most_common, 150.0);
    assert_eq!((s.circles, s.sliders, s.spinners), (2, 0, 0));
}

#[test]
fn beat_lengths_within_a_microsecond_group_together() {
    // 333.3331 and 333.3334 round to 333.333 together (8 s) and outlast 500 (6 s); the
    // rounded length is clamped into the raw range.
    let b = beatmap(
        "0,333.3334,4,2,0,70,1,0\n4000,500,4,2,0,70,1,0\n10000,333.3331,4,2,0,70,1,0\n",
        "",
        "256,192,0,1,0\n256,192,14000,1,0\n",
    );
    let common = most_common_beat_length(&b);
    assert_eq!(common, 333.3331f64.max(333.333));
}

#[test]
fn bpm_rounds_half_to_even() {
    assert_eq!(round_bpm(120.5, 1.0), 120);
    assert_eq!(round_bpm(121.5, 1.0), 122);
    assert_eq!(round_bpm(180.0, 1.5), 270);
}

fn difficulty(approach_rate: f32, overall_difficulty: f32) -> Difficulty {
    Difficulty {
        drain_rate: 5.0,
        circle_size: 5.0,
        overall_difficulty,
        approach_rate,
        slider_multiplier: 1.4,
        slider_tick_rate: 1.0,
    }
}

fn mods(list: Vec<Mod>) -> ModSet {
    ModSet::new(list).unwrap()
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/OsuRateAdjustedDisplayDifficultyTest.cs
#[test]
fn rate_changes_the_perceived_approach_rate_and_overall_difficulty() {
    for tenth in 0..=20 {
        let value = tenth as f32 / 2.0;
        let same = adjusted_display_difficulty(difficulty(value, value), &mods(vec![]));
        assert_eq!(
            (same.approach_rate, same.overall_difficulty),
            (value, value)
        );
    }
    let half = adjusted_display_difficulty(
        difficulty(5.0, 5.0),
        &mods(vec![Mod::HalfTime(RateAdjust {
            speed_change: 0.75,
            adjust_pitch: false,
        })]),
    );
    assert!(
        (half.approach_rate - 1.67).abs() < 0.01,
        "{}",
        half.approach_rate
    );
    assert!((half.overall_difficulty - 2.22).abs() < 0.01);
    let double = adjusted_display_difficulty(
        difficulty(5.0, 5.0),
        &mods(vec![Mod::DoubleTime(RateAdjust {
            speed_change: 1.5,
            adjust_pitch: false,
        })]),
    );
    assert!((double.approach_rate - 7.67).abs() < 0.01);
    assert!((double.overall_difficulty - 7.77).abs() < 0.01);
}

#[test]
fn hard_rock_applies_before_the_rate() {
    let hr = adjusted_display_difficulty(difficulty(9.0, 8.0), &mods(vec![Mod::HardRock]));
    assert_eq!((hr.approach_rate, hr.overall_difficulty), (10.0, 10.0));
    assert_eq!(hr.circle_size, 6.5);
    assert_eq!(hr.drain_rate, 7.0);
}
