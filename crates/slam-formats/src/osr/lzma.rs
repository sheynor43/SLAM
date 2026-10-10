//! "LZMA alone" container used for the frame data: 5 property bytes (`lc`/`lp`/`pb` and the
//! dictionary size), an `i64` uncompressed size, then the raw LZMA stream.

use std::io::Read;

use lzma_rust2::{EncodeMode, LzmaOptions, LzmaReader, LzmaWriter, MfType};

use super::error::OsrError;

/// Upper bound for decompressed frame data; larger streams are rejected.
///
/// Real replays are a few hundred KiB at most, so this only guards against decompression bombs.
pub const MAX_DECOMPRESSED_SIZE: usize = 64 * 1024 * 1024;

const HEADER_LEN: usize = 13;

/// Decompresses a `.lzma` blob (also the format of the lazer score-info block) (declared size known, or -1 with an end marker).
pub(crate) fn decompress(data: &[u8]) -> Result<Vec<u8>, OsrError> {
    if data.len() < HEADER_LEN {
        return Err(OsrError::CompressedTooShort);
    }
    let props = data[0];
    let dict_size = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
    let mut size_bytes = [0u8; 8];
    size_bytes.copy_from_slice(&data[5..13]);
    let declared = i64::from_le_bytes(size_bytes);

    let (uncompressed, dict_size) = if declared < 0 {
        // Unknown size: the stream must carry an end marker. The dictionary cannot be shrunk
        // to the data size, so bound it by the output cap.
        (u64::MAX, dict_size.min(MAX_DECOMPRESSED_SIZE as u32))
    } else {
        let size = declared as u64;
        if size > MAX_DECOMPRESSED_SIZE as u64 {
            return Err(OsrError::DecompressedTooLarge {
                limit: MAX_DECOMPRESSED_SIZE,
            });
        }
        (size, dict_size)
    };

    let mut reader =
        LzmaReader::new_with_props(&data[HEADER_LEN..], uncompressed, props, dict_size, None)
            .map_err(|e| OsrError::Lzma(e.to_string()))?;
    let mut out = Vec::new();
    let read = (&mut reader)
        .take(MAX_DECOMPRESSED_SIZE as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| OsrError::Lzma(e.to_string()))?;
    if read > MAX_DECOMPRESSED_SIZE {
        return Err(OsrError::DecompressedTooLarge {
            limit: MAX_DECOMPRESSED_SIZE,
        });
    }
    Ok(out)
}

/// Compresses like lazer: lc=3, lp=0, pb=2, 2 MiB dictionary, 255 fast bytes, known size and
/// no end marker.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreEncoder.cs
pub(crate) fn compress(content: &[u8]) -> Vec<u8> {
    let options = LzmaOptions::new(1 << 21, 3, 0, 2, EncodeMode::Normal, 255, MfType::Bt4, 0);
    let mut writer = LzmaWriter::new_use_header(Vec::new(), &options, Some(content.len() as u64))
        .expect("LZMA options are valid");
    std::io::Write::write_all(&mut writer, content).expect("writing to a Vec cannot fail");
    writer.finish().expect("writing to a Vec cannot fail")
}
