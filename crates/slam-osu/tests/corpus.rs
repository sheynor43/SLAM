//! Replay corpus tests: the full run (`just corpus`), the CI sample, and self-tests of the
//! harness on a synthetic corpus.

mod common;
#[path = "common/corpus.rs"]
mod harness;

use std::path::{Path, PathBuf};

use harness::{Divergence, Outcome, Report, corpus_root, run_corpus};
use sha2::{Digest, Sha256};
use slam_formats::osr::{self, ApiMod, Replay, ScoreInfo, ScoreRank};
use slam_osu::judgement::{HitResult, Judgement, JudgementLog, ObjectRef};
use slam_osu::replay::{Rules, StepInput};

#[test]
#[ignore = "runs the whole corpus; use `just corpus`"]
fn full_corpus() {
    let root = corpus_root().expect("corpus not found: set SLAM_CORPUS or create ../slam-corpus");
    let filter = std::env::var("SLAM_CORPUS_FILTER").unwrap_or_default();
    let report = run_corpus(&root, &filter, false);
    println!("{report}");
    assert!(!report.failed(), "corpus run failed");
}

#[test]
fn ci_sample() {
    let Some(root) = corpus_root() else {
        eprintln!("corpus not found, skipped");
        return;
    };
    let report = run_corpus(&root, "", true);
    println!("{report}");
    assert!(!report.failed(), "corpus sample failed");
}

// ---- Harness self-tests ----

const LAZER_VERSION: i32 = osr::VERSION_LAZER_BLOCK + 1;
const STABLE_VERSION: i32 = 20230101;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("corpus-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for s in ["lazer", "stable"] {
            std::fs::create_dir_all(root.join(s).join("maps")).unwrap();
            std::fs::create_dir_all(root.join(s).join("replays")).unwrap();
        }
        Fixture { root }
    }

    /// Writes the map and returns its SHA-256.
    fn map(&self, source: &str) -> String {
        let sha: String = Sha256::digest(common::MAP.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = self
            .root
            .join(source)
            .join("maps")
            .join(format!("{sha}.osu"));
        std::fs::write(path, common::MAP).unwrap();
        sha
    }

    fn replay(&self, source: &str, file: &str, replay: &Replay) {
        std::fs::write(self.root.join(source).join(file), osr::encode(replay)).unwrap();
    }

    fn manifest(&self, source: &str, text: &str) {
        std::fs::write(self.root.join(source).join("manifest.toml"), text).unwrap();
    }

    fn run(&self, filter: &str, ci_only: bool) -> Report {
        run_corpus(&self.root, filter, ci_only)
    }
}

fn entry(file: &str, sha: &str, source: &str, extra: &str) -> String {
    format!(
        "[[replay]]\nfile = \"{file}\"\nmap_sha256 = \"{sha}\"\nsource = \"{source}\"\n{extra}\n"
    )
}

fn lazer_replay(statistics: Vec<(osr::HitResult, i32)>, max_combo: u16) -> Replay {
    let info = ScoreInfo {
        statistics,
        ..Default::default()
    };
    Replay {
        version: LAZER_VERSION,
        max_combo,
        lazer_block: Some(info.to_block()),
        ..common::replay(&common::noisy_frames(1), 14)
    }
}

fn stable_replay(count_300: u16, max_combo: u16) -> Replay {
    Replay {
        version: STABLE_VERSION,
        count_300,
        max_combo,
        ..common::replay(&common::noisy_frames(1), 14)
    }
}

fn judgement(index: u32, nested: Option<u32>, time: f64, result: HitResult) -> Judgement {
    Judgement {
        object: ObjectRef { index, nested },
        time,
        result,
        max_result: HitResult::Great,
    }
}

#[test]
fn manifest_defaults_and_bad_source() {
    let entries = harness::parse_manifest(&entry("replays/a.osr", "ab", "lazer", "")).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].ci);
    assert_eq!(entries[0].client_version, None);

    let entries = harness::parse_manifest(&entry(
        "replays/a.osr",
        "ab",
        "stable",
        "ci = true\nnotes = \"x\"\nclient_version = \"1\"",
    ))
    .unwrap();
    assert!(entries[0].ci);
    assert_eq!(entries[0].notes.as_deref(), Some("x"));

    assert!(
        harness::parse_manifest("# only a comment\n")
            .unwrap()
            .is_empty()
    );
    assert!(harness::parse_manifest("[[replay]]\nfile = 1").is_err());

    let f = Fixture::new("bad-source");
    let sha = f.map("lazer");
    f.manifest("lazer", &entry("replays/a.osr", &sha, "osu", ""));
    let report = f.run("", false);
    assert!(
        matches!(&report.entries[0].outcome, Outcome::Error(m) if m.contains("unknown source"))
    );
    assert!(report.failed());
}

#[test]
fn missing_manifest_is_empty() {
    let f = Fixture::new("no-manifest");
    let report = f.run("", false);
    assert!(report.entries.is_empty());
    assert!(!report.failed());
    assert!(report.to_string().contains("total 0"));
}

#[test]
fn map_hash_mismatch_is_error() {
    let f = Fixture::new("sha");
    let sha = f.map("lazer");
    // The right file name, content that no longer matches it.
    std::fs::write(
        f.root.join("lazer/maps").join(format!("{sha}.osu")),
        format!("{}\n", common::MAP),
    )
    .unwrap();
    f.replay("lazer", "replays/a.osr", &lazer_replay(vec![], 0));
    f.manifest("lazer", &entry("replays/a.osr", &sha, "lazer", ""));
    let report = f.run("", false);
    assert!(matches!(&report.entries[0].outcome, Outcome::Error(m) if m.contains("hash")));
}

#[test]
fn uppercase_hash_is_accepted() {
    let f = Fixture::new("sha-upper");
    let sha = f.map("lazer");
    f.replay("lazer", "replays/a.osr", &lazer_replay(vec![], 0));
    f.manifest(
        "lazer",
        &entry("replays/a.osr", &sha.to_uppercase(), "lazer", ""),
    );
    assert_eq!(f.run("", false).entries[0].outcome, Outcome::Match);
}

#[test]
fn filter_and_ci_selection() {
    let f = Fixture::new("filter");
    let sha = f.map("lazer");
    f.replay("lazer", "replays/alpha.osr", &lazer_replay(vec![], 0));
    f.replay("lazer", "replays/beta.osr", &lazer_replay(vec![], 0));
    f.manifest(
        "lazer",
        &format!(
            "{}{}",
            entry("replays/alpha.osr", &sha, "lazer", "ci = true"),
            entry("replays/beta.osr", &sha, "lazer", "")
        ),
    );
    assert_eq!(f.run("", false).entries.len(), 2);
    assert_eq!(f.run("alpha", false).entries.len(), 1);
    assert_eq!(f.run("Alpha", false).entries.len(), 0);
    let ci = f.run("", true);
    assert_eq!(ci.entries.len(), 1);
    assert!(ci.entries[0].name.ends_with("alpha.osr"));
}

#[test]
fn lazer_exact_match() {
    let f = Fixture::new("lazer-match");
    let sha = f.map("lazer");
    f.replay("lazer", "replays/a.osr", &lazer_replay(vec![], 0));
    f.manifest("lazer", &entry("replays/a.osr", &sha, "lazer", ""));
    let report = f.run("", false);
    assert_eq!(report.entries[0].outcome, Outcome::Match, "{report}");
    assert_eq!(report.matched(), 1);
    assert!(!report.failed());
}

#[test]
fn lazer_difference_under_placeholder_rules_is_not_judged() {
    let f = Fixture::new("lazer-notjudged");
    let sha = f.map("lazer");
    f.replay(
        "lazer",
        "replays/a.osr",
        &lazer_replay(vec![(osr::HitResult::Great, 3)], 3),
    );
    f.manifest("lazer", &entry("replays/a.osr", &sha, "lazer", ""));
    let report = f.run("", false);
    let Outcome::NotJudged { diffs } = &report.entries[0].outcome else {
        panic!("{report}");
    };
    assert!(
        diffs
            .iter()
            .any(|d| d.name == "Great" && d.expected == 3 && d.got == 0)
    );
    assert!(!report.failed());
    let text = report.to_string();
    assert!(text.contains("not judged 1"), "{text}");
    assert!(text.contains("rules not implemented yet (#49)"), "{text}");
}

#[test]
fn judging_rules_mismatch_fails() {
    let log = JudgementLog::with_capacity(0);
    let expected = [(osr::HitResult::Great, 3)];
    let Outcome::Mismatch {
        first_divergence, ..
    } = harness::lazer_outcome(&expected, 3, &log, true).unwrap()
    else {
        panic!()
    };
    assert_eq!(first_divergence, Divergence::Undershoot);
    let report = Report {
        entries: vec![harness::EntryReport {
            name: "x".into(),
            outcome: harness::lazer_outcome(&expected, 3, &log, true).unwrap(),
        }],
    };
    assert!(report.failed());
    let text = report.to_string();
    assert!(text.contains("earliest provable divergence"), "{text}");
    assert!(text.contains("no object exceeds"), "{text}");
    assert!(text.contains("score: not computed"), "{text}");
    assert!(matches!(
        harness::lazer_outcome(&expected, 3, &log, false).unwrap(),
        Outcome::NotJudged { .. }
    ));
}

#[test]
fn lazer_without_block_is_error() {
    let f = Fixture::new("lazer-noblock");
    let sha = f.map("lazer");
    let replay = Replay {
        lazer_block: None,
        ..lazer_replay(vec![], 0)
    };
    f.replay("lazer", "replays/a.osr", &replay);
    f.manifest("lazer", &entry("replays/a.osr", &sha, "lazer", ""));
    assert!(matches!(
        &f.run("", false).entries[0].outcome,
        Outcome::Error(_)
    ));
}

#[test]
fn other_ruleset_is_error() {
    let f = Fixture::new("mode1");
    let sha = f.map("lazer");
    let replay = Replay {
        mode: 1,
        ..lazer_replay(vec![], 0)
    };
    f.replay("lazer", "replays/a.osr", &replay);
    f.manifest("lazer", &entry("replays/a.osr", &sha, "lazer", ""));
    let report = f.run("", false);
    assert!(
        matches!(&report.entries[0].outcome, Outcome::Error(m) if m.contains("unsupported ruleset"))
    );
}

#[test]
fn divergence_is_first_overshoot() {
    let mut log = JudgementLog::with_capacity(8);
    log.apply(judgement(0, None, 100.0, HitResult::Great));
    log.apply(judgement(1, None, 200.0, HitResult::Great));
    log.apply(judgement(2, Some(1), 300.0, HitResult::Great));
    log.apply(judgement(3, None, 400.0, HitResult::Miss));

    // Two Greats and one Miss expected: the third Great is the first excess.
    let expected = [(osr::HitResult::Great, 2), (osr::HitResult::Miss, 1)];
    let (diffs, divergence) = harness::compare_lazer(&expected, 3, &log).unwrap();
    assert_eq!(
        divergence,
        Some(Divergence::Object {
            index: 2,
            nested: Some(1),
            time: 300.0,
            result: HitResult::Great
        })
    );
    assert!(
        diffs
            .iter()
            .any(|d| d.name == "Great" && d.expected == 2 && d.got == 3)
    );

    // Everything matches.
    let expected = [(osr::HitResult::Great, 3), (osr::HitResult::Miss, 1)];
    let (diffs, divergence) = harness::compare_lazer(&expected, 3, &log).unwrap();
    assert!(diffs.is_empty() && divergence.is_none());

    // Only the combo differs, and ours is lower.
    let (diffs, divergence) = harness::compare_lazer(&expected, 4, &log).unwrap();
    assert_eq!(diffs.len(), 1);
    assert_eq!(divergence, Some(Divergence::ComboOnlyUndershoot));

    // Ours is higher: the second Great is the first judgement above combo 1.
    let (_, divergence) = harness::compare_lazer(&expected, 1, &log).unwrap();
    assert_eq!(
        divergence,
        Some(Divergence::ComboOnly {
            index: 1,
            nested: None,
            time: 200.0
        })
    );
}

#[test]
fn stable_tolerance_never_fails() {
    let f = Fixture::new("stable");
    let sha = f.map("stable");
    // We judge nothing: 1 expected 300 is within max(1, ...), 50 is far outside.
    f.replay("stable", "replays/near.osr", &stable_replay(1, 0));
    f.replay("stable", "replays/far.osr", &stable_replay(50, 40));
    f.replay("stable", "replays/exact.osr", &stable_replay(0, 0));
    f.manifest(
        "stable",
        &format!(
            "{}{}{}",
            entry("replays/near.osr", &sha, "stable", ""),
            entry("replays/far.osr", &sha, "stable", ""),
            entry("replays/exact.osr", &sha, "stable", "")
        ),
    );
    let report = f.run("", false);
    assert!(matches!(
        report.entries[0].outcome,
        Outcome::StableWithinTolerance { .. }
    ));
    assert!(matches!(
        report.entries[1].outcome,
        Outcome::StableOutsideTolerance { .. }
    ));
    assert_eq!(report.entries[2].outcome, Outcome::Match);
    assert_eq!((report.stable_within(), report.stable_outside()), (1, 1));
    assert!(!report.failed());
    let text = report.to_string();
    assert!(text.contains("stable within tolerance 1"), "{text}");
    assert!(text.contains("stable outside tolerance 1"), "{text}");
}

#[test]
fn stable_counts_only_top_level_objects() {
    let mut log = JudgementLog::with_capacity(4);
    log.apply(judgement(0, None, 100.0, HitResult::Great));
    log.apply(judgement(1, Some(0), 150.0, HitResult::Great));
    log.apply(judgement(1, None, 200.0, HitResult::Ok));
    let replay = Replay {
        count_300: 1,
        count_100: 1,
        max_combo: 3,
        ..Default::default()
    };
    assert_eq!(harness::compare_stable(&replay, &log), Outcome::Match);
}

#[test]
fn compare_flags_results_the_replay_lacks() {
    let mut log = JudgementLog::with_capacity(4);
    log.apply(judgement(0, None, 100.0, HitResult::Great));
    log.apply(judgement(0, Some(0), 120.0, HitResult::LargeTickHit));
    log.apply(judgement(1, None, 200.0, HitResult::IgnoreHit));
    let expected = [(osr::HitResult::Great, 1)];
    let Outcome::Mismatch { diffs, .. } = harness::lazer_outcome(&expected, 2, &log, true).unwrap()
    else {
        panic!()
    };
    assert!(
        diffs
            .iter()
            .any(|d| d.name == "LargeTickHit" && d.expected == 0 && d.got == 1)
    );
    assert!(
        diffs
            .iter()
            .any(|d| d.name == "IgnoreHit" && d.expected == 0 && d.got == 1)
    );
}

#[test]
fn legacy_combo_increase_is_not_compared() {
    let mut log = JudgementLog::with_capacity(4);
    log.apply(judgement(0, None, 100.0, HitResult::Great));
    log.apply(judgement(0, Some(0), 120.0, HitResult::LegacyComboIncrease));
    // Combo 2 comes from the increase; the count is not part of the comparison.
    let (diffs, divergence) =
        harness::compare_lazer(&[(osr::HitResult::Great, 1)], 2, &log).unwrap();
    assert!(diffs.is_empty() && divergence.is_none());
}

#[test]
fn bad_statistics_are_errors() {
    let log = JudgementLog::with_capacity(0);
    let unknown = [(osr::HitResult::Unknown("bogus".into()), 1)];
    let e = harness::compare_lazer(&unknown, 0, &log).unwrap_err();
    assert!(e.contains("unknown statistic key"), "{e}");
    let negative = [(osr::HitResult::Great, -1)];
    let e = harness::compare_lazer(&negative, 0, &log).unwrap_err();
    assert!(e.contains("negative count"), "{e}");

    let f = Fixture::new("bad-stats");
    let sha = f.map("lazer");
    f.replay(
        "lazer",
        "replays/a.osr",
        &lazer_replay(vec![(osr::HitResult::Unknown("bogus".into()), 1)], 0),
    );
    f.manifest("lazer", &entry("replays/a.osr", &sha, "lazer", ""));
    let report = f.run("", false);
    assert!(
        matches!(&report.entries[0].outcome, Outcome::Error(m) if m.contains("unknown statistic key"))
    );
}

fn lazer_replay_with(info: ScoreInfo) -> Replay {
    Replay {
        version: LAZER_VERSION,
        lazer_block: Some(info.to_block()),
        ..common::replay(&common::noisy_frames(1), 14)
    }
}

fn api_mod(acronym: &str) -> ApiMod {
    ApiMod {
        acronym: acronym.into(),
        settings: vec![],
        extra: vec![],
    }
}

#[test]
fn failed_play_is_skipped() {
    let f = Fixture::new("failed");
    let sha = f.map("lazer");
    let info = ScoreInfo {
        rank: Some(ScoreRank::F),
        ..Default::default()
    };
    f.replay("lazer", "replays/a.osr", &lazer_replay_with(info));
    f.manifest("lazer", &entry("replays/a.osr", &sha, "lazer", ""));
    let report = f.run("", false);
    assert!(matches!(&report.entries[0].outcome, Outcome::Skipped(m) if m.contains("failed play")));
    assert!(!report.failed());
    let text = report.to_string();
    assert!(
        text.contains("skipped 1") && text.contains("health not simulated"),
        "{text}"
    );
}

#[test]
fn unsupported_mod_is_skipped() {
    let f = Fixture::new("unsupported");
    let sha = f.map("lazer");
    let info = ScoreInfo {
        mods: vec![api_mod("ZZ")],
        ..Default::default()
    };
    f.replay("lazer", "replays/a.osr", &lazer_replay_with(info));
    f.manifest("lazer", &entry("replays/a.osr", &sha, "lazer", ""));
    let report = f.run("", false);
    assert!(matches!(&report.entries[0].outcome, Outcome::Skipped(m) if m.contains("ZZ")));
    assert!(!report.failed());
}

#[test]
fn version_guards() {
    let f = Fixture::new("versions");
    let lazer_sha = f.map("lazer");
    let stable_sha = f.map("stable");
    let converted = Replay {
        version: LAZER_VERSION - 1,
        ..lazer_replay(vec![], 0)
    };
    f.replay("lazer", "replays/converted.osr", &converted);
    f.replay("lazer", "replays/old.osr", &stable_replay(0, 0));
    let lazer_as_stable = lazer_replay(vec![], 0);
    f.replay("stable", "replays/lazer.osr", &lazer_as_stable);
    f.manifest(
        "lazer",
        &format!(
            "{}{}",
            entry("replays/converted.osr", &lazer_sha, "lazer", ""),
            entry("replays/old.osr", &lazer_sha, "lazer", "")
        ),
    );
    f.manifest(
        "stable",
        &entry("replays/lazer.osr", &stable_sha, "stable", ""),
    );
    let report = f.run("", false);
    assert_eq!(report.errors(), 3, "{report}");
    assert!(
        matches!(&report.entries[0].outcome, Outcome::Error(m) if m.contains("converted legacy score"))
    );
}

#[test]
fn unknown_manifest_fields_are_rejected() {
    assert!(harness::parse_manifest(&entry("a.osr", "ab", "lazer", "extra = 1")).is_err());
    assert!(harness::parse_manifest("replays = []\n").is_err());
}

#[test]
fn mods_are_applied_to_the_beatmap() {
    let info = ScoreInfo {
        mods: vec![api_mod("HR")],
        ..Default::default()
    };
    let prepared = harness::prepare(&lazer_replay_with(info), common::MAP.as_bytes()).unwrap();
    // HardRock flips the playfield vertically: y 100 -> 284.
    let y = prepared.beatmap.hit_objects[0].stacked_position().y;
    assert_eq!(y, 284.0);

    let plain = harness::prepare(&lazer_replay(vec![], 0), common::MAP.as_bytes()).unwrap();
    assert_eq!(plain.beatmap.hit_objects[0].stacked_position().y, 100.0);
}

#[test]
fn old_format_version_shifts_the_input() {
    let replay = lazer_replay(vec![], 0);
    let old_map = common::MAP.replacen("v14", "v4", 1);
    let old = harness::prepare(&replay, old_map.as_bytes()).unwrap();
    let new = harness::prepare(&replay, common::MAP.as_bytes()).unwrap();
    assert!(old.beatmap.format_version < 5);
    let a = old.input.frames()[0].time;
    let b = new.input.frames()[0].time;
    assert_eq!((a - b).abs(), 24.0);
}

/// Rules that never complete.
struct Never;

impl Rules for Never {
    fn max_judgements(&self) -> usize {
        0
    }
    fn update(&mut self, _: &StepInput<'_>, _: &mut JudgementLog) {}
    fn is_complete(&self) -> bool {
        false
    }
}

#[test]
fn step_cap_stops_endless_rules() {
    let beatmap = common::beatmap();
    let input = slam_osu::replay::ReplayInput::from_replay(&common::replay(&[], 14), 14);
    let e = harness::simulate(&beatmap, input, Never).unwrap_err();
    assert!(e.contains("did not finish"), "{e}");

    let input = slam_osu::replay::ReplayInput::from_replay(&common::replay(&[], 14), 14);
    let log = harness::simulate(&beatmap, input, slam_osu::replay::NoRules::new(&beatmap)).unwrap();
    assert!(log.events().is_empty());
}
