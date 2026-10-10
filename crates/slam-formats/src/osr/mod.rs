//! Reader and writer for stable `.osr` replay files.
//!
//! Layout (little-endian): mode `u8`, version `i32`, three strings (beatmap MD5, player,
//! replay MD5), six hit counts `u16`, total score `i32`, max combo `u16`, perfect flag,
//! legacy mods `i32`, life bar string, date as .NET ticks `i64`, the LZMA-compressed frame
//! text as a byte array, the online score id (`i64` from version 20140721, `i32` from
//! 20121008), and for lazer versions (>= 30000001) a byte array with the score-info block.
//! Strings are a marker byte (0 is null) followed by a .NET 7-bit length prefix and UTF-8.
//!
//! The lazer block is kept raw in [`Replay::lazer_block`] (still compressed) and stays the
//! source of truth; [`Replay::score_info`] parses it into a [`ScoreInfo`] and
//! [`ScoreInfo::to_block`] builds one. Any bytes after the known fields are kept in
//! [`Replay::trailing`]. Decode followed by encode is lossless (except for
//! the exact LZMA byte stream and the string marker byte, always written as `0x0b`) only when
//! decoding produced no warnings. Otherwise this is lost: dropped malformed or short frames,
//! invalid or extra seeds, the seed's position (always written last), invalid UTF-8 (replaced
//! by U+FFFD), and a `perfect` byte other than 0/1 (normalised to 1). A seed is also dropped
//! when the frames are [`FrameData::Null`] or [`FrameData::Empty`].
//!
//! The frame text is `delta|x|y|buttons,` per frame, with an optional `-12345|0|0|seed`
//! entry; frames are stored as in the file ([`ReplayFrame`]). [`lazer_timeline`] applies
//! osu!lazer's time processing on top.
//!
//! Deviations from osu!lazer, which fails the whole import when a frame entry cannot be
//! parsed: such an entry is dropped with a [`Warning`] (its delta is lost, so later frames
//! shift in time). A frame entry with fewer than four fields is skipped silently if empty and
//! with a warning otherwise. The seed is kept (lazer ignores it). Invalid UTF-8 is replaced
//! by U+FFFD as in .NET, but with a warning. A leading UTF-8 BOM in the frame text is
//! stripped. Truncated input, a malformed length, invalid date ticks or a corrupt LZMA stream
//! are errors. Decompressed data is capped at [`MAX_DECOMPRESSED_SIZE`], lengths are checked
//! against the input before allocating, and at most [`MAX_WARNINGS`] warnings are stored
//! (the rest is counted in [`Decoded::suppressed_warnings`]).
//!
//! ```
//! use slam_formats::osr::{decode, encode, FrameData, Replay, ReplayFrame};
//!
//! let replay = Replay {
//!     mode: 0,
//!     version: 20151228,
//!     player_name: Some("player".into()),
//!     frames: FrameData::Frames(vec![ReplayFrame { delta: 16, x: 256.0, y: 192.0, buttons: 1 }]),
//!     ..Replay::default()
//! };
//! let bytes = encode(&replay);
//! let decoded = decode(&bytes).unwrap();
//! assert_eq!(decoded.replay, Replay { online_id: Some(0), ..replay });
//! assert!(decoded.warnings.is_empty());
//! ```

mod decode;
mod encode;
mod error;
mod frames;
mod lzma;
mod model;
mod score_info;

pub use decode::{Decoded, decode};
pub use encode::encode;
pub use error::{MAX_WARNINGS, OsrError, Warning, WarningKind};
pub use frames::{GeneratedFrame, TimedFrame, frames_from_timeline, lazer_timeline};
pub use lzma::MAX_DECOMPRESSED_SIZE;
pub use model::{
    ButtonState, FrameData, LifeBarPoint, Replay, ReplayFrame, VERSION_LAZER_BLOCK,
    VERSION_ONLINE_ID_I32, VERSION_ONLINE_ID_I64, ticks_from_unix_millis, unix_millis_from_ticks,
};
pub use score_info::{ApiMod, HitResult, RawJson, ScoreInfo, ScoreRank, SettingValue};
