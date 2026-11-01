//! Saves passed plays as `.osr` replays into `replays/` of the data folder (ARCHITECTURE
//! §17), as lazer imports every passed play with its replay, and failed ones the fail menu
//! saves (ADR-0084). Runs on the loader thread.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use slam_formats::osr;
use slam_osu::replay::{PlayScore, ReplayInfo, replay_of_play};
use slam_play::FinishedAttempt;

/// The player's name in replays: there are no accounts yet (lazer's guest user).
pub const PLAYER_NAME: &str = "Guest";

/// What a replay names its beatmap by, taken when the play is prepared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayBeatmap {
    /// MD5 of the beatmap file, lowercase hex.
    pub hash: String,
    /// The beatmap's format version.
    pub format_version: i32,
    /// The beatmap file's name without its extension, for the replay's.
    pub name: String,
}

/// An attempt that passed or whose fail was saved, with what its replay needs besides; made
/// as it is taken.
pub struct ReplayJob {
    pub attempt: Box<FinishedAttempt>,
    /// Shared by the play's attempts.
    pub beatmap: Arc<ReplayBeatmap>,
    /// When the attempt was taken.
    pub ended: SystemTime,
}

/// The `.osr` file of `job`'s attempt: its name and contents. `None` if its recording is
/// incomplete.
pub fn build(job: &ReplayJob) -> Option<(String, Vec<u8>)> {
    let attempt = &job.attempt;
    let frames = attempt.frames()?;
    let unix_millis = job
        .ended
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let replay = replay_of_play(
        frames,
        &PlayScore {
            mods: attempt.mods(),
            statistics: attempt.statistics(),
            maximum_statistics: attempt.maximum_statistics(),
            total_score: attempt.score(),
            total_score_without_mods: attempt.score_without_mods(),
            rank: attempt.rank(),
        },
        &ReplayInfo {
            beatmap_hash: job.beatmap.hash.clone(),
            beatmap_format_version: job.beatmap.format_version,
            player_name: PLAYER_NAME.to_owned(),
            date_ticks: osr::ticks_from_unix_millis(unix_millis),
            client_version: concat!("slam-", env!("CARGO_PKG_VERSION")).to_owned(),
        },
    );
    let name = if job.beatmap.name.is_empty() {
        "replay"
    } else {
        &job.beatmap.name
    };
    Some((format!("{name} ({unix_millis}).osr"), osr::encode(&replay)))
}

/// Saves `job`'s replay into `dir`, logging the outcome.
pub fn save(job: &ReplayJob, dir: &Path) {
    let Some((name, bytes)) = build(job) else {
        tracing::warn!("replay not saved: the recording overflowed");
        return;
    };
    let path = dir.join(name);
    match std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, bytes)) {
        Ok(()) => tracing::info!("replay saved: {}", path.display()),
        Err(e) => tracing::warn!("replay not saved: {}: {e}", path.display()),
    }
}

/// Lowercase hex MD5, as lazer stores beatmap hashes.
pub(crate) fn md5_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(32);
    for byte in Md5::digest(bytes) {
        // Writing to a String does not fail.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_is_lowercase_hex() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(
            md5_hex(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
    }
}
