//! Texture atlas: small images packed into shared pages so that sprites using them
//! draw in one batch. Each image is surrounded by a border of its own edge pixels
//! repeated, so linear filtering at its edges never reads a neighbour.

use crate::hal::{
    DescError, Device, DeviceError, Extent, Origin, TextureDesc, TextureFormat, TextureUsage,
};
use crate::textures::{TextureId, Textures};

/// Texture coordinates of a region: `u` left to right, `v` top to bottom (ADR-0019).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvRect {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

impl UvRect {
    /// The whole texture.
    pub const FULL: Self = Self {
        u0: 0.0,
        v0: 0.0,
        u1: 1.0,
        v1: 1.0,
    };

    /// The pixels `size` at `origin` of a texture of `texture` size.
    pub fn of(origin: Origin, size: Extent, texture: Extent) -> Self {
        let (w, h) = (texture.width as f32, texture.height as f32);
        Self {
            u0: origin.x as f32 / w,
            v0: origin.y as f32 / h,
            u1: (origin.x + size.width) as f32 / w,
            v1: (origin.y + size.height) as f32 / h,
        }
    }
}

/// Where an image ended up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtlasRegion {
    pub texture: TextureId,
    /// The image's pixels inside the page, without the border.
    pub origin: Origin,
    pub size: Extent,
    pub uv: UvRect,
}

/// A row of a page: images are placed left to right on it.
#[derive(Clone, Copy, Debug)]
struct Shelf {
    y: u32,
    height: u32,
    /// Next free x.
    x: u32,
}

/// Packs rectangles into a fixed-size area on shelves: a rectangle goes on the
/// lowest-waste shelf it fits, or opens a new shelf below the last one.
#[derive(Debug)]
pub struct ShelfPacker {
    size: Extent,
    shelves: Vec<Shelf>,
    /// Top of the unused area below the last shelf.
    bottom: u32,
}

impl ShelfPacker {
    pub fn new(size: Extent) -> Self {
        Self {
            size,
            shelves: Vec::new(),
            bottom: 0,
        }
    }

    /// Top-left corner for a `size` rectangle, or `None` if it does not fit.
    pub fn allocate(&mut self, size: Extent) -> Option<Origin> {
        if size.is_empty() || size.width > self.size.width || size.height > self.size.height {
            return None;
        }
        // A shelf at most this much taller than the rectangle is used as is; others
        // are skipped, so tall shelves are not filled with small images.
        let max_waste = size.height / 2;
        let best = self
            .shelves
            .iter_mut()
            .filter(|s| {
                s.height >= size.height
                    && s.height - size.height <= max_waste
                    && self.size.width - s.x >= size.width
            })
            .min_by_key(|s| s.height - size.height);
        if let Some(shelf) = best {
            let origin = Origin::new(shelf.x, shelf.y);
            shelf.x += size.width;
            return Some(origin);
        }
        if self.size.height - self.bottom < size.height {
            return None;
        }
        let origin = Origin::new(0, self.bottom);
        self.shelves.push(Shelf {
            y: self.bottom,
            height: size.height,
            x: size.width,
        });
        self.bottom += size.height;
        Some(origin)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AtlasError {
    /// A page was removed from the texture set given to [`Atlas::add`] (or another set
    /// was given).
    #[error("atlas page {0:?} is not in the texture set")]
    MissingPage(TextureId),
    #[error(transparent)]
    Device(#[from] DeviceError),
}

impl From<DescError> for AtlasError {
    fn from(e: DescError) -> Self {
        Self::Device(e.into())
    }
}

/// Pages of one format, each a `COPY_DST` texture registered in a [`Textures`] set.
/// An image larger than a page gets a page stretched to fit it (whose spare room
/// takes other images too). Images stay until the pages are destroyed; their
/// regions name pages by [`TextureId`], so they must not outlive the pages: ids of
/// destroyed textures are reused.
pub struct Atlas {
    page_size: Extent,
    border: u32,
    format: TextureFormat,
    pages: Vec<(TextureId, ShelfPacker)>,
    /// Padded image being uploaded; kept to reuse its memory.
    scratch: Vec<u8>,
}

impl Atlas {
    /// Border pixels around each image: enough for linear filtering without mipmaps.
    pub const DEFAULT_BORDER: u32 = 1;

    pub const DEFAULT_PAGE_SIZE: Extent = Extent::new(2048, 2048);

    /// `page_size` is clamped to the device's texture size limit when pages are made.
    pub fn new(page_size: Extent, format: TextureFormat) -> Self {
        Self {
            page_size,
            border: Self::DEFAULT_BORDER,
            format,
            pages: Vec::new(),
            scratch: Vec::new(),
        }
    }

    pub fn format(&self) -> TextureFormat {
        self.format
    }

    /// Ids of the page textures, oldest first.
    pub fn pages(&self) -> impl Iterator<Item = TextureId> + '_ {
        self.pages.iter().map(|(id, _)| *id)
    }

    /// Copies an image (tightly packed rows of the atlas format, top row first) into a
    /// page, creating one if none has room.
    pub fn add<D: Device>(
        &mut self,
        device: &mut D,
        textures: &mut Textures<D::Texture>,
        size: Extent,
        pixels: &[u8],
    ) -> Result<AtlasRegion, AtlasError> {
        let bpp = self.format.bytes_per_pixel() as usize;
        if size.is_empty() {
            return Err(DescError::EmptySize.into());
        }
        let expected = size.width as u64 * size.height as u64 * bpp as u64;
        if pixels.len() as u64 != expected {
            return Err(DescError::DataSize {
                expected,
                actual: pixels.len() as u64,
            }
            .into());
        }
        let b = self.border;
        let padded = Extent::new(
            size.width.saturating_add(2 * b),
            size.height.saturating_add(2 * b),
        );
        let found = self
            .pages
            .iter_mut()
            .find_map(|(id, packer)| packer.allocate(padded).map(|o| (*id, o)));
        let (texture, corner) = match found {
            Some(found) => found,
            None => {
                let max = device.limits().max_texture_size;
                let page = Extent::new(
                    self.page_size.width.min(max).max(padded.width),
                    self.page_size.height.min(max).max(padded.height),
                );
                let desc = TextureDesc {
                    size: page,
                    format: self.format,
                    usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
                };
                // Before allocating: an image near the size limit cannot fit with its
                // border.
                desc.validate(&device.limits(), None)?;
                // Zeroed, so unused space samples as transparent black.
                let zeros = vec![0; desc.byte_len() as usize];
                let id = textures.insert(device.create_texture(&desc, Some(&zeros))?);
                let mut packer = ShelfPacker::new(page);
                let corner = packer
                    .allocate(padded)
                    .expect("a new page is at least as large as the image");
                self.pages.push((id, packer));
                (id, corner)
            }
        };
        extrude(&mut self.scratch, pixels, size, b, bpp);
        let page = textures
            .get(texture)
            .ok_or(AtlasError::MissingPage(texture))?;
        device.write_texture(page, corner, padded, &self.scratch)?;
        let page_size = self
            .pages
            .iter()
            .find(|(id, _)| *id == texture)
            .map(|(_, p)| p.size)
            .expect("the page was just found or made");
        let origin = Origin::new(corner.x + b, corner.y + b);
        Ok(AtlasRegion {
            texture,
            origin,
            size,
            uv: UvRect::of(origin, size, page_size),
        })
    }
}

/// Writes `pixels` into `out` with a `border` of repeated edge pixels on every side.
fn extrude(out: &mut Vec<u8>, pixels: &[u8], size: Extent, border: u32, bpp: usize) {
    let (w, h, b) = (size.width as usize, size.height as usize, border as usize);
    let row = (w + 2 * b) * bpp;
    out.clear();
    out.reserve(row * (h + 2 * b));
    for y in 0..h + 2 * b {
        let src = &pixels[y.saturating_sub(b).min(h - 1) * w * bpp..][..w * bpp];
        for _ in 0..b {
            out.extend_from_slice(&src[..bpp]);
        }
        out.extend_from_slice(src);
        for _ in 0..b {
            out.extend_from_slice(&src[(w - 1) * bpp..]);
        }
    }
    debug_assert_eq!(out.len(), row * (h + 2 * b));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_device::{Call, Fake};

    // `count_allocs` below relies on the counting allocator that the `sprite` tests
    // install for this (single) unit-test binary.

    fn overlaps(a: (Origin, Extent), b: (Origin, Extent)) -> bool {
        a.0.x < b.0.x + b.1.width
            && b.0.x < a.0.x + a.1.width
            && a.0.y < b.0.y + b.1.height
            && b.0.y < a.0.y + a.1.height
    }

    #[test]
    fn packs_without_overlap_inside_the_area() {
        let area = Extent::new(64, 64);
        let mut p = ShelfPacker::new(area);
        let mut placed = Vec::new();
        // Deterministic pseudo-random sizes.
        let mut seed = 12345u32;
        for _ in 0..200 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let size = Extent::new(1 + (seed >> 8) % 12, 1 + (seed >> 16) % 12);
            if let Some(o) = p.allocate(size) {
                assert!(o.x + size.width <= area.width && o.y + size.height <= area.height);
                for &other in &placed {
                    assert!(
                        !overlaps((o, size), other),
                        "{o:?} {size:?} overlaps {other:?}"
                    );
                }
                placed.push((o, size));
            }
        }
        assert!(placed.len() > 20, "only {} placed", placed.len());
    }

    #[test]
    fn fills_shelves_left_to_right_then_opens_new_ones() {
        let mut p = ShelfPacker::new(Extent::new(10, 10));
        assert_eq!(p.allocate(Extent::new(4, 4)), Some(Origin::new(0, 0)));
        assert_eq!(p.allocate(Extent::new(4, 3)), Some(Origin::new(4, 0)));
        // Does not fit the remaining width of the first shelf.
        assert_eq!(p.allocate(Extent::new(4, 4)), Some(Origin::new(0, 4)));
        // Fits the first shelf exactly.
        assert_eq!(p.allocate(Extent::new(2, 4)), Some(Origin::new(8, 0)));
        // Too short for any shelf without wasting more than half its height.
        assert_eq!(p.allocate(Extent::new(1, 1)), Some(Origin::new(0, 8)));
        assert_eq!(p.allocate(Extent::new(1, 1)), Some(Origin::new(1, 8)));
        // Fits the second shelf, wasting one row.
        assert_eq!(p.allocate(Extent::new(3, 3)), Some(Origin::new(4, 4)));
        // Too tall for the last shelf and no room for a new one.
        assert_eq!(p.allocate(Extent::new(10, 2)), None);
        assert_eq!(p.allocate(Extent::new(11, 1)), None);
        assert_eq!(p.allocate(Extent::new(0, 1)), None);
    }

    #[test]
    fn picks_the_shelf_with_least_waste() {
        let mut p = ShelfPacker::new(Extent::new(10, 20));
        assert_eq!(p.allocate(Extent::new(8, 6)), Some(Origin::new(0, 0)));
        // Too wide for what is left of the first shelf.
        assert_eq!(p.allocate(Extent::new(9, 5)), Some(Origin::new(0, 6)));
        // Both shelves fit a 1x4; the 5-high one wastes less.
        assert_eq!(p.allocate(Extent::new(1, 4)), Some(Origin::new(9, 6)));
        assert_eq!(p.allocate(Extent::new(1, 4)), Some(Origin::new(8, 0)));
    }

    #[test]
    fn extrudes_edges() {
        // 2x2 image, one byte per pixel:
        // 1 2
        // 3 4
        let mut out = Vec::new();
        extrude(&mut out, &[1, 2, 3, 4], Extent::new(2, 2), 1, 1);
        #[rustfmt::skip]
        let expected = [
            1, 1, 2, 2,
            1, 1, 2, 2,
            3, 3, 4, 4,
            3, 3, 4, 4,
        ];
        assert_eq!(out, expected);
        extrude(&mut out, &[1, 2, 3, 4, 5, 6, 7, 8], Extent::new(2, 1), 2, 4);
        assert_eq!(out.len(), 6 * 5 * 4);
        assert_eq!(&out[..8], &[1, 2, 3, 4, 1, 2, 3, 4]);
        assert_eq!(&out[out.len() - 4..], &[5, 6, 7, 8]);
    }

    #[test]
    fn uv_of_region() {
        let uv = UvRect::of(Origin::new(1, 2), Extent::new(2, 4), Extent::new(4, 8));
        assert_eq!(
            uv,
            UvRect {
                u0: 0.25,
                v0: 0.25,
                u1: 0.75,
                v1: 0.75
            }
        );
    }

    fn add(
        atlas: &mut Atlas,
        d: &mut Fake,
        t: &mut Textures<u32>,
        w: u32,
        h: u32,
    ) -> Result<AtlasRegion, AtlasError> {
        let pixels = vec![7; (w * h * 4) as usize];
        atlas.add(d, t, Extent::new(w, h), &pixels)
    }

    #[test]
    fn atlas_fills_pages_and_uploads_with_border() {
        let mut d = Fake::default();
        let mut t = Textures::new();
        let mut atlas = Atlas::new(Extent::new(32, 32), TextureFormat::Rgba8);

        let a = add(&mut atlas, &mut d, &mut t, 8, 4).unwrap();
        assert_eq!((a.origin, a.size), (Origin::new(1, 1), Extent::new(8, 4)));
        assert_eq!(
            a.uv,
            UvRect {
                u0: 1.0 / 32.0,
                v0: 1.0 / 32.0,
                u1: 9.0 / 32.0,
                v1: 5.0 / 32.0
            }
        );
        let page = *t.get(a.texture).unwrap();
        assert_eq!(
            d.log,
            [Call::WriteTexture {
                texture: page,
                origin: Origin::new(0, 0),
                size: Extent::new(10, 6)
            }]
        );
        // Same shelf, after the first image's border.
        let b = add(&mut atlas, &mut d, &mut t, 4, 4).unwrap();
        assert_eq!((b.texture, b.origin), (a.texture, Origin::new(11, 1)));

        // Does not fit what is left of the page: a new one.
        let c = add(&mut atlas, &mut d, &mut t, 30, 28).unwrap();
        assert_ne!(c.texture, a.texture);
        // Larger than a page: a page stretched to fit, then reused.
        let big = add(&mut atlas, &mut d, &mut t, 40, 2).unwrap();
        let big_page = *t.get(big.texture).unwrap();
        let (_, desc) = d
            .live_textures
            .iter()
            .find(|(id, _)| *id == big_page)
            .unwrap();
        assert_eq!(desc.size, Extent::new(42, 32));
        assert!(
            desc.usage
                .contains(TextureUsage::SAMPLED | TextureUsage::COPY_DST)
        );
        assert_eq!(big.uv.u1, 41.0 / 42.0);
        let small = add(&mut atlas, &mut d, &mut t, 2, 2).unwrap();
        assert_eq!(small.texture, a.texture);
        assert_eq!(atlas.pages().count(), 3);
    }

    #[test]
    fn atlas_rejects_bad_images_before_allocating_pages() {
        let mut d = Fake::default();
        let mut t = Textures::new();
        let mut atlas = Atlas::new(Atlas::DEFAULT_PAGE_SIZE, TextureFormat::Rgba8);
        assert!(matches!(
            atlas.add(&mut d, &mut t, Extent::new(2, 2), &[0; 15]),
            Err(AtlasError::Device(DeviceError::Desc(
                DescError::DataSize { .. }
            )))
        ));
        assert!(matches!(
            add(&mut atlas, &mut d, &mut t, 0, 2),
            Err(AtlasError::Device(DeviceError::Desc(DescError::EmptySize)))
        ));
        // At the device limit the border no longer fits; nothing is made.
        let (result, allocs) =
            slam_testkit::count_allocs(|| atlas.add(&mut d, &mut t, Extent::new(4096, 1), &[]));
        assert!(matches!(
            result,
            Err(AtlasError::Device(DeviceError::Desc(
                DescError::DataSize { .. }
            )))
        ));
        assert_eq!(allocs.total(), 0);
        let pixels = vec![0; 4096 * 4];
        let (result, allocs) =
            slam_testkit::count_allocs(|| atlas.add(&mut d, &mut t, Extent::new(4096, 1), &pixels));
        assert!(matches!(
            result,
            Err(AtlasError::Device(DeviceError::Desc(
                DescError::TooLarge { .. }
            )))
        ));
        assert_eq!(allocs.total(), 0, "{allocs}");
        assert!(d.live_textures.is_empty());
    }

    #[test]
    fn atlas_reports_a_missing_page() {
        let mut d = Fake::default();
        let mut t = Textures::new();
        let mut atlas = Atlas::new(Extent::new(32, 32), TextureFormat::Rgba8);
        let a = add(&mut atlas, &mut d, &mut t, 2, 2).unwrap();
        t.remove(a.texture);
        assert!(matches!(
            add(&mut atlas, &mut d, &mut t, 2, 2),
            Err(AtlasError::MissingPage(id)) if id == a.texture
        ));
    }
}
