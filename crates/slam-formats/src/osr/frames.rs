//! Frame text parsing/formatting and lazer's frame timeline processing.

use std::fmt::Write;

use crate::osu::parsing::{self, MAX_COORDINATE_VALUE};

use super::error::{WarningKind, WarningSink};
use super::model::ReplayFrame;

/// Marker in the first field of the seed frame.
const SEED_MARKER: &str = "-12345";
/// Mania stores the pressed keys in the low 20 bits of x.
const MANIA_X_LIMIT: f32 = ((1 << 20) - 1) as f32;
use crate::osu::EARLY_VERSION_TIMING_OFFSET;
const EXCERPT_LIMIT: usize = 50;

fn excerpt(s: &str) -> String {
    let mut out: String = s.chars().take(EXCERPT_LIMIT).collect();
    if s.chars().count() > EXCERPT_LIMIT {
        out.push('…');
    }
    out
}

/// Parses one frame entry (already split off at `,`), `None` if malformed.
fn parse_frame(split: &[&str; 4], mode: u8) -> Option<ReplayFrame> {
    let x_limit = if mode == 3 {
        MANIA_X_LIMIT
    } else {
        MAX_COORDINATE_VALUE as f32
    };
    // Integer first; fractional deltas from old lazer builds are rounded half to even.
    // The `as i32` cast saturates (e.g. "2147483648" gives i32::MAX), like the .NET 9+ conversion
    // lazer runs on. NaN is already rejected by the float parser.
    let delta = match parsing::net_parse_i32(split[0]) {
        Ok(d) => d,
        Err(_) => (f64::from(parsing::float(split[0]).ok()?)).round_ties_even() as i32,
    };
    let x = parsing::parse_float(split[1], x_limit, false).ok()?;
    let y = parsing::parse_float(split[2], MAX_COORDINATE_VALUE as f32, false).ok()?;
    let buttons = parsing::int(split[3]).ok()?;
    Some(ReplayFrame {
        delta,
        x,
        y,
        buttons,
    })
}

/// Parses the decompressed frame text into frames (file order) and the seed.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreDecoder.cs (readLegacyReplay)
pub(super) fn parse_frames(
    text: &str,
    mode: u8,
    warnings: &mut WarningSink,
) -> (Vec<ReplayFrame>, Option<i32>) {
    let mut frames = Vec::new();
    let mut seed = None;
    let mut seed_frames = 0usize;
    for entry in text.split(',') {
        let mut it = entry.split('|');
        let (Some(a), Some(b), Some(c), Some(d)) = (it.next(), it.next(), it.next(), it.next())
        else {
            if !entry.is_empty() {
                warnings.push(WarningKind::ShortFrame, || excerpt(entry));
            }
            continue;
        };
        let split = [a, b, c, d];
        if a == SEED_MARKER {
            seed_frames += 1;
            if seed_frames > 1 {
                warnings.push(WarningKind::MultipleSeeds, || excerpt(entry));
            }
            seed = parsing::net_parse_i32(d).ok();
            if seed.is_none() {
                warnings.push(WarningKind::InvalidSeed, || excerpt(entry));
            }
            continue;
        }
        match parse_frame(&split, mode) {
            Some(f) => frames.push(f),
            None => warnings.push(WarningKind::MalformedFrame, || excerpt(entry)),
        }
    }
    (frames, seed)
}

/// Formats frames (and the optional seed frame) as stable frame text.
///
/// Floats use Rust's shortest round-trip `Display` (`256`, `0.00001`); .NET would print
/// `1E-05` for tiny values. Both parse back to the same number.
pub(super) fn format_frames(frames: &[ReplayFrame], seed: Option<i32>) -> String {
    let mut out = String::new();
    for f in frames {
        let _ = write!(out, "{}|{}|{}|{},", f.delta, f.x, f.y, f.buttons);
    }
    if let Some(s) = seed {
        let _ = write!(out, "{SEED_MARKER}|0|0|{s}");
    }
    out
}

/// A frame with an absolute time, as produced by [`lazer_timeline`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimedFrame {
    /// Absolute time in milliseconds.
    pub time: i64,
    /// Cursor x.
    pub x: f32,
    /// Cursor y.
    pub y: f32,
    /// Raw button bits.
    pub buttons: i32,
}

/// A frame with a fractional absolute time, input of [`frames_from_timeline`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeneratedFrame {
    /// Absolute time in milliseconds (rounded half to even when encoded).
    pub time: f64,
    /// Cursor x.
    pub x: f32,
    /// Cursor y.
    pub y: f32,
    /// Raw button bits.
    pub buttons: i32,
}

impl From<TimedFrame> for GeneratedFrame {
    fn from(f: TimedFrame) -> Self {
        Self {
            time: f.time as f64,
            x: f.x,
            y: f.y,
            buttons: f.buttons,
        }
    }
}

fn is_stable_dummy(f: &TimedFrame) -> bool {
    f.x == 256.0 && f.y == -500.0
}

/// Builds the absolute timeline from stored frames, exactly like osu!lazer's decoder.
///
/// Time starts at 24 for beatmap format versions below 5. The two stable fix-ups for the first
/// three frames are applied, the dummy `(256, -500)` frames stable places at the start are
/// removed, and frames that go back in time relative to the last kept frame are dropped.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreDecoder.cs (readLegacyReplay)
pub fn lazer_timeline(frames: &[ReplayFrame], beatmap_format_version: i32) -> Vec<TimedFrame> {
    let mut last_time: i64 = if beatmap_format_version < 5 {
        i64::from(EARLY_VERSION_TIMING_OFFSET)
    } else {
        0
    };
    let mut legacy: Vec<TimedFrame> = frames
        .iter()
        .map(|f| {
            last_time = last_time.wrapping_add(i64::from(f.delta));
            TimedFrame {
                time: last_time,
                x: f.x,
                y: f.y,
                buttons: f.buttons,
            }
        })
        .collect();

    if legacy.len() >= 2 && legacy[1].time < legacy[0].time {
        legacy[1].time = legacy[0].time;
        legacy[0].time = 0;
    }
    if legacy.len() >= 3 && legacy[0].time > legacy[2].time {
        let t = legacy[2].time;
        legacy[0].time = t;
        legacy[1].time = t;
    }
    if legacy.len() >= 2 && is_stable_dummy(&legacy[1]) {
        legacy.remove(1);
    }
    if !legacy.is_empty() && is_stable_dummy(&legacy[0]) {
        legacy.remove(0);
    }

    let mut out: Vec<TimedFrame> = Vec::with_capacity(legacy.len());
    for f in legacy {
        if out.last().is_some_and(|cur| f.time < cur.time) {
            continue;
        }
        out.push(f);
    }
    out
}

/// Converts absolute-time frames to stored delta frames, like lazer's encoder.
///
/// For beatmap format versions below 5 the times are shifted by -24 first. Each time is
/// rounded half to even to an `i32` (saturating, as .NET 9+), and the delta is the difference
/// to the previous rounded time (wrapping). Lazer takes `double` times here, hence
/// [`GeneratedFrame`].
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreEncoder.cs (replayStringContent)
pub fn frames_from_timeline(
    timeline: &[GeneratedFrame],
    beatmap_format_version: i32,
) -> Vec<ReplayFrame> {
    let offset = if beatmap_format_version < 5 {
        -f64::from(EARLY_VERSION_TIMING_OFFSET)
    } else {
        0.0
    };
    let mut last_time: i32 = 0;
    timeline
        .iter()
        .map(|f| {
            let time = (f.time + offset).round_ties_even() as i32;
            let frame = ReplayFrame {
                delta: time.wrapping_sub(last_time),
                x: f.x,
                y: f.y,
                buttons: f.buttons,
            };
            last_time = time;
            frame
        })
        .collect()
}
