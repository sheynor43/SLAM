//! The selected difficulty's length, BPM and object counts on the song select (ADR-0067).
//!
//! The update thread's [`DetailsPicker`] waits for the selection to settle on a difficulty
//! and asks the library thread's [`DetailsReader`] to decode its file; the answer is a
//! `Copy` value. The picker neither blocks nor allocates. The reader also lists the
//! beatmap's best local scores, hashing the bytes it read (ADR-0125).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use slam_osu::Beatmap;
use slam_osu::statistics::BeatmapStatistics;
use slam_ui::song_select::LocalScores;

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
}

/// The answer for the selection: what the song select shows besides the difficulty values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Details {
    pub statistics: Option<BeatmapStatistics>,
    pub scores: Option<LocalScores>,
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

    /// Asks for the settled selection unless it was asked for already.
    fn ask(&mut self) {
        let asked_at = self.asked.as_ref().and_then(Selected::file);
        let Some(seen) = &self.seen else { return };
        if seen.file() == asked_at {
            return;
        }
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
        self.details = Details::default();
        self.asked = self.seen.clone();
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
        let (statistics, scores) = match file_of(&request.files, request.set, request.difficulty)
            .and_then(|path| {
                std::fs::read(path)
                    .inspect_err(|e| tracing::warn!("details {}: {e}", path.display()))
                    .ok()
                    .map(|bytes| (path, bytes))
            }) {
            Some((path, bytes)) => (
                statistics_of(path, &bytes),
                self.scores.as_deref().map(|dir| {
                    let hash = crate::replays::md5_hex(&bytes);
                    crate::scores::rows_of(&crate::scores::load_top(
                        dir,
                        &hash,
                        slam_ui::song_select::MAX_LOCAL_SCORES,
                    ))
                }),
            ),
            None => (None, None),
        };
        let reply = DetailsReply {
            generation: request.generation,
            statistics,
            scores,
        };
        if let Err(rtrb::PushError::Full(reply)) = self.replies.push(reply) {
            self.unsent = Some(reply);
        }
    }
}

/// The statistics of the beatmap file `bytes` read from `path`.
fn statistics_of(path: &Path, bytes: &[u8]) -> Option<BeatmapStatistics> {
    // A file that breaks the decoder must not take the library thread with it: the scan
    // and the backgrounds live there too.
    let statistics = std::panic::catch_unwind(|| {
        Beatmap::decode(bytes).map(|beatmap| BeatmapStatistics::of(&beatmap))
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

#[cfg(test)]
mod tests;
