//! Control point storage, lookup and redundancy rules.
//!
//! Cases ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/NonVisual/ControlPointInfoTest.cs

use slam_osu::control_points::{
    ControlPoints, DifficultyPoint, EffectPoint, EqualitySelection, SamplePoint, TimingPoint,
    binary_search,
};
use slam_osu::samples::Bank;

fn timing(time: f64) -> TimingPoint {
    TimingPoint::new(time, TimingPoint::DEFAULT_BEAT_LENGTH, 4, false)
}

fn timing_list(times: &[i32]) -> Vec<TimingPoint> {
    times.iter().map(|&t| timing(f64::from(t))).collect()
}

#[test]
fn add_timing() {
    let mut cpi = ControlPoints::new();
    cpi.add_timing(timing(0.0));
    cpi.add_timing(TimingPoint::new(1000.0, 500.0, 4, false));
    assert_eq!(cpi.timing_points().len(), 2);
}

#[test]
fn add_redundant_timing() {
    let mut cpi = ControlPoints::new();
    cpi.add_timing(timing(0.0));
    cpi.add_timing(timing(1000.0));
    assert_eq!(cpi.timing_points().len(), 2);

    // Same time: replaces the existing point.
    cpi.add_timing(timing(1000.0));
    assert_eq!(cpi.timing_points().len(), 2);

    cpi.add_timing(TimingPoint::new(1200.0, 1000.0, 4, true));
    assert_eq!(cpi.timing_points().len(), 3);

    cpi.add_timing(TimingPoint::new(1500.0, 1000.0, 4, true));
    assert_eq!(cpi.timing_points().len(), 4);
}

#[test]
fn add_redundant_difficulty() {
    let mut cpi = ControlPoints::new();
    assert!(!cpi.add_difficulty(DifficultyPoint::new(0.0, 1.0, true)));
    assert!(!cpi.add_difficulty(DifficultyPoint::new(1000.0, 1.0, true)));
    assert_eq!(cpi.difficulty_points().len(), 0);

    assert!(cpi.add_difficulty(DifficultyPoint::new(1000.0, 2.0, true)));
    assert_eq!(cpi.difficulty_points().len(), 1);
}

#[test]
fn add_redundant_sample() {
    let mut cpi = ControlPoints::new();
    // The first sample point is never redundant.
    assert!(cpi.add_sample(SamplePoint::new(0.0, Bank::Normal, 100, 0)));
    assert!(!cpi.add_sample(SamplePoint::new(1000.0, Bank::Normal, 100, 0)));
    assert_eq!(cpi.sample_points().len(), 1);

    assert!(cpi.add_sample(SamplePoint::new(1000.0, Bank::Normal, 50, 0)));
    assert_eq!(cpi.sample_points().len(), 2);
}

#[test]
fn add_redundant_sample_custom_bank() {
    let mut cpi = ControlPoints::new();
    cpi.add_sample(SamplePoint::new(0.0, Bank::Normal, 100, 0));
    assert!(cpi.add_sample(SamplePoint::new(1000.0, Bank::Normal, 100, 2)));
    assert!(cpi.add_sample(SamplePoint::new(2000.0, Bank::Other(4), 100, 2)));
    assert!(!cpi.add_sample(SamplePoint::new(3000.0, Bank::Other(4), 100, 2)));
    assert_eq!(cpi.sample_points().len(), 3);
}

#[test]
fn add_redundant_effect() {
    let mut cpi = ControlPoints::new();
    assert!(!cpi.add_effect(EffectPoint {
        time: 0.0,
        kiai: false
    }));
    assert!(!cpi.add_effect(EffectPoint {
        time: 1000.0,
        kiai: false
    }));
    assert_eq!(cpi.effect_points().len(), 0);

    assert!(cpi.add_effect(EffectPoint {
        time: 1000.0,
        kiai: true
    }));
    assert!(!cpi.add_effect(EffectPoint {
        time: 1400.0,
        kiai: true
    }));
    assert_eq!(cpi.effect_points().len(), 1);
}

#[test]
fn replacing_a_point_at_the_same_time() {
    let mut cpi = ControlPoints::new();
    cpi.add_difficulty(DifficultyPoint::new(1000.0, 2.0, true));
    cpi.add_difficulty(DifficultyPoint::new(1000.0, 3.0, true));
    assert_eq!(
        cpi.difficulty_points(),
        &[DifficultyPoint::new(1000.0, 3.0, true)]
    );
}

#[test]
fn ordering() {
    let mut cpi = ControlPoints::new();
    cpi.add_timing(timing(0.0));
    cpi.add_timing(TimingPoint::new(1000.0, 500.0, 4, false));
    cpi.add_timing(TimingPoint::new(10000.0, 200.0, 4, false));
    cpi.add_timing(TimingPoint::new(5000.0, 100.0, 4, false));

    let times: Vec<f64> = cpi.timing_points().iter().map(|p| p.time).collect();
    assert_eq!(times, [0.0, 1000.0, 5000.0, 10000.0]);
}

#[test]
fn lookup_fallbacks() {
    let mut cpi = ControlPoints::new();
    assert_eq!(cpi.timing_point_at(0.0), TimingPoint::DEFAULT);
    assert_eq!(cpi.sample_point_at(0.0), SamplePoint::DEFAULT);
    assert_eq!(cpi.difficulty_point_at(0.0), DifficultyPoint::DEFAULT);
    assert_eq!(cpi.effect_point_at(0.0), EffectPoint::DEFAULT);

    cpi.add_timing(TimingPoint::new(1000.0, 500.0, 3, false));
    cpi.add_sample(SamplePoint::new(1000.0, Bank::Soft, 60, 0));
    cpi.add_difficulty(DifficultyPoint::new(1000.0, 2.0, true));
    cpi.add_effect(EffectPoint {
        time: 1000.0,
        kiai: true,
    });

    // Before the first point, timing and sample lookups fall back to the first point...
    assert_eq!(cpi.timing_point_at(0.0).beat_length, 500.0);
    assert_eq!(cpi.sample_point_at(0.0).bank, Bank::Soft);
    // ...while difficulty and effect lookups fall back to the defaults.
    assert_eq!(cpi.difficulty_point_at(0.0), DifficultyPoint::DEFAULT);
    assert_eq!(cpi.effect_point_at(0.0), EffectPoint::DEFAULT);

    assert_eq!(cpi.difficulty_point_at(1000.0).slider_velocity, 2.0);
    assert!(cpi.effect_point_at(5000.0).kiai);
}

#[test]
fn bindable_ranges() {
    assert_eq!(TimingPoint::new(0.0, 1.0, 4, false).beat_length, 6.0);
    assert_eq!(TimingPoint::new(0.0, 1e9, 4, false).beat_length, 60000.0);
    assert_eq!(DifficultyPoint::new(0.0, 0.01, true).slider_velocity, 0.1);
    assert_eq!(DifficultyPoint::new(0.0, 20.0, true).slider_velocity, 10.0);
    assert_eq!(SamplePoint::new(0.0, Bank::Normal, 150, 0).volume, 100);
    assert_eq!(SamplePoint::new(0.0, Bank::Normal, -5, 0).volume, 0);
}

#[test]
fn binary_search_empty_list() {
    let empty: [TimingPoint; 0] = [];
    for selection in [
        EqualitySelection::FirstFound,
        EqualitySelection::Leftmost,
        EqualitySelection::Rightmost,
    ] {
        assert_eq!(binary_search(&empty, 0.0, selection), -1);
    }
}

#[test]
fn binary_search_unique_scenarios() {
    let cases: &[(&[i32], i32, isize)] = &[
        (&[1], 0, -1),
        (&[1], 1, 0),
        (&[1], 2, -2),
        (&[1, 3], 0, -1),
        (&[1, 3], 1, 0),
        (&[1, 3], 2, -2),
        (&[1, 3], 3, 1),
        (&[1, 3], 4, -3),
    ];

    for &(values, search, expected) in cases {
        let items = timing_list(values);
        for selection in [
            EqualitySelection::FirstFound,
            EqualitySelection::Leftmost,
            EqualitySelection::Rightmost,
        ] {
            assert_eq!(
                binary_search(&items, f64::from(search), selection),
                expected,
                "{values:?} {search} {selection:?}"
            );
        }
    }
}

#[test]
fn binary_search_duplicate_scenarios() {
    // (values, search, first found, leftmost, rightmost)
    let cases: &[(&[i32], i32, isize, isize, isize)] = &[
        (&[1, 1], 1, 0, 0, 1),
        (&[1, 2, 2], 2, 1, 1, 2),
        (&[1, 2, 2, 2], 2, 1, 1, 3),
        (&[1, 2, 2, 2, 3], 2, 2, 1, 3),
        (&[1, 2, 2, 3], 2, 1, 1, 2),
    ];

    for &(values, search, first, left, right) in cases {
        let items = timing_list(values);
        let search = f64::from(search);
        assert_eq!(
            binary_search(&items, search, EqualitySelection::FirstFound),
            first
        );
        assert_eq!(
            binary_search(&items, search, EqualitySelection::Leftmost),
            left
        );
        assert_eq!(
            binary_search(&items, search, EqualitySelection::Rightmost),
            right
        );
    }
}
