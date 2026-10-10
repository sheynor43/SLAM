//! Mods that change the beatmap, through `ModSet::playable_beatmap`.
//!
//! The asserted values come from osu!lazer 2026.1005.0-lazer:
//! osu.Game.Rulesets.Osu.Tests/Mods/TestSceneOsuModMirror.cs and
//! TestSceneOsuModDifficultyAdjust.cs. Bit-exact parity on many maps and mod combinations is
//! checked by `map_mods_lazer.rs`.

use slam_formats::osr::SettingValue;
use slam_formats::osu::{Vec2, decode_str};
use slam_osu::mods::MirrorType;
use slam_osu::objects::{NestedKind, OsuHitObject, OsuHitObjectKind};
use slam_osu::{Beatmap, Mod, ModSet};

const MAP: &str = "osu file format v14

[General]
StackLeniency: 0.7

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:8
ApproachRate:9
SliderMultiplier:1
SliderTickRate:2

[TimingPoints]
0,500,4,2,0,100,1,0

[HitObjects]
100,80,1000,2,0,B|200:40|300:120,2,250
400,300,3000,1,0
400,300,3100,1,0
256,192,4000,8,0,5000
";

fn load(mods: Vec<Mod>) -> Beatmap {
    let decoded = decode_str(MAP).unwrap();
    assert!(decoded.warnings.is_empty(), "{:?}", decoded.warnings);
    ModSet::new(mods)
        .unwrap()
        .playable_beatmap(decoded.beatmap)
        .unwrap()
}

fn mirror(reflection: MirrorType) -> Mod {
    Mod::Mirror { reflection }
}

fn da(key: &str, value: f64) -> Mod {
    let mut m = Mod::from_acronym("DA");
    assert!(m.set_setting(key, &SettingValue::Float(value)));
    m
}

/// Every nested object sits where lazer's `updateNestedPositions` puts it.
fn assert_nested_follow(h: &OsuHitObject) {
    let OsuHitObjectKind::Slider(slider) = &h.kind else {
        panic!("not a slider");
    };
    assert!(
        slider
            .nested
            .iter()
            .any(|n| matches!(n.kind, NestedKind::Tick { .. }))
    );
    assert!(
        slider
            .nested
            .iter()
            .any(|n| matches!(n.kind, NestedKind::Repeat { .. }))
    );
    for n in &slider.nested {
        let expected = match n.kind {
            NestedKind::Head => h.position,
            NestedKind::Tail { .. } => h.end_position(),
            NestedKind::Tick { path_progress, .. } | NestedKind::Repeat { path_progress, .. } => {
                h.position + slider.path.position_at(path_progress)
            }
        };
        assert_eq!(n.position, expected, "{:?}", n.kind);
    }
}

fn path_points(h: &OsuHitObject) -> Vec<Vec2> {
    let OsuHitObjectKind::Slider(slider) = &h.kind else {
        panic!("not a slider");
    };
    slider.path.calculated_path().to_vec()
}

#[test]
fn mirror_flips_objects_and_their_nested_objects() {
    let plain = load(vec![]);
    let cases = [
        (MirrorType::Horizontal, true, false),
        (MirrorType::Vertical, false, true),
        (MirrorType::Both, true, true),
    ];
    for (reflection, flip_x, flip_y) in cases {
        let flipped = load(vec![mirror(reflection)]);
        for (a, b) in plain.hit_objects.iter().zip(&flipped.hit_objects) {
            let x = if flip_x {
                512.0 - a.position.x
            } else {
                a.position.x
            };
            let y = if flip_y {
                384.0 - a.position.y
            } else {
                a.position.y
            };
            assert_eq!(b.position, Vec2::new(x, y), "{reflection:?}");
        }

        let slider = &flipped.hit_objects[0];
        assert_nested_follow(slider);
        // The path itself is mirrored relative to the head.
        for (p, q) in path_points(&plain.hit_objects[0])
            .iter()
            .zip(path_points(slider))
        {
            let x = if flip_x { -p.x } else { p.x };
            let y = if flip_y { -p.y } else { p.y };
            assert!(
                (q.x - x).abs() < 1e-3 && (q.y - y).abs() < 1e-3,
                "{p:?} {q:?}"
            );
        }
        // Stacking still runs, after the flip.
        assert_eq!(flipped.hit_objects[1].stack_height, 1);
    }

    // A value without a name flips nothing.
    assert_eq!(load(vec![mirror(MirrorType::Undefined(5))]), plain);
}

#[test]
fn hard_rock_flips_vertically_and_raises_difficulty() {
    let plain = load(vec![]);
    let hr = load(vec![Mod::HardRock]);
    let vertical = load(vec![mirror(MirrorType::Vertical)]);

    for ((a, b), c) in plain
        .hit_objects
        .iter()
        .zip(&hr.hit_objects)
        .zip(&vertical.hit_objects)
    {
        assert_eq!(b.position, Vec2::new(a.position.x, 384.0 - a.position.y));
        assert_eq!(b.position, c.position);
    }
    assert_nested_follow(&hr.hit_objects[0]);

    assert_eq!(hr.difficulty.circle_size, 4.0 * 1.3);
    assert_eq!(hr.difficulty.approach_rate, 10.0);
    // The defaults use the changed difficulty.
    assert_eq!(hr.hit_objects[1].defaults.time_preempt, 450.0);
    assert!(hr.hit_objects[1].defaults.scale < plain.hit_objects[1].defaults.scale);
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/Mods/TestSceneOsuModDifficultyAdjust.cs
#[test]
fn difficulty_adjust_sets_scale_and_preempt() {
    let scale = |cs| {
        load(vec![da("circle_size", cs)]).hit_objects[1]
            .defaults
            .scale
    };
    let preempt = |ar| {
        load(vec![da("approach_rate", ar)]).hit_objects[1]
            .defaults
            .time_preempt
    };
    // Lazer compares approximately: the scale includes the 1.00041 fudge factor.
    assert!((scale(1.0) - 0.78).abs() < 1e-3);
    assert!((scale(10.0) - 0.15).abs() < 1e-3);
    assert_eq!(preempt(1.0), 1680.0);
    assert_eq!(preempt(10.0), 450.0);

    // The nested objects of sliders get the same scale.
    let b = load(vec![da("circle_size", 1.0)]);
    let OsuHitObjectKind::Slider(slider) = &b.hit_objects[0].kind else {
        panic!("not a slider");
    };
    assert!(
        slider
            .nested
            .iter()
            .all(|n| n.defaults.scale == b.hit_objects[0].defaults.scale)
    );
}

#[test]
fn easy_lowers_difficulty() {
    let ez = load(vec![Mod::from_acronym("EZ")]);
    assert_eq!(
        (
            ez.difficulty.circle_size,
            ez.difficulty.approach_rate,
            ez.difficulty.drain_rate,
            ez.difficulty.overall_difficulty
        ),
        (2.0, 4.5, 2.5, 4.0)
    );
    assert_eq!(ez.hit_objects, {
        let mut plain = load(vec![]).hit_objects;
        let d = ez.difficulty;
        let control_points = &ez.control_points;
        for h in &mut plain {
            h.apply_defaults(control_points, &d);
        }
        // Lower AR changes the stacking window: recompute it like the pipeline does.
        let mut b = load(vec![]);
        b.hit_objects = plain;
        b.difficulty = d;
        b.apply_stacking();
        b.hit_objects
    });
}
