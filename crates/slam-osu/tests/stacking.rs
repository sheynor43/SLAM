//! Stacking through `Beatmap::from_file`.
//!
//! The edge case maps and the asserted values come from osu!lazer 2026.1005.0-lazer:
//! osu.Game.Rulesets.Osu.Tests/StackingTest.cs. Bit-exact parity on many maps is checked by
//! `stacking_lazer.rs`.

use slam_formats::osu::{Vec2, decode_str};
use slam_osu::Beatmap;
use slam_osu::objects::OsuHitObjectKind;

fn load_text(text: &str) -> Beatmap {
    let decoded = decode_str(text).unwrap();
    assert!(decoded.warnings.is_empty(), "{:?}", decoded.warnings);
    Beatmap::from_file(decoded.beatmap).unwrap()
}

fn heights(beatmap: &Beatmap) -> Vec<i32> {
    beatmap.hit_objects.iter().map(|h| h.stack_height).collect()
}

#[test]
fn stacking_edge_case_one() {
    let beatmap = load_text(
        "osu file format v14

[General]
StackLeniency: 0.2

[Difficulty]
ApproachRate:9.2
SliderMultiplier:1
SliderTickRate:0.5

[TimingPoints]
217871,6400,4,2,1,20,1,0
217871,-800,4,2,1,20,0,0
218071,-787.5,4,2,1,20,0,0
218271,-775,4,2,1,20,0,0
218471,-762.5,4,2,1,20,0,0
218671,-750,4,2,1,20,0,0
240271,-10,4,2,0,5,0,0

[HitObjects]
311,185,217871,6,0,L|318:158,1,25
311,185,218071,2,0,L|335:170,1,25
311,185,218271,2,0,L|338:192,1,25
311,185,218471,2,0,L|325:209,1,25
311,185,218671,2,0,L|304:212,1,25
311,185,240271,5,0,0:0:0:0:
",
    );

    // The last hitobject triggers the stacking
    let h = heights(&beatmap);
    assert_eq!(h.len(), 6);
    assert!(h[..h.len() - 1].iter().all(|&h| h == 0), "{h:?}");
}

#[test]
fn stacking_edge_case_two() {
    let beatmap = load_text(
        "osu file format v14
// extracted from https://osu.ppy.sh/beatmapsets/365006#osu/801165

[General]
StackLeniency: 0.2

[Difficulty]
HPDrainRate:6
CircleSize:4
OverallDifficulty:8
ApproachRate:9.3
SliderMultiplier:2
SliderTickRate:1

[TimingPoints]
5338,444.444444444444,4,2,0,50,1,0
82893,-76.9230769230769,4,2,8,50,0,0
85115,-76.9230769230769,4,2,0,50,0,0
85337,-100,4,2,8,60,0,0
85893,-100,4,2,7,60,0,0
86226,-100,4,2,8,60,0,0
88893,-58.8235294117647,4,1,8,70,0,1

[HitObjects]
427,124,84226,1,0,3:0:0:0:
427,124,84337,1,0,3:0:0:0:
427,124,84449,1,8,0:0:0:0:
",
    );

    // The last hitobject triggers the stacking
    let h = heights(&beatmap);
    assert_eq!(h.len(), 3);
    assert!(h[..h.len() - 1].iter().all(|&h| h == 0), "{h:?}");
}

const SLIDER_STACK: &str = "osu file format v14

[General]
StackLeniency: 0.7

[Difficulty]
CircleSize:4
ApproachRate:9
SliderMultiplier:1.4
SliderTickRate:1

[TimingPoints]
0,500,4,2,0,100,1,0

[HitObjects]
100,100,1000,2,0,L|200:100,2,100
100,100,1400,2,0,L|200:100,2,100
";

#[test]
fn nested_objects_share_the_stack_offset() {
    let beatmap = load_text(SLIDER_STACK);
    assert_eq!(heights(&beatmap), [1, 0]);

    let h = &beatmap.hit_objects[0];
    // Lazer: StackHeight * Scale * -6.4f.
    let offset = 1.0 * h.defaults.scale * -6.4;
    assert_eq!(
        h.stacked_position(),
        Vec2::new(100.0 + offset, 100.0 + offset)
    );
    // Two spans: the slider ends at its start.
    assert_eq!(h.stacked_end_position(), h.stacked_position());

    let OsuHitObjectKind::Slider(slider) = &h.kind else {
        panic!("not a slider");
    };
    assert!(!slider.nested.is_empty());
    for n in &slider.nested {
        assert_eq!(n.stack_height, 1);
        assert_eq!(
            n.stacked_position(),
            n.position + Vec2::new(offset, offset),
            "{:?}",
            n.kind
        );
    }
}

#[test]
fn stacking_can_be_reapplied() {
    let mut beatmap = load_text(SLIDER_STACK);
    let processed = beatmap.clone();

    beatmap.apply_stacking();
    assert_eq!(beatmap, processed);

    // Moving the second slider away removes the stack, nested objects included.
    beatmap.hit_objects[1].position = Vec2::new(400.0, 300.0);
    beatmap.apply_stacking();
    assert_eq!(heights(&beatmap), [0, 0]);
    let OsuHitObjectKind::Slider(slider) = &beatmap.hit_objects[0].kind else {
        panic!("not a slider");
    };
    assert!(slider.nested.iter().all(|n| n.stack_height == 0));
}

#[test]
fn reapplying_defaults_keeps_the_stack_height() {
    let mut beatmap = load_text(SLIDER_STACK);
    let processed = beatmap.clone();

    // Lazer creates nested objects with the slider's current stack height.
    let (control_points, difficulty) = (beatmap.control_points.clone(), beatmap.difficulty);
    for h in &mut beatmap.hit_objects {
        h.apply_defaults(&control_points, &difficulty);
    }
    assert_eq!(beatmap, processed);
}

#[test]
fn old_format_stacks_on_the_end_of_the_first_span() {
    // Before format version 6 a slider's stack end is the end of its path, even when it
    // repeats back to its start.
    let text = SLIDER_STACK
        .replace("v14", "v5")
        .replace("100,100,1400,2,0,L|200:100,2,100", "200,100,1400,1,0");
    let beatmap = load_text(&text);
    assert_eq!(heights(&beatmap), [0, -1]);

    let new =
        load_text(&SLIDER_STACK.replace("100,100,1400,2,0,L|200:100,2,100", "200,100,1400,1,0"));
    assert_eq!(heights(&new), [0, 0]);
}

#[test]
fn spinners_are_never_moved() {
    // Before format version 6 a spinner can get a stack height, but lazer's spinner has no
    // stack offset.
    let beatmap = load_text(
        "osu file format v5

[General]
StackLeniency: 0.7

[Difficulty]
ApproachRate:9

[TimingPoints]
0,500,4,2,0,100,1,0

[HitObjects]
256,192,1000,8,0,1100
256,192,1200,1,0
",
    );
    assert_eq!(heights(&beatmap), [1, 0]);
    let spinner = &beatmap.hit_objects[0];
    assert_eq!(spinner.stack_offset(), Vec2::ZERO);
    assert_eq!(spinner.stacked_position(), Vec2::new(256.0, 192.0));
    assert_eq!(spinner.stacked_end_position(), Vec2::new(256.0, 192.0));
}
