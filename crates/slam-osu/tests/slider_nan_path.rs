//! A slider whose path ends in NaN, as lazer computes it (#124), goes through beatmap
//! processing and replay simulation without panicking.

use slam_formats::osu::{PathType, decode_str};
use slam_osu::objects::OsuHitObjectKind;
use slam_osu::replay::{ReplayInput, Simulator};
use slam_osu::rules::OsuRules;
use slam_osu::{Beatmap, ModSet};

/// A zero-length linear segment, then an optimised Catmull segment, trimmed to 1 px.
const MAP: &str = "osu file format v14

[Difficulty]
SliderMultiplier:1.4

[TimingPoints]
0,500,4,2,0,100,1,0

[HitObjects]
256,192,1000,2,0,L|C|256:192|-67:-314.84814|-67:-314.84814,1,1
256,192,2000,1,0
";

#[test]
fn slider_with_a_nan_path_end_plays_through() {
    let map = Beatmap::from_file(decode_str(MAP).unwrap().beatmap).unwrap();

    let OsuHitObjectKind::Slider(slider) = &map.hit_objects[0].kind else {
        panic!("expected a slider");
    };
    let types: Vec<_> = slider
        .path
        .control_points()
        .iter()
        .map(|p| p.path_type)
        .collect();
    assert_eq!(
        types,
        [Some(PathType::Linear), Some(PathType::Catmull), None, None]
    );
    let end = slider.path.calculated_path()[1];
    assert!(end.x.is_nan() && end.y.is_nan());

    let mods = ModSet::default();
    let mut sim = Simulator::new(&map, ReplayInput::default(), OsuRules::new(&map, &mods));
    sim.run();
    assert!(sim.is_finished());
}
