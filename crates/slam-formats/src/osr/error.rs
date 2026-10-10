//! Errors and warnings of the `.osr` codec.

use thiserror::Error;

/// A fatal problem while reading or decompressing a replay.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OsrError {
    /// The input ended inside the named field.
    #[error("unexpected end of data while reading {field}")]
    UnexpectedEof {
        /// Name of the field being read.
        field: &'static str,
    },
    /// A string length prefix is malformed or negative.
    #[error("invalid string length in {field}")]
    InvalidStringLength {
        /// Name of the field being read.
        field: &'static str,
    },
    /// The date is not a valid .NET `DateTime` tick count.
    #[error("invalid date ticks {0}")]
    InvalidDateTicks(i64),
    /// The compressed frame data is shorter than its 13-byte header.
    #[error("compressed replay data is too short")]
    CompressedTooShort,
    /// The LZMA stream is corrupt or truncated.
    #[error("corrupt LZMA data: {0}")]
    Lzma(String),
    /// The decompressed data would exceed [`MAX_DECOMPRESSED_SIZE`](super::MAX_DECOMPRESSED_SIZE).
    #[error("decompressed replay data exceeds the {limit} byte limit")]
    DecompressedTooLarge {
        /// The limit that was exceeded.
        limit: usize,
    },
    /// The score-info block is not valid JSON (or not valid UTF-8, or not an object).
    #[error("invalid score-info JSON: {0}")]
    InvalidScoreInfoJson(String),
    /// A field of the score-info block has the wrong type or an out-of-range value.
    #[error("invalid score-info field {field}: expected {expected}")]
    InvalidScoreInfoField {
        /// Path of the field, e.g. `mods[0].acronym`.
        field: String,
        /// What was expected there.
        expected: &'static str,
    },
}

/// Kind of a non-fatal problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// A string was not valid UTF-8 and was decoded lossily.
    InvalidUtf8,
    /// The frame text was not valid UTF-8 and was decoded lossily.
    InvalidFrameText,
    /// A non-empty frame entry had fewer than four fields and was skipped.
    ShortFrame,
    /// A frame entry could not be parsed and was dropped.
    MalformedFrame,
    /// The seed frame value could not be parsed; the seed is `None`.
    InvalidSeed,
    /// More than one seed frame was present; the last one wins.
    MultipleSeeds,
}

/// A non-fatal problem found while decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// What went wrong.
    pub kind: WarningKind,
    /// The field name or the offending frame entry (truncated).
    pub context: String,
}

/// Maximum number of warnings stored; further ones are only counted.
pub const MAX_WARNINGS: usize = 100;

/// Collects warnings up to [`MAX_WARNINGS`] and counts the rest.
#[derive(Debug, Default)]
pub(super) struct WarningSink {
    pub(super) list: Vec<Warning>,
    pub(super) suppressed: usize,
}

impl WarningSink {
    pub(super) fn push(&mut self, kind: WarningKind, context: impl FnOnce() -> String) {
        if self.list.len() < MAX_WARNINGS {
            self.list.push(Warning {
                kind,
                context: context(),
            });
        } else {
            self.suppressed += 1;
        }
    }
}
