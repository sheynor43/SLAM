//! Making one thumbnail (ADR-0127): find the difficulty's background, take the cached
//! PNG if there is one, else decode the background, crop it to the thumbnail's aspect
//! (`cover`), shrink it to the height and cache it. Runs on the thumbnail thread only.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use slam_render::hal::Extent;
use slam_render::image::{Image, ImageLimits, decode_image, decode_png};

use crate::play::find_beatmap_file;

/// The largest rectangle of aspect `aspect` (width over height) in a `src` image,
/// centred: `(x, y, width, height)` in pixels. An empty image or a bad aspect gives the
/// whole image.
pub fn cover_crop(src: (u32, u32), aspect: f32) -> (u32, u32, u32, u32) {
    let (w, h) = src;
    if w == 0 || h == 0 || aspect.is_nan() || aspect <= 0.0 {
        return (0, 0, w, h);
    }
    let image_aspect = w as f32 / h as f32;
    if image_aspect > aspect {
        // Wider than the frame: cut the sides.
        let cw = ((h as f32 * aspect).round() as u32).clamp(1, w);
        ((w - cw) / 2, 0, cw, h)
    } else {
        let ch = ((w as f32 / aspect).round() as u32).clamp(1, h);
        (0, (h - ch) / 2, w, ch)
    }
}

/// The size a `crop_w` x `crop_h` crop shrinks to for thumbnails `height` pixels tall: the
/// same aspect, never larger than the crop (a small background is not blown up).
pub fn output_size(crop_w: u32, crop_h: u32, height: u32) -> (u32, u32) {
    if crop_h <= height {
        return (crop_w.max(1), crop_h.max(1));
    }
    let width = (u64::from(crop_w) * u64::from(height) + u64::from(crop_h) / 2) / u64::from(crop_h);
    ((width as u32).max(1), height.max(1))
}

/// Shrinks the `crop` rectangle `(x, y, width, height)` of `src` to `out_w` x `out_h` by
/// averaging the source pixels each output pixel covers (a box filter with fractional
/// edges). The crop must lie inside `src`; sizes are not checked beyond that.
pub fn downscale(src: &Image, crop: (u32, u32, u32, u32), out_w: u32, out_h: u32) -> Image {
    let (cx, cy, cw, ch) = crop;
    let stride = src.size.width as usize * 4;
    let mut pixels = Vec::with_capacity(out_w as usize * out_h as usize * 4);
    let (step_x, step_y) = (
        f64::from(cw) / f64::from(out_w),
        f64::from(ch) / f64::from(out_h),
    );
    for oy in 0..out_h {
        let (y0, y1) = (f64::from(oy) * step_y, f64::from(oy + 1) * step_y);
        for ox in 0..out_w {
            let (x0, x1) = (f64::from(ox) * step_x, f64::from(ox + 1) * step_x);
            let mut sum = [0.0_f64; 4];
            let mut weight = 0.0;
            for iy in y0.floor() as u32..(y1.ceil() as u32).min(ch) {
                let wy = (f64::from(iy + 1)).min(y1) - f64::from(iy).max(y0);
                let row = (cy + iy) as usize * stride;
                for ix in x0.floor() as u32..(x1.ceil() as u32).min(cw) {
                    let wx = (f64::from(ix + 1)).min(x1) - f64::from(ix).max(x0);
                    let at = row + (cx + ix) as usize * 4;
                    let w = wx * wy;
                    for (acc, v) in sum.iter_mut().zip(&src.pixels[at..at + 4]) {
                        *acc += f64::from(*v) * w;
                    }
                    weight += w;
                }
            }
            for acc in sum {
                pixels.push(if weight > 0.0 {
                    (acc / weight).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                });
            }
        }
    }
    Image {
        size: Extent::new(out_w, out_h),
        pixels,
    }
}

/// The cache key of a background: a hash of its path, size and modification time, so an
/// edited or replaced file gets a new thumbnail (FNV-1a, stable between runs and builds).
pub fn cache_key(path: &Path, len: u64, modified_ns: u128) -> u64 {
    let mut hash = fnv_start();
    hash = fnv(hash, path.as_os_str().as_encoded_bytes());
    hash = fnv(hash, &[0]);
    hash = fnv(hash, &len.to_le_bytes());
    fnv(hash, &modified_ns.to_le_bytes())
}

/// FNV-1a over `bytes`, continuing from `hash`.
pub fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn fnv_start() -> u64 {
    0xcbf2_9ce4_8422_2325
}

/// Makes thumbnails; owns the cache folder.
pub struct Maker {
    /// The `cache` folder; `None` without one: nothing is cached.
    cache: Option<PathBuf>,
    /// Width over height.
    aspect: f32,
    limits: ImageLimits,
}

impl Maker {
    pub fn new(cache: Option<PathBuf>, aspect: f32, limits: ImageLimits) -> Maker {
        Maker {
            cache,
            aspect,
            limits,
        }
    }

    /// The cached file for key `key` at `height`.
    fn cache_file(&self, height: u32, key: u64) -> Option<PathBuf> {
        let cache = self.cache.as_ref()?;
        Some(
            cache
                .join("thumbnails")
                .join(height.to_string())
                .join(format!("{key:016x}.png")),
        )
    }

    /// The thumbnail of the background called `name` in the set folder `dir`; `None` when
    /// the file is missing or is not an image.
    pub fn make(&self, dir: &Path, name: &str, height: u32) -> Option<Image> {
        let path = find_beatmap_file(dir, name)?;
        let meta = std::fs::metadata(&path).ok()?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let key = cache_key(&path, meta.len(), modified);
        let cached = self.cache_file(height, key);
        if let Some(file) = &cached
            && let Ok(bytes) = std::fs::read(file)
        {
            match decode_png(&bytes, self.limits) {
                Ok(image) => return Some(image),
                Err(e) => tracing::warn!("thumbnail cache {}: {e}", file.display()),
            }
        }
        let bytes = std::fs::read(&path).ok()?;
        let source = decode_image(&bytes, self.limits)
            .inspect_err(|e| tracing::warn!("thumbnail of {}: {e}", path.display()))
            .ok()?;
        drop(bytes);
        let crop = cover_crop((source.size.width, source.size.height), self.aspect);
        let (w, h) = output_size(crop.2, crop.3, height);
        let image = downscale(&source, crop, w, h);
        if let Some(file) = cached {
            store(&file, &image);
        }
        Some(image)
    }
}

/// Writes `image` as a PNG at `file` through a temporary file, so a reader never sees half
/// of it. A failure only costs the cache.
fn store(file: &Path, image: &Image) {
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, image.size.width, image.size.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
        writer
            .write_image_data(&image.pixels)
            .map_err(std::io::Error::other)?;
        writer.finish().map_err(std::io::Error::other)?;
        let temp = file.with_extension("png.tmp");
        std::fs::write(&temp, &bytes)?;
        std::fs::rename(&temp, file)
    })();
    if let Err(e) = result {
        tracing::warn!("thumbnail cache {}: {e}", file.display());
    }
}
