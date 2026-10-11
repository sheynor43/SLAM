//! Health on synthetic beatmaps: lazer's health processor tests, drain timing, the combo bonus,
//! the legacy multiplier, and failing through the replay simulator with No Fail and Easy.

use slam_formats::osu::{Break, decode_str};
use slam_osu::Beatmap;
use slam_osu::health::{HealthKind, HealthOptions, HealthProcessor};
use slam_osu::judgement::{HitResult, Judgement, ObjectRef};
use slam_osu::objects::{NestedKind, OsuHitObjectKind};
use slam_osu::replay::{ReplayInput, Simulator};
use slam_osu::rules::OsuRules;
use slam_osu::{Mod, ModSet};

/// A beatmap of `objects` at HP `hp`, everything else lazer's default (5).
fn beatmap(hp: f32, objects: &str) -> Beatmap {
    let text = format!(
        "osu file format v14\n\n[Difficulty]\nHPDrainRate:{hp}\nCircleSize:5\nOverallDifficulty:5\n\
         ApproachRate:5\nSliderMultiplier:1.4\nSliderTickRate:1\n\n[TimingPoints]\n\
         0,1000,4,2,0,100,1,0\n\n[HitObjects]\n{objects}"
    );
    Beatmap::from_file(decode_str(&text).unwrap().beatmap).unwrap()
}

fn options(kind: HealthKind, classic: bool, drain_start_time: f64) -> HealthOptions {
    HealthOptions {
        kind,
        classic_slider_behaviour: classic,
        drain_start_time,
    }
}

fn judgement(
    index: u32,
    nested: Option<u32>,
    result: HitResult,
    max_result: HitResult,
) -> Judgement {
    Judgement {
        object: ObjectRef { index, nested },
        time: 0.0,
        result,
        max_result,
    }
}

/// Lazer's `OsuHealthProcessorTest` objects, each the only object of its beatmap: the
/// judgement's object, its best result, the starting health and whether its worst result fails.
/// A case of [`fail_cases`]: name, beatmap, nested object, best result, starting health, whether
/// the worst result fails.
type FailCase = (&'static str, Beatmap, Option<u32>, HitResult, f64, bool);

fn fail_cases() -> Vec<FailCase> {
    let circle = beatmap(5.0, "256,192,1000,1,0\n");
    // One repeat; ticks every beat.
    let slider = beatmap(5.0, "100,100,1000,2,0,L|400:100,2,300\n");
    let spinner = beatmap(5.0, "256,192,1000,12,0,4000\n");

    let nested = |kind: fn(&NestedKind) -> bool| -> Option<u32> {
        let OsuHitObjectKind::Slider(s) = &slider.hit_objects[0].kind else {
            panic!("not a slider")
        };
        Some(
            s.nested
                .iter()
                .position(|n| kind(&n.kind))
                .expect("nested kind") as u32,
        )
    };
    let head = nested(|k| matches!(k, NestedKind::Head));
    let tick = nested(|k| matches!(k, NestedKind::Tick { .. }));
    let repeat = nested(|k| matches!(k, NestedKind::Repeat { .. }));
    let tail = nested(|k| matches!(k, NestedKind::Tail { .. }));
    let OsuHitObjectKind::Spinner(s) = &spinner.hit_objects[0].kind else {
        panic!("not a spinner")
    };
    let bonus_tick = Some(s.spins_required_for_bonus() as u32);

    use HitResult::{
        Great, IgnoreHit, LargeBonus, LargeTickHit, SliderTailHit, SmallBonus, SmallTickHit,
    };
    vec![
        ("circle", circle, None, Great, 0.01, true),
        ("head", slider.clone(), head, Great, 0.01, true),
        (
            "classic head",
            slider.clone(),
            head,
            LargeTickHit,
            0.01,
            true,
        ),
        ("tick", slider.clone(), tick, LargeTickHit, 0.01, true),
        ("repeat", slider.clone(), repeat, LargeTickHit, 0.01, true),
        ("tail", slider.clone(), tail, SliderTailHit, 0.0, true),
        (
            "classic tail",
            slider.clone(),
            tail,
            SmallTickHit,
            0.01,
            true,
        ),
        ("slider", slider.clone(), None, IgnoreHit, 0.0, true),
        ("classic slider", slider, None, Great, 0.01, true),
        (
            "spinner tick",
            spinner.clone(),
            Some(0),
            SmallBonus,
            0.0,
            false,
        ),
        (
            "spinner bonus tick",
            spinner.clone(),
            bonus_tick,
            LargeBonus,
            0.0,
            false,
        ),
        ("spinner", spinner, None, Great, 0.01, true),
    ]
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/OsuHealthProcessorTest.cs (TestFailAfterMinResult, TestNoFailAfterMaxResult)
#[test]
fn worst_result_fails_and_best_does_not() {
    for (name, map, nested, max, starting, fail_expected) in fail_cases() {
        let classic = name.starts_with("classic");
        for (result, expected) in [(max.min_result(), fail_expected), (max, false)] {
            let mut hp = HealthProcessor::new(&map, options(HealthKind::Standard, classic, 0.0));
            hp.set_health(starting);
            hp.apply(&judgement(0, nested, result, max), &mut ModSet::default());
            assert_eq!(hp.has_failed(), expected, "{name}: {result:?}");
        }
    }
}

#[test]
fn no_fail_keeps_the_player_alive_at_zero_health() {
    let map = beatmap(5.0, "256,192,1000,1,0\n");
    let mut no_fail = ModSet::new(vec![Mod::NoFail]).unwrap();
    let mut hp = HealthProcessor::new(&map, options(HealthKind::Standard, false, 0.0));
    hp.set_health(0.01);
    for _ in 0..3 {
        hp.apply(
            &judgement(0, None, HitResult::Miss, HitResult::Great),
            &mut no_fail,
        );
        assert!(!hp.has_failed());
        assert_eq!(hp.health(), 0.0);
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModEasyWithExtraLives.cs (PerformFail)
#[test]
fn easy_refills_health_until_the_lives_run_out() {
    let map = beatmap(5.0, "256,192,1000,1,0\n");
    let mut easy = ModSet::new(vec![Mod::Easy {
        retries: 2,
        lives_used: 0,
    }])
    .unwrap();
    let mut hp = HealthProcessor::new(&map, options(HealthKind::Standard, false, 0.0));
    let miss = judgement(0, None, HitResult::Miss, HitResult::Great);
    for _ in 0..2 {
        hp.set_health(0.01);
        hp.apply(&miss, &mut easy);
        assert!(!hp.has_failed());
        assert_eq!(hp.health(), 1.0);
    }
    hp.set_health(0.01);
    hp.apply(&miss, &mut easy);
    assert!(hp.has_failed());

    // Once failed, judgements change nothing.
    hp.apply(
        &judgement(0, None, HitResult::Great, HitResult::Great),
        &mut easy,
    );
    assert_eq!(hp.health(), 0.0);
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuHealthProcessor.cs (getHealthIncreaseFor)
#[test]
fn health_changes_by_result() {
    let map = beatmap(10.0, "256,192,1000,1,0\n");
    let mut hp = HealthProcessor::new(&map, options(HealthKind::Standard, false, 0.0));
    let mut mods = ModSet::default();
    let mut change = |result, max| {
        hp.set_health(0.5);
        hp.apply(&judgement(0, None, result, max), &mut mods);
        hp.health() - 0.5
    };
    use HitResult::{Great, LargeBonus, Meh, Miss, Ok};
    assert_eq!(change(Great, Great), 0.5 + 0.03 - 0.5);
    assert_eq!(change(Ok, Great), 0.5 + 0.011 - 0.5);
    assert_eq!(change(Meh, Great), 0.5 + 0.002 - 0.5);
    // HP 10: the hardest end of the ranges.
    assert_eq!(change(Miss, Great), 0.5 + -0.2 - 0.5);
    assert_eq!(change(LargeBonus, LargeBonus), 0.5 + 0.01 - 0.5);
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuHealthProcessor.cs (GetHealthIncreaseFor)
#[test]
fn last_object_of_a_combo_gives_a_bonus_by_how_the_combo_went() {
    // Combos: [0, 1], [2, 3], [4, 5]; the last object of the map ends no combo.
    let map = beatmap(
        5.0,
        "100,100,1000,5,0\n100,100,1500,1,0\n100,100,2000,5,0\n100,100,2500,1,0\n\
         100,100,3000,5,0\n100,100,3500,1,0\n",
    );
    let mut hp = HealthProcessor::new(&map, options(HealthKind::Standard, false, 0.0));
    let mut mods = ModSet::default();
    let mut apply = |index, result| {
        hp.set_health(0.5);
        hp.apply(&judgement(index, None, result, HitResult::Great), &mut mods);
        hp.health()
    };
    use HitResult::{Great, Meh, Ok};
    // A perfect combo.
    assert_eq!(apply(0, Great), 0.5 + 0.03);
    assert_eq!(apply(1, Great), 0.5 + (0.03 + 0.07));
    // An Ok makes it good.
    assert_eq!(apply(2, Ok), 0.5 + 0.011);
    assert_eq!(apply(3, Great), 0.5 + (0.03 + 0.05));
    // A Meh ruins it.
    assert_eq!(apply(4, Meh), 0.5 + 0.002);
    assert_eq!(apply(5, Great), 0.5 + 0.03);
}

#[test]
fn legacy_multiplies_gains_but_not_losses() {
    let map = beatmap(
        8.0,
        "100,100,1000,1,0\n200,100,1300,1,0\n300,100,1600,1,0\n100,200,5000,1,0\n",
    );
    let mut hp = HealthProcessor::new(&map, options(HealthKind::Legacy, true, 1000.0));
    let multiplier = hp.hp_multiplier_normal();
    assert!(multiplier >= 1.0);
    let mut mods = ModSet::default();

    hp.set_health(0.5);
    hp.apply(
        &judgement(0, None, HitResult::Great, HitResult::Great),
        &mut mods,
    );
    assert_eq!(hp.health(), 0.5 + multiplier * 0.03);

    hp.set_health(0.5);
    hp.apply(
        &judgement(1, None, HitResult::Miss, HitResult::Great),
        &mut mods,
    );
    // HP 8: 3/5 of the way from -0.125 to -0.2.
    let miss = -0.125 + (-0.2 - -0.125) * ((8.0 - 5.0) / 5.0);
    assert_eq!(hp.health(), 0.5 + miss);
}

/// Circles every 100 ms from `start` to `end` at HP 10, as lazer's `createBeatmap`.
fn drain_map(start: f64, end: f64, breaks: &[(f64, f64)]) -> Beatmap {
    let mut objects = String::new();
    let mut time = start;
    while time <= end {
        objects += &format!("256,192,{time},1,0\n");
        time += 100.0;
    }
    let mut map = beatmap(10.0, &objects);
    map.breaks = breaks
        .iter()
        .map(|&(start, end)| Break { start, end })
        .collect();
    map
}

fn drain_processor(map: &Beatmap) -> HealthProcessor {
    HealthProcessor::for_play(map, &ModSet::default())
}

/// `Precision.AlmostEquals(value, health, 0.0001f)`.
fn health_is(hp: &HealthProcessor, value: f64) -> bool {
    (value - hp.health()).abs() <= f64::from(0.0001f32)
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Gameplay/TestSceneDrainingHealthProcessor.cs (TestInitialHealthStartsAtOne, TestHealthNotDrainedBeforeGameplayStart, TestHealthDrainedDuringGameplay)
#[test]
fn health_drains_during_gameplay_only() {
    let map = drain_map(1000.0, 2000.0, &[]);
    let mut hp = drain_processor(&map);
    assert!(hp.drain_rate() > 0.0);
    assert!(health_is(&hp, 1.0));
    hp.update(100.0);
    assert!(health_is(&hp, 1.0));
    hp.update(900.0);
    assert!(health_is(&hp, 1.0));

    let map = drain_map(0.0, 1000.0, &[]);
    let mut hp = drain_processor(&map);
    hp.update(0.0);
    hp.update(500.0);
    assert!(!health_is(&hp, 1.0));
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Gameplay/TestSceneDrainingHealthProcessor.cs (TestHealthNotDrainedAfterGameplayEnd)
#[test]
fn health_does_not_drain_after_gameplay_end() {
    let map = drain_map(1000.0, 2000.0, &[]);
    let mut hp = drain_processor(&map);
    hp.update(2001.0);
    hp.set_health(1.0);
    hp.update(2100.0);
    assert!(health_is(&hp, 1.0));
    hp.update(3000.0);
    assert!(health_is(&hp, 1.0));
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Gameplay/TestSceneDrainingHealthProcessor.cs (TestHealthDrainBetweenBreakAndObjects)
#[test]
fn no_drain_from_the_object_before_a_break_to_the_one_after() {
    let map = drain_map(0.0, 2000.0, &[(325.0, 375.0)]);
    let mut hp = drain_processor(&map);
    hp.update(285.0);
    hp.set_health(1.0);
    hp.update(295.0);
    assert!(!health_is(&hp, 1.0));
    hp.update(305.0);
    hp.set_health(1.0);
    for time in [315.0, 365.0, 395.0] {
        hp.update(time);
        assert!(health_is(&hp, 1.0), "{time}");
    }
    hp.update(425.0);
    assert!(!health_is(&hp, 1.0));
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Gameplay/TestSceneDrainingHealthProcessor.cs (TestHealthDrainDuringMaximalBreak)
#[test]
fn no_drain_during_a_maximal_break() {
    let map = drain_map(0.0, 2000.0, &[(300.0, 400.0)]);
    let mut hp = drain_processor(&map);
    hp.update(285.0);
    hp.set_health(1.0);
    hp.update(295.0);
    assert!(!health_is(&hp, 1.0));
    hp.update(305.0);
    hp.set_health(1.0);
    hp.update(395.0);
    assert!(health_is(&hp, 1.0));
    hp.update(425.0);
    assert!(!health_is(&hp, 1.0));
}

#[test]
fn drain_follows_the_rate_between_updates() {
    let map = drain_map(0.0, 2000.0, &[]);
    let mut hp = drain_processor(&map);
    hp.update(100.0);
    hp.update(150.0);
    assert_eq!(hp.health(), 1.0 - hp.drain_rate() * 50.0);
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/TestSceneOsuLegacyHealthProcessor.cs
#[test]
fn legacy_drain_rate_matches_lazer_tests() {
    for (breaks, expected) in [
        (&[][..], 1.4e-5),
        (&[(500.0, 1500.0)][..], 4.3e-5),
        (&[(500.0, 1400.0), (750.0, 1500.0)][..], 4.3e-5),
        (&[(500.0, 1000.0), (1000.0, 1500.0)][..], 4.3e-5),
    ] {
        let mut map = beatmap(5.0, "256,192,0,1,0\n256,192,2000,1,0\n");
        map.breaks = breaks
            .iter()
            .map(|&(start, end)| Break { start, end })
            .collect();
        let hp = HealthProcessor::new(&map, options(HealthKind::Legacy, false, -1000.0));
        assert!(
            (hp.drain_rate() - expected).abs() <= 0.1e-5,
            "{breaks:?}: {}",
            hp.drain_rate()
        );
    }
}

#[test]
fn classic_health_selects_the_legacy_processor() {
    let map = beatmap(5.0, "256,192,0,1,0\n256,192,2000,1,0\n");
    let classic = ModSet::new(vec![Mod::from_acronym("CL")]).unwrap();
    assert_eq!(
        HealthOptions::for_play(&map, &classic).kind,
        HealthKind::Legacy
    );
    assert!(HealthOptions::for_play(&map, &classic).classic_slider_behaviour);
    assert_eq!(
        HealthOptions::for_play(&map, &ModSet::default()).kind,
        HealthKind::Standard
    );

    let mut no_classic_health = Mod::from_acronym("CL");
    let Mod::Classic(settings) = &mut no_classic_health else {
        panic!("not Classic")
    };
    settings.classic_health = false;
    let mods = ModSet::new(vec![no_classic_health]).unwrap();
    assert_eq!(
        HealthOptions::for_play(&map, &mods).kind,
        HealthKind::Standard
    );
    assert_eq!(HealthOptions::for_play(&map, &mods).drain_start_time, 0.0);
}

/// Plays `map` without input with `mods`: every object is missed.
fn play_without_input(map: &Beatmap, mods: &ModSet) -> Simulator<OsuRules> {
    let rules = OsuRules::new(map, mods);
    let mut sim = Simulator::new(map, ReplayInput::default(), rules)
        .with_health(HealthProcessor::for_play(map, mods), mods.clone());
    sim.run();
    assert!(sim.is_finished());
    sim
}

#[test]
fn missing_everything_fails_unless_no_fail() {
    let mut objects = String::new();
    for i in 0..20 {
        objects += &format!("256,192,{},1,0\n", 1000 + i * 300);
    }
    let map = beatmap(8.0, &objects);

    let sim = play_without_input(&map, &ModSet::default());
    let health = sim.health().expect("health");
    assert!(health.has_failed());
    let misses = sim.log().statistics().count(HitResult::Miss);
    assert!(misses > 0 && misses < 20, "{misses}");

    let no_fail = ModSet::new(vec![Mod::NoFail]).unwrap();
    let sim = play_without_input(&map, &no_fail);
    assert!(!sim.health().expect("health").has_failed());
    assert_eq!(sim.log().statistics().count(HitResult::Miss), 20);
    assert_eq!(sim.health().expect("health").health(), 0.0);

    // Two extra lives: two more refills of misses before failing.
    let easy = ModSet::new(vec![Mod::Easy {
        retries: 2,
        lives_used: 0,
    }])
    .unwrap();
    let sim = play_without_input(&map, &easy);
    assert!(sim.health().expect("health").has_failed());
    let easy_misses = sim.log().statistics().count(HitResult::Miss);
    assert!(easy_misses > misses, "{easy_misses} <= {misses}");
}

#[test]
fn without_health_a_play_never_fails() {
    let map = beatmap(10.0, "256,192,1000,1,0\n256,192,1300,1,0\n");
    let mut sim = Simulator::new(
        &map,
        ReplayInput::default(),
        OsuRules::new(&map, &ModSet::default()),
    );
    sim.run();
    assert!(sim.health().is_none());
    assert_eq!(sim.log().statistics().count(HitResult::Miss), 2);
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Scoring/OsuHealthProcessor.cs (GetHealthIncreaseFor)
#[test]
fn nested_judgements_track_the_combo_of_their_slider() {
    // Combos [circle 0, slider 1, circle 2], [slider 3, circle 4], [circle 5, circle 6]; one
    // repeat, ticks every beat.
    let map = beatmap(
        5.0,
        "100,100,500,1,0\n100,100,1000,2,0,L|400:100,2,300\n100,100,6000,1,0\n\
         100,100,7000,6,0,L|400:100,2,300\n100,100,12000,1,0\n100,100,12500,5,0\n\
         100,100,13000,1,0\n",
    );
    let tail = |index: usize| {
        let OsuHitObjectKind::Slider(s) = &map.hit_objects[index].kind else {
            panic!("not a slider")
        };
        let tail = s
            .nested
            .iter()
            .position(|n| matches!(n.kind, NestedKind::Tail { .. }));
        Some(tail.expect("tail") as u32)
    };
    let (tail1, tail3) = (tail(1), tail(3));
    let mut hp = HealthProcessor::new(&map, options(HealthKind::Standard, false, 0.0));
    let mut mods = ModSet::default();
    let mut apply = |index, nested, result, max| {
        hp.set_health(0.5);
        hp.apply(&judgement(index, nested, result, max), &mut mods);
        hp.health()
    };
    use HitResult::{Great, IgnoreHit, IgnoreMiss, SliderTailHit};

    // A missed tail makes the combo good; the slider (not last in its combo) gives nothing.
    apply(0, None, Great, Great);
    apply(1, tail1, IgnoreMiss, SliderTailHit);
    assert_eq!(apply(1, None, IgnoreHit, IgnoreHit), 0.5);
    assert_eq!(apply(2, None, Great, Great), 0.5 + (0.03 + 0.05));

    // A slider is judged after its nested objects, and starting a new combo it resets the combo
    // to perfect: its missed tail is forgotten.
    apply(3, tail3, IgnoreMiss, SliderTailHit);
    apply(3, None, IgnoreHit, IgnoreHit);
    assert_eq!(apply(4, None, Great, Great), 0.5 + (0.03 + 0.07));

    // A new combo starts perfect again after a miss.
    apply(5, None, HitResult::Miss, Great);
    assert_eq!(apply(6, None, Great, Great), 0.5 + 0.03);
}

#[test]
fn draining_to_zero_fails_only_on_the_next_judgement() {
    let map = drain_map(0.0, 2000.0, &[]);
    let mut hp = drain_processor(&map);
    hp.set_health(0.0);
    hp.update(0.0);
    hp.update(500.0);
    assert_eq!(hp.health(), 0.0);
    assert!(!hp.has_failed());

    // A bonus judgement does not fail; one that keeps the health at 0 does.
    let mut mods = ModSet::default();
    hp.apply(
        &judgement(5, None, HitResult::IgnoreMiss, HitResult::LargeBonus),
        &mut mods,
    );
    assert!(!hp.has_failed());
    hp.apply(
        &judgement(5, None, HitResult::IgnoreMiss, HitResult::IgnoreHit),
        &mut mods,
    );
    assert!(hp.has_failed());
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (CheckModsAllowFailure)
#[test]
fn mods_prevent_failing_in_order() {
    let map = beatmap(5.0, "256,192,1000,1,0\n");
    let easy = || Mod::Easy {
        retries: 1,
        lives_used: 0,
    };
    let miss = judgement(0, None, HitResult::Miss, HitResult::Great);
    for (mods, life_used) in [
        (vec![easy(), Mod::NoFail], true),
        (vec![Mod::NoFail, easy()], false),
    ] {
        let mut mods = ModSet::new(mods).unwrap();
        let mut hp = HealthProcessor::new(&map, options(HealthKind::Standard, false, 0.0));
        hp.set_health(0.01);
        hp.apply(&miss, &mut mods);
        assert!(!hp.has_failed());
        assert_eq!(hp.health(), if life_used { 1.0 } else { 0.0 });
        let used = mods
            .mods()
            .iter()
            .any(|m| matches!(m, Mod::Easy { lives_used: 1, .. }));
        assert_eq!(used, life_used);
    }
}

#[test]
fn a_new_play_starts_with_all_extra_lives() {
    let mut mods = ModSet::new(vec![Mod::Easy {
        retries: 2,
        lives_used: 2,
    }])
    .unwrap();
    let mut health = 0.0;
    assert!(mods.clone().perform_fail(&mut health));
    mods.reset_play_state();
    assert!(!mods.perform_fail(&mut health));
}
