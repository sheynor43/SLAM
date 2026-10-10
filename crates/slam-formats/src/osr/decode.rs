//! Binary reader for `.osr` files.

use super::error::{OsrError, Warning, WarningKind, WarningSink};
use super::frames::parse_frames;
use super::lzma::decompress;
use super::model::{
    FrameData, MAX_DATE_TICKS, Replay, VERSION_LAZER_BLOCK, VERSION_ONLINE_ID_I32,
    VERSION_ONLINE_ID_I64,
};

/// Result of a successful decode.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// The replay.
    pub replay: Replay,
    /// Non-fatal problems.
    pub warnings: Vec<Warning>,
    /// Number of further warnings dropped after the first [`MAX_WARNINGS`](super::MAX_WARNINGS).
    pub suppressed_warnings: usize,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    warnings: WarningSink,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn take(&mut self, n: usize, field: &'static str) -> Result<&'a [u8], OsrError> {
        if n > self.remaining() {
            return Err(OsrError::UnexpectedEof { field });
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn array<const N: usize>(&mut self, field: &'static str) -> Result<[u8; N], OsrError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N, field)?);
        Ok(out)
    }

    fn u8(&mut self, f: &'static str) -> Result<u8, OsrError> {
        Ok(self.array::<1>(f)?[0])
    }
    fn u16(&mut self, f: &'static str) -> Result<u16, OsrError> {
        Ok(u16::from_le_bytes(self.array(f)?))
    }
    fn i32(&mut self, f: &'static str) -> Result<i32, OsrError> {
        Ok(i32::from_le_bytes(self.array(f)?))
    }
    fn i64(&mut self, f: &'static str) -> Result<i64, OsrError> {
        Ok(i64::from_le_bytes(self.array(f)?))
    }

    /// `BinaryReader.Read7BitEncodedInt` (at most 5 bytes).
    fn len7(&mut self, field: &'static str) -> Result<usize, OsrError> {
        let mut value: u32 = 0;
        for i in 0..5 {
            let b = self.u8(field)?;
            // The fifth byte may only carry the top 4 bits and no continuation.
            if i == 4 && b > 0x0f {
                return Err(OsrError::InvalidStringLength { field });
            }
            value |= u32::from(b & 0x7f) << (7 * i);
            if b & 0x80 == 0 {
                return i32::try_from(value)
                    .map(|v| v as usize)
                    .map_err(|_| OsrError::InvalidStringLength { field });
            }
        }
        Err(OsrError::InvalidStringLength { field })
    }

    /// `SerializationReader.ReadString`: a zero marker is null, anything else is followed by a
    /// .NET `BinaryReader` string.
    fn string(&mut self, field: &'static str) -> Result<Option<String>, OsrError> {
        if self.u8(field)? == 0 {
            return Ok(None);
        }
        let len = self.len7(field)?;
        let bytes = self.take(len, field)?;
        Ok(Some(match std::str::from_utf8(bytes) {
            Ok(s) => s.to_owned(),
            Err(_) => {
                self.warnings
                    .push(WarningKind::InvalidUtf8, || field.to_owned());
                String::from_utf8_lossy(bytes).into_owned()
            }
        }))
    }

    /// `SerializationReader.ReadByteArray`: negative length is null.
    fn byte_array(&mut self, field: &'static str) -> Result<Option<&'a [u8]>, OsrError> {
        let len = self.i32(field)?;
        if len < 0 {
            return Ok(None);
        }
        self.take(len as usize, field).map(Some)
    }
}

/// Decodes a stable (or lazer-written) `.osr` file.
///
/// See the [module documentation](super) for the layout and the tolerance rules.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreDecoder.cs (Parse)
pub fn decode(bytes: &[u8]) -> Result<Decoded, OsrError> {
    let mut r = Reader {
        data: bytes,
        pos: 0,
        warnings: WarningSink::default(),
    };
    let mode = r.u8("mode")?;
    let version = r.i32("version")?;
    let beatmap_hash = r.string("beatmap hash")?;
    let player_name = r.string("player name")?;
    let replay_hash = r.string("replay hash")?;
    let count_300 = r.u16("count 300")?;
    let count_100 = r.u16("count 100")?;
    let count_50 = r.u16("count 50")?;
    let count_geki = r.u16("count geki")?;
    let count_katu = r.u16("count katu")?;
    let count_miss = r.u16("count miss")?;
    let total_score = r.i32("total score")?;
    let max_combo = r.u16("max combo")?;
    let perfect = r.u8("perfect")? != 0;
    let mods = r.i32("mods")? as u32;
    let life_bar = r.string("life bar")?;
    let date_ticks = r.i64("date")?;
    if !(0..=MAX_DATE_TICKS).contains(&date_ticks) {
        return Err(OsrError::InvalidDateTicks(date_ticks));
    }
    let compressed = r.byte_array("replay data")?;
    let online_id = if version >= VERSION_ONLINE_ID_I64 {
        Some(r.i64("online id")?)
    } else if version >= VERSION_ONLINE_ID_I32 {
        Some(i64::from(r.i32("online id")?))
    } else {
        None
    };
    let lazer_block = if version >= VERSION_LAZER_BLOCK {
        r.byte_array("score info")?.map(<[u8]>::to_vec)
    } else {
        None
    };
    let trailing = r.data[r.pos..].to_vec();

    let (frames, seed) = match compressed {
        None => (FrameData::Null, None),
        Some([]) => (FrameData::Empty, None),
        Some(data) => {
            let raw = decompress(data)?;
            let text = match String::from_utf8(raw) {
                Ok(t) => t,
                Err(e) => {
                    r.warnings.push(WarningKind::InvalidFrameText, String::new);
                    String::from_utf8_lossy(e.as_bytes()).into_owned()
                }
            };
            let (frames, seed) = parse_frames(
                text.strip_prefix('\u{feff}').unwrap_or(&text),
                mode,
                &mut r.warnings,
            );
            (FrameData::Frames(frames), seed)
        }
    };

    Ok(Decoded {
        replay: Replay {
            mode,
            version,
            beatmap_hash,
            player_name,
            replay_hash,
            count_300,
            count_100,
            count_50,
            count_geki,
            count_katu,
            count_miss,
            total_score,
            max_combo,
            perfect,
            mods,
            life_bar,
            date_ticks,
            frames,
            seed,
            online_id,
            lazer_block,
            trailing,
        },
        warnings: r.warnings.list,
        suppressed_warnings: r.warnings.suppressed,
    })
}
