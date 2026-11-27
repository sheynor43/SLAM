//! The selected difficulty's length, BPM and object counts on the song select (ADR-0067).
//!
//! The update thread's [`DetailsPicker`] waits for the selection to settle on a difficulty
//! and asks the library thread's [`DetailsReader`] to decode its file; the answer is a
//! `Copy` value. The picker neither blocks nor allocates. The reader also lists the
//! beatmap's best local scores, hashing the bytes it read (ADR-0125).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use slam_osu::Beatmap;
use slam_osu::statistics::BeatmapStatistics;
use slam_ui::song_select::LocalScores;
use slam_ui::song_select::beat::{BeatPoint, BeatTiming};

use crate::backgrounds::SELECTION_DEBOUNCE_NS;
use crate::songs::SongFiles;

/// Requests and answers waiting between the update and the library thread.
const RING: usize = 4;

/// What the update thread asks of the library thread: the statistics of
/// `files[set][difficulty]`.
pub struct DetailsRequest {
    pub generation: u32,
    pub files: Arc<SongFiles>,
    pub set: usize,
    pub difficulty: usize,
}

/// The library thread's answer: `None` when the file does not read or decode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailsReply {
    pub generation: u32,
    pub statistics: Option<BeatmapStatistics>,
    /// The best local scores; `None` when the file does not read or no folder is known.
    pub scores: Option<LocalScores>,
    /// The beat from the preview on, for the selection's glow (ADR-0167).
    pub timing: Option<BeatTiming>,
}

/// The answer for the selection: what the song select shows besides the difficulty values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Details {
    pub statistics: Option<BeatmapStatistics>,
    pub scores: Option<LocalScores>,
    pub timing: Option<BeatTiming>,
}

/// Connects the two ends.
pub fn link() -> (DetailsPicker, DetailsReader) {
    let (requests, requests_rx) = rtrb::RingBuffer::new(RING);
    let (replies_tx, replies) = rtrb::RingBuffer::new(RING);
    (
        DetailsPicker {
            requests,
            replies,
            generation: 0,
            seen: None,
            seen_ns: 0,
            asked: None,
            details: Details::default(),
            scores_written: &crate::scores::SCORES_WRITTEN,
            scores_seen: crate::scores::SCORES_WRITTEN.load(Ordering::Acquire),
        },
        DetailsReader {
            requests: requests_rx,
            replies: replies_tx,
            unsent: None,
            scores: None,
        },
    )
}

fn file_of(files: &SongFiles, set: usize, difficulty: usize) -> Option<&Path> {
    Some(&files.get(set)?.get(difficulty)?.path)
}

/// A selection the picker remembers, with the list it indexes.
#[derive(Clone)]
struct Selected {
    files: Arc<SongFiles>,
    at: Option<(usize, usize)>,
}

impl Selected {
    fn file(&self) -> Option<&Path> {
        let (set, difficulty) = self.at?;
        file_of(&self.files, set, difficulty)
    }
}

/// The update thread's end.
pub struct DetailsPicker {
    requests: rtrb::Producer<DetailsRequest>,
    replies: rtrb::Consumer<DetailsReply>,
    /// The newest generation asked for; older answers are dropped.
    generation: u32,
    seen: Option<Selected>,
    seen_ns: u64,
    /// The selection the newest generation is for.
    asked: Option<Selected>,
    /// The answer for `asked`.
    details: Details,
    /// Counts the scores saved ([`crate::scores::SCORES_WRITTEN`]).
    scores_written: &'static AtomicU32,
    /// Its count when `asked` was asked for.
    scores_seen: u32,
}

impl DetailsPicker {
    /// Follows the selection `selected` of `files` at `now_ns`; the statistics and scores of
    /// the selected difficulty once read, else none. Neither blocks nor allocates; clones of
    /// `files` are freed by the library thread, which keeps its lists until only it holds
    /// them.
    pub fn tick(
        &mut self,
        now_ns: u64,
        files: &Arc<SongFiles>,
        selected: Option<(usize, usize)>,
    ) -> Details {
        while let Ok(reply) = self.replies.pop() {
            if reply.generation == self.generation {
                self.details = Details {
                    statistics: reply.statistics,
                    scores: reply.scores,
                    timing: reply.timing,
                };
            }
        }
        let now_at = selected.and_then(|(s, d)| file_of(files, s, d));
        let seen_at = self.seen.as_ref().and_then(Selected::file);
        if self.seen.is_none() || now_at != seen_at {
            self.seen = Some(Selected {
                files: Arc::clone(files),
                at: selected,
            });
            self.seen_ns = now_ns;
        } else if now_ns.saturating_sub(self.seen_ns) >= SELECTION_DEBOUNCE_NS {
            self.ask();
        }
        let asked_at = self.asked.as_ref().and_then(Selected::file);
        if now_at.is_some() && now_at == asked_at {
            self.details
        } else {
            Details::default()
        }
    }

    /// Asks for the settled selection unless it was asked for already and no score was
    /// saved since (a play's new score shows on the way back, ADR-0167).
    fn ask(&mut self) {
        let asked_at = self.asked.as_ref().and_then(Selected::file);
        let Some(seen) = &self.seen else { return };
        let written = self.scores_written.load(Ordering::Acquire);
        if seen.file() == asked_at && written == self.scores_seen {
            return;
        }
        let renew = seen.file() == asked_at;
        let generation = self.generation.wrapping_add(1);
        if let Some((set, difficulty)) = seen.at.filter(|_| seen.file().is_some()) {
            let request = DetailsRequest {
                generation,
                files: Arc::clone(&seen.files),
                set,
                difficulty,
            };
            if self.requests.push(request).is_err() {
                return;
            }
        }
        self.generation = generation;
        // The same difficulty keeps showing its old answer until the new one comes.
        if !renew {
            self.details = Details::default();
        }
        self.asked = self.seen.clone();
        self.scores_seen = written;
    }
}

/// The library thread's end.
pub struct DetailsReader {
    requests: rtrb::Consumer<DetailsRequest>,
    replies: rtrb::Producer<DetailsReply>,
    /// An answer the full ring did not take yet.
    unsent: Option<DetailsReply>,
    /// Where the local scores are; `None` lists none.
    scores: Option<PathBuf>,
}

impl DetailsReader {
    /// A reader that also lists the best local scores found in `dir` (ADR-0125).
    pub fn with_scores(mut self, dir: PathBuf) -> DetailsReader {
        self.scores = Some(dir);
        self
    }

    /// Answers the newest waiting request; older ones are dropped unanswered. Reads and
    /// decodes the file here, so the library thread waits for it.
    pub fn poll(&mut self) {
        if let Some(reply) = self.unsent.take()
            && let Err(rtrb::PushError::Full(reply)) = self.replies.push(reply)
        {
            self.unsent = Some(reply);
            return;
        }
        let mut newest = None;
        while let Ok(request) = self.requests.pop() {
            newest = Some(request);
        }
        let Some(request) = newest else { return };
        let preview_ms = request
            .files
            .get(request.set)
            .and_then(|set| set.get(request.difficulty))
            .map_or(0.0, |file| f64::from(file.preview_ms.max(0)));
        let (statistics, timing, scores) =
            match file_of(&request.files, request.set, request.difficulty).and_then(|path| {
                std::fs::read(path)
                    .inspect_err(|e| tracing::warn!("details {}: {e}", path.display()))
                    .ok()
                    .map(|bytes| (path, bytes))
            }) {
                Some((path, bytes)) => {
                    let (statistics, timing) = statistics_of(path, &bytes, preview_ms).unzip();
                    (
                        statistics,
                        timing,
                        self.scores.as_deref().map(|dir| {
                            let hash = crate::replays::md5_hex(&bytes);
                            crate::scores::rows_of(&crate::scores::load_top(
                                dir,
                                &hash,
                                slam_ui::song_select::MAX_LOCAL_SCORES,
                            ))
                        }),
                    )
                }
                None => (None, None, None),
            };
        let reply = DetailsReply {
            generation: request.generation,
            statistics,
            scores,
            timing,
        };
        if let Err(rtrb::PushError::Full(reply)) = self.replies.push(reply) {
            self.unsent = Some(reply);
        }
    }
}

/// The statistics of the beatmap file `bytes` read from `path`, and its beat from
/// `preview_ms` on.
fn statistics_of(
    path: &Path,
    bytes: &[u8],
    preview_ms: f64,
) -> Option<(BeatmapStatistics, BeatTiming)> {
    // A file that breaks the decoder must not take the library thread with it: the scan
    // and the backgrounds live there too.
    let statistics = std::panic::catch_unwind(|| {
        Beatmap::decode(bytes).map(|beatmap| {
            (
                BeatmapStatistics::of(&beatmap),
                beat_timing(&beatmap, preview_ms),
            )
        })
    });
    match statistics {
        Ok(Ok(statistics)) => Some(statistics),
        Ok(Err(e)) => {
            tracing::warn!("details {}: {e}", path.display());
            None
        }
        Err(_) => {
            tracing::warn!("details {}: the decoder panicked", path.display());
            None
        }
    }
}

/// The red lines and kiai sections of `beatmap` that matter from `from_ms` on, as many as
/// [`BeatTiming`] keeps.
fn beat_timing(beatmap: &Beatmap, from_ms: f64) -> BeatTiming {
    let mut timing = BeatTiming::default();
    let points = beatmap.control_points.timing_points();
    // All of them when they fit, so that music started before the preview (the menu's
    // track) keeps its beat; else from the one in force at the preview.
    let first = if points.len() <= slam_ui::song_select::beat::MAX_BEAT_POINTS {
        0
    } else {
        points.iter().rposition(|p| p.time <= from_ms).unwrap_or(0)
    };
    for p in &points[first..] {
        timing.push_point(BeatPoint {
            time: p.time,
            beat_length: p.beat_length,
            meter: u32::try_from(p.time_signature).unwrap_or(4),
        });
    }
    let effects = beatmap.control_points.effect_points();
    for (i, e) in effects.iter().enumerate().filter(|(_, e)| e.kiai) {
        let end = effects
            .get(i + 1..)
            .and_then(|rest| rest.iter().find(|n| !n.kiai))
            .map_or(f64::INFINITY, |n| n.time);
        if end > from_ms {
            timing.push_kiai(e.time, end);
        }
    }
    timing
}

#[cfg(test)]
mod tests;
