//! Scoring of simulated plays: the score processor driven by the simulator, with health.

use slam_formats::osu::{Vec2, decode_str};
use slam_osu::judgement::HitResult;
use slam_osu::mods::DifficultyAdjustSettings;
use slam_osu::replay::{Actions, InputFrame, ReplayInput, Simulator};
use slam_osu::rules::OsuRules;
use slam_osu::scoring::{ScoreRank, ScoringOptions};
use slam_osu::{Beatmap, HealthProcessor, Mod, ModSet, ScoreProcessor};

/// Twenty circles 300 ms apart, alternating between two spots.
fn circles(hp: f32) -> Beatmap {
    let mut objects = String::new();
    for i in 0..20 {
        let x = if i % 2 == 0 { 150 } else { 350 };
        objects += &format!("{x},192,{},1,0\n", 1000 + i * 300);
    }
    let text = format!(
        "osu file format v14\n\n[Difficulty]\nHPDrainRate:{hp}\nCircleSize:5\nOverallDifficulty:5\n\
         ApproachRate:5\nSliderMultiplier:1.4\nSliderTickRate:1\n\n[TimingPoints]\n\
         0,1000,4,2,0,100,1,0\n\n[HitObjects]\n{objects}"
    );
    Beatmap::from_file(decode_str(&text).unwrap().beatmap).unwrap()
}

fn frame(time: f64, position: Vec2, actions: Actions) -> InputFrame {
    InputFrame {
        time,
        position,
        actions,
    }
}

/// Presses every circle on time except those in `skip`, which are left alone. Starts, as
/// lazer's tests do, with two frames far above the playfield.
fn input(map: &Beatmap, skip: &[usize]) -> ReplayInput {
    let away = Vec2::new(256.0, -500.0);
    let mut frames = vec![
        frame(0.0, away, Actions::NONE),
        frame(0.0, away, Actions::NONE),
    ];
    for (i, o) in map.hit_objects.iter().enumerate() {
        if !skip.contains(&i) {
            frames.push(frame(o.start_time, o.stacked_position(), Actions::LEFT));
        }
        frames.push(frame(
            o.start_time + 50.0,
            o.stacked_position(),
            Actions::NONE,
        ));
    }
    ReplayInput::new(frames)
}

fn play(map: &Beatmap, mods: &ModSet, skip: &[usize]) -> Simulator<OsuRules> {
    let mut sim = Simulator::new(map, input(map, skip), OsuRules::new(map, mods))
        .with_health(HealthProcessor::for_play(map, mods), mods.clone())
        .with_scoring(ScoreProcessor::for_play(map, mods));
    sim.run();
    assert!(sim.is_finished());
    sim
}

#[test]
fn a_perfect_play_scores_the_maximum() {
    let map = circles(5.0);
    let mods = ModSet::default();
    let sim = play(&map, &mods, &[]);
    let score = sim.score().expect("score");
    assert_eq!(score.statistics().count(HitResult::Great), 20);
    assert_eq!(score.total_score(), 1_000_000);
    assert_eq!(score.total_score(), score.maximum_total_score());
    assert_eq!(score.accuracy(), 1.0);
    assert_eq!(score.statistics().max_combo, 20);
    assert_eq!(score.maximum_statistics().max_combo, 20);
    assert_eq!(score.rank(&mods), ScoreRank::X);
}

#[test]
fn a_miss_keeps_the_rank_at_a() {
    let map = circles(5.0);
    let mods = ModSet::default();
    let sim = play(&map, &mods, &[10]);
    let score = sim.score().expect("score");
    assert_eq!(score.statistics().count(HitResult::Miss), 1);
    assert_eq!(score.statistics().max_combo, 10);
    assert_eq!(score.accuracy(), 19.0 / 20.0);
    assert_eq!(score.rank(&mods), ScoreRank::A);
    assert!(score.total_score() < 1_000_000);
    // The score's counts are the log's while the player has not failed.
    assert_eq!(score.statistics(), sim.log().statistics());
}

#[test]
fn the_multiplier_scales_the_score() {
    let map = circles(5.0);
    let mods = ModSet::new(vec![Mod::NoFail]).unwrap();
    let sim = play(&map, &mods, &[]);
    let score = sim.score().expect("score");
    assert_eq!(score.score_multiplier(), 0.5);
    assert_eq!(score.total_score_without_mods(), 1_000_000);
    assert_eq!(score.total_score(), 500_000);
    assert_eq!(score.maximum_total_score(), 500_000);
}

#[test]
fn a_failed_play_is_ranked_f_and_scores_nothing_after_the_fail() {
    let map = circles(10.0);
    let mods = ModSet::default();
    let all: Vec<usize> = (0..20).collect();
    let sim = play(&map, &mods, &all);
    assert!(sim.health().expect("health").has_failed());
    let score = sim.score().expect("score");
    assert!(score.has_failed());
    assert_eq!(score.rank(&mods), ScoreRank::F);
    assert_eq!(score.total_score(), 0);
    assert_eq!(score.accuracy(), 0.0);
    let misses = score.statistics().count(HitResult::Miss);
    assert!(misses > 0 && misses < 20, "{misses}");
    assert!(misses <= sim.log().statistics().count(HitResult::Miss));
}

/// Pairs of circles at the same time, `offset` ms apart between pairs.
fn pairs(offset: i32) -> Beatmap {
    let mut objects = String::new();
    for i in 0..15 {
        let time = 1000 + i * offset;
        objects += &format!("100,192,{time},1,0\n400,192,{time},1,0\n");
    }
    let text = format!(
        "osu file format v14\n\n[Difficulty]\nHPDrainRate:10\nCircleSize:5\nOverallDifficulty:5\n\
         ApproachRate:5\nSliderMultiplier:1.4\nSliderTickRate:1\n\n[TimingPoints]\n\
         0,1000,4,2,0,100,1,0\n\n[HitObjects]\n{objects}"
    );
    Beatmap::from_file(decode_str(&text).unwrap().beatmap).unwrap()
}

#[test]
fn the_failing_judgement_scores_and_the_rest_of_its_step_does_not() {
    let map = pairs(400);
    let mods = ModSet::default();
    let all: Vec<usize> = (0..map.hit_objects.len()).collect();
    let sim = play(&map, &mods, &all);
    assert!(sim.health().expect("health").has_failed());

    // Replay the log through a fresh health processor: the judgement that fails is the last
    // one scored.
    let mut health = HealthProcessor::for_play(&map, &mods);
    let mut health_mods = mods.clone();
    let events = sim.log().events();
    let failing = events
        .iter()
        .position(|j| {
            health.update(j.time);
            health.apply(j, &mut health_mods);
            health.has_failed()
        })
        .expect("a failing judgement");
    let score = sim.score().expect("score");
    let scored: u32 = score.statistics().counts.iter().sum();
    assert_eq!(scored as usize, failing + 1);
    // Both circles of the failing pair are judged in the failing step.
    assert_eq!(
        events.len(),
        failing + 2,
        "the fail is on the first of a pair"
    );
}

#[test]
fn no_fail_scores_every_miss() {
    let map = circles(10.0);
    let mods = ModSet::new(vec![Mod::NoFail]).unwrap();
    let all: Vec<usize> = (0..20).collect();
    let sim = play(&map, &mods, &all);
    let score = sim.score().expect("score");
    assert!(!score.has_failed());
    assert_eq!(score.statistics().count(HitResult::Miss), 20);
    assert_eq!(score.rank(&mods), ScoreRank::D);
    assert_eq!(score.total_score(), 0);
}

#[test]
fn hidden_ranks_are_silver() {
    let map = circles(5.0);
    let hidden = ModSet::new(vec![Mod::from_acronym("HD")]).unwrap();
    let sim = play(&map, &hidden, &[]);
    assert_eq!(sim.score().expect("score").rank(&hidden), ScoreRank::XH);
}

#[test]
fn difficulty_adjust_multiplier_compares_with_the_difficulty_before_mods() {
    let map_text = "osu file format v14\n\n[Difficulty]\nHPDrainRate:5\nCircleSize:5\n\
                    OverallDifficulty:5\nApproachRate:5\nSliderMultiplier:1.4\nSliderTickRate:1\n\n\
                    [TimingPoints]\n0,1000,4,2,0,100,1,0\n\n[HitObjects]\n256,192,1000,1,0\n";
    let file = || decode_str(map_text).unwrap().beatmap;
    let mut da = ModSet::new(vec![Mod::DifficultyAdjust(DifficultyAdjustSettings {
        circle_size: Some(6.0),
        ..Default::default()
    })])
    .unwrap();
    let map = da.playable_beatmap(file()).unwrap();
    assert_eq!(map.difficulty.circle_size, 6.0);
    assert_eq!(map.difficulty_without_mods.circle_size, 5.0);
    // One step of 1.0 in CS: 1 - 1.0 * 0.5.
    assert_eq!(ScoringOptions::for_play(&map, &da).score_multiplier, 0.5);
}
