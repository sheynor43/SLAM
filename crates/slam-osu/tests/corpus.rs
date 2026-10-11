//! Replay corpus tests: the full run (`just corpus`), the CI sample, and self-tests of the
//! harness on a synthetic corpus.

mod common;
#[path = "common/corpus.rs"]
mod harness;

slam_testkit::install_counting_allocator!();

use std::path::{Path, PathBuf};

use harness::{Divergence, Outcome, Report, corpus_root, run_corpus};
use sha2::{Digest, Sha256};
use slam_formats::osr::{self, ApiMod, GeneratedFrame, Replay, ScoreInfo, ScoreRank};
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

/// The map of the fixture corpora: no objects, so any replay yields no judgements.
const FIXTURE_MAP: &str = "osu file format v14

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:8
ApproachRate:9
SliderMultiplier:1
SliderTickRate:1

[TimingPoints]
0,500,4,2,0,100,1,0

[HitObjects]
";

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

    /// Writes [`FIXTURE_MAP`] and returns its SHA-256.
    fn map(&self, source: &str) -> String {
        self.map_text(source, FIXTURE_MAP)
    }

    /// Writes the map `text` and returns its SHA-256.
    fn map_text(&self, source: &str, text: &str) -> String {
        let sha: String = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = self
            .root
            .join(source)
            .join("maps")
            .join(format!("{sha}.osu"));
        std::fs::write(path, text).unwrap();
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
fn judging_rules_mismatch_fails() {
    let log = JudgementLog::with_capacity(0);
    let expected = [(osr::HitResult::Great, 3)];
    let Outcome::Mismatch {
        first_divergence, ..
    } = harness::lazer_outcome(&expected, 3, &log).unwrap()
    else {
        panic!()
    };
    assert_eq!(first_divergence, Divergence::Undershoot);
    let report = Report {
        entries: vec![harness::EntryReport {
            name: "x".into(),
            outcome: harness::lazer_outcome(&expected, 3, &log).unwrap(),
        }],
    };
    assert!(report.failed());
    let text = report.to_string();
    assert!(text.contains("earliest provable divergence"), "{text}");
    assert!(text.contains("no object exceeds"), "{text}");
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
    // One tolerated result over is within tolerance, and still shown.
    let Outcome::LazerWithinTolerance { diffs, .. } =
        harness::lazer_outcome(&expected, 2, &log).unwrap()
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
fn lazer_score_is_compared() {
    // The fixture map has no objects: the score is 0 and the rank SS.
    let f = Fixture::new("score");
    let sha = f.map("lazer");
    let scored = |info: ScoreInfo| Replay {
        version: harness::FIRST_COMBO_EXPONENT_VERSION,
        ..lazer_replay_with(info)
    };
    let matching = ScoreInfo {
        rank: Some(ScoreRank::X),
        ..Default::default()
    };
    f.replay("lazer", "replays/a.osr", &scored(matching));
    let wrong_rank = ScoreInfo {
        rank: Some(ScoreRank::S),
        ..Default::default()
    };
    f.replay("lazer", "replays/b.osr", &scored(wrong_rank));
    let wrong_score = Replay {
        total_score: 5,
        ..scored(ScoreInfo::default())
    };
    f.replay("lazer", "replays/c.osr", &wrong_score);
    // Before the combo exponent the score is not compared.
    let old_score = Replay {
        total_score: 5,
        version: harness::FIRST_COMBO_EXPONENT_VERSION - 1,
        ..lazer_replay_with(ScoreInfo::default())
    };
    f.replay("lazer", "replays/d.osr", &old_score);
    f.manifest(
        "lazer",
        &[
            entry("replays/a.osr", &sha, "lazer", ""),
            entry("replays/b.osr", &sha, "lazer", ""),
            entry("replays/c.osr", &sha, "lazer", ""),
            entry("replays/d.osr", &sha, "lazer", ""),
        ]
        .concat(),
    );
    let report = f.run("", false);
    assert_eq!(report.entries[0].outcome, Outcome::Match);
    let Outcome::Mismatch {
        diffs,
        first_divergence,
    } = &report.entries[1].outcome
    else {
        panic!("{:?}", report.entries[1].outcome)
    };
    assert_eq!(*first_divergence, Divergence::ScoreOnly);
    assert_eq!(diffs[0].name, "rank (expected S, got X)");
    let Outcome::Mismatch { diffs, .. } = &report.entries[2].outcome else {
        panic!("{:?}", report.entries[2].outcome)
    };
    assert_eq!(
        (diffs[0].name.as_str(), diffs[0].expected, diffs[0].got),
        ("total score", 5, 0)
    );
    assert_eq!(report.entries[3].outcome, Outcome::Match);
    assert!(report.failed());
    assert!(report.to_string().contains("score only"), "{report}");
}

#[test]
fn score_diffs_join_count_diffs() {
    let log = JudgementLog::with_capacity(0);
    let counts = harness::lazer_outcome(&[(osr::HitResult::Great, 1)], 0, &log).unwrap();
    let score = vec![harness::Diff {
        name: "total score".into(),
        expected: 1,
        got: 0,
    }];
    let Outcome::Mismatch {
        diffs,
        first_divergence,
    } = harness::with_score_diffs(counts, score, true)
    else {
        panic!()
    };
    assert_eq!(first_divergence, Divergence::Undershoot);
    assert_eq!(diffs.len(), 2);
    assert_eq!(
        harness::with_score_diffs(Outcome::Match, Vec::new(), true),
        Outcome::Match
    );
}

#[test]
fn tolerated_results_are_compared_within_tolerance() {
    // Ten circles hit and ten sliders: heads hit, tails as given.
    let log_with = |tails_hit: u32| {
        let mut log = JudgementLog::with_capacity(40);
        for i in 0..10 {
            log.apply(judgement(i, None, f64::from(i), HitResult::Great));
        }
        for i in 10..20 {
            log.apply(judgement(i, Some(0), f64::from(i), HitResult::Great));
            let tail = if i - 10 < tails_hit {
                HitResult::SliderTailHit
            } else {
                HitResult::IgnoreMiss
            };
            log.apply(judgement(i, Some(1), f64::from(i), tail));
        }
        log
    };
    let expected = |tails_hit: i32| {
        [
            (osr::HitResult::Great, 20),
            (osr::HitResult::SliderTailHit, tails_hit),
            (osr::HitResult::IgnoreMiss, 10 - tails_hit),
        ]
    };
    let within = |o: &Outcome| matches!(o, Outcome::LazerWithinTolerance { .. });
    let mismatch = |o: &Outcome| matches!(o, Outcome::Mismatch { .. });

    // Combo is broken by nothing: 20 + the tails hit, all in a row only with every tail hit.
    assert_eq!(
        harness::lazer_outcome(&expected(10), 30, &log_with(10)).unwrap(),
        Outcome::Match
    );
    // Two tails off: within max(2, 0.5 %).
    let o = harness::lazer_outcome(&expected(10), 30, &log_with(8)).unwrap();
    assert!(within(&o), "{o:?}");
    // Three off: outside.
    let o = harness::lazer_outcome(&expected(10), 30, &log_with(7)).unwrap();
    assert!(mismatch(&o), "{o:?}");
    // A circle off is a mismatch whatever the tails do.
    let mut circle_expected = expected(9).to_vec();
    circle_expected[0].1 = 21;
    let o = harness::lazer_outcome(&circle_expected, 29, &log_with(9)).unwrap();
    assert!(mismatch(&o), "{o:?}");

    // Equal counts with another max combo: the tail missed may be another one.
    let o = harness::lazer_outcome(&expected(9), 5, &log_with(9)).unwrap();
    assert!(within(&o), "{o:?}");
    // Without a tolerated miss the order is pinned down: a combo difference is a mismatch,
    // tails or not.
    let o = harness::lazer_outcome(&expected(10), 5, &log_with(10)).unwrap();
    assert!(mismatch(&o), "{o:?}");
    assert!(!harness::has_tolerated_misses(&log_with(10)));
    assert!(harness::has_tolerated_misses(&log_with(9)));

    // A score-only difference follows the same rule.
    let score = || {
        vec![harness::Diff {
            name: "total score".into(),
            expected: 1,
            got: 0,
        }]
    };
    assert!(within(&harness::with_score_diffs(
        Outcome::Match,
        score(),
        true
    )));
    assert!(mismatch(&harness::with_score_diffs(
        Outcome::Match,
        score(),
        false
    )));
}

#[test]
fn tolerated_counts_are_bounded_in_sum() {
    // Two sliders with a tick and a bonus each, all hit.
    let mut log = JudgementLog::with_capacity(8);
    for i in 0..2 {
        log.apply(judgement(i, Some(1), 0.0, HitResult::SliderTailHit));
        log.apply(judgement(i, Some(2), 0.0, HitResult::LargeTickHit));
        log.apply(judgement(i, Some(3), 0.0, HitResult::LargeBonus));
    }
    // One tail and one tick missed in lazer: four counts one off, 4 in sum: within.
    let expected = [
        (osr::HitResult::SliderTailHit, 1),
        (osr::HitResult::IgnoreMiss, 1),
        (osr::HitResult::LargeTickHit, 1),
        (osr::HitResult::LargeTickMiss, 1),
        (osr::HitResult::LargeBonus, 2),
    ];
    let o = harness::lazer_outcome(&expected, 6, &log).unwrap();
    assert!(matches!(o, Outcome::LazerWithinTolerance { .. }), "{o:?}");
    // Both missed: each count two off, still within on its own, 8 in sum: a mismatch.
    let expected = [
        (osr::HitResult::IgnoreMiss, 2),
        (osr::HitResult::LargeTickMiss, 2),
        (osr::HitResult::LargeBonus, 2),
    ];
    let o = harness::lazer_outcome(&expected, 6, &log).unwrap();
    assert!(matches!(o, Outcome::Mismatch { .. }), "{o:?}");
    assert_eq!(harness::total_tolerance(0), 4);
    assert_eq!(harness::total_tolerance(1600), 4);
    assert_eq!(harness::total_tolerance(1601), 5);
}

#[test]
fn too_many_within_tolerance_fails_the_run() {
    let entry = |outcome: Outcome| harness::EntryReport {
        name: "lazer/a.osr".into(),
        outcome,
    };
    let within = || Outcome::LazerWithinTolerance {
        diffs: Vec::new(),
        first_divergence: Divergence::ScoreOnly,
    };
    // Three of twelve: a quarter, not more.
    let mut report = Report::default();
    report.entries.extend((0..9).map(|_| entry(Outcome::Match)));
    report.entries.extend((0..3).map(|_| entry(within())));
    assert!(!report.failed(), "{report}");
    // Three of eleven: more than a quarter.
    report.entries.remove(0);
    assert!(report.failed());
    assert!(report.to_string().contains("TOO MANY"), "{report}");
    // A small sample is not judged by its share.
    let report = Report {
        entries: vec![entry(within()), entry(within())],
    };
    assert!(!report.failed());
}

#[test]
fn tolerance_grows_with_the_count() {
    assert_eq!(harness::tolerance(0), 2);
    assert_eq!(harness::tolerance(400), 2);
    assert_eq!(harness::tolerance(401), 3);
    assert_eq!(harness::tolerance(1000), 5);
    assert!(harness::is_tolerated(HitResult::SliderTailHit));
    assert!(harness::is_tolerated(HitResult::LargeBonus));
    for strict in [
        HitResult::Great,
        HitResult::Ok,
        HitResult::Meh,
        HitResult::Miss,
    ] {
        assert!(!harness::is_tolerated(strict));
    }
}

#[test]
fn known_divergence_marks_a_mismatch_and_must_stay_current() {
    let diff = |name: &str, expected, got| harness::Diff {
        name: name.into(),
        expected,
        got,
    };
    let mismatch = |meh: i64| Outcome::Mismatch {
        diffs: vec![
            diff("Meh", 3, meh),
            diff("max combo", 10, 9),
            diff("total score", 5, 4),
        ],
        first_divergence: Divergence::Undershoot,
    };
    let known = harness::KnownDivergence {
        reason: "why".into(),
        counts: [("Meh".to_string(), [3, 6])].into(),
    };
    let Outcome::KnownDivergence { reason, .. } =
        harness::with_known_divergence(mismatch(6), Some(&known)).unwrap()
    else {
        panic!()
    };
    assert_eq!(reason, "why");
    assert_eq!(
        harness::with_known_divergence(mismatch(6), None).unwrap(),
        mismatch(6)
    );
    // Another divergence on the same replay is not hidden.
    let e = harness::with_known_divergence(mismatch(7), Some(&known)).unwrap_err();
    assert!(e.contains("changed"), "{e}");
    assert!(harness::with_known_divergence(Outcome::Match, Some(&known)).is_err());

    let report = Report {
        entries: vec![harness::EntryReport {
            name: "lazer/a.osr".into(),
            outcome: harness::with_known_divergence(mismatch(6), Some(&known)).unwrap(),
        }],
    };
    assert!(!report.failed());
    assert_eq!(report.known(), 1);
    assert!(
        report.to_string().contains("known divergence, why"),
        "{report}"
    );

    // The manifest field is accepted.
    let entries = harness::parse_manifest(
        "[[replay]]\nfile = \"a.osr\"\nmap_sha256 = \"ab\"\nsource = \"lazer\"\n\
         known_divergence = { reason = \"why\", counts = { Meh = [3, 6] } }\n",
    )
    .unwrap();
    assert_eq!(entries[0].known_divergence.as_ref(), Some(&known));
}

#[test]
fn failed_play_is_compared_up_to_the_fail() {
    // Three circles, a replay that never presses: Sudden Death and Perfect fail on the first
    // miss, and lazer's statistics stop there.
    let f = Fixture::new("failed");
    let sha = f.map_text("lazer", common::MAP);
    let idle: Vec<GeneratedFrame> = (0..300)
        .map(|i| GeneratedFrame {
            time: f64::from(i) * 16.0,
            x: 0.0,
            y: 0.0,
            buttons: 0,
        })
        .collect();
    let played = |acronym: &str, misses: i32, rank: ScoreRank| Replay {
        version: harness::FIRST_COMBO_EXPONENT_VERSION,
        lazer_block: Some(
            ScoreInfo {
                mods: vec![api_mod(acronym)],
                statistics: vec![(osr::HitResult::Miss, misses)],
                rank: Some(rank),
                ..Default::default()
            }
            .to_block(),
        ),
        ..common::replay(&idle, 14)
    };
    let failed = |acronym: &str, misses: i32| played(acronym, misses, ScoreRank::F);
    f.replay("lazer", "replays/sd.osr", &failed("SD", 1));
    f.replay("lazer", "replays/pf.osr", &failed("PF", 1));
    f.replay("lazer", "replays/all.osr", &failed("SD", 3));
    // Lazer says the play passed: our fail is named, not left to the count diffs.
    f.replay(
        "lazer",
        "replays/passed.osr",
        &played("SD", 3, ScoreRank::D),
    );
    f.manifest(
        "lazer",
        &[
            entry("replays/sd.osr", &sha, "lazer", ""),
            entry("replays/pf.osr", &sha, "lazer", ""),
            entry("replays/all.osr", &sha, "lazer", ""),
            entry("replays/passed.osr", &sha, "lazer", ""),
        ]
        .concat(),
    );
    let report = f.run("", false);
    assert_eq!(report.entries[0].outcome, Outcome::Match, "{report}");
    assert_eq!(report.entries[1].outcome, Outcome::Match, "{report}");
    let Outcome::Mismatch { diffs, .. } = &report.entries[2].outcome else {
        panic!("{report}")
    };
    assert_eq!(
        (diffs[0].name.as_str(), diffs[0].expected, diffs[0].got),
        ("Miss", 3, 1)
    );
    assert!(
        matches!(&report.entries[3].outcome, Outcome::Error(e) if e.contains("simulated fail")),
        "{report}"
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
    let e = harness::simulate(&beatmap, input, Never, None, None).unwrap_err();
    assert!(e.contains("did not finish"), "{e}");

    let input = slam_osu::replay::ReplayInput::from_replay(&common::replay(&[], 14), 14);
    let harness::Simulated { log, score, .. } = harness::simulate(
        &beatmap,
        input,
        slam_osu::replay::NoRules::new(&beatmap),
        None,
        None,
    )
    .unwrap();
    assert!(log.events().is_empty());
    assert!(score.is_none());
}
