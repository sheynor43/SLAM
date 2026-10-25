//! `replay-stats`: runs an `.osr` replay through the headless simulator and prints its counts
//! and hit error statistics (ADR-0074). Developer tool: console output is not localised.

use std::path::PathBuf;

use slam_formats::{osr, osu};
use slam_osu::hit_events::{HitErrorSummary, HitEvents};
use slam_osu::judgement::{HitResult, JudgementLog, Statistics};
use slam_osu::replay::{ReplayInput, Simulator};
use slam_osu::rules::OsuRules;
use slam_osu::{GameplayMod, HealthProcessor, ModSet};

pub const USAGE: &str = "usage: slam-app replay-stats MAP.osu REPLAY.osr";

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub map: PathBuf,
    pub replay: PathBuf,
}

impl Options {
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut paths = Vec::new();
        for arg in args {
            if arg.starts_with("--") {
                return Err(format!("unknown argument {arg}"));
            }
            paths.push(PathBuf::from(arg));
        }
        match <[PathBuf; 2]>::try_from(paths) {
            Ok([map, replay]) => Ok(Options { map, replay }),
            Err(_) => Err("expected a beatmap and a replay".to_owned()),
        }
    }
}

/// What a simulated replay gives.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayStats {
    pub mods: String,
    /// Whether the play failed (lazer replays only: they carry the health of `Player`).
    pub failed: bool,
    pub statistics: Statistics,
    pub hit_errors: HitErrorSummary,
}

pub fn run(options: &Options) -> Result<(), String> {
    let map = std::fs::read(&options.map).map_err(|e| format!("{}: {e}", options.map.display()))?;
    let replay =
        std::fs::read(&options.replay).map_err(|e| format!("{}: {e}", options.replay.display()))?;
    let stats = simulate(&map, &replay)?;
    println!(
        "mods: {}{}",
        if stats.mods.is_empty() {
            "none"
        } else {
            &stats.mods
        },
        if stats.failed { ", failed" } else { "" }
    );
    let statistics = &stats.statistics;
    println!(
        "300: {}, 100: {}, 50: {}, miss: {}, max combo: {}",
        statistics.count(HitResult::Great),
        statistics.count(HitResult::Ok),
        statistics.count(HitResult::Meh),
        statistics.count(HitResult::Miss),
        statistics.max_combo
    );
    println!("{}", stats.hit_errors);
    Ok(())
}

/// Simulates the replay `replay` (an `.osr` file) on the beatmap `map` (an `.osu` file).
pub fn simulate(map: &[u8], replay: &[u8]) -> Result<ReplayStats, String> {
    let replay = osr::decode(replay)
        .map_err(|e| format!("replay: {e}"))?
        .replay;
    if replay.mode != 0 {
        return Err(format!("unsupported ruleset {}", replay.mode));
    }
    let file = osu::decode(map)
        .map_err(|e| format!("beatmap: {e}"))?
        .beatmap;
    let mut mods = ModSet::from_replay(&replay).map_err(|e| format!("mods: {e}"))?;
    if !mods.is_supported() {
        return Err(format!("unsupported mods: {}", acronyms(&mods)));
    }
    let beatmap = mods
        .playable_beatmap(file)
        .map_err(|e| format!("beatmap: {e}"))?;
    let input = ReplayInput::from_replay(&replay, beatmap.format_version);
    let rules = OsuRules::new(&beatmap, &mods);
    let mut sim = Simulator::new(&beatmap, input, rules)
        .with_hit_events(HitEvents::new(&beatmap, mods.true_gameplay_rate()));
    // As the corpus: a lazer score was played by `Player`, which fails; stable never saves a
    // failed play and its health differs from lazer's legacy processor.
    let lazer = replay
        .score_info()
        .map_err(|e| format!("score info: {e}"))?
        .is_some();
    if lazer {
        sim = sim.with_health(HealthProcessor::for_play(&beatmap, &mods), mods.clone());
    }
    sim.run();

    let mut counted = JudgementLog::with_capacity(sim.counted_judgements().len());
    for &judgement in sim.counted_judgements() {
        counted.apply(judgement);
    }
    Ok(ReplayStats {
        mods: acronyms(&mods),
        failed: sim.health().is_some_and(HealthProcessor::has_failed),
        statistics: *counted.statistics(),
        hit_errors: sim.hit_events().map(HitEvents::summary).unwrap_or_default(),
    })
}

fn acronyms(mods: &ModSet) -> String {
    mods.mods().iter().map(|m| m.acronym()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_two_paths() {
        assert_eq!(
            parse(&["a.osu", "b.osr"]),
            Ok(Options {
                map: "a.osu".into(),
                replay: "b.osr".into(),
            })
        );
        assert!(parse(&["a.osu"]).is_err());
        assert!(parse(&["a.osu", "b.osr", "c"]).is_err());
        assert!(parse(&["a.osu", "--x"]).is_err());
    }
}
