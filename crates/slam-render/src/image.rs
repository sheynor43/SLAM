//! Image decoding into `Rgba8` pixels for textures (ADR-0034). Runs on any thread;
//! the draw thread only uploads the result.

use std::io::Cursor;

use crate::hal::Extent;

/// Decoded pixels: tightly packed `Rgba8` rows, top row first, straight alpha.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub size: Extent,
    pub pixels: Vec<u8>,
}

/// Bounds checked before pixel memory is allocated, so a hostile file cannot take
/// unbounded memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageLimits {
    /// Longest side in pixels, normally the device's texture size limit.
    pub max_side: u32,
    /// Bytes of one buffer of decoded pixels, checked against the RGBA size and by the
    /// decoder for each of its own buffers; a decode may briefly hold two such
    /// buffers (the decoder's output and its RGBA conversion).
    pub max_bytes: usize,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_side: 16384,
            max_bytes: 64 << 20,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("image {width}x{height} exceeds the limits")]
    TooLarge { width: u32, height: u32 },
    #[error("decoded image does not match the PNG header")]
    FrameMismatch,
    #[error("invalid PNG: {0}")]
    Png(#[source] png::DecodingError),
}

/// Decodes a PNG of any colour type and bit depth into `Rgba8`: palettes and `tRNS`
/// are expanded, 16-bit channels keep their high byte, grey and RGB get opaque
/// alpha. Gamma and colour profiles are ignored; of an APNG, the default image is
/// decoded.
pub fn decode_png(bytes: &[u8], limits: ImageLimits) -> Result<Image, ImageError> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: limits.max_bytes,
        },
    );
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let header = decoder.read_header_info().map_err(ImageError::Png)?;
    let (width, height) = (header.width, header.height);
    let too_large = || ImageError::TooLarge { width, height };
    let rgba_len = u64::from(width) * u64::from(height) * 4;
    if width > limits.max_side || height > limits.max_side || rgba_len > limits.max_bytes as u64 {
        return Err(too_large());
    }
    let mut reader = decoder.read_info().map_err(|e| match e {
        png::DecodingError::LimitsExceeded => too_large(),
        e => ImageError::Png(e),
    })?;
    let len = reader.output_buffer_size().ok_or_else(too_large)?;
    let mut buf = vec![0; len];
    let frame = reader.next_frame(&mut buf).map_err(|e| match e {
        png::DecodingError::LimitsExceeded => too_large(),
        e => ImageError::Png(e),
    })?;
    if (frame.width, frame.height) != (width, height) {
        // `png` checks the default image against the header; kept as a guard for the
        // row slicing below.
        return Err(ImageError::FrameMismatch);
    }
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        // `EXPAND` turns palettes into RGB(A).
        png::ColorType::Indexed => unreachable!("palettes are expanded"),
    };
    debug_assert_eq!(frame.bit_depth, png::BitDepth::Eight);
    let (w, h) = (width as usize, height as usize);
    let rows = buf.chunks(frame.line_size).take(h);
    let pixels = if channels == 4 && frame.line_size == w * 4 {
        buf.truncate(w * h * 4);
        buf
    } else {
        let mut out = Vec::with_capacity(w * h * 4);
        for row in rows {
            for px in row[..w * channels].chunks_exact(channels) {
                out.extend_from_slice(&match *px {
                    [l] => [l, l, l, 255],
                    [l, a] => [l, l, l, a],
                    [r, g, b] => [r, g, b, 255],
                    [r, g, b, a] => [r, g, b, a],
                    _ => unreachable!("1 to 4 channels"),
                });
            }
        }
        out
    };
    debug_assert_eq!(pixels.len(), w * h * 4);
    Ok(Image {
        size: Extent::new(width, height),
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Png {
        size: (u32, u32),
        color: png::ColorType,
        depth: png::BitDepth,
        palette: Option<Vec<u8>>,
        trns: Option<Vec<u8>>,
    }

    impl Png {
        fn new(width: u32, height: u32, color: png::ColorType) -> Self {
            Self {
                size: (width, height),
                color,
                depth: png::BitDepth::Eight,
                palette: None,
                trns: None,
            }
        }

        fn encode(&self, data: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            let mut e = png::Encoder::new(&mut out, self.size.0, self.size.1);
            e.set_color(self.color);
            e.set_depth(self.depth);
            if let Some(p) = &self.palette {
                e.set_palette(p.clone());
            }
            if let Some(t) = &self.trns {
                e.set_trns(t.clone());
            }
            let mut w = e.write_header().unwrap();
            w.write_image_data(data).unwrap();
            w.finish().unwrap();
            out
        }
    }

    fn decode(bytes: &[u8]) -> Image {
        decode_png(bytes, ImageLimits::default()).unwrap()
    }

    #[test]
    fn rgba_is_kept_as_is() {
        let data: Vec<u8> = (0..2 * 2 * 4).map(|i| i as u8 * 7).collect();
        let img = decode(&Png::new(2, 2, png::ColorType::Rgba).encode(&data));
        assert_eq!(img.size, Extent::new(2, 2));
        assert_eq!(img.pixels, data);
    }

    #[test]
    fn rgb_gets_opaque_alpha() {
        let img = decode(&Png::new(2, 1, png::ColorType::Rgb).encode(&[1, 2, 3, 4, 5, 6]));
        assert_eq!(img.pixels, [1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn grey_and_grey_alpha_spread_to_rgb() {
        let img = decode(&Png::new(2, 1, png::ColorType::Grayscale).encode(&[10, 200]));
        assert_eq!(img.pixels, [10, 10, 10, 255, 200, 200, 200, 255]);
        let img = decode(&Png::new(2, 1, png::ColorType::GrayscaleAlpha).encode(&[10, 0, 200, 99]));
        assert_eq!(img.pixels, [10, 10, 10, 0, 200, 200, 200, 99]);
    }

    #[test]
    fn low_bit_grey_is_scaled_to_eight_bits() {
        let mut p = Png::new(4, 1, png::ColorType::Grayscale);
        p.depth = png::BitDepth::Two;
        // Levels 0, 1, 2, 3 packed into one byte.
        let img = decode(&p.encode(&[0b00_01_10_11]));
        let grey: Vec<u8> = img.pixels.chunks(4).map(|px| px[0]).collect();
        assert_eq!(grey, [0, 85, 170, 255]);
    }

    #[test]
    fn palette_with_transparency_expands_to_rgba() {
        let mut p = Png::new(3, 1, png::ColorType::Indexed);
        p.palette = Some(vec![255, 0, 0, 0, 255, 0, 0, 0, 255]);
        // Alpha for the first two entries; the rest are opaque.
        p.trns = Some(vec![0, 128]);
        let img = decode(&p.encode(&[0, 1, 2]));
        assert_eq!(img.pixels, [255, 0, 0, 0, 0, 255, 0, 128, 0, 0, 255, 255]);
    }

    #[test]
    fn palette_without_transparency_is_opaque() {
        let mut p = Png::new(2, 1, png::ColorType::Indexed);
        p.depth = png::BitDepth::Four;
        p.palette = Some(vec![1, 2, 3, 4, 5, 6]);
        let img = decode(&p.encode(&[0x10]));
        assert_eq!(img.pixels, [4, 5, 6, 255, 1, 2, 3, 255]);
    }

    #[test]
    fn rgb_colour_key_becomes_transparent() {
        let mut p = Png::new(2, 1, png::ColorType::Rgb);
        p.trns = Some(vec![0, 1, 0, 2, 0, 3]);
        let img = decode(&p.encode(&[1, 2, 3, 4, 5, 6]));
        assert_eq!(img.pixels, [1, 2, 3, 0, 4, 5, 6, 255]);
    }

    #[test]
    fn sixteen_bits_keep_the_high_byte() {
        let mut p = Png::new(1, 1, png::ColorType::Rgba);
        p.depth = png::BitDepth::Sixteen;
        let img = decode(&p.encode(&[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0]));
        assert_eq!(img.pixels, [0x12, 0x56, 0x9a, 0xde]);
    }

    #[test]
    fn interlaced_images_decode_in_full() {
        // Adam7, made with ImageMagick from bytes `i * 5` (5x3 RGB): the `png`
        // encoder cannot interlace.
        let img = decode(include_bytes!("../tests/data/interlaced-5x3.png"));
        assert_eq!(img.size, Extent::new(5, 3));
        let expected: Vec<u8> = (0..5 * 3 * 3u32)
            .map(|i| (i * 5) as u8)
            .collect::<Vec<_>>()
            .chunks(3)
            .flat_map(|px| [px[0], px[1], px[2], 255])
            .collect();
        assert_eq!(img.pixels, expected);
    }

    #[test]
    fn sizes_over_the_limits_are_rejected_before_decoding() {
        let bytes = Png::new(64, 8, png::ColorType::Rgba).encode(&[0; 64 * 8 * 4]);
        let limits = ImageLimits {
            max_side: 63,
            ..ImageLimits::default()
        };
        assert!(matches!(
            decode_png(&bytes, limits),
            Err(ImageError::TooLarge {
                width: 64,
                height: 8
            })
        ));
        let limits = ImageLimits {
            max_bytes: 64 * 8 * 4 - 1,
            ..ImageLimits::default()
        };
        assert!(matches!(
            decode_png(&bytes, limits),
            Err(ImageError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_huge_header_is_rejected_without_allocating_its_pixels() {
        // A valid 1x1 image whose header claims 16384x16384 (1 GiB of RGBA).
        let mut bytes = Png::new(1, 1, png::ColorType::Rgba).encode(&[0; 4]);
        // IHDR data starts after the signature (8), length (4) and type (4).
        bytes[16..24].copy_from_slice(&[0, 0, 0x40, 0, 0, 0, 0x40, 0]);
        let crc = crc32(&bytes[12..29]);
        bytes[29..33].copy_from_slice(&crc.to_be_bytes());
        let limits = ImageLimits {
            max_side: 16384,
            max_bytes: 64 << 20,
        };
        let (result, stats) = slam_testkit::count_allocs(|| decode_png(&bytes, limits));
        assert!(matches!(result, Err(ImageError::TooLarge { .. })));
        assert!(stats.allocs < 16, "{stats}");
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
            }
        }
        !crc
    }

    #[test]
    fn broken_files_are_errors_not_panics() {
        let data: Vec<u8> = (0..8 * 8 * 4).map(|i| (i * 31) as u8).collect();
        let good = Png::new(8, 8, png::ColorType::Rgba).encode(&data);
        assert!(decode_png(&[], ImageLimits::default()).is_err());
        assert!(decode_png(b"not a png at all", ImageLimits::default()).is_err());
        // Cut before the end of the pixel data: an error. Cut later (only the `IEND`
        // chunk lost): the whole image, as stb_image does.
        for len in 0..good.len() {
            if let Ok(img) = decode_png(&good[..len], ImageLimits::default()) {
                assert!(len >= good.len() - 12, "truncated to {len}");
                assert_eq!(img.pixels, data);
            }
        }
        // Every byte corrupted in turn: any result but a panic, and a decoded image is
        // whole.
        for i in 0..good.len() {
            for flip in [0x01, 0x80, 0xff] {
                let mut bad = good.clone();
                bad[i] ^= flip;
                if let Ok(img) = decode_png(&bad, ImageLimits::default()) {
                    let Extent { width, height } = img.size;
                    assert_eq!(img.pixels.len(), width as usize * height as usize * 4);
                }
            }
        }
    }
}
