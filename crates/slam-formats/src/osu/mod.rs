//! Decoder for the `.osu` beatmap format: header sections, timing points and hit objects.
//!
//! The decoder returns the file's data as osu!lazer's `LegacyBeatmapDecoder` reads it, with
//! the same number parsing rules, defaults and per-line error tolerance. It deliberately does
//! no model-level post-processing. These steps lazer applies after parsing are not done here:
//! timing points are not merged, difficulty values are not clamped, hit objects are not sorted
//! (file order is kept), sample banks and volumes are not resolved from the sample control
//! points (`applySamples`), `postProcessBreaks` is not run (it forces a new combo on the first
//! object after each break), `ApplyDefaults` is not run, and slider paths are neither
//! evaluated nor measured, so the zero-length-slider rule of `createSlider` (repeats reset to
//! 0, node samples trimmed to first and last) is not applied either.
//! The rest of `ConvertHitObjectParser` is ported: combo flags, timing offset, curve
//! segmentation, repeats, node samples and `hitSample`.
//!
//! Malformed lines never abort decoding: the line is dropped and a [`Warning`] is recorded.
//!
//! ```
//! let text = "osu file format v14\n\n[Metadata]\nTitle:Song\n\n[TimingPoints]\n1000,500,4,2,0,70,1,0\n";
//! let decoded = slam_formats::osu::decode_str(text).unwrap();
//! assert_eq!(decoded.beatmap.metadata.title, "Song");
//! assert_eq!(decoded.beatmap.timing_points[0].beat_length, 500.0);
//! assert!(decoded.warnings.is_empty());
//! ```

mod beatmap;
mod decode;
mod hit_object;
mod hit_object_parser;
pub mod parsing;

pub use beatmap::{
    Beatmap, Break, Colours, Difficulty, Events, General, Metadata, Rgb, TimingPoint,
};
pub use decode::{
    DecodeError, DecodeOptions, Decoded, EARLY_VERSION_TIMING_OFFSET, Warning, WarningKind, decode,
    decode_str, decode_str_with, decode_with,
};
pub use hit_object::{
    HitObject, HitObjectKind, HitSample, HitSampleName, PathControlPoint, PathType, SampleBank,
    Slider, Vec2,
};
