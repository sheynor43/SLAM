//! Image decoding into `Rgba8` pixels for textures (ADR-0034, ADR-0040). Runs on any
//! thread; the draw thread only uploads the result.

use std::io::Cursor;

use zune_jpeg::JpegDecoder;
use zune_jpeg::errors::DecodeErrors;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

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
    #[error("invalid JPEG: {0}")]
    Jpeg(#[source] zune_jpeg::errors::DecodeErrors),
    #[error("unsupported image format")]
    Unsupported,
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const JPEG_SIGNATURE: [u8; 3] = [0xFF, 0xD8, 0xFF];

/// Decodes a PNG or a JPEG, chosen by the file signature and not by the extension
/// (ADR-0040); anything else is [`ImageError::Unsupported`].
pub fn decode_image(bytes: &[u8], limits: ImageLimits) -> Result<Image, ImageError> {
    if bytes.starts_with(&PNG_SIGNATURE) {
        decode_png(bytes, limits)
    } else if bytes.starts_with(&JPEG_SIGNATURE) {
        decode_jpeg(bytes, limits)
    } else {
        Err(ImageError::Unsupported)
    }
}

/// Decodes a baseline or progressive JPEG of any colour space (grey, YCbCr, CMYK,
/// YCCK) into `Rgba8` with opaque alpha. EXIF orientation and ICC profiles are
/// ignored. The header is read first, so an oversized image is rejected before any
/// pixel memory is allocated.
pub fn decode_jpeg(bytes: &[u8], limits: ImageLimits) -> Result<Image, ImageError> {
    // JPEG dimensions are at most 65535; the limits are checked here so that the
    // error is `TooLarge` and not a decoder-specific one.
    let options = DecoderOptions::default()
        .set_max_width(usize::from(u16::MAX))
        .set_max_height(usize::from(u16::MAX))
        .jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder.decode_headers().map_err(ImageError::Jpeg)?;
    // Set by a successful `decode_headers`; a missing one is treated as a broken file.
    let info = decoder
        .info()
        .ok_or(ImageError::Jpeg(DecodeErrors::FormatStatic(
            "no JPEG header",
        )))?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    let rgba_len = u64::from(width) * u64::from(height) * 4;
    if width > limits.max_side || height > limits.max_side || rgba_len > limits.max_bytes as u64 {
        return Err(ImageError::TooLarge { width, height });
    }
    let four_components = matches!(
        decoder.input_colorspace(),
        Some(ColorSpace::CMYK | ColorSpace::YCCK)
    );
    if four_components {
        // zune-jpeg 0.5.15 writes garbage for CMYK/YCCK straight into RGBA (it
        // steps its output by 3 bytes), so these are decoded to RGB and widened.
        decoder.set_options(options.jpeg_set_out_colorspace(ColorSpace::RGB));
    }
    let len = decoder
        .output_buffer_size()
        .ok_or(ImageError::TooLarge { width, height })?;
    let channels = if four_components { 3 } else { 4 };
    if len as u64 != u64::from(width) * u64::from(height) * channels {
        // The decoder did not honour the requested output colour space.
        return Err(ImageError::FrameMismatch);
    }
    let mut pixels = vec![0; len];
    decoder.decode_into(&mut pixels).map_err(ImageError::Jpeg)?;
    if four_components {
        let mut rgba = Vec::with_capacity(rgba_len as usize);
        for px in pixels.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
        pixels = rgba;
    }
    Ok(Image {
        size: Extent::new(width, height),
        pixels,
    })
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

    const RGB_JPEG: &[u8] = include_bytes!("../tests/data/rgb-32x16.jpg");
    const GRAY_JPEG: &[u8] = include_bytes!("../tests/data/gray-32x16.jpg");
    const PROGRESSIVE_JPEG: &[u8] = include_bytes!("../tests/data/progressive-32x16.jpg");
    const CMYK_JPEG: &[u8] = include_bytes!("../tests/data/cmyk-32x16.jpg");

    fn decode_jpeg_ok(bytes: &[u8]) -> Image {
        decode_jpeg(bytes, ImageLimits::default()).unwrap()
    }

    fn pixel(img: &Image, x: usize, y: usize) -> [u8; 4] {
        let i = (y * img.size.width as usize + x) * 4;
        img.pixels[i..i + 4].try_into().unwrap()
    }

    fn assert_near(actual: [u8; 4], expected: [u8; 3], tolerance: u8) {
        assert_eq!(actual[3], 255, "alpha of {actual:?}");
        for c in 0..3 {
            assert!(
                actual[c].abs_diff(expected[c]) <= tolerance,
                "{actual:?} is not near {expected:?}"
            );
        }
    }

    // Fixtures: two solid halves, (200, 50, 30) and (20, 120, 220), made with PIL at
    // quality 90 (4:4:4 for the colour ones).
    const LEFT: [u8; 3] = [200, 50, 30];
    const RIGHT: [u8; 3] = [20, 120, 220];

    #[test]
    fn baseline_jpeg_decodes_to_opaque_rgba() {
        let img = decode_jpeg_ok(RGB_JPEG);
        assert_eq!(img.size, Extent::new(32, 16));
        assert_eq!(img.pixels.len(), 32 * 16 * 4);
        assert_near(pixel(&img, 4, 8), LEFT, 3);
        assert_near(pixel(&img, 27, 8), RIGHT, 3);
    }

    #[test]
    fn progressive_jpeg_matches_the_baseline_colours() {
        let img = decode_jpeg_ok(PROGRESSIVE_JPEG);
        assert_eq!(img.size, Extent::new(32, 16));
        assert_near(pixel(&img, 4, 8), LEFT, 3);
        assert_near(pixel(&img, 27, 8), RIGHT, 3);
    }

    #[test]
    fn grey_jpeg_spreads_to_rgb() {
        let img = decode_jpeg_ok(GRAY_JPEG);
        assert_eq!(img.size, Extent::new(32, 16));
        // BT.601 luma of the two halves: 92.6 and 101.5.
        let left = pixel(&img, 4, 8);
        let right = pixel(&img, 27, 8);
        assert_near(left, [93, 93, 93], 3);
        assert_near(right, [101, 101, 101], 3);
    }

    #[test]
    fn cmyk_jpeg_converts_to_rgb() {
        let img = decode_jpeg_ok(CMYK_JPEG);
        assert_eq!(img.size, Extent::new(32, 16));
        assert_near(pixel(&img, 4, 8), LEFT, 6);
        assert_near(pixel(&img, 27, 8), RIGHT, 6);
    }

    #[test]
    fn jpeg_sizes_over_the_limits_are_rejected() {
        let limits = ImageLimits {
            max_side: 31,
            ..ImageLimits::default()
        };
        assert!(matches!(
            decode_jpeg(RGB_JPEG, limits),
            Err(ImageError::TooLarge {
                width: 32,
                height: 16
            })
        ));
        let limits = ImageLimits {
            max_bytes: 32 * 16 * 4 - 1,
            ..ImageLimits::default()
        };
        assert!(matches!(
            decode_jpeg(RGB_JPEG, limits),
            Err(ImageError::TooLarge { .. })
        ));
        let exact = ImageLimits {
            max_side: 32,
            max_bytes: 32 * 16 * 4,
        };
        assert!(decode_jpeg(RGB_JPEG, exact).is_ok());
    }

    #[test]
    fn broken_jpegs_are_errors_not_panics() {
        let limits = ImageLimits::default();
        assert!(decode_jpeg(&[], limits).is_err());
        assert!(decode_jpeg(&[0xff, 0xd8, 0xff], limits).is_err());
        // Pseudo-random bytes after the signature.
        let mut garbage = vec![0xff, 0xd8, 0xff];
        let mut x = 12345u32;
        for _ in 0..512 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            garbage.push((x >> 24) as u8);
        }
        assert!(decode_jpeg(&garbage, limits).is_err());
        // Truncated at every length: never a panic, and a decoded image is whole.
        for bytes in [RGB_JPEG, PROGRESSIVE_JPEG, GRAY_JPEG, CMYK_JPEG] {
            for len in 0..bytes.len() {
                if let Ok(img) = decode_jpeg(&bytes[..len], limits) {
                    let Extent { width, height } = img.size;
                    assert_eq!(img.pixels.len(), width as usize * height as usize * 4);
                }
            }
        }
        assert!(decode_jpeg(&RGB_JPEG[..RGB_JPEG.len() / 2], limits).is_err());
        // Every byte corrupted in turn.
        for i in 0..RGB_JPEG.len() {
            let mut bad = RGB_JPEG.to_vec();
            bad[i] ^= 0xff;
            let _ = decode_jpeg(&bad, limits);
        }
    }

    #[test]
    fn decode_image_picks_the_decoder_by_signature() {
        let limits = ImageLimits::default();
        let png = Png::new(1, 1, png::ColorType::Rgba).encode(&[1, 2, 3, 4]);
        assert_eq!(decode_image(&png, limits).unwrap().pixels, [1, 2, 3, 4]);
        let jpeg = decode_image(RGB_JPEG, limits).unwrap();
        assert_eq!(jpeg.size, Extent::new(32, 16));
        for bytes in [&b"GIF89a"[..], b"", b"\xff\xd8", b"BM"] {
            assert!(matches!(
                decode_image(bytes, limits),
                Err(ImageError::Unsupported)
            ));
        }
    }
}
