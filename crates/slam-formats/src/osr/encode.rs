//! Binary writer for `.osr` files.

use super::frames::format_frames;
use super::lzma::compress;
use super::model::{
    FrameData, Replay, VERSION_LAZER_BLOCK, VERSION_ONLINE_ID_I32, VERSION_ONLINE_ID_I64,
};

fn write_7bit(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push((v & 0x7f) as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// `SerializationWriter.Write(string)`: null is a zero byte, otherwise `0x0b` and a
/// .NET-style length-prefixed UTF-8 string.
fn write_string(out: &mut Vec<u8>, s: Option<&str>) {
    match s {
        None => out.push(0),
        Some(s) => {
            out.push(0x0b);
            // Lengths of real data are far below i32::MAX; the cast cannot truncate.
            debug_assert!(i32::try_from(s.len()).is_ok());
            write_7bit(out, s.len() as u32);
            out.extend_from_slice(s.as_bytes());
        }
    }
}

fn write_byte_array(out: &mut Vec<u8>, data: Option<&[u8]>) {
    match data {
        None => out.extend_from_slice(&(-1i32).to_le_bytes()),
        Some(d) => {
            debug_assert!(i32::try_from(d.len()).is_ok());
            out.extend_from_slice(&(d.len() as i32).to_le_bytes());
            out.extend_from_slice(d);
        }
    }
}

/// Encodes a replay as an `.osr` file.
///
/// * Strings are written with marker `0x0b` (decoding accepts any non-zero marker).
/// * The online id is written as `i64` (version >= 20140721), `i32` (>= 20121008, truncating)
///   or not at all; a missing id is written as 0 when the version requires one.
/// * The lazer block is written only for versions >= 30000001 (`None` as a null array);
///   for older versions it is ignored. `trailing` follows.
/// * Coordinates are written as-is: non-finite values or values beyond the decoder limits
///   (131072, mania 2^20-1) give files lazer rejects; that is the caller's responsibility.
/// * Lazer always writes `-12345|0|0|0`; use `seed: Some(0)` to reproduce that. The seed
///   frame is always written last, whatever its position was in a decoded file.
/// * The frames are LZMA-compressed like lazer does it; the seed frame is written only when
///   `seed` is `Some`. [`FrameData::Null`] and [`FrameData::Empty`] are written as a null and
///   an empty array, and then `seed` has nowhere to go and is dropped.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreEncoder.cs (Encode)
pub fn encode(replay: &Replay) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(replay.mode);
    out.extend_from_slice(&replay.version.to_le_bytes());
    write_string(&mut out, replay.beatmap_hash.as_deref());
    write_string(&mut out, replay.player_name.as_deref());
    write_string(&mut out, replay.replay_hash.as_deref());
    for c in [
        replay.count_300,
        replay.count_100,
        replay.count_50,
        replay.count_geki,
        replay.count_katu,
        replay.count_miss,
    ] {
        out.extend_from_slice(&c.to_le_bytes());
    }
    out.extend_from_slice(&replay.total_score.to_le_bytes());
    out.extend_from_slice(&replay.max_combo.to_le_bytes());
    out.push(u8::from(replay.perfect));
    out.extend_from_slice(&replay.mods.to_le_bytes());
    write_string(&mut out, replay.life_bar.as_deref());
    out.extend_from_slice(&replay.date_ticks.to_le_bytes());

    match &replay.frames {
        FrameData::Null => write_byte_array(&mut out, None),
        FrameData::Empty => write_byte_array(&mut out, Some(&[])),
        FrameData::Frames(frames) => {
            let text = format_frames(frames, replay.seed);
            write_byte_array(&mut out, Some(&compress(text.as_bytes())));
        }
    }

    if replay.version >= VERSION_ONLINE_ID_I64 {
        out.extend_from_slice(&replay.online_id.unwrap_or(0).to_le_bytes());
    } else if replay.version >= VERSION_ONLINE_ID_I32 {
        out.extend_from_slice(&(replay.online_id.unwrap_or(0) as i32).to_le_bytes());
    }
    if replay.version >= VERSION_LAZER_BLOCK {
        write_byte_array(&mut out, replay.lazer_block.as_deref());
    }
    out.extend_from_slice(&replay.trailing);
    out
}
