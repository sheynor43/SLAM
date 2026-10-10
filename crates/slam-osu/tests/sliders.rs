//! Slider judgement on synthetic beatmaps through the replay simulator.
//!
//! The cases are lazer's `TestSceneSliderInput`, `TestSceneSliderFollowCircleInput`,
//! `TestSceneSliderLateHitJudgement`, `TestSceneSliderEarlyHitJudgement` and the slider cases
//! of the hit policy tests. Lazer builds its sliders in code; here they come from `.osu` text,
//! with the properties `.osu` cannot express (tick distance multiplier, exact velocity, paths
//! without an expected distance) set on the slider before its defaults are applied again.
//! Lazer's `TestSceneStartTimeOrderedHitPolicy` uses custom hit windows; its cases that depend
//! on them are adapted to the real windows.

use slam_formats::osu::{PathControlPoint, PathType, Vec2, decode_str};
use slam_osu::Beatmap;
use slam_osu::SliderPath;
use slam_osu::judgement::HitResult;
use slam_osu::objects::{NestedKind, OsuHitObjectKind, Slider};
use slam_osu::replay::{Actions, InputFrame, ReplayInput, Rules, Simulator};
use slam_osu::rules::{HitPolicy, OsuRules, RulesOptions};
use slam_osu::{Mod, ModSet};

use HitResult::{
    Great, IgnoreHit, IgnoreMiss, LargeTickHit, LargeTickMiss, Meh, Miss, Ok, SliderTailHit,
    SmallTickHit,
};

const NONE: Actions = Actions::NONE;
const LEFT: Actions = Actions::LEFT;
const RIGHT: Actions = Actions::RIGHT;

fn both() -> Actions {
    LEFT | RIGHT
}

/// What a judgement is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Circle,
    Slider,
    Head,
    Tick,
    Repeat,
    Tail,
}

/// One judgement of a play.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Judged {
    index: u32,
    part: Part,
    result: HitResult,
    max_result: HitResult,
    /// Offset from the start time of the judged object (nested or not).
    offset: f64,
}

/// A beatmap from `.osu` sections.
fn beatmap(difficulty: &str, timing: &str, objects: &str, stack_leniency: f32) -> Beatmap {
    let text = format!(
        "osu file format v14\n\n[General]\nStackLeniency:{stack_leniency}\n\n[Difficulty]\n\
         {difficulty}\n\n[TimingPoints]\n{timing}\n\n[HitObjects]\n{objects}"
    );
    Beatmap::from_file(decode_str(&text).unwrap().beatmap).unwrap()
}

/// Lazer's default `BeatmapDifficulty` (everything 5, slider multiplier 1.4) with changes.
fn difficulty(od: f32, cs: f32, slider_multiplier: f64, tick_rate: f64) -> String {
    format!(
        "HPDrainRate:5\nCircleSize:{cs}\nOverallDifficulty:{od}\nApproachRate:5\n\
         SliderMultiplier:{slider_multiplier}\nSliderTickRate:{tick_rate}"
    )
}

/// Lazer's default timing point: a 1000 ms beat.
const BEAT_1000: &str = "0,1000,4,2,0,100,1,0";

/// Changes slider `index` and applies its defaults again, as lazer's tests set slider
/// properties before the beatmap is loaded.
fn adjust(map: &mut Beatmap, index: usize, f: impl FnOnce(&mut Slider)) {
    let Beatmap {
        hit_objects,
        control_points,
        difficulty,
        ..
    } = map;
    let o = &mut hit_objects[index];
    let OsuHitObjectKind::Slider(s) = &mut o.kind else {
        panic!("object {index} is not a slider");
    };
    f(s);
    o.apply_defaults(control_points, difficulty);
}

/// A path of `kind` through `points` without an expected distance.
fn path(kind: PathType, points: &[(f32, f32)]) -> SliderPath {
    let control_points = points
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| PathControlPoint {
            position: Vec2::new(x, y),
            path_type: (i == 0).then_some(kind),
        })
        .collect();
    SliderPath::new(control_points, None, false)
}

fn frame(time: f64, x: f32, y: f32, actions: Actions) -> InputFrame {
    InputFrame {
        time,
        position: Vec2::new(x, y),
        actions,
    }
}

fn play(map: &Beatmap, options: RulesOptions, frames: &[InputFrame]) -> Vec<Judged> {
    let rules = OsuRules::with_options(map, options);
    let capacity = rules.max_judgements();
    let mut sim = Simulator::new(map, ReplayInput::new(frames.to_vec()), rules);
    sim.run();
    assert!(sim.is_finished());
    let events = sim.log().events();
    assert!(events.len() <= capacity);
    events
        .iter()
        .map(|j| {
            let o = &map.hit_objects[j.object.index as usize];
            let (part, start_time) = match (&o.kind, j.object.nested) {
                (OsuHitObjectKind::Slider(s), Some(k)) => {
                    let n = &s.nested[k as usize];
                    let part = match n.kind {
                        NestedKind::Head => Part::Head,
                        NestedKind::Tick { .. } => Part::Tick,
                        NestedKind::Repeat { .. } => Part::Repeat,
                        NestedKind::Tail { .. } => Part::Tail,
                    };
                    (part, n.start_time)
                }
                (OsuHitObjectKind::Slider(_), None) => (Part::Slider, o.start_time),
                _ => (Part::Circle, o.start_time),
            };
            Judged {
                index: j.object.index,
                part,
                result: j.result,
                max_result: j.max_result,
                offset: j.time - start_time,
            }
        })
        .collect()
}

/// [`play`] from the start of the beatmap. Lazer's tests play their replays in real time from
/// before the first object, so objects are alive when the first frame arrives; a catching up
/// simulation would jump straight to the first frame instead. A frame at 0 at the position of
/// the first one starts the replay early without moving the cursor.
fn play_from_start(map: &Beatmap, options: RulesOptions, frames: &[InputFrame]) -> Vec<Judged> {
    let mut all = vec![frame(0.0, frames[0].position.x, frames[0].position.y, NONE)];
    all.extend_from_slice(frames);
    play(map, options, &all)
}

fn lazer(policy: HitPolicy) -> RulesOptions {
    RulesOptions::with_policy(policy)
}

fn classic() -> RulesOptions {
    RulesOptions::for_mods(&ModSet::new(vec![Mod::from_acronym("CL")]).unwrap())
}

/// The results of the parts of object `index`, in judgement order.
fn parts(j: &[Judged], index: u32, part: Part) -> Vec<HitResult> {
    j.iter()
        .filter(|j| j.index == index && j.part == part)
        .map(|j| j.result)
        .collect()
}

/// The single judgement of a part of object `index`.
fn single(j: &[Judged], index: u32, part: Part) -> Judged {
    let found: Vec<&Judged> = j
        .iter()
        .filter(|j| j.index == index && j.part == part)
        .collect();
    assert_eq!(found.len(), 1, "{part:?} of object {index}: {j:#?}");
    *found[0]
}

fn result(j: &[Judged], index: u32, part: Part) -> HitResult {
    single(j, index, part).result
}

/// Lazer's `assertAllMaxJudgements`.
fn assert_all_max(j: &[Judged]) {
    assert!(!j.is_empty());
    for judged in j {
        assert_eq!(judged.result, judged.max_result, "{j:#?}");
    }
}

/// The second to last judgement (lazer's tail assertions in `TestSceneSliderInput`).
fn second_last(j: &[Judged]) -> Judged {
    let tail = j[j.len() - 2];
    assert_eq!(tail.part, Part::Tail);
    assert_eq!(j[j.len() - 1].part, Part::Slider);
    tail
}

// TestSceneSliderInput

const TIME_BEFORE_SLIDER: f64 = 250.0;
const TIME_SLIDER_START: f64 = 1500.0;
const TIME_DURING_SLIDE_1: f64 = 2500.0;
const TIME_DURING_SLIDE_2: f64 = 3000.0;
const TIME_DURING_SLIDE_3: f64 = 3500.0;
const TIME_DURING_SLIDE_4: f64 = 3800.0;
const TIME_SLIDER_END: f64 = 4000.0;
const SLIDER_PATH_LENGTH: f32 = 25.0;

/// `TestSceneSliderInput.performTest`'s slider: 25 px to the right of (0, 0) at a slider
/// velocity of 0.1, from 1500 to 4000 ms, with ticks.
fn input_slider() -> Beatmap {
    let map = beatmap(
        &difficulty(5.0, 5.0, 1.0, 3.0),
        &format!("{BEAT_1000}\n0,-1000,4,2,0,100,0,0"),
        "0,0,1500,2,0,L|25:0,1,25\n",
        0.7,
    );
    assert_eq!(map.hit_objects[0].end_time(), TIME_SLIDER_END);
    map
}

fn input_test(frames: &[InputFrame]) -> Vec<Judged> {
    play_from_start(&input_slider(), lazer(HitPolicy::StartTimeOrdered), frames)
}

/// Lazer's `assertMidSliderJudgements`: tracking acquired by the tail.
fn assert_tail_tracked(j: &[Judged]) {
    assert_eq!(second_last(j).result, SliderTailHit, "{j:#?}");
}

/// Lazer's `assertMidSliderJudgementFail`: tracking lost at the tail.
fn assert_tail_lost(j: &[Judged]) {
    assert_eq!(second_last(j).result, IgnoreMiss, "{j:#?}");
}

#[test]
fn input_press_both_keys_simultaneously_and_release_one() {
    let j = input_test(&[
        frame(TIME_SLIDER_START, 0.0, 0.0, both()),
        frame(TIME_DURING_SLIDE_1, 0.0, 0.0, RIGHT),
    ]);
    assert_all_max(&j);
    assert_eq!(j[0].part, Part::Head);
}

#[test]
fn input_invalid_key_transfer() {
    let j = input_test(&[
        frame(TIME_BEFORE_SLIDER, 0.0, 0.0, LEFT),
        frame(TIME_SLIDER_START, 0.0, 0.0, both()),
        frame(TIME_DURING_SLIDE_1, 0.0, 0.0, LEFT),
    ]);
    assert_tail_lost(&j);
}

#[test]
fn input_left_before_slider_then_right_then_letting_go_of_left() {
    let j = input_test(&[
        frame(TIME_SLIDER_START, 0.0, 0.0, LEFT),
        frame(TIME_DURING_SLIDE_1, 0.0, 0.0, both()),
        frame(TIME_DURING_SLIDE_2, 0.0, 0.0, RIGHT),
    ]);
    assert_all_max(&j);
}

#[test]
fn input_tracking_retention_left_right_left() {
    let j = input_test(&[
        frame(TIME_BEFORE_SLIDER, 0.0, 0.0, LEFT),
        frame(TIME_SLIDER_START, 0.0, 0.0, both()),
        frame(TIME_DURING_SLIDE_1, 0.0, 0.0, RIGHT),
    ]);
    assert_all_max(&j);
}

#[test]
fn input_tracking_preclicked() {
    let j = input_test(&[frame(TIME_BEFORE_SLIDER, 0.0, 0.0, LEFT)]);
    assert_eq!(second_last(&j).result, SliderTailHit);
    assert_eq!(j[0].part, Part::Head);
    assert!(!j[0].result.is_hit());
}

#[test]
fn input_tracking_return_mid_slider() {
    let j = input_test(&[
        frame(TIME_SLIDER_START, 0.0, 0.0, LEFT),
        frame(TIME_DURING_SLIDE_1, 150.0, 150.0, LEFT),
        frame(TIME_DURING_SLIDE_2, 200.0, 200.0, LEFT),
        frame(TIME_DURING_SLIDE_3, 0.0, 0.0, LEFT),
        frame(TIME_DURING_SLIDE_4, 0.0, 0.0, LEFT),
    ]);
    assert_tail_tracked(&j);
    // Ticks while the cursor is away are missed.
    assert!(parts(&j, 0, Part::Tick).contains(&LargeTickMiss));
}

#[test]
fn input_tracking_return_mid_slider_key_down_before() {
    let j = input_test(&[
        frame(TIME_BEFORE_SLIDER, 0.0, 0.0, LEFT),
        frame(TIME_SLIDER_START, 0.0, 0.0, both()),
        frame(TIME_DURING_SLIDE_1, 0.0, 0.0, LEFT),
        frame(TIME_DURING_SLIDE_2, 200.0, 200.0, LEFT),
        frame(TIME_DURING_SLIDE_3, 0.0, 0.0, LEFT),
        frame(TIME_DURING_SLIDE_4, 0.0, 0.0, LEFT),
    ]);
    assert_tail_lost(&j);
}

#[test]
fn input_tracking_mid_slider() {
    let j = input_test(&[
        frame(TIME_DURING_SLIDE_1, 150.0, 150.0, LEFT),
        frame(TIME_DURING_SLIDE_2, 200.0, 200.0, LEFT),
        frame(TIME_DURING_SLIDE_3, 0.0, 0.0, LEFT),
        frame(TIME_DURING_SLIDE_4, 0.0, 0.0, LEFT),
    ]);
    assert_tail_tracked(&j);
}

#[test]
fn input_mid_slider_tracking_acquired() {
    let j = input_test(&[
        frame(TIME_BEFORE_SLIDER, 0.0, 0.0, LEFT),
        frame(TIME_SLIDER_START, 0.0, 0.0, both()),
        frame(TIME_DURING_SLIDE_1, 100.0, 100.0, NONE),
        frame(TIME_DURING_SLIDE_2, 0.0, 0.0, LEFT),
    ]);
    assert_tail_tracked(&j);
}

#[test]
fn input_mid_slider_tracking_acquired_with_mouse_down_outside_slider() {
    let j = input_test(&[
        frame(TIME_BEFORE_SLIDER, 0.0, 0.0, LEFT),
        frame(TIME_SLIDER_START, 0.0, 0.0, both()),
        frame(TIME_DURING_SLIDE_1, 100.0, 100.0, RIGHT),
        frame(TIME_DURING_SLIDE_2, 0.0, 0.0, RIGHT),
    ]);
    assert_tail_tracked(&j);
}

#[test]
fn input_tracking_released_valid_key() {
    let j = input_test(&[
        frame(TIME_SLIDER_START, 0.0, 0.0, LEFT),
        frame(TIME_DURING_SLIDE_1, 100.0, 100.0, LEFT),
        frame(TIME_DURING_SLIDE_2, 100.0, 100.0, NONE),
        frame(TIME_DURING_SLIDE_3, 100.0, 100.0, LEFT),
        frame(TIME_DURING_SLIDE_4, 0.0, 0.0, LEFT),
    ]);
    assert_tail_tracked(&j);
}

#[test]
fn input_tracking_area_edge() {
    let j = input_test(&[
        frame(TIME_SLIDER_START, 0.0, 0.0, LEFT),
        frame(TIME_SLIDER_START + 250.0, 0.0, 64.0 * 1.19, LEFT),
        frame(TIME_SLIDER_END, SLIDER_PATH_LENGTH, 64.0 * 1.199, LEFT),
    ]);
    assert_all_max(&j);
}

#[test]
fn input_tracking_area_outside_edge() {
    let j = input_test(&[
        frame(TIME_SLIDER_START, 0.0, 0.0, LEFT),
        frame(TIME_SLIDER_START + 250.0, 0.0, 64.0 * 1.21, LEFT),
        frame(TIME_SLIDER_END, SLIDER_PATH_LENGTH, 64.0 * 1.201, LEFT),
    ]);
    assert_tail_lost(&j);
}

#[test]
fn input_hit_next_circle_during_tail_leniency() {
    // 240 BPM; the slider lasts one beat and has no ticks (lazer sets a tick distance
    // multiplier of 10, here the tick rate is low enough). The next circle is 1/4 beat later.
    const BEAT_LENGTH: f64 = 60000.0 / 240.0;
    const SLIDER_END: f64 = TIME_SLIDER_START + BEAT_LENGTH;
    const LAST_TICK_TIME: f64 = SLIDER_END - 36.0;
    let mut map = beatmap(
        &difficulty(5.0, 5.0, 1.0, 3.0),
        "0,250,4,2,0,100,1,0",
        "0,0,1500,2,0,P|100:0,1,100\n140,0,1812.5,1,0\n",
        0.7,
    );
    adjust(&mut map, 0, |s| s.tick_distance_multiplier = 10.0);
    assert_eq!(map.hit_objects[0].end_time(), SLIDER_END);
    let j = play_from_start(
        &map,
        lazer(HitPolicy::StartTimeOrdered),
        &[
            frame(TIME_SLIDER_START, 0.0, 0.0, LEFT),
            frame(LAST_TICK_TIME + 20.0, 140.0, 0.0, RIGHT),
        ],
    );
    assert!(j.iter().all(|j| j.result.is_hit()), "{j:#?}");
    // The tail is hit within its leniency, before the slider ends.
    let tail = single(&j, 0, Part::Tail);
    assert!(tail.offset < 0.0 && tail.offset >= -36.0, "{tail:?}");
}

// TestSceneSliderFollowCircleInput

#[test]
fn follow_circle_maximum_distance_tracking_without_movement() {
    for circle_size in [0.0f32, 5.0, 10.0] {
        for velocity in [0.0f64, 5.0, 10.0] {
            // `LegacyRulesetExtensions.CalculateScaleFromCircleSize` with the fudge.
            let scale = (1.0 - 0.7 * (circle_size - 5.0) / 5.0) / 2.0 * 1.00041;
            let circle_radius = 64.0 * scale;
            let follow_circle_radius = circle_radius * 1.2;
            let mut map = beatmap(
                &difficulty(5.0, circle_size, 1.4, 1.0),
                BEAT_1000,
                "0,0,1000,2,0,L|10:0,1,10\n",
                0.7,
            );
            adjust(&mut map, 0, |s| {
                s.path = SliderPath::new(
                    vec![
                        PathControlPoint {
                            position: Vec2::new(0.0, 0.0),
                            path_type: Some(PathType::Linear),
                        },
                        PathControlPoint {
                            position: Vec2::new(follow_circle_radius, 0.0),
                            path_type: None,
                        },
                    ],
                    Some(f64::from(follow_circle_radius)),
                    false,
                );
                // Lazer's slider velocity bindable has a minimum of 0.1.
                s.slider_velocity_multiplier = velocity.max(0.1);
            });
            let j = play_from_start(
                &map,
                lazer(HitPolicy::StartTimeOrdered),
                &[frame(1000.0, -circle_radius + 1.0, 0.0, LEFT)],
            );
            assert_all_max(&j);
        }
    }
}

// TestSceneSliderLateHitJudgement

const LATE_START: f64 = 1000.0;
const LATE_END: f64 = 1500.0;
const LATE_START_X: f32 = 156.0;
const LATE_END_X: f32 = 356.0;
const LATE_Y: f32 = 192.0;

/// `TestSceneSliderLateHitJudgement.performTest`'s slider: 200 px from (156, 192) at slider
/// multiplier 4 (1000 to 1500 ms), tick distance multiplier 3, no stacking.
///
/// Lazer's test sets a slider multiplier of 4 in code; `.osu` files clamp it to 3.6, so it is
/// set after decoding, before the slider's defaults are applied again.
fn late_slider(f: impl FnOnce(&mut Slider)) -> Beatmap {
    let mut map = beatmap(
        &difficulty(5.0, 5.0, 4.0, 3.0),
        BEAT_1000,
        "156,192,1000,2,0,L|356:192,1,200\n",
        0.0,
    );
    map.difficulty.slider_multiplier = 4.0;
    adjust(&mut map, 0, |s| {
        s.tick_distance_multiplier = 3.0;
        f(s);
    });
    map
}

/// The same slider with one repeat over `length` px.
fn late_repeat_slider(length: f32, f: impl FnOnce(&mut Slider)) -> Beatmap {
    let end = LATE_START_X + length;
    let mut map = beatmap(
        &difficulty(5.0, 5.0, 4.0, 3.0),
        BEAT_1000,
        &format!("156,192,1000,2,0,L|{end}:192,2,{length}\n"),
        0.0,
    );
    map.difficulty.slider_multiplier = 4.0;
    adjust(&mut map, 0, |s| {
        s.tick_distance_multiplier = 3.0;
        f(s);
    });
    map
}

fn late_test(map: &Beatmap, frames: &[InputFrame]) -> Vec<Judged> {
    play_from_start(map, lazer(HitPolicy::StartTimeOrdered), frames)
}

fn late_frames(offset: f64, end_x: f32) -> [InputFrame; 2] {
    [
        frame(LATE_START + offset, LATE_START_X, LATE_Y, LEFT),
        frame(LATE_END + offset, end_x, LATE_Y, LEFT),
    ]
}

#[test]
fn late_hit_in_range_tracks() {
    let j = late_test(&late_slider(|_| {}), &late_frames(99.0, LATE_END_X));
    assert_eq!(result(&j, 0, Part::Head), Ok);
    assert_eq!(result(&j, 0, Part::Tail), SliderTailHit);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn late_hit_out_of_range_does_not_track() {
    let map = late_slider(|s| s.slider_velocity_multiplier = 2.0);
    let j = late_test(&map, &late_frames(99.0, LATE_END_X));
    assert_eq!(result(&j, 0, Part::Head), Ok);
    assert_eq!(result(&j, 0, Part::Tail), IgnoreMiss);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn late_hit_in_range_hits_ticks() {
    let map = late_slider(|s| s.tick_distance_multiplier = 0.2);
    let j = late_test(&map, &late_frames(149.0, LATE_END_X));
    assert_eq!(result(&j, 0, Part::Head), Meh);
    assert_eq!(parts(&j, 0, Part::Tick)[..4], [LargeTickHit; 4]);
    assert_eq!(result(&j, 0, Part::Tail), SliderTailHit);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn late_hit_out_of_range_does_not_hit_ticks() {
    let map = late_slider(|s| {
        s.slider_velocity_multiplier = 2.0;
        s.tick_distance_multiplier = 0.2;
    });
    let j = late_test(&map, &late_frames(149.0, LATE_END_X));
    assert_eq!(result(&j, 0, Part::Head), Meh);
    assert_eq!(parts(&j, 0, Part::Tick)[..2], [LargeTickMiss; 2]);
    assert_eq!(result(&j, 0, Part::Tail), IgnoreMiss);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn late_miss_head_in_range_does_not_track() {
    let map = late_slider(|s| s.tick_distance_multiplier = 0.2);
    let j = late_test(&map, &late_frames(151.0, LATE_END_X));
    assert_eq!(result(&j, 0, Part::Head), Miss);
    assert_eq!(parts(&j, 0, Part::Tick)[..4], [LargeTickMiss; 4]);
    assert_eq!(result(&j, 0, Part::Tail), IgnoreMiss);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreMiss);
}

#[test]
fn late_hit_short_slider_hits_all() {
    let map = late_repeat_slider(20.0, |s| s.tick_distance_multiplier = 0.01);
    let j = late_test(&map, &late_frames(149.0, LATE_START_X));
    assert_eq!(result(&j, 0, Part::Head), Meh);
    let ticks = parts(&j, 0, Part::Tick);
    assert!(!ticks.is_empty() && ticks.iter().all(|&r| r == LargeTickHit));
    assert_eq!(result(&j, 0, Part::Repeat), LargeTickHit);
    assert_eq!(result(&j, 0, Part::Tail), SliderTailHit);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn late_hit_in_range_hits_repeat() {
    let map = late_repeat_slider(50.0, |_| {});
    let j = late_test(&map, &late_frames(149.0, LATE_START_X));
    assert_eq!(result(&j, 0, Part::Head), Meh);
    assert_eq!(result(&j, 0, Part::Repeat), LargeTickHit);
    assert_eq!(result(&j, 0, Part::Tail), SliderTailHit);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn late_hit_does_not_hit_ticks_if_any_out_of_range() {
    let map = late_slider(|s| {
        s.path = path(
            PathType::PerfectCurve,
            &[(0.0, 0.0), (70.0, 70.0), (20.0, 0.0)],
        );
        s.tick_distance_multiplier = 0.03;
        s.slider_velocity_multiplier = 6.0;
    });
    let j = late_test(&map, &late_frames(149.0, LATE_START_X));
    assert_eq!(result(&j, 0, Part::Head), Meh);
    // One tick was out of range, so all of them are missed.
    let ticks = parts(&j, 0, Part::Tick);
    assert!(!ticks.is_empty() && ticks.iter().all(|&r| r == LargeTickMiss));
    // Tracking starts just before the end, so the leniency lets the tail be hit.
    assert_eq!(result(&j, 0, Part::Tail), SliderTailHit);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn late_hit_in_range_does_not_hit_out_of_range_tick() {
    let map = late_slider(|s| {
        s.path = path(
            PathType::PerfectCurve,
            &[(0.0, 0.0), (50.0, 50.0), (20.0, 0.0)],
        );
        s.tick_distance_multiplier = 0.3;
        s.slider_velocity_multiplier = 3.0;
    });
    let j = late_test(&map, &late_frames(149.0, LATE_START_X));
    assert_eq!(result(&j, 0, Part::Head), Meh);
    assert_eq!(parts(&j, 0, Part::Tick)[0], LargeTickMiss);
    assert_eq!(result(&j, 0, Part::Tail), SliderTailHit);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn late_hit_in_range_does_not_hit_out_of_range_tick_and_tracking_limited_to_ball() {
    let map = late_slider(|s| {
        s.path = path(
            PathType::PerfectCurve,
            &[(0.0, 0.0), (50.0, 50.0), (20.0, 0.0)],
        );
        s.tick_distance_multiplier = 0.25;
        s.slider_velocity_multiplier = 3.0;
    });
    let j = late_test(&map, &late_frames(149.0, LATE_START_X));
    assert_eq!(result(&j, 0, Part::Head), Meh);
    assert_eq!(parts(&j, 0, Part::Tick)[..2], [LargeTickMiss; 2]);
    assert_eq!(result(&j, 0, Part::Tail), SliderTailHit);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn late_hit_with_edge_hit() {
    let map = late_slider(|s| {
        s.path = path(
            PathType::PerfectCurve,
            &[(0.0, 0.0), (50.0, 50.0), (20.0, 0.0)],
        );
        s.tick_distance_multiplier = 0.35;
        s.slider_velocity_multiplier = 4.0;
    });
    let (x, y) = (LATE_START_X - 20.0, LATE_Y - 20.0);
    let j = late_test(
        &map,
        &[
            frame(LATE_START + 149.0, x, y, LEFT),
            frame(LATE_END + 149.0, x, y, LEFT),
        ],
    );
    assert_eq!(result(&j, 0, Part::Head), Meh);
    assert_eq!(parts(&j, 0, Part::Tick)[0], LargeTickMiss);
    assert_eq!(result(&j, 0, Part::Tail), IgnoreMiss);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn late_hit_slider_stream() {
    // 20 sliders at 200 BPM 1/4, each head pressed 75 ms late with alternating keys.
    let objects: String = (0..20)
        .map(|i| format!("156,192,{},2,0,L|176:192,1,20\n", 1000 + 75 * i))
        .collect();
    let mut map = beatmap(&difficulty(5.0, 5.0, 4.0, 3.0), BEAT_1000, &objects, 0.0);
    map.difficulty.slider_multiplier = 4.0;
    for i in 0..20 {
        adjust(&mut map, i, |s| {
            s.path = path(PathType::Linear, &[(0.0, 0.0), (20.0, 0.0)]);
            s.tick_distance_multiplier = 3.0;
        });
    }
    let frames: Vec<InputFrame> = (0..20)
        .flat_map(|i| {
            let start = LATE_START + 75.0 * f64::from(i);
            let key = if i % 2 == 0 { LEFT } else { RIGHT };
            [
                frame(start + 75.0, LATE_START_X, LATE_Y, key),
                frame(start + 140.0, LATE_START_X, LATE_Y, NONE),
            ]
        })
        .collect();
    let j = late_test(&map, &frames);
    let heads: Vec<HitResult> = j
        .iter()
        .filter(|j| j.part == Part::Head)
        .map(|j| j.result)
        .collect();
    assert_eq!(heads, [Ok; 20]);
}

// TestSceneSliderEarlyHitJudgement

const EARLY_START: f64 = 1000.0;
const EARLY_END: f64 = 3000.0;
const EARLY_START_X: f32 = 156.0;
const EARLY_END_X: f32 = 356.0;
const EARLY_Y: f32 = 192.0;
const INSIDE_FOLLOW: f32 = 35.0;
const OUTSIDE_FOLLOW: f32 = INSIDE_FOLLOW * 2.0;

/// `TestSceneSliderEarlyHitJudgement.performTest`'s slider: 200 px from (156, 192), OD 0,
/// 1000 to 3000 ms, tick distance multiplier 3 (one tick).
fn early_test(frames: &[InputFrame]) -> Vec<Judged> {
    let mut map = beatmap(
        &difficulty(0.0, 5.0, 1.0, 3.0),
        BEAT_1000,
        "156,192,1000,2,0,L|356:192,1,200\n",
        0.7,
    );
    adjust(&mut map, 0, |s| s.tick_distance_multiplier = 3.0);
    assert_eq!(map.hit_objects[0].end_time(), EARLY_END);
    play_from_start(&map, lazer(HitPolicy::StartTimeOrdered), frames)
}

#[test]
fn early_hit_move_into_follow_region() {
    let j = early_test(&[
        frame(EARLY_START - 150.0, EARLY_START_X, EARLY_Y, LEFT),
        frame(
            EARLY_START - 100.0,
            EARLY_START_X + INSIDE_FOLLOW,
            EARLY_Y,
            LEFT,
        ),
        frame(
            EARLY_END - 100.0,
            EARLY_END_X + INSIDE_FOLLOW,
            EARLY_Y,
            LEFT,
        ),
    ]);
    assert_eq!(result(&j, 0, Part::Head), Meh);
    assert_eq!(result(&j, 0, Part::Tick), LargeTickHit);
    assert_eq!(result(&j, 0, Part::Tail), SliderTailHit);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn early_hit_and_release_in_follow_region() {
    let j = early_test(&[
        frame(EARLY_START - 150.0, EARLY_START_X, EARLY_Y, LEFT),
        frame(
            EARLY_START - 100.0,
            EARLY_START_X + INSIDE_FOLLOW,
            EARLY_Y,
            LEFT,
        ),
        frame(
            EARLY_START - 50.0,
            EARLY_START_X + INSIDE_FOLLOW,
            EARLY_Y,
            NONE,
        ),
        frame(EARLY_END - 50.0, EARLY_END_X + INSIDE_FOLLOW, EARLY_Y, LEFT),
    ]);
    assert_eq!(result(&j, 0, Part::Head), Meh);
    assert_eq!(result(&j, 0, Part::Tick), LargeTickMiss);
    assert_eq!(result(&j, 0, Part::Tail), IgnoreMiss);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn early_hit_and_repress_in_follow_region() {
    let j = early_test(&[
        frame(EARLY_START - 150.0, EARLY_START_X, EARLY_Y, LEFT),
        frame(
            EARLY_START - 100.0,
            EARLY_START_X + INSIDE_FOLLOW,
            EARLY_Y,
            LEFT,
        ),
        frame(
            EARLY_START - 75.0,
            EARLY_START_X + INSIDE_FOLLOW,
            EARLY_Y,
            NONE,
        ),
        frame(
            EARLY_START - 50.0,
            EARLY_START_X + INSIDE_FOLLOW,
            EARLY_Y,
            LEFT,
        ),
        frame(EARLY_END - 50.0, EARLY_END_X + INSIDE_FOLLOW, EARLY_Y, LEFT),
    ]);
    assert_eq!(result(&j, 0, Part::Head), Meh);
    assert_eq!(result(&j, 0, Part::Tick), LargeTickMiss);
    assert_eq!(result(&j, 0, Part::Tail), IgnoreMiss);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

#[test]
fn early_hit_move_outside_follow_region() {
    let j = early_test(&[
        frame(EARLY_START - 150.0, EARLY_START_X, EARLY_Y, LEFT),
        frame(
            EARLY_START - 100.0,
            EARLY_START_X + OUTSIDE_FOLLOW,
            EARLY_Y,
            LEFT,
        ),
        frame(
            EARLY_END - 100.0,
            EARLY_END_X + OUTSIDE_FOLLOW,
            EARLY_Y,
            LEFT,
        ),
    ]);
    assert_eq!(result(&j, 0, Part::Head), Meh);
    assert_eq!(result(&j, 0, Part::Tick), LargeTickMiss);
    assert_eq!(result(&j, 0, Part::Tail), IgnoreMiss);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
}

// TestSceneLegacyHitPolicy (Classic: note lock, classic slider behaviour, OD 0, tick rate 3)

/// Lazer's hit policy tests: OD 0, tick rate 3, a 1000 ms beat, and two frames far above the
/// playfield before the replay.
fn policy_test(objects: &str, options: RulesOptions, frames: &[InputFrame]) -> Vec<Judged> {
    let map = beatmap(&difficulty(0.0, 5.0, 1.4, 3.0), BEAT_1000, objects, 0.7);
    let mut all = vec![
        frame(0.0, 256.0, -500.0, NONE),
        frame(0.0, 256.0, -500.0, NONE),
    ];
    all.extend_from_slice(frames);
    play(&map, options, &all)
}

/// Lazer's `referenceHitWindows.WindowFor(HitResult.Meh)` at OD 0.
const MEH_OD0: f64 = 199.5;

fn assert_offset(judged: Judged, expected: f64) {
    assert!(
        (judged.offset - expected).abs() <= 50.0,
        "{judged:?}: expected offset {expected}"
    );
}

#[test]
fn legacy_hit_circle_before_slider_head() {
    let j = policy_test(
        "0,0,1510,1,0\n80,80,1500,2,0,L|130:80,1,50\n",
        classic(),
        &[
            frame(1500.0, 0.0, 0.0, LEFT),
            frame(1510.0, 80.0, 80.0, RIGHT),
        ],
    );
    // Objects are sorted by start time: the slider is object 0.
    assert_eq!(result(&j, 1, Part::Circle), Great);
    assert_eq!(result(&j, 0, Part::Slider), Great);
    assert_eq!(result(&j, 0, Part::Head), LargeTickHit);
    assert_eq!(parts(&j, 0, Part::Tick), [LargeTickHit]);
    assert_eq!(result(&j, 0, Part::Tail), SmallTickHit);
}

#[test]
fn legacy_hit_slider_ticks_before_circle() {
    let j = policy_test(
        "0,0,1510,1,0\n30,30,1500,2,0,L|80:30,1,50\n",
        classic(),
        &[
            frame(1500.0, 30.0, 30.0, LEFT),
            frame(1510.0 + MEH_OD0 - 100.0, 0.0, 0.0, RIGHT),
            frame(1510.0 + MEH_OD0 - 90.0, 30.0, 30.0, LEFT),
        ],
    );
    assert_eq!(result(&j, 1, Part::Circle), Ok);
    assert_eq!(result(&j, 0, Part::Slider), Great);
    assert_eq!(result(&j, 0, Part::Head), LargeTickHit);
    assert_eq!(parts(&j, 0, Part::Tick), [LargeTickHit]);
}

#[test]
fn legacy_hit_slider_head_before_hit_circle() {
    let j = policy_test(
        "0,0,1000,1,0\n80,80,1200,2,0,L|105:80,1,25\n",
        classic(),
        &[
            frame(900.0, 80.0, 80.0, LEFT),
            frame(1000.0, 0.0, 0.0, RIGHT),
            frame(1200.0, 80.0, 80.0, LEFT),
        ],
    );
    assert_eq!(result(&j, 0, Part::Circle), Great);
    assert_eq!(result(&j, 1, Part::Slider), Great);
}

#[test]
fn legacy_overlapping_sliders() {
    let mid = (100.0, 65.0);
    let j = policy_test(
        "100,50,1000,2,0,L|125:50,1,25\n100,80,1200,2,0,L|125:80,1,25\n",
        classic(),
        &[
            frame(1000.0, mid.0, mid.1, RIGHT),
            frame(1025.0, mid.0, mid.1, both()),
            frame(1050.0, mid.0, mid.1, NONE),
            frame(1200.0, 100.0, 90.0, LEFT),
        ],
    );
    assert_eq!(result(&j, 0, Part::Slider), Ok);
    assert_eq!(result(&j, 1, Part::Slider), Great);
}

#[test]
fn legacy_input_does_not_fall_through_overlapping_sliders() {
    let mid = (100.0, 65.0);
    let j = policy_test(
        "100,50,1000,2,0,L|125:50,1,25\n100,80,1250,2,0,L|125:80,1,25\n",
        classic(),
        &[
            frame(1000.0, mid.0, mid.1, RIGHT),
            frame(1025.0, mid.0, mid.1, LEFT),
            frame(1050.0, mid.0, mid.1, NONE),
        ],
    );
    assert_eq!(result(&j, 0, Part::Slider), Ok);
    assert_offset(single(&j, 0, Part::Head), 0.0);
    assert_eq!(result(&j, 1, Part::Slider), Miss);
    // The first head takes the press, so the second head is missed late.
    assert_offset(single(&j, 1, Part::Head), MEH_OD0);
}

#[test]
fn legacy_overlapping_sliders_dont_block_each_other_when_fully_judged() {
    let mid = (100.0, 65.0);
    let j = policy_test(
        "100,50,1000,2,0,L|125:50,1,25\n100,80,1600,2,0,L|125:80,1,25\n",
        classic(),
        &[
            frame(1000.0, mid.0, mid.1, RIGHT),
            frame(1025.0, mid.0, mid.1, NONE),
            frame(1250.0, mid.0, mid.1, NONE),
            frame(1650.0, mid.0, mid.1, LEFT),
            frame(1675.0, mid.0, mid.1, NONE),
        ],
    );
    assert_eq!(result(&j, 0, Part::Slider), Ok);
    assert_offset(single(&j, 0, Part::Head), 0.0);
    assert_eq!(result(&j, 1, Part::Slider), Ok);
    assert_offset(single(&j, 1, Part::Head), 50.0);
}

// TestSceneStartTimeOrderedHitPolicy (lazer's default rules, OD 5 with the real windows)

/// Lazer's `TestSlider`: slider velocity 0.1.
fn slow_slider(x: i32, y: i32, time: f64) -> String {
    format!("{x},{y},{time},2,0,L|{}:{y},1,25\n", x + 25)
}

fn ordered_test(objects: &str, frames: &[InputFrame]) -> Vec<Judged> {
    let map = beatmap(
        &difficulty(5.0, 5.0, 1.4, 3.0),
        &format!("{BEAT_1000}\n0,-1000,4,2,0,100,0,0"),
        objects,
        0.7,
    );
    play_from_start(&map, lazer(HitPolicy::StartTimeOrdered), frames)
}

#[test]
fn ordered_miss_slider_head_and_hit_all_slider_ticks() {
    let objects = format!("0,0,1510,1,0\n{}", slow_slider(80, 80, 1500.0));
    let j = ordered_test(
        &objects,
        &[
            frame(1500.0, 0.0, 0.0, LEFT),
            frame(1510.0, 80.0, 80.0, RIGHT),
        ],
    );
    assert_eq!(result(&j, 1, Part::Circle), Great);
    assert_eq!(result(&j, 0, Part::Slider), IgnoreHit);
    // Hitting the circle misses the earlier head.
    assert_eq!(result(&j, 0, Part::Head), Miss);
    let ticks = parts(&j, 0, Part::Tick);
    assert!(!ticks.is_empty() && ticks.iter().all(|&r| r == LargeTickHit));
}

#[test]
fn ordered_hit_slider_head_before_hit_circle() {
    let objects = format!("0,0,1000,1,0\n{}", slow_slider(80, 80, 1200.0));
    let j = ordered_test(
        &objects,
        &[
            frame(900.0, 80.0, 80.0, LEFT),
            frame(1000.0, 0.0, 0.0, RIGHT),
            frame(1200.0, 80.0, 80.0, LEFT),
        ],
    );
    assert_eq!(result(&j, 0, Part::Circle), Great);
    assert_eq!(result(&j, 1, Part::Head), Great);
    assert_eq!(result(&j, 1, Part::Slider), IgnoreHit);
}

/// Lazer's `TestInputFallsThroughJudgedSliders` with the second slider moved to where the
/// real windows let the fallen through press hit it.
#[test]
fn ordered_input_falls_through_judged_sliders() {
    let mid = (100.0, 65.0);
    let objects = format!(
        "{}{}",
        slow_slider(100, 50, 1000.0),
        slow_slider(100, 80, 1100.0)
    );
    let j = ordered_test(
        &objects,
        &[
            frame(1000.0, mid.0, mid.1, RIGHT),
            frame(1025.0, mid.0, mid.1, LEFT),
            frame(1050.0, mid.0, mid.1, NONE),
        ],
    );
    assert_eq!(result(&j, 0, Part::Head), Great);
    assert_offset(single(&j, 0, Part::Head), 0.0);
    // OD 5: the ok window is 99.5 ms.
    assert_eq!(result(&j, 1, Part::Head), Ok);
    assert_offset(single(&j, 1, Part::Head), -75.0);
}

// Classic slider results and bookkeeping

/// Classic sliders are judged in proportion to the nested objects hit.
#[test]
fn classic_slider_result_follows_the_nested_objects_hit() {
    // The input slider: head, 7 ticks and a tail. Tracking is held until `release`.
    let map = input_slider();
    let total = match &map.hit_objects[0].kind {
        OsuHitObjectKind::Slider(s) => s.nested.len(),
        _ => unreachable!(),
    };
    assert_eq!(total, 9);
    let run = |release: f64| {
        let mut frames = vec![frame(TIME_SLIDER_START, 0.0, 0.0, LEFT)];
        if release < TIME_SLIDER_END {
            frames.push(frame(release, 0.0, 0.0, NONE));
        }
        play_from_start(&map, classic(), &frames)
    };
    let hits = |j: &[Judged]| {
        j.iter()
            .filter(|j| j.part != Part::Slider && j.result.is_hit())
            .count()
    };

    let full = run(TIME_SLIDER_END);
    assert_eq!(hits(&full), 9);
    assert_eq!(result(&full, 0, Part::Slider), Great);
    assert_eq!(result(&full, 0, Part::Tail), SmallTickHit);

    // Head and the ticks at 1/7, 2/7 and 3/7 of the slider: 4 of 9.
    let early = run(TIME_SLIDER_START + 2500.0 * 3.5 / 7.5);
    assert_eq!(hits(&early), 4);
    assert_eq!(result(&early, 0, Part::Slider), Meh);

    // Five of nine.
    let half = run(TIME_SLIDER_START + 2500.0 * 4.5 / 7.5);
    assert_eq!(hits(&half), 5);
    assert_eq!(result(&half, 0, Part::Slider), Ok);

    let none = play_from_start(&map, classic(), &[frame(3900.0, 300.0, 300.0, LEFT)]);
    assert_eq!(hits(&none), 0);
    assert_eq!(result(&none, 0, Part::Slider), Miss);
    assert_eq!(result(&none, 0, Part::Head), LargeTickMiss);
}

/// With classic slider behaviour, a press on the head outside every window still judges it
/// (as a miss) when the policy lets it through.
#[test]
fn classic_head_pressed_too_early_is_missed() {
    let map = input_slider();
    let options = RulesOptions {
        policy: HitPolicy::StartTimeOrdered,
        classic_slider_behaviour: true,
        block_input_under_slider_heads: false,
    };
    let j = play_from_start(&map, options, &[frame(1000.0, 0.0, 0.0, LEFT)]);
    let head = single(&j, 0, Part::Head);
    assert_eq!(head.result, LargeTickMiss);
    assert_eq!(head.offset, -500.0);
    // Lazer's default behaviour ignores the press.
    let j = play_from_start(
        &map,
        lazer(HitPolicy::StartTimeOrdered),
        &[frame(1000.0, 0.0, 0.0, LEFT)],
    );
    assert!(single(&j, 0, Part::Head).offset > 0.0);
}

#[test]
fn judgements_are_ordered_like_lazer() {
    let j = input_test(&[frame(TIME_SLIDER_START, 0.0, 0.0, LEFT)]);
    let order: Vec<Part> = j.iter().map(|j| j.part).collect();
    let mut expected = vec![Part::Head];
    expected.extend([Part::Tick; 7]);
    expected.extend([Part::Tail, Part::Slider]);
    assert_eq!(order, expected);
    // The slider is judged at the first update from its end time, the tail within its
    // leniency.
    let slider = single(&j, 0, Part::Slider).offset;
    assert!(
        (2500.0..2500.0 + 1000.0 / 60.0).contains(&slider),
        "{slider}"
    );
    assert!(single(&j, 0, Part::Tail).offset < 0.0);
}

#[test]
fn max_judgements_count_nested_objects() {
    let map = input_slider();
    let rules = OsuRules::with_options(&map, lazer(HitPolicy::StartTimeOrdered));
    assert_eq!(rules.max_judgements(), 9 + 1);
}

/// The misses a hit forces come before it; the nested objects a late head hit catches up on
/// come right after the head.
#[test]
fn judgement_log_order_around_heads() {
    let objects = format!("0,0,1510,1,0\n{}", slow_slider(80, 80, 1500.0));
    let j = ordered_test(
        &objects,
        &[
            frame(1500.0, 0.0, 0.0, LEFT),
            frame(1510.0, 80.0, 80.0, RIGHT),
        ],
    );
    assert_eq!((j[0].part, j[0].result), (Part::Head, Miss));
    assert_eq!((j[1].part, j[1].result), (Part::Circle, Great));

    let map = late_slider(|s| s.tick_distance_multiplier = 0.2);
    let j = late_test(&map, &late_frames(149.0, LATE_END_X));
    let order: Vec<Part> = j.iter().take(4).map(|j| j.part).collect();
    assert_eq!(order, [Part::Head, Part::Tick, Part::Tick, Part::Tick]);
    // The ticks passed are judged by the press, after their start times.
    assert_eq!(j[0].offset, 149.0);
    for tick in &j[1..4] {
        assert_eq!(tick.result, LargeTickHit);
        assert!(tick.offset > 0.0);
    }
}

/// A slider of zero length lasts no time; its progress is NaN, which must not panic.
#[test]
fn zero_length_slider_is_judged() {
    for options in [lazer(HitPolicy::StartTimeOrdered), classic()] {
        let mut map = beatmap(
            &difficulty(5.0, 5.0, 1.4, 3.0),
            BEAT_1000,
            "100,100,1000,2,0,L|150:100,2,50\n",
            0.7,
        );
        adjust(&mut map, 0, |s| {
            s.path = SliderPath::new(
                vec![
                    PathControlPoint {
                        position: Vec2::new(0.0, 0.0),
                        path_type: Some(PathType::Linear),
                    },
                    PathControlPoint {
                        position: Vec2::new(0.0, 0.0),
                        path_type: None,
                    },
                ],
                Some(0.0),
                false,
            );
        });
        assert_eq!(map.hit_objects[0].end_time(), 1000.0);
        let j = play_from_start(&map, options, &[frame(1010.0, 100.0, 100.0, LEFT)]);
        assert_eq!(single(&j, 0, Part::Slider).part, Part::Slider);
        assert!(single(&j, 0, Part::Head).result.is_hit());
    }
}
