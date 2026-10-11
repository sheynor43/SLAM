//! Fail conditions of Sudden Death and Perfect.

use slam_formats::osr::SettingValue;
use slam_formats::osu::{Vec2, decode_str};
use slam_osu::health::HealthProcessor;
use slam_osu::judgement::{HitResult, Judgement, ObjectRef, Target};
use slam_osu::replay::{Actions, InputFrame, ReplayInput, Simulator};
use slam_osu::rules::OsuRules;
use slam_osu::scoring::{ScoreProcessor, ScoreRank};
use slam_osu::{Beatmap, GameplayMod, Mod, ModSet};

fn sudden_death(fail_on_slider_tail: bool) -> Mod {
    Mod::SuddenDeath {
        restart: false,
        fail_on_slider_tail,
    }
}

fn judgement(result: HitResult, max_result: HitResult) -> Judgement {
    Judgement {
        object: ObjectRef {
            index: 0,
            nested: None,
        },
        time: 0.0,
        result,
        max_result,
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModSuddenDeath.cs (FailCondition)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModSuddenDeath.cs (FailCondition)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModPerfect.cs (FailCondition)
#[test]
fn conditions_per_result() {
    use HitResult::*;
    use Target::*;
    // Target, result, best result; whether Sudden Death, Sudden Death failing on slider tails
    // and Perfect fail.
    let cases = [
        (Circle, Great, Great, false, false, false),
        (Circle, Ok, Great, false, false, true),
        (Circle, Meh, Great, false, false, true),
        (Circle, Miss, Great, true, true, true),
        (SliderHead, Great, Great, false, false, false),
        (SliderHead, Miss, Great, true, true, true),
        // Classic head.
        (SliderHead, LargeTickHit, LargeTickHit, false, false, false),
        (SliderHead, LargeTickMiss, LargeTickHit, true, true, true),
        (SliderTick, LargeTickHit, LargeTickHit, false, false, false),
        (SliderTick, LargeTickMiss, LargeTickHit, true, true, true),
        (SliderRepeat, LargeTickMiss, LargeTickHit, true, true, true),
        (
            SliderTail,
            SliderTailHit,
            SliderTailHit,
            false,
            false,
            false,
        ),
        (SliderTail, IgnoreMiss, SliderTailHit, false, true, true),
        // Classic tail.
        (SliderTail, SmallTickHit, SmallTickHit, false, false, false),
        (SliderTail, SmallTickMiss, SmallTickHit, false, true, true),
        // A slider judges nothing of its own outside Classic.
        (Slider, IgnoreHit, IgnoreHit, false, false, false),
        (Slider, IgnoreMiss, IgnoreHit, false, false, false),
        // Classic slider.
        (Slider, Ok, Great, false, false, true),
        (Slider, Miss, Great, true, true, true),
        (Spinner, Meh, Great, false, false, true),
        (Spinner, Miss, Great, true, true, true),
        (SpinnerTick, SmallBonus, SmallBonus, false, false, false),
        (SpinnerTick, IgnoreMiss, SmallBonus, false, false, false),
        (
            SpinnerBonusTick,
            IgnoreMiss,
            LargeBonus,
            false,
            false,
            false,
        ),
    ];
    for (target, result, max, sd, sd_tail, pf) in cases {
        let j = judgement(result, max);
        let name = format!("{target:?} {result:?}/{max:?}");
        assert_eq!(
            sudden_death(false).fail_condition(&j, target),
            sd,
            "SD {name}"
        );
        assert_eq!(
            sudden_death(true).fail_condition(&j, target),
            sd_tail,
            "SD tail {name}"
        );
        let perfect = Mod::Perfect { restart: true };
        assert_eq!(perfect.fail_condition(&j, target), pf, "PF {name}");
    }
}

#[test]
fn other_mods_have_no_condition() {
    let miss = judgement(HitResult::Miss, HitResult::Great);
    for acronym in ["NF", "EZ", "HR", "DT", "HD", "CL", "ZZ"] {
        assert!(!Mod::from_acronym(acronym).fail_condition(&miss, Target::Circle));
    }
    assert!(!ModSet::default().meets_fail_condition(&miss, Target::Circle));
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModFailCondition.cs (Restart, RestartOnFail)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModPerfect.cs (ModPerfect)
#[test]
fn settings_and_restart_on_fail() {
    let sd = Mod::from_acronym("sd");
    assert_eq!(sd, sudden_death(false));
    assert!(sd.uses_default_configuration());
    assert!(!sd.restart_on_fail());

    let pf = Mod::from_acronym("PF");
    assert_eq!(pf, Mod::Perfect { restart: true });
    assert!(pf.uses_default_configuration());
    assert!(pf.restart_on_fail());

    let mut sd = Mod::from_acronym("SD");
    assert!(sd.set_setting("restart", &SettingValue::Bool(true)));
    assert!(sd.set_setting("fail_on_slider_tail", &SettingValue::Bool(true)));
    assert!(sd.restart_on_fail());
    let settings = sd.settings();
    assert_eq!(
        settings,
        vec![
            ("fail_on_slider_tail".to_owned(), SettingValue::Bool(true)),
            ("restart".to_owned(), SettingValue::Bool(true)),
        ]
    );
    assert_eq!(Mod::from_api(&sd.to_api()), sd);

    let mut pf = Mod::from_acronym("PF");
    assert!(pf.set_setting("restart", &SettingValue::Bool(false)));
    assert!(!pf.set_setting("fail_on_slider_tail", &SettingValue::Bool(true)));
    assert_eq!(
        pf.settings(),
        vec![("restart".to_owned(), SettingValue::Bool(false))]
    );
    assert_eq!(Mod::from_api(&pf.to_api()), pf);

    assert!(ModSet::new(vec![sudden_death(false), Mod::Perfect { restart: false }]).is_ok());
    assert!(
        !ModSet::new(vec![sudden_death(false)])
            .unwrap()
            .restart_on_fail()
    );
    assert!(ModSet::from_legacy(1 << 14).restart_on_fail());
    assert!(ModSet::from_legacy(1 << 5).is_supported());
}

/// A beatmap of `objects` with lazer's default difficulty, slider velocity and beat length,
/// as lazer's test scenes build them.
fn beatmap(objects: &str) -> Beatmap {
    let text = format!(
        "osu file format v14\n\n[Difficulty]\nHPDrainRate:5\nCircleSize:5\nOverallDifficulty:5\n\
         ApproachRate:5\nSliderMultiplier:1.4\nSliderTickRate:1\n\n[TimingPoints]\n\
         0,1000,4,2,0,100,1,0\n\n[HitObjects]\n{objects}"
    );
    Beatmap::from_file(decode_str(&text).unwrap().beatmap).unwrap()
}

fn frame(time: f64, actions: Actions) -> InputFrame {
    InputFrame {
        time,
        position: Vec2::new(256.0, 192.0),
        actions,
    }
}

/// Plays `frames` with health and `mods`; a frame at 0 starts the replay before the first
/// object, as lazer's test scenes play their replays in real time.
fn play(map: &Beatmap, mods: Vec<Mod>, frames: &[InputFrame]) -> Simulator<OsuRules> {
    let mods = ModSet::new(mods).unwrap();
    let mut all = vec![frame(0.0, Actions::NONE)];
    all.extend_from_slice(frames);
    let mut sim = Simulator::new(map, ReplayInput::new(all), OsuRules::new(map, &mods))
        .with_health(HealthProcessor::for_play(map, &mods), mods);
    sim.run();
    assert!(sim.is_finished());
    sim
}

fn failed(sim: &Simulator<OsuRules>) -> bool {
    sim.health().expect("health").has_failed()
}

/// Pressing the head of a slider at 1000 ms and letting go right after.
fn press_and_release() -> [InputFrame; 2] {
    [frame(1000.0, Actions::LEFT), frame(1001.0, Actions::NONE)]
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/Mods/TestSceneOsuModSuddenDeath.cs (TestMissTail)
#[test]
fn sudden_death_fails_on_a_missed_tail_only_when_asked() {
    // 100 px: shorter than the tick distance, so no ticks.
    let map = beatmap("256,192,1000,2,0,L|356:192,1,100\n");
    for tail_miss in [false, true] {
        let sim = play(&map, vec![sudden_death(tail_miss)], &press_and_release());
        assert_eq!(failed(&sim), tail_miss, "fail on slider tail: {tail_miss}");
        if !tail_miss {
            assert_eq!(sim.counted_judgements(), sim.log().events());
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/Mods/TestSceneOsuModSuddenDeath.cs (TestMissTick)
#[test]
fn sudden_death_fails_on_a_missed_tick() {
    let map = beatmap("256,192,1000,2,0,L|456:192,1,200\n");
    let sim = play(&map, vec![sudden_death(false)], &press_and_release());
    assert!(failed(&sim));
    let last = sim.counted_judgements().last().expect("judgements");
    assert_eq!(last.result, HitResult::LargeTickMiss);
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/Mods/TestSceneOsuModPerfect.cs (TestMissSliderTail)
#[test]
fn perfect_fails_on_a_missed_tail() {
    let map = beatmap("256,192,1000,2,0,L|356:192,1,100\n");
    let sim = play(
        &map,
        vec![Mod::Perfect { restart: true }],
        &press_and_release(),
    );
    assert!(failed(&sim));
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/Mods/TestSceneOsuModPerfect.cs (TestHitCircle, TestSlider, TestSpinner)
#[test]
fn perfect_fails_on_a_miss_and_passes_a_perfect_play() {
    let circle = beatmap("256,192,1000,1,0\n");
    let slider = beatmap("256,192,1000,2,0,L|356:192,1,100\n");
    let spinner = beatmap("256,192,1000,12,0,3000\n");
    for map in [&circle, &slider, &spinner] {
        assert!(failed(&play(
            map,
            vec![Mod::Perfect { restart: true }],
            &[]
        )));
    }

    let sim = play(
        &circle,
        vec![Mod::Perfect { restart: true }],
        &[frame(1000.0, Actions::LEFT)],
    );
    assert!(!failed(&sim));
    // Held through the slider, following the ball (the slider sees the cursor only when it
    // moves): every judgement is the best.
    let held: Vec<InputFrame> = (0..=20)
        .map(|i| InputFrame {
            time: 1000.0 + f64::from(i) * 50.0,
            position: Vec2::new(256.0 + (i as f32 * 7.0).min(100.0), 192.0),
            actions: Actions::LEFT,
        })
        .collect();
    let sim = play(&slider, vec![Mod::Perfect { restart: true }], &held);
    assert!(!failed(&sim), "{:?}", sim.log().events());
}

#[test]
fn a_fail_stops_the_counted_judgements_at_the_failing_one() {
    let map = beatmap("256,192,1000,1,0\n256,192,1300,1,0\n256,192,1600,1,0\n");
    let sim = play(&map, vec![sudden_death(false)], &[]);
    assert!(failed(&sim));
    let counted = sim.counted_judgements();
    assert_eq!(counted.len(), 1);
    assert_eq!(counted[0].result, HitResult::Miss);
    // Without a fail every judgement counts.
    let sim = play(&map, vec![], &[]);
    assert!(!failed(&sim));
    assert_eq!(sim.counted_judgements(), sim.log().events());
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HealthProcessor.cs (meetsAnyFailCondition, TriggerFailure)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModEasyWithExtraLives.cs (PerformFail)
#[test]
fn a_met_condition_still_asks_the_mods_to_fail() {
    // Easy's extra lives absorb Sudden Death's fails, refilling the health each time.
    let map = beatmap("256,192,1000,1,0\n");
    let mut mods = ModSet::new(vec![
        sudden_death(false),
        Mod::Easy {
            retries: 2,
            lives_used: 0,
        },
    ])
    .unwrap();
    let mut hp = HealthProcessor::for_play(&map, &mods);
    let miss = judgement(HitResult::Miss, HitResult::Great);
    for _ in 0..2 {
        hp.apply(&miss, &mut mods);
        assert!(!hp.has_failed());
        assert_eq!(hp.health(), 1.0);
    }
    hp.apply(&miss, &mut mods);
    assert!(hp.has_failed());

    // A judgement that meets no condition at full health does not fail.
    let mut mods = ModSet::new(vec![sudden_death(false)]).unwrap();
    let mut hp = HealthProcessor::for_play(&map, &mods);
    hp.apply(&judgement(HitResult::Meh, HitResult::Great), &mut mods);
    assert!(!hp.has_failed());
    hp.apply(&miss, &mut mods);
    assert!(hp.has_failed());
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HealthProcessor.cs (ApplyResultInternal)
#[test]
fn judgements_after_the_fail_in_the_same_step_do_not_count() {
    // Two circles at the same time are missed in the same step; the first fails the play, the
    // second is `FailedAtJudgement` and scores nothing.
    let map = beatmap("256,192,1000,1,0\n100,100,1000,1,0\n");
    let mods = ModSet::new(vec![sudden_death(false)]).unwrap();
    let mut sim = Simulator::new(
        &map,
        ReplayInput::new([frame(0.0, Actions::NONE)]),
        OsuRules::new(&map, &mods),
    )
    .with_health(HealthProcessor::for_play(&map, &mods), mods.clone())
    .with_scoring(ScoreProcessor::for_play(&map, &mods));
    sim.run();
    assert!(failed(&sim));
    assert_eq!(sim.log().events().len(), 2);
    assert_eq!(sim.log().events()[0].time, sim.log().events()[1].time);
    assert_eq!(sim.counted_judgements(), &sim.log().events()[..1]);
    let score = sim.score().expect("score");
    assert_eq!(score.statistics().count(HitResult::Miss), 1);
    assert_eq!(score.rank(&mods), ScoreRank::F);
}
