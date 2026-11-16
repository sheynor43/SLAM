use std::path::PathBuf;

use slam_formats::osu::decode_str;

use super::*;
use crate::judgement::HitResult;
use crate::mods::{GameplayMod as _, Mod};
use crate::replay::{ReplayInput, Simulator};
use crate::rules::OsuRules;

/// Circles far apart and in a stream, a slider with repeats, a spinner and a short slider.
const MAP: &str = "osu file format v14

[General]
AudioLeadIn: 0

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:8
ApproachRate:9
SliderMultiplier:1.4
SliderTickRate:1

[TimingPoints]
0,300,4,2,0,100,1,0

[HitObjects]
64,64,1000,1,0
448,320,1300,1,0
200,200,1600,1,0
220,200,1675,1,0
240,200,1750,1,0
260,200,1825,1,0
100,80,2400,2,0,B|200:40|300:120,2,140
256,192,4000,8,0,6000
400,100,6500,2,0,L|300:100,1,100
64,300,7200,1,0
";

fn playable(text: &str, mods: Vec<Mod>) -> (Beatmap, ModSet) {
    let decoded = decode_str(text).unwrap();
    let mut mods = ModSet::new(mods).unwrap();
    let beatmap = mods.playable_beatmap(decoded.beatmap).unwrap();
    (beatmap, mods)
}

/// Plays the Autoplay frames through the rules and returns the judgements that fell short.
fn shortfalls(beatmap: &Beatmap, mods: &ModSet) -> Vec<(f64, HitResult, HitResult)> {
    let frames = generate(beatmap, mods);
    let rules = OsuRules::new(beatmap, mods);
    let mut sim = Simulator::new(beatmap, ReplayInput::new(frames), rules);
    sim.run();
    sim.counted_judgements()
        .iter()
        .filter(|j| !j.max_result.is_bonus() && j.result != j.max_result)
        .map(|j| (j.time, j.result, j.max_result))
        .collect()
}

#[test]
fn autoplay_plays_the_map_perfectly_with_any_speed() {
    for mods in [
        vec![],
        vec![Mod::from_acronym("HR")],
        vec![Mod::from_acronym("DT")],
        vec![Mod::from_acronym("HT")],
        vec![Mod::from_acronym("EZ"), Mod::from_acronym("NC")],
    ] {
        let names: Vec<String> = mods.iter().map(|m| m.acronym().to_owned()).collect();
        let (beatmap, mods) = playable(MAP, mods);
        assert_eq!(shortfalls(&beatmap, &mods), [], "{names:?}");
    }
}

#[test]
fn frames_are_sorted_deterministic_and_start_below_the_playfield() {
    let (beatmap, mods) = playable(MAP, vec![]);
    let frames = generate(&beatmap, &mods);
    assert!(frames.windows(2).all(|w| w[0].time <= w[1].time));
    assert_eq!(frames, generate(&beatmap, &mods));
    assert_eq!(frames[0].time, 1000.0 - 1500.0);
    assert_eq!(frames[0].position, START_POSITION);
    assert_eq!(frames[0].actions, Actions::NONE);
    // The press of the first circle, on it, at its time.
    let press = frames.iter().find(|f| !f.actions.is_empty()).unwrap();
    assert_eq!(press.time, 1000.0);
    assert_eq!(press.position, beatmap.hit_objects[0].stacked_position());
}

#[test]
fn the_cursor_moves_over_the_whole_gap_eased_once_and_linearly_in_streams() {
    let (beatmap, mods) = playable(MAP, vec![]);
    let frames = generate(&beatmap, &mods);
    let (a, b) = (
        beatmap.hit_objects[0].stacked_position(),
        beatmap.hit_objects[1].stacked_position(),
    );
    // Half way between the first two circles (1000 and 1300 ms, far apart): eased, so
    // three quarters of the way, `-p * (p - 2)` at 0.5.
    let mid = frames
        .iter()
        .filter(|f| f.time > 1000.0 && f.time < 1300.0)
        .min_by(|x, y| (x.time - 1150.0).abs().total_cmp(&(y.time - 1150.0).abs()))
        .unwrap();
    let p = ((mid.time - 1000.0) / 300.0) as f32;
    let eased = -p * (p - 2.0);
    let expected = a + (b - a) * eased;
    assert!(
        mid.position.distance(expected) < 0.01,
        "{mid:?} vs {expected:?}"
    );
    // It starts moving at once: not after a reaction delay as in lazer.
    let early = frames
        .iter()
        .find(|f| f.time > 1000.0 + KEY_UP_DELAY && f.time < 1100.0)
        .unwrap();
    assert!(early.position.distance(a) > 1.0);
    // In the stream (20 px apart, 75 ms), linear.
    let (s, t) = (
        beatmap.hit_objects[3].stacked_position(),
        beatmap.hit_objects[4].stacked_position(),
    );
    let inside = frames
        .iter()
        .find(|f| f.time > 1675.0 && f.time < 1750.0)
        .unwrap();
    let p = ((inside.time - 1675.0) / 75.0) as f32;
    assert!(inside.position.distance(s + (t - s) * p) < 0.01);
}

#[test]
fn close_objects_alternate_buttons_and_far_ones_use_the_left_one() {
    let (beatmap, mods) = playable(MAP, vec![]);
    let frames = generate(&beatmap, &mods);
    let press_at = |time: f64| {
        frames
            .iter()
            .find(|f| f.time == time && !f.actions.is_empty())
            .unwrap()
            .actions
    };
    assert_eq!(press_at(1000.0), Actions::LEFT);
    // 250 ms from the key-up to the next press, as lazer counts: alternation.
    assert_eq!(press_at(1300.0), Actions::RIGHT);
    // 550 ms after the last circle of the stream: the left button again.
    assert_eq!(press_at(2400.0), Actions::LEFT);
    let stream = [1675.0, 1750.0, 1825.0].map(press_at);
    assert_ne!(stream[0], stream[1]);
    assert_ne!(stream[1], stream[2]);
}

#[test]
fn spinners_spin_round_the_centre() {
    let (beatmap, mods) = playable(MAP, vec![]);
    let frames = generate(&beatmap, &mods);
    let spinning: Vec<_> = frames
        .iter()
        .filter(|f| f.time > 4000.0 && f.time < 6000.0)
        .collect();
    assert!(spinning.len() > 100);
    for f in spinning {
        assert!((f.position.distance(SPINNER_CENTRE) - SPIN_RADIUS).abs() < 0.01);
        assert!(!f.actions.is_empty());
    }
}

#[test]
fn an_empty_map_has_no_frames() {
    let text = MAP.split("[HitObjects]").next().unwrap().to_owned() + "[HitObjects]\n";
    let (beatmap, mods) = playable(&text, vec![]);
    assert!(generate(&beatmap, &mods).is_empty());
}

/// lazer's test beatmaps, when `../slam-refs` is there.
#[test]
fn autoplay_plays_lazer_test_beatmaps_perfectly() {
    let dir = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../slam-refs/osu/osu.Game.Rulesets.Osu.Tests/Resources/Testing/Beatmaps"
    ));
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skipped: {} not found", dir.display());
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "osu"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty());
    for path in paths {
        let text = std::fs::read_to_string(&path).unwrap();
        for mods in [
            vec![],
            vec![Mod::from_acronym("HR"), Mod::from_acronym("DT")],
        ] {
            let (beatmap, mods) = playable(&text, mods);
            let short = shortfalls(&beatmap, &mods);
            assert!(
                short.is_empty(),
                "{}: {:?}",
                path.display(),
                &short[..short.len().min(5)]
            );
        }
    }
}

/// A circle inside a slider: the slider is followed to its end after the circle (as lazer
/// does), and the next movement starts from the slider's end.
#[test]
fn a_circle_inside_a_slider_does_not_lose_the_slider() {
    let text = MAP.split("[HitObjects]").next().unwrap().to_owned()
        + "[HitObjects]\n100,100,1000,2,0,L|400:100,1,300\n250,100,1300,1,0\n256,300,3500,1,0\n";
    let (beatmap, mods) = playable(&text, vec![]);
    assert_eq!(shortfalls(&beatmap, &mods), []);
    let slider_end = beatmap.hit_objects[0].end_time();
    let frames = generate(&beatmap, &mods);
    // Just before the slider's end the cursor is still on its path.
    let late = frames.iter().rfind(|f| f.time < slider_end).unwrap();
    assert!((late.position.y - 100.0).abs() < 1.0, "{late:?}");
}

#[test]
fn spinners_are_entered_from_outside_inside_and_the_centre() {
    // From outside: the tangent's length away, along lazer's rotation (which uses the new
    // X for Y, so the point is near the circle, not on it).
    let previous = Vec2 { x: 256.0, y: 400.0 };
    let (outside, direction) = spinner_start(previous);
    let tangent = (208.0f32 * 208.0 - SPIN_RADIUS * SPIN_RADIUS).sqrt();
    assert!(
        (outside.distance(previous) - tangent).abs() < 0.01,
        "{outside:?}"
    );
    assert!((outside.distance(SPINNER_CENTRE) - SPIN_RADIUS).abs() < 25.0);
    assert_eq!(direction, -1.0);
    let (inside, direction) = spinner_start(Vec2 { x: 266.0, y: 192.0 });
    assert_eq!(inside, Vec2 { x: 306.0, y: 192.0 });
    assert_eq!(direction, 1.0);
    let (centre, direction) = spinner_start(SPINNER_CENTRE);
    assert_eq!(centre, Vec2 { x: 256.0, y: 142.0 });
    assert_eq!(direction, 1.0);
}

#[test]
fn a_spinner_that_needs_no_spins_is_left_alone() {
    // A very short spinner: no spins required.
    let text = MAP.split("[HitObjects]").next().unwrap().to_owned()
        + "[HitObjects]\n100,100,1000,1,0\n256,192,1500,8,0,1550\n300,300,2500,1,0\n";
    let (beatmap, mods) = playable(&text, vec![]);
    let OsuHitObjectKind::Spinner(spinner) = &beatmap.hit_objects[1].kind else {
        panic!("not a spinner");
    };
    assert_eq!(spinner.spins_required, 0);
    let frames = generate(&beatmap, &mods);
    assert!(
        !frames
            .iter()
            .any(|f| f.time >= 1500.0 && f.time <= 1550.0 && !f.actions.is_empty())
    );
    assert_eq!(shortfalls(&beatmap, &mods), []);
}
