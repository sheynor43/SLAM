//! Local scores: one JSON file per beatmap in `scores/` of the data folder (ADR-0125).
//!
//! The loader thread appends a record after a passed attempt's replay is saved; the library
//! thread reads the best ones with the selected difficulty's statistics. Nothing here runs
//! on the update or draw thread.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::replays::ReplayJob;
use serde::{Deserialize, Serialize};
use slam_osu::scoring::ScoreRank;
use slam_play::FinishedAttempt;
use slam_ui::song_select::{LocalScores, MAX_LOCAL_SCORES, ScoreMods, ScoreRow};

/// The file's format version.
pub const VERSION: u32 = 1;

/// One saved score.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Unix milliseconds.
    pub date: i64,
    pub total_score: i64,
    /// 0..=1.
    pub accuracy: f64,
    pub max_combo: u32,
    /// Lazer's letters: `XH`, `X`, `SH`, `S`, `A`, `B`, `C`, `D`.
    pub rank: String,
    /// Lazer's acronyms.
    pub mods: Vec<String>,
    /// Hits by kind, named as lazer names them.
    pub statistics: BTreeMap<String, u32>,
    pub player: String,
    /// The `.osr` file's name in `replays/`, if it was saved.
    pub replay: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    scores: Vec<Record>,
}

impl Record {
    /// The record of a finished attempt saved at `date_ms` (Unix milliseconds); `replay` is
    /// its replay file's name.
    pub fn of(attempt: &FinishedAttempt, date_ms: i64, replay: Option<String>) -> Record {
        let statistics = attempt.statistics();
        Record {
            date: date_ms,
            total_score: attempt.score(),
            accuracy: attempt.accuracy(),
            max_combo: statistics.max_combo,
            rank: attempt.rank().to_score_key().as_str().to_owned(),
            mods: attempt
                .mods()
                .to_api()
                .into_iter()
                .map(|m| m.acronym)
                .collect(),
            statistics: slam_osu::judgement::HitResult::ALL
                .iter()
                .map(|&r| (r, statistics.count(r)))
                .filter(|&(_, n)| n != 0)
                .map(|(r, n)| (r.to_score_key().as_str().to_owned(), n))
                .collect(),
            player: crate::replays::PLAYER_NAME.to_owned(),
            replay,
        }
    }
}

/// Whether `hash` is a lowercase MD5 hex string: it names a file, so nothing else goes
/// into a path.
pub fn valid_hash(hash: &str) -> bool {
    hash.len() == 32 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn file_of(dir: &Path, hash: &str) -> Option<PathBuf> {
    valid_hash(hash).then(|| dir.join(format!("{hash}.json")))
}

/// The records in `path`: `Ok(None)` if there is no file, `Err` if it is unreadable or of
/// another version.
fn read(path: &Path) -> Result<Option<Vec<Record>>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let file: File = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if file.version != VERSION {
        return Err(format!("version {}", file.version));
    }
    Ok(Some(file.scores))
}

/// Adds `record` to the beatmap's file in `dir`, creating it. The file is written to a
/// temporary name and renamed over; one that does not read is renamed to `<md5>.json.bad`
/// first.
pub fn append(dir: &Path, hash: &str, record: Record) -> std::io::Result<()> {
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidInput, "not an md5 hash");
    let path = file_of(dir, hash).ok_or_else(invalid)?;
    std::fs::create_dir_all(dir)?;
    let mut scores = match read(&path) {
        Ok(found) => found.unwrap_or_default(),
        Err(reason) => {
            tracing::warn!("scores {}: {reason}; kept as .bad", path.display());
            let bad = path.with_extension("json.bad");
            std::fs::rename(&path, bad)?;
            Vec::new()
        }
    };
    scores.push(record);
    let bytes = serde_json::to_vec_pretty(&File {
        version: VERSION,
        scores,
    })
    .map_err(std::io::Error::other)?;
    let temporary = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, &path)
}

/// The best `count` records of the beatmap by total score, best first. A missing or
/// unreadable file gives none.
pub fn load_top(dir: &Path, hash: &str, count: usize) -> Vec<Record> {
    let Some(path) = file_of(dir, hash) else {
        return Vec::new();
    };
    let mut scores = match read(&path) {
        Ok(found) => found.unwrap_or_default(),
        Err(reason) => {
            tracing::warn!("scores {}: {reason}", path.display());
            Vec::new()
        }
    };
    scores.sort_by_key(|r| std::cmp::Reverse(r.total_score));
    scores.truncate(count);
    scores
}

/// The song select's rows for `records`, best first.
pub fn rows_of(records: &[Record]) -> LocalScores {
    let mut scores = LocalScores::default();
    for (slot, r) in scores
        .rows
        .iter_mut()
        .zip(records.iter().take(MAX_LOCAL_SCORES))
    {
        let mut rank = [0; 2];
        for (to, from) in rank.iter_mut().zip(r.rank.bytes()) {
            *to = from;
        }
        *slot = Some(ScoreRow {
            total_score: r.total_score.max(0) as u64,
            accuracy: r.accuracy,
            max_combo: r.max_combo,
            rank,
            mods: ScoreMods::from_acronyms(r.mods.iter().map(String::as_str)),
            date_ms: r.date,
        });
    }
    scores
}

/// Whether the attempt is saved as a score: lazer does not keep failed ones.
pub fn is_kept(rank: ScoreRank) -> bool {
    rank != ScoreRank::F
}

/// Saves the score of `job`'s attempt into `dir` if it passed; `replay` is its replay
/// file's name. Logs the outcome.
pub fn save_passed(job: &ReplayJob, replay: Option<String>, dir: &Path) {
    if !is_kept(job.attempt.rank()) {
        return;
    }
    let date = job
        .ended
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let record = Record::of(&job.attempt, date, replay);
    if let Err(e) = append(dir, &job.beatmap.hash, record) {
        tracing::warn!("score not saved: {e}");
    }
}

#[cfg(test)]
mod tests;
