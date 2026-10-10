//! Hit circles under both hit policies, on synthetic beatmaps through the replay simulator.
//!
//! The Legacy cases are lazer's `TestSceneLegacyHitPolicy` (OD 0, so the meh window is
//! 199.5 ms). Lazer's `TestSceneStartTimeOrderedHitPolicy` uses custom hit windows; its cases
//! are replayed here with the real OD 0 windows, so some expected results differ from lazer's
//! test and are derived from the rules instead.

use slam_formats::osu::{Vec2, decode_str};
use slam_osu::Beatmap;
use slam_osu::hit_windows::HitWindows;
use slam_osu::judgement::{HitResult, JudgementLog, Statistics};
use slam_osu::replay::{ActionEvent, Rules, StepInput};
use slam_osu::replay::{Actions, InputFrame, ReplayInput, Simulator};
use slam_osu::rules::{HitPolicy, OsuRules};
use slam_osu::{Mod, ModSet};

use HitResult::{Great, Meh, Miss, Ok};

const LEGACY: HitPolicy = HitPolicy::Legacy {
    hittable_range: 400.0,
};
const ORDERED: HitPolicy = HitPolicy::StartTimeOrdered;
const NONE: Actions = Actions::NONE;
const LEFT: Actions = Actions::LEFT;
const RIGHT: Actions = Actions::RIGHT;
/// Lazer's `referenceHitWindows.WindowFor(HitResult.Meh)` at OD 0.
const MEH: f64 = 199.5;

/// A beatmap of circles `(time, x, y)` at OD 0, AR 5 (preempt 1200) and CS 5 (radius 32),
/// with a 1000 ms beat as in lazer's tests.
fn circles(objects: &[(f64, i32, i32)]) -> Beatmap {
    let mut text = String::from(
        "osu file format v14\n\n[Difficulty]\nHPDrainRate:5\nCircleSize:5\nOverallDifficulty:0\n\
         ApproachRate:5\nSliderMultiplier:1\nSliderTickRate:3\n\n[TimingPoints]\n0,1000,4,2,0,100,1,0\n\n\
         [HitObjects]\n",
    );
    for (time, x, y) in objects {
        text.push_str(&format!("{x},{y},{time},1,0\n"));
    }
    Beatmap::from_file(decode_str(&text).unwrap().beatmap).unwrap()
}

fn frame(time: f64, x: f32, y: f32, actions: Actions) -> InputFrame {
    InputFrame {
        time,
        position: Vec2::new(x, y),
        actions,
    }
}

/// The judgements of a play as `(object index, result, offset from the start time)`, in order.
fn play(map: &Beatmap, policy: HitPolicy, frames: &[InputFrame]) -> Vec<(u32, HitResult, f64)> {
    play_stats(map, policy, frames).0
}

/// [`play`] with the final statistics.
fn play_stats(
    map: &Beatmap,
    policy: HitPolicy,
    frames: &[InputFrame],
) -> (Vec<(u32, HitResult, f64)>, Statistics) {
    // Lazer's tests start the replay with two frames far above the playfield.
    let mut all = vec![
        frame(0.0, 256.0, -500.0, NONE),
        frame(0.0, 256.0, -500.0, NONE),
    ];
    all.extend_from_slice(frames);
    let mut sim = Simulator::new(
        map,
        ReplayInput::new(all),
        OsuRules::with_policy(map, policy),
    );
    sim.run();
    assert!(sim.is_finished());
    (judgements(map, sim.log()), *sim.log().statistics())
}

fn judgements(map: &Beatmap, log: &JudgementLog) -> Vec<(u32, HitResult, f64)> {
    log.events()
        .iter()
        .map(|j| {
            let start = map.hit_objects[j.object.index as usize].start_time;
            (j.object.index, j.result, j.time - start)
        })
        .collect()
}

/// The results ordered by object.
fn results(judgements: &[(u32, HitResult, f64)]) -> Vec<HitResult> {
    let mut sorted = judgements.to_vec();
    sorted.sort_by_key(|j| j.0);
    sorted.into_iter().map(|j| j.1).collect()
}

fn offset(judgements: &[(u32, HitResult, f64)], index: u32) -> f64 {
    judgements.iter().find(|j| j.0 == index).unwrap().2
}

/// Lazer's `addJudgementOffsetAssert`: within 50 ms.
fn assert_offset(judgements: &[(u32, HitResult, f64)], index: u32, expected: f64) {
    let actual = offset(judgements, index);
    assert!(
        (actual - expected).abs() <= 50.0,
        "object {index}: offset {actual}, expected {expected}"
    );
}

fn two_circles() -> Beatmap {
    circles(&[(1500.0, 0, 0), (1600.0, 80, 80)])
}

// TestSceneLegacyHitPolicy

#[test]
fn legacy_click_second_circle_before_first_circle_time() {
    let j = play(&two_circles(), LEGACY, &[frame(1400.0, 80.0, 80.0, LEFT)]);
    assert_eq!(results(&j), [Miss, Miss]);
    assert_offset(&j, 0, MEH);
}

#[test]
fn legacy_click_second_circle_at_first_circle_time() {
    let j = play(&two_circles(), LEGACY, &[frame(1500.0, 80.0, 80.0, LEFT)]);
    assert_eq!(results(&j), [Miss, Miss]);
    assert_offset(&j, 0, MEH);
}

#[test]
fn legacy_click_second_circle_after_first_circle_time() {
    let j = play(&two_circles(), LEGACY, &[frame(1600.0, 80.0, 80.0, LEFT)]);
    assert_eq!(results(&j), [Miss, Miss]);
    assert_offset(&j, 0, MEH);
}

#[test]
fn legacy_click_second_circle_before_first_circle_time_with_first_circle_judged() {
    let j = play(
        &two_circles(),
        LEGACY,
        &[
            frame(1310.0, 0.0, 0.0, LEFT),
            frame(1410.0, 80.0, 80.0, RIGHT),
        ],
    );
    assert_eq!(j, [(0, Meh, -190.0), (1, Meh, -190.0)]);
}

#[test]
fn legacy_click_second_circle_after_first_circle_time_with_first_circle_judged() {
    let j = play(
        &two_circles(),
        LEGACY,
        &[
            frame(1310.0, 0.0, 0.0, LEFT),
            frame(1500.0, 80.0, 80.0, RIGHT),
        ],
    );
    assert_eq!(j, [(0, Meh, -190.0), (1, Ok, -100.0)]);
}

#[test]
fn legacy_stacks_do_not_shake() {
    // Twenty circles stacked at (80, 80). At 550 ms the cursor is over the stacked copies of
    // circles 5 to 7, under circle 4, which is unjudged: the press is ignored.
    let objects: Vec<_> = (0..20)
        .map(|i| (1000.0 + 100.0 * i as f64, 80, 80))
        .collect();
    let map = circles(&objects);
    assert!(map.hit_objects[4].stack_height > 0);
    let j = play(&map, LEGACY, &[frame(550.0, 55.0, 55.0, LEFT)]);
    assert!(j.iter().all(|&(_, r, _)| r == Miss));
}

#[test]
fn legacy_reduced_hittable_range() {
    // Lazer's TestAutopilotReducesHittableRange: Autopilot halves the range to 200 ms.
    let map = circles(&[(1500.0, 0, 0)]);
    let frames = [frame(1250.0, 0.0, 0.0, LEFT)];
    let j = play(
        &map,
        HitPolicy::Legacy {
            hittable_range: 200.0,
        },
        &frames,
    );
    assert_eq!(results(&j), [Miss]);
    assert_offset(&j, 0, MEH);

    // With the full range the press lands, in the miss window.
    assert_eq!(play(&map, LEGACY, &frames), [(0, Miss, -250.0)]);
}

#[test]
fn legacy_overlapping_hit_circles_do_not_block_each_other_when_both_visible() {
    let map = circles(&[(1000.0, 100, 100), (1200.0, 120, 120)]);
    let j = play(
        &map,
        LEGACY,
        &[
            frame(1000.0, 110.0, 110.0, LEFT),
            frame(1025.0, 110.0, 110.0, NONE),
            frame(1050.0, 110.0, 110.0, RIGHT),
        ],
    );
    assert_eq!(j, [(0, Great, 0.0), (1, Meh, -150.0)]);
}

#[test]
fn legacy_overlapping_hit_circles_do_not_block_each_other_when_fully_faded_out() {
    // Circles 0 and 2 share a position and stack.
    let map = circles(&[(1000.0, 100, 100), (1200.0, 200, 200), (1400.0, 100, 100)]);
    assert_eq!(map.hit_objects[0].stack_height, 1);
    let j = play(
        &map,
        LEGACY,
        &[
            frame(1000.0, 100.0, 100.0, LEFT),
            frame(1050.0, 100.0, 100.0, NONE),
            frame(1150.0, 200.0, 200.0, NONE),
            frame(1200.0, 200.0, 200.0, LEFT),
            frame(1250.0, 200.0, 200.0, NONE),
            frame(1350.0, 100.0, 100.0, NONE),
            frame(1400.0, 100.0, 100.0, LEFT),
            frame(1450.0, 100.0, 100.0, NONE),
        ],
    );
    assert_eq!(j, [(0, Great, 0.0), (1, Great, 0.0), (2, Great, 0.0)]);
}

// TestSceneStartTimeOrderedHitPolicy, with OD 0 windows

#[test]
fn ordered_click_just_before_and_after_the_miss_window_starts() {
    let map = circles(&[(1500.0, 256, 192)]);
    // 401 ms early is outside every window: nothing happens, the circle times out.
    let j = play(&map, ORDERED, &[frame(1099.0, 256.0, 192.0, LEFT)]);
    assert_eq!(results(&j), [Miss]);
    assert!(offset(&j, 0) > MEH);
    // 399 ms early is in the miss window.
    let j = play(&map, ORDERED, &[frame(1101.0, 256.0, 192.0, LEFT)]);
    assert_eq!(j, [(0, Miss, -399.0)]);
}

#[test]
fn ordered_click_second_circle_before_first_circle_time() {
    let j = play(&two_circles(), ORDERED, &[frame(1400.0, 80.0, 80.0, LEFT)]);
    assert_eq!(results(&j), [Miss, Miss]);
    assert_offset(&j, 0, MEH);
    assert_offset(&j, 1, MEH);
}

#[test]
fn ordered_click_second_circle_at_first_circle_time() {
    // Allowed at exactly the first circle's start time; the first circle is missed, and the
    // miss is recorded before the hit that forced it.
    let (j, stats) = play_stats(&two_circles(), ORDERED, &[frame(1500.0, 80.0, 80.0, LEFT)]);
    assert_eq!(j, [(0, Miss, 0.0), (1, Ok, -100.0)]);
    assert_eq!((stats.combo, stats.max_combo), (1, 1));
}

#[test]
fn ordered_click_second_circle_after_first_circle_time() {
    let j = play(&two_circles(), ORDERED, &[frame(1600.0, 80.0, 80.0, LEFT)]);
    assert_eq!(j, [(0, Miss, 100.0), (1, Great, 0.0)]);
}

#[test]
fn ordered_click_second_circle_before_first_circle_time_with_first_circle_judged() {
    let j = play(
        &two_circles(),
        ORDERED,
        &[
            frame(1310.0, 0.0, 0.0, LEFT),
            frame(1410.0, 80.0, 80.0, RIGHT),
        ],
    );
    assert_eq!(j, [(0, Meh, -190.0), (1, Meh, -190.0)]);
}

// One press, one hit

#[test]
fn one_press_hits_one_circle() {
    // Two circles at the same time and place: one press judges only the first.
    let map = circles(&[(1000.0, 100, 100), (1000.0, 100, 100)]);
    let j = play(&map, ORDERED, &[frame(1000.0, 100.0, 100.0, LEFT)]);
    assert_eq!(j[0], (0, Great, 0.0));
    assert_eq!(j[1].0, 1);
    assert_eq!(j[1].1, Miss);
}

#[test]
fn right_of_the_same_step_hits_the_second_circle_after_left() {
    let map = circles(&[(1000.0, 100, 100), (1000.0, 100, 100)]);
    let j = play(&map, ORDERED, &[frame(1000.0, 100.0, 100.0, LEFT | RIGHT)]);
    assert_eq!(j, [(0, Great, 0.0), (1, Great, 0.0)]);
}

#[test]
fn holding_a_button_does_not_hit_again() {
    let map = circles(&[(1000.0, 100, 100), (1100.0, 100, 100)]);
    let j = play(
        &map,
        ORDERED,
        &[
            frame(1000.0, 100.0, 100.0, LEFT),
            frame(1100.0, 100.0, 100.0, LEFT),
        ],
    );
    assert_eq!(j[0], (0, Great, 0.0));
    assert_eq!(j[1].1, Miss);
}

#[test]
fn press_outside_the_circle_does_nothing() {
    // CS 5: radius 32 (times the stable rounding allowance).
    let map = circles(&[(1000.0, 100, 100)]);
    let j = play(&map, ORDERED, &[frame(1000.0, 132.5, 100.0, LEFT)]);
    assert_eq!(results(&j), [Miss]);
    let j = play(&map, ORDERED, &[frame(1000.0, 131.9, 100.0, LEFT)]);
    assert_eq!(j, [(0, Great, 0.0)]);
}

#[test]
fn smoke_does_not_hit() {
    let map = circles(&[(1000.0, 100, 100)]);
    let j = play(
        &map,
        ORDERED,
        &[frame(1000.0, 100.0, 100.0, Actions::SMOKE)],
    );
    assert_eq!(results(&j), [Miss]);
}

#[test]
fn unhit_circle_is_missed_on_the_first_step_after_its_meh_window() {
    let map = circles(&[(1000.0, 100, 100)]);
    let j = play(&map, ORDERED, &[]);
    assert_eq!(j.len(), 1);
    assert!(j[0].2 > MEH && j[0].2 <= MEH + 1000.0 / 60.0, "{j:?}");
}

#[test]
fn stacked_circle_is_hit_at_its_stacked_position() {
    // The first of two stacked circles is drawn 6.4 * scale up and left of its position.
    let map = circles(&[(1000.0, 100, 100), (1100.0, 100, 100)]);
    let stacked = map.hit_objects[0].stacked_position();
    assert!(stacked.x < 100.0);
    // 30 px left of the stacked centre: inside it, but 33.4 from the unstacked centre.
    let j = play(
        &map,
        ORDERED,
        &[frame(1000.0, stacked.x - 30.0, stacked.y, LEFT)],
    );
    assert_eq!(j[0], (0, Great, 0.0));
}

// Modelling decisions

#[test]
fn circles_timing_out_in_one_step_are_recorded_earliest_first() {
    // Both windows end between the steps at 1190 and 1206.67.
    let map = circles(&[(1000.0, 100, 100), (1005.0, 300, 300)]);
    let frames = [frame(1190.0, 0.0, 0.0, NONE), frame(1210.0, 0.0, 0.0, NONE)];
    let step = 1190.0 + 1000.0 / 60.0;
    // The later circle times out first, and its HandleHit misses the earlier one before its own
    // result is recorded.
    let j = play(&map, ORDERED, &frames);
    assert_eq!(j, [(0, Miss, step - 1000.0), (1, Miss, step - 1005.0)]);
    // The legacy policy has no HandleHit: the update order shows.
    let j = play(&map, LEGACY, &frames);
    assert_eq!(j, [(1, Miss, step - 1005.0), (0, Miss, step - 1000.0)]);
}

#[test]
fn presses_see_the_objects_alive_at_the_previous_update() {
    // AR 10: preempt 450, so the circle at 1000 comes alive at 550.
    let text = "osu file format v14\n\n[Difficulty]\nCircleSize:5\nOverallDifficulty:0\n\
                ApproachRate:10\n\n[TimingPoints]\n0,1000,4,2,0,100,1,0\n\n[HitObjects]\n\
                100,100,1000,1,0\n";
    let map = Beatmap::from_file(decode_str(text).unwrap().beatmap).unwrap();
    let mut rules = OsuRules::with_policy(&map, ORDERED);
    let mut log = JudgementLog::with_capacity(rules.max_judgements());
    let press = |time| ActionEvent {
        time,
        position: Vec2::new(100.0, 100.0),
        action: LEFT,
        pressed: true,
    };
    let update = |rules: &mut OsuRules, log: &mut JudgementLog, time, events: &[_]| {
        rules.update(
            &StepInput {
                time,
                position: Vec2::new(100.0, 100.0),
                held: NONE,
                events,
            },
            log,
        );
    };
    update(&mut rules, &mut log, 500.0, &[]);
    // Alive at 610, but not at the previous update: the press finds nothing.
    update(&mut rules, &mut log, 610.0, &[press(610.0)]);
    assert!(log.events().is_empty());
    // Now it is: a press 380 ms early lands in the miss window.
    update(&mut rules, &mut log, 620.0, &[press(620.0)]);
    assert_eq!(log.events().len(), 1);
    assert_eq!(log.events()[0].result, Miss);
    assert!(rules.is_complete());
}

#[test]
fn rules_use_the_overall_difficulty_after_mods() {
    let text = "osu file format v14\n\n[Difficulty]\nOverallDifficulty:5\n\n[TimingPoints]\n\
                0,1000,4,2,0,100,1,0\n\n[HitObjects]\n100,100,1000,1,0\n";
    let mut mods = ModSet::new(vec![Mod::HardRock]).unwrap();
    let map = mods
        .playable_beatmap(decode_str(text).unwrap().beatmap)
        .unwrap();
    // Hard Rock: OD 5 * 1.4 = 7.
    assert_eq!(
        *OsuRules::new(&map, &mods).hit_windows(),
        HitWindows::new(7.0)
    );
}

#[test]
fn objects_stepped_over_are_skipped_and_the_play_ends() {
    // The replay starts long after the only circle: lazer never brings it alive.
    let map = circles(&[(1000.0, 100, 100)]);
    let mut sim = Simulator::new(
        &map,
        ReplayInput::new([frame(5000.0, 0.0, 0.0, NONE)]),
        OsuRules::with_policy(&map, ORDERED),
    );
    sim.run();
    assert!(sim.is_finished());
    assert!(sim.log().events().is_empty());
    assert_eq!(sim.rules().skipped_objects(), 1);
}
