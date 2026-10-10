//! Replay corpus harness: runs the replays of the corpus through the headless simulator and
//! compares the outcome with the result stored in the replay.
//!
//! Test-only by design: it reads files and hashes, which `slam-osu` itself must never do.

#![allow(dead_code)]

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use slam_formats::osr::{self, Replay, ScoreRank};
use slam_formats::osu;
use slam_osu::judgement::{HitResult, JudgementLog, Statistics};
use slam_osu::mods::FIRST_LAZER_VERSION;
use slam_osu::replay::{NoRules, ReplayInput, Rules, Simulator};
use slam_osu::{Beatmap, GameplayMod, ModSet};

/// The score is not compared yet.
const SCORE_NOTE: &str = "score: not computed (scoring lands in #53)";

/// Steps allowed on top of the replay's frames and twice the 60 Hz steps of the map's length.
///
/// The slack also covers the steps before time zero: gameplay starts at a negative time
/// (first object minus its preempt, at least 2000 ms) and those steps are not part of the
/// map's length.
const STEP_CAP_SLACK: u64 = 100_000;

/// Lazer versions up to this one are stable replays converted by lazer
/// (`LegacyScoreDecoder`): their statistics are not lazer's own, so they cannot be a lazer
/// reference.
const LAST_CONVERTED_LAZER_VERSION: i32 = FIRST_LAZER_VERSION + 1;

/// The corpus directory: `$SLAM_CORPUS`, else `slam-corpus` next to the SLAM repository.
/// `None` if it does not exist.
pub fn corpus_root() -> Option<PathBuf> {
    let root = match std::env::var_os("SLAM_CORPUS") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../slam-corpus"),
    };
    root.is_dir().then_some(root)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Lazer,
    Stable,
}

impl Source {
    fn dir(self) -> &'static str {
        match self {
            Source::Lazer => "lazer",
            Source::Stable => "stable",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[serde(default)]
    replay: Vec<RawEntry>,
}

/// One `[[replay]]` of a manifest, `source` still unchecked.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawEntry {
    pub file: String,
    pub map_sha256: String,
    pub source: String,
    pub client_version: Option<String>,
    pub notes: Option<String>,
    #[serde(default)]
    pub ci: bool,
}

/// Parses the text of a `manifest.toml`.
pub fn parse_manifest(text: &str) -> Result<Vec<RawEntry>, String> {
    toml::from_str::<Manifest>(text)
        .map(|m| m.replay)
        .map_err(|e| e.to_string())
}

/// One differing count: `name` is a result name, or `max combo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    pub name: String,
    pub expected: i64,
    pub got: i64,
}

/// The earliest provable divergence: the first judgement whose running count exceeds the
/// expected total. The real first divergence is at or before it, since a count that is too low
/// shows only at the end.
#[derive(Debug, Clone, PartialEq)]
pub enum Divergence {
    Object {
        index: u32,
        nested: Option<u32>,
        time: f64,
        result: HitResult,
    },
    /// All counts match, our running max combo first exceeded the expected one here.
    ComboOnly {
        index: u32,
        nested: Option<u32>,
        time: f64,
    },
    /// All counts match and our max combo is lower than expected.
    ComboOnlyUndershoot,
    /// Counts too low, nothing exceeded.
    Undershoot,
}

impl fmt::Display for Divergence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Divergence::Object {
                index,
                nested,
                time,
                result,
            } => {
                write!(f, "object {index}")?;
                if let Some(n) = nested {
                    write!(f, " (nested {n})")?;
                }
                write!(f, " at {time:.1} ms got {result:?}")
            }
            Divergence::ComboOnly {
                index,
                nested,
                time,
            } => {
                write!(f, "combo only: object {index}")?;
                if let Some(n) = nested {
                    write!(f, " (nested {n})")?;
                }
                write!(f, " at {time:.1} ms exceeds the expected max combo")
            }
            Divergence::ComboOnlyUndershoot => f.write_str("combo only, undershoot"),
            Divergence::Undershoot => {
                f.write_str("undershoot, no object exceeds its expected count")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Match,
    Mismatch {
        diffs: Vec<Diff>,
        first_divergence: Divergence,
    },
    StableWithinTolerance {
        diffs: Vec<Diff>,
    },
    StableOutsideTolerance {
        diffs: Vec<Diff>,
    },
    /// A lazer replay differs, but the rules in use do not judge yet (see `rules_for`).
    NotJudged {
        diffs: Vec<Diff>,
    },
    /// Not comparable; the reason is shown. Never fails the run.
    Skipped(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntryReport {
    /// `source/file`.
    pub name: String,
    pub outcome: Outcome,
}

#[derive(Debug, Default)]
pub struct Report {
    pub entries: Vec<EntryReport>,
}

impl Report {
    fn count(&self, f: impl Fn(&Outcome) -> bool) -> usize {
        self.entries.iter().filter(|e| f(&e.outcome)).count()
    }

    pub fn matched(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Match))
    }

    pub fn mismatched(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Mismatch { .. }))
    }

    pub fn errors(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Error(_)))
    }

    pub fn not_judged(&self) -> usize {
        self.count(|o| matches!(o, Outcome::NotJudged { .. }))
    }

    pub fn skipped(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Skipped(_)))
    }

    pub fn stable_within(&self) -> usize {
        self.count(|o| matches!(o, Outcome::StableWithinTolerance { .. }))
    }

    pub fn stable_outside(&self) -> usize {
        self.count(|o| matches!(o, Outcome::StableOutsideTolerance { .. }))
    }

    /// Any lazer mismatch or any error; stable differences never fail the run.
    pub fn failed(&self) -> bool {
        self.mismatched() > 0 || self.errors() > 0
    }
}

fn write_diffs(f: &mut fmt::Formatter<'_>, diffs: &[Diff]) -> fmt::Result {
    writeln!(f, "  {:<20} {:>9} {:>9}", "result", "expected", "got")?;
    for d in diffs {
        writeln!(f, "  {:<20} {:>9} {:>9}", d.name, d.expected, d.got)?;
    }
    Ok(())
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "corpus: total {}, matched {}, mismatched {}, errors {}, stable within tolerance {}, stable outside tolerance {}, not judged {}, skipped {}",
            self.entries.len(),
            self.matched(),
            self.mismatched(),
            self.errors(),
            self.stable_within(),
            self.stable_outside(),
            self.not_judged(),
            self.skipped(),
        )?;
        writeln!(f, "{SCORE_NOTE}")?;
        for e in &self.entries {
            match &e.outcome {
                Outcome::Match => {}
                Outcome::Mismatch {
                    diffs,
                    first_divergence,
                } => {
                    writeln!(f, "\n{}: MISMATCH", e.name)?;
                    write_diffs(f, diffs)?;
                    writeln!(
                        f,
                        "  earliest provable divergence (the real first divergence is at or before it): {first_divergence}"
                    )?;
                }
                Outcome::StableWithinTolerance { diffs } => {
                    writeln!(f, "\n{}: stable, within tolerance", e.name)?;
                    write_diffs(f, diffs)?;
                }
                Outcome::StableOutsideTolerance { diffs } => {
                    writeln!(f, "\n{}: stable, OUTSIDE tolerance", e.name)?;
                    write_diffs(f, diffs)?;
                }
                Outcome::NotJudged { diffs } => {
                    writeln!(
                        f,
                        "\n{}: not judged (rules not implemented yet (#49))",
                        e.name
                    )?;
                    write_diffs(f, diffs)?;
                }
                Outcome::Skipped(reason) => writeln!(f, "\n{}: skipped, {reason}", e.name)?,
                Outcome::Error(msg) => writeln!(f, "\n{}: ERROR {msg}", e.name)?,
            }
        }
        Ok(())
    }
}

/// The rules the corpus is simulated with, and whether they judge.
///
/// This is the single place to plug in the real osu rules (#49), which must then return
/// `true`: while it is `false`, a lazer difference is reported as `NotJudged` and does not
/// fail the run. The rules must honour `mods` (Classic included: stable replays get it from
/// `ModSet::from_replay_parts`).
fn rules_for(beatmap: &Beatmap, _mods: &ModSet) -> (NoRules, bool) {
    (NoRules::new(beatmap), false)
}

/// Runs the corpus under `root`. `filter` is a case-sensitive substring of an entry's `file`
/// (empty selects all); `ci_only` keeps only entries with `ci = true`.
pub fn run_corpus(root: &Path, filter: &str, ci_only: bool) -> Report {
    let mut report = Report::default();
    for source in [Source::Lazer, Source::Stable] {
        let dir = root.join(source.dir());
        if !dir.is_dir() {
            continue;
        }
        let manifest = dir.join("manifest.toml");
        let text = match std::fs::read_to_string(&manifest) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                report.entries.push(EntryReport {
                    name: format!("{}/manifest.toml", source.dir()),
                    outcome: Outcome::Error(e.to_string()),
                });
                continue;
            }
        };
        let entries = match parse_manifest(&text) {
            Ok(entries) => entries,
            Err(e) => {
                report.entries.push(EntryReport {
                    name: format!("{}/manifest.toml", source.dir()),
                    outcome: Outcome::Error(format!("invalid manifest: {e}")),
                });
                continue;
            }
        };
        for entry in entries {
            if !entry.file.contains(filter) || (ci_only && !entry.ci) {
                continue;
            }
            let name = format!("{}/{}", source.dir(), entry.file);
            let outcome = match run_entry(&dir, source, &entry) {
                Ok(o) => o,
                Err(e) => Outcome::Error(e),
            };
            report.entries.push(EntryReport { name, outcome });
        }
    }
    report
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The beatmap, mods and input of a replay, ready to simulate.
pub struct Prepared {
    pub beatmap: Beatmap,
    pub mods: ModSet,
    pub input: ReplayInput,
}

/// Applies the replay's mods to the map and builds its input (with the format-version shift).
pub fn prepare(replay: &Replay, map_bytes: &[u8]) -> Result<Prepared, String> {
    let file = osu::decode(map_bytes)
        .map_err(|e| format!("map decode: {e}"))?
        .beatmap;
    let mut mods = ModSet::from_replay(replay).map_err(|e| format!("mods: {e}"))?;
    let beatmap = mods
        .playable_beatmap(file)
        .map_err(|e| format!("beatmap: {e}"))?;
    let input = ReplayInput::from_replay(replay, beatmap.format_version);
    Ok(Prepared {
        beatmap,
        mods,
        input,
    })
}

/// Steps the simulation to its end, failing if it does not finish within a generous cap.
pub fn simulate<R: Rules>(
    beatmap: &Beatmap,
    input: ReplayInput,
    rules: R,
) -> Result<JudgementLog, String> {
    let frames = input.frames().len() as u64;
    let mut sim = Simulator::new(beatmap, input, rules);
    // Every frame, twice the 60 Hz steps of the whole map, and a fixed slack.
    let end_time = beatmap
        .hit_objects
        .iter()
        .map(|o| o.end_time())
        .fold(0.0, f64::max);
    let cap = frames + (end_time / (1000.0 / 60.0) * 2.0) as u64 + STEP_CAP_SLACK;
    while sim.step() {
        if sim.steps() > cap {
            return Err(format!("simulation did not finish within {cap} steps"));
        }
    }
    Ok(sim.log().clone())
}

fn run_entry(dir: &Path, manifest_source: Source, entry: &RawEntry) -> Result<Outcome, String> {
    let source = match entry.source.as_str() {
        "lazer" => Source::Lazer,
        "stable" => Source::Stable,
        other => return Err(format!("unknown source {other:?}")),
    };
    if source != manifest_source {
        return Err(format!(
            "source {:?} in the {} manifest",
            entry.source,
            manifest_source.dir()
        ));
    }

    // Map files are named by the lowercase hex digest.
    let wanted = entry.map_sha256.to_lowercase();
    let map_bytes = std::fs::read(dir.join("maps").join(format!("{wanted}.osu")))
        .map_err(|e| format!("map: {e}"))?;
    let digest = sha256_hex(&map_bytes);
    if digest != wanted {
        return Err(format!(
            "map hash mismatch: file is {digest}, manifest says {}",
            entry.map_sha256
        ));
    }
    let replay_bytes = std::fs::read(dir.join(&entry.file)).map_err(|e| format!("replay: {e}"))?;
    let replay = osr::decode(&replay_bytes)
        .map_err(|e| format!("replay decode: {e}"))?
        .replay;
    if replay.mode != 0 {
        return Err(format!("unsupported ruleset {}", replay.mode));
    }
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreDecoder.cs (version checks)
    match source {
        Source::Lazer if replay.version <= LAST_CONVERTED_LAZER_VERSION => {
            return Err(
                "converted legacy score: re-exported stable replay (lazer source needs version >= 30000002)"
                    .into(),
            );
        }
        Source::Stable if replay.version >= FIRST_LAZER_VERSION => {
            return Err(format!(
                "stable source with lazer version {}",
                replay.version
            ));
        }
        _ => {}
    }

    let info = match source {
        Source::Lazer => Some(
            replay
                .score_info()
                .map_err(|e| format!("score info: {e}"))?
                .ok_or("lazer replay without a score-info block")?,
        ),
        Source::Stable => None,
    };
    if let Some(info) = &info
        && info.rank == Some(ScoreRank::F)
    {
        return Ok(Outcome::Skipped("failed play: health not simulated".into()));
    }

    let Prepared {
        beatmap,
        mods,
        input,
    } = prepare(&replay, &map_bytes)?;
    if !mods.is_supported() {
        let unsupported: Vec<&str> = mods
            .mods()
            .iter()
            .filter(|m| !m.is_supported())
            .map(|m| m.acronym())
            .collect();
        return Ok(Outcome::Skipped(format!(
            "unsupported mods: {}",
            unsupported.join(", ")
        )));
    }
    let (rules, judges) = rules_for(&beatmap, &mods);
    let log = simulate(&beatmap, input, rules)?;

    match info {
        Some(info) => lazer_outcome(&info.statistics, u32::from(replay.max_combo), &log, judges),
        None => Ok(compare_stable(&replay, &log)),
    }
}

/// The lazer verdict: `Match`, `Mismatch`, or `NotJudged` when the rules do not judge.
pub fn lazer_outcome(
    expected: &[(osr::HitResult, i32)],
    expected_max_combo: u32,
    log: &JudgementLog,
    judges: bool,
) -> Result<Outcome, String> {
    let (diffs, divergence) = compare_lazer(expected, expected_max_combo, log)?;
    Ok(match divergence {
        None => Outcome::Match,
        Some(_) if !judges => Outcome::NotJudged { diffs },
        Some(first_divergence) => Outcome::Mismatch {
            diffs,
            first_divergence,
        },
    })
}

/// Exact comparison of the hit counts and max combo; the divergence is `Some` iff there are
/// diffs. A statistic lazer would reject (unknown key, negative count) is an error.
///
/// `LegacyComboIncrease` is not compared: lazer's `PopulateScore` iterates
/// `HitResultExtensions.ALL_TYPES`, which excludes it. It still counts towards the combo.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitResult.cs (ALL_TYPES), osu.Game/Rulesets/Scoring/ScoreProcessor.cs (PopulateScore)
pub fn compare_lazer(
    expected: &[(osr::HitResult, i32)],
    expected_max_combo: u32,
    log: &JudgementLog,
) -> Result<(Vec<Diff>, Option<Divergence>), String> {
    for (key, count) in expected {
        if let osr::HitResult::Unknown(name) = key {
            return Err(format!("unknown statistic key {name:?}"));
        }
        if *count < 0 {
            return Err(format!("negative count {count} for {}", key.as_str()));
        }
    }
    let mut expected_counts = [0i64; HitResult::COUNT];
    let mut diffs = Vec::new();
    for result in HitResult::ALL {
        if result == HitResult::None || result == HitResult::LegacyComboIncrease {
            continue;
        }
        let key = result.to_score_key();
        let want = expected
            .iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, n)| i64::from(*n))
            .sum::<i64>();
        expected_counts[result.index()] = want;
        let got = i64::from(log.statistics().count(result));
        if want != got {
            diffs.push(Diff {
                name: format!("{result:?}"),
                expected: want,
                got,
            });
        }
    }
    let counts_match = diffs.is_empty();
    let got_combo = i64::from(log.statistics().max_combo);
    if i64::from(expected_max_combo) != got_combo {
        diffs.push(Diff {
            name: "max combo".into(),
            expected: i64::from(expected_max_combo),
            got: got_combo,
        });
    }
    if diffs.is_empty() {
        return Ok((diffs, None));
    }
    let divergence = if counts_match {
        combo_divergence(expected_max_combo, log)
    } else {
        first_divergence(&expected_counts, log)
    };
    Ok((diffs, Some(divergence)))
}

/// The first judgement at which our running max combo exceeds the expected one.
fn combo_divergence(expected_max_combo: u32, log: &JudgementLog) -> Divergence {
    let mut running = Statistics::default();
    for j in log.events() {
        running.apply(j.result);
        if running.max_combo > expected_max_combo {
            return Divergence::ComboOnly {
                index: j.object.index,
                nested: j.object.nested,
                time: j.time,
            };
        }
    }
    Divergence::ComboOnlyUndershoot
}

/// Walks the log keeping running counts; the first judgement pushing a count over its expected
/// total is the divergence. `LegacyComboIncrease` is skipped as in the comparison.
fn first_divergence(expected: &[i64; HitResult::COUNT], log: &JudgementLog) -> Divergence {
    let mut running = [0i64; HitResult::COUNT];
    for j in log.events() {
        if j.result == HitResult::LegacyComboIncrease {
            continue;
        }
        let i = j.result.index();
        running[i] += 1;
        if running[i] > expected[i] {
            return Divergence::Object {
                index: j.object.index,
                nested: j.object.nested,
                time: j.time,
                result: j.result,
            };
        }
    }
    Divergence::Undershoot
}

/// Informational comparison against the legacy counts of a stable replay.
///
/// Stable replays are played with Classic (added by `ModSet::from_replay_parts` for versions
/// below `FIRST_LAZER_VERSION`), and `rules_for` must honour the mods. The fold assumes that
/// sliders and spinners get a top-level Great/Ok/Meh/Miss, as Classic gives.
pub fn compare_stable(replay: &Replay, log: &JudgementLog) -> Outcome {
    let mut got = [0i64; 4]; // 300, 100, 50, miss
    for j in log.events().iter().filter(|j| j.object.nested.is_none()) {
        match j.result {
            HitResult::Great => got[0] += 1,
            HitResult::Ok => got[1] += 1,
            HitResult::Meh => got[2] += 1,
            HitResult::Miss => got[3] += 1,
            _ => {}
        }
    }
    // (name, expected, got, relative tolerance in percent, absolute minimum)
    let rows = [
        ("300", i64::from(replay.count_300), got[0], 1, 1),
        ("100", i64::from(replay.count_100), got[1], 1, 1),
        ("50", i64::from(replay.count_50), got[2], 1, 1),
        ("miss", i64::from(replay.count_miss), got[3], 1, 1),
        (
            "max combo",
            i64::from(replay.max_combo),
            i64::from(log.statistics().max_combo),
            2,
            2,
        ),
    ];
    let mut diffs = Vec::new();
    let mut within = true;
    for (name, expected, got, percent, minimum) in rows {
        if expected == got {
            continue;
        }
        let tolerance = minimum.max((expected * percent + 99) / 100);
        within &= (expected - got).abs() <= tolerance;
        diffs.push(Diff {
            name: name.into(),
            expected,
            got,
        });
    }
    match (diffs.is_empty(), within) {
        (true, _) => Outcome::Match,
        (false, true) => Outcome::StableWithinTolerance { diffs },
        (false, false) => Outcome::StableOutsideTolerance { diffs },
    }
}
