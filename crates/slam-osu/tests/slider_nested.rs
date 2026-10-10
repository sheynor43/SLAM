//! Nested objects of sliders built from `.osu` text: their samples and the defaults
//! `Beatmap::from_file` applies. Exact times and positions are checked against lazer in
//! `slider_nested_lazer.rs`.

use slam_formats::osu::decode_str;
use slam_osu::Beatmap;
use slam_osu::control_points::ControlPoints;
use slam_osu::objects::{NestedKind, OsuHitObject, OsuHitObjectKind, Slider};
use slam_osu::samples::{HitSample, SampleName};

fn load_text(text: &str) -> Beatmap {
    let decoded = decode_str(text).unwrap();
    assert!(decoded.warnings.is_empty(), "{:?}", decoded.warnings);
    Beatmap::from_file(decoded.beatmap).unwrap()
}

fn slider(obj: &OsuHitObject) -> &Slider {
    match &obj.kind {
        OsuHitObjectKind::Slider(s) => s,
        other => panic!("not a slider: {other:?}"),
    }
}

fn names(samples: &[HitSample]) -> Vec<SampleName> {
    samples.iter().map(|s| s.name).collect()
}

/// 140 px per beat of 500 ms: a 200 px span has one tick at 140 px. Two spans, node sounds
/// whistle, clap, finish.
const TWO_SPANS: &str = "osu file format v14\n\n[Difficulty]\nApproachRate:9\nCircleSize:4\nSliderMultiplier:1.4\nSliderTickRate:1\n\n[TimingPoints]\n0,500,4,2,0,70,1,0\n\n[HitObjects]\n0,0,1000,2,0,L|200:0,2,200,2|8|4,0:0|0:0|0:0,0:0:0:0:\n";

#[test]
fn nested_objects_are_created_on_load() {
    let b = load_text(TWO_SPANS);
    let obj = &b.hit_objects[0];
    let s = slider(obj);

    let kinds: Vec<&str> = s
        .nested
        .iter()
        .map(|n| match n.kind {
            NestedKind::Head => "head",
            NestedKind::Tick { .. } => "tick",
            NestedKind::Repeat { .. } => "repeat",
            NestedKind::Tail { .. } => "tail",
        })
        .collect();
    assert_eq!(kinds, ["head", "tick", "repeat", "tick", "tail"]);
    assert!(
        s.nested
            .windows(2)
            .all(|w| w[0].start_time <= w[1].start_time)
    );

    // AR 9: 1200 - 750 * 0.8, truncated.
    assert_eq!(obj.defaults.time_preempt, 600.0);
    assert_eq!(obj.defaults.time_fade_in, 400.0);
    assert_eq!(s.nested[0].defaults, obj.defaults);
}

#[test]
fn nested_samples_come_from_nodes() {
    let b = load_text(TWO_SPANS);
    let s = slider(&b.hit_objects[0]);

    assert!(names(&s.nested[0].samples).contains(&SampleName::Whistle));
    assert!(names(&s.nested[2].samples).contains(&SampleName::Clap));
    assert!(names(&s.tail_samples).contains(&SampleName::Finish));
    // The tail circle plays nothing itself.
    assert!(s.nested[4].samples.is_empty());

    // Ticks play the body's normal sample renamed, keeping its bank and volume.
    let body = &b.hit_objects[0].samples[0];
    for tick in [&s.nested[1], &s.nested[3]] {
        assert_eq!(tick.samples.len(), 1);
        assert_eq!(tick.samples[0].name, SampleName::SliderTick);
        assert_eq!(tick.samples[0].bank, body.bank);
        assert_eq!(tick.samples[0].volume, 70);
    }
}

#[test]
fn nodes_without_edge_sounds_play_body_sounds() {
    // No edge sounds: every node gets the body samples (a clap).
    let b = load_text(
        "osu file format v14\n\n[Difficulty]\nSliderMultiplier:1.4\n\n[TimingPoints]\n0,500,4,1,0,70,1,0\n\n[HitObjects]\n0,0,1000,2,8,L|200:0,2,200\n",
    );
    let s = slider(&b.hit_objects[0]);
    assert_eq!(s.node_samples.len(), 3);
    for n in &s.nested {
        if matches!(n.kind, NestedKind::Head | NestedKind::Repeat { .. }) {
            assert!(names(&n.samples).contains(&SampleName::Clap), "{n:?}");
        }
    }
    assert!(names(&s.tail_samples).contains(&SampleName::Clap));
}

#[test]
fn missing_nodes_are_filled_with_body_samples() {
    // A slider built by hand (an editor or a mod) may carry fewer node samples than nodes;
    // lazer's PopulateNodeSamples fills the rest with copies of the body samples.
    let mut b = load_text(TWO_SPANS);
    let body = b.hit_objects[0].samples.clone();
    let OsuHitObjectKind::Slider(s) = &mut b.hit_objects[0].kind else {
        unreachable!()
    };
    let head = s.node_samples[0].clone();
    s.node_samples.truncate(1);

    let control_points = b.control_points.clone();
    b.hit_objects[0].apply_defaults(&control_points, &b.difficulty);
    let s = slider(&b.hit_objects[0]);

    assert_eq!(s.node_samples, [head.clone(), body.clone(), body.clone()]);
    assert_eq!(s.nested[0].samples, head);
    assert_eq!(s.nested[2].samples, body);
    assert_eq!(s.tail_samples, body);
}

#[test]
fn apply_defaults_can_be_repeated() {
    let mut b = load_text(TWO_SPANS);
    let before = b.hit_objects[0].clone();

    let mut harder = b.difficulty;
    harder.approach_rate = 10.0;
    harder.slider_tick_rate = 2.0;
    let control_points: ControlPoints = b.control_points.clone();
    b.hit_objects[0].apply_defaults(&control_points, &harder);

    let after = &b.hit_objects[0];
    assert_eq!(after.defaults.time_preempt, 450.0);
    // Twice the ticks; head, repeat and tail stay.
    assert_eq!(slider(after).nested.len(), slider(&before).nested.len() + 2);

    b.hit_objects[0].apply_defaults(&control_points, &b.difficulty);
    assert_eq!(b.hit_objects[0], before);
}

#[test]
fn spinner_gets_object_defaults() {
    let b = load_text(
        "osu file format v14\n\n[Difficulty]\nApproachRate:10\nCircleSize:5\n\n[TimingPoints]\n0,500,4,1,0,70,1,0\n\n[HitObjects]\n256,192,1000,12,0,3000,0:0:0:0:\n",
    );
    let obj = &b.hit_objects[0];
    assert_eq!(obj.defaults.time_preempt, 450.0);
    assert_eq!(obj.defaults.scale, 0.5 * 1.00041);
}
