//! Backend-independent render API: resource descriptions, their validation and the
//! device trait every backend implements. Only what M0 needs; grows with use.

use std::ops::BitOr;

/// RGBA colour, components in `0.0..=1.0`, written to the target as is: on the
/// default framebuffer (no sRGB conversion) the values are shown as sRGB-encoded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }
}

/// Size in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

impl Extent {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFormat {
    /// 8-bit RGBA, linear.
    Rgba8,
    /// 8-bit RGBA, sRGB-encoded colour, linear alpha.
    Rgba8Srgb,
    /// 8-bit single channel (masks, glyph coverage).
    R8,
}

impl TextureFormat {
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba8 | Self::Rgba8Srgb => 4,
            Self::R8 => 1,
        }
    }
}

/// How a texture is used. Combine with `|`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextureUsage(u8);

impl TextureUsage {
    /// Read by shaders.
    pub const SAMPLED: Self = Self(1);
    /// Drawn into as a render target.
    pub const RENDER_TARGET: Self = Self(1 << 1);
    /// Updated after creation.
    pub const COPY_DST: Self = Self(1 << 2);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl BitOr for TextureUsage {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureDesc {
    pub size: Extent,
    pub format: TextureFormat,
    pub usage: TextureUsage,
}

impl TextureDesc {
    /// Bytes of tightly packed pixel data for the whole texture, saturating at
    /// `u64::MAX`.
    pub const fn byte_len(&self) -> u64 {
        (self.size.width as u64 * self.size.height as u64)
            .saturating_mul(self.format.bytes_per_pixel() as u64)
    }

    /// Checks the description against device limits and, if given, the length of the
    /// initial pixel data (tightly packed rows, top row first).
    pub fn validate(&self, limits: &Limits, data: Option<&[u8]>) -> Result<(), DescError> {
        let Extent { width, height } = self.size;
        if self.size.is_empty() {
            return Err(DescError::EmptySize);
        }
        if width > limits.max_texture_size || height > limits.max_texture_size {
            return Err(DescError::TooLarge {
                width,
                height,
                max: limits.max_texture_size,
            });
        }
        if !self
            .usage
            .intersects(TextureUsage::SAMPLED | TextureUsage::RENDER_TARGET)
        {
            return Err(DescError::Unusable);
        }
        if let Some(data) = data {
            let expected = self.byte_len();
            if data.len() as u64 != expected {
                return Err(DescError::DataSize {
                    expected,
                    actual: data.len() as u64,
                });
            }
        }
        Ok(())
    }
}

/// Device limits that descriptions are validated against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_texture_size: u32,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DescError {
    #[error("texture has zero size")]
    EmptySize,
    #[error("texture size {width}x{height} exceeds the device limit {max}")]
    TooLarge { width: u32, height: u32, max: u32 },
    #[error("texture usage includes neither SAMPLED nor RENDER_TARGET")]
    Unusable,
    #[error("initial data is {actual} bytes, expected {expected}")]
    DataSize { expected: u64, actual: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error(transparent)]
    Desc(#[from] DescError),
    #[error("surface: {0}")]
    Surface(String),
    #[error("unsupported device: {0}")]
    Unsupported(String),
    #[error("backend: {0}")]
    Backend(String),
}

/// A render device bound to one surface. Used by the draw thread only; `begin_frame`
/// and `present` are the hot path and must not allocate except when reporting an
/// error.
pub trait Device {
    type Texture;

    fn limits(&self) -> Limits;

    /// Creates a texture, optionally filled with `data` (see [`TextureDesc::validate`]).
    fn create_texture(
        &mut self,
        desc: &TextureDesc,
        data: Option<&[u8]>,
    ) -> Result<Self::Texture, DeviceError>;

    fn destroy_texture(&mut self, texture: Self::Texture);

    /// Starts a frame: targets the surface, sets the viewport to its current size and
    /// clears it. Returns that size; it is empty while the window is minimised.
    fn begin_frame(&mut self, clear: Color) -> Extent;

    /// Shows the frame without waiting for vertical sync.
    fn present(&mut self) -> Result<(), DeviceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        max_texture_size: 4096,
    };

    fn desc(width: u32, height: u32, format: TextureFormat) -> TextureDesc {
        TextureDesc {
            size: Extent::new(width, height),
            format,
            usage: TextureUsage::SAMPLED,
        }
    }

    #[test]
    fn accepts_valid_texture() {
        let d = desc(256, 128, TextureFormat::Rgba8);
        assert_eq!(d.validate(&LIMITS, None), Ok(()));
        assert_eq!(d.validate(&LIMITS, Some(&vec![0; 256 * 128 * 4])), Ok(()));
        let max = desc(4096, 4096, TextureFormat::R8);
        assert_eq!(max.validate(&LIMITS, None), Ok(()));
    }

    #[test]
    fn rejects_empty_size() {
        for (w, h) in [(0, 1), (1, 0), (0, 0)] {
            let d = desc(w, h, TextureFormat::Rgba8);
            assert_eq!(d.validate(&LIMITS, None), Err(DescError::EmptySize));
        }
    }

    #[test]
    fn rejects_size_over_limit() {
        for (w, h) in [(4097, 1), (1, 4097)] {
            let d = desc(w, h, TextureFormat::Rgba8);
            assert_eq!(
                d.validate(&LIMITS, None),
                Err(DescError::TooLarge {
                    width: w,
                    height: h,
                    max: 4096
                })
            );
        }
    }

    #[test]
    fn requires_sampled_or_render_target() {
        let mut d = desc(4, 4, TextureFormat::Rgba8);
        d.usage = TextureUsage::COPY_DST;
        assert_eq!(d.validate(&LIMITS, None), Err(DescError::Unusable));
        d.usage = TextureUsage::default();
        assert_eq!(d.validate(&LIMITS, None), Err(DescError::Unusable));
        d.usage = TextureUsage::RENDER_TARGET | TextureUsage::COPY_DST;
        assert_eq!(d.validate(&LIMITS, None), Ok(()));
    }

    #[test]
    fn checks_initial_data_length() {
        let d = desc(3, 2, TextureFormat::R8);
        assert_eq!(d.byte_len(), 6);
        assert_eq!(
            d.validate(&LIMITS, Some(&[0; 5])),
            Err(DescError::DataSize {
                expected: 6,
                actual: 5
            })
        );
        let d = desc(3, 2, TextureFormat::Rgba8Srgb);
        assert_eq!(
            d.validate(&LIMITS, Some(&[0; 6])),
            Err(DescError::DataSize {
                expected: 24,
                actual: 6
            })
        );
    }

    #[test]
    fn byte_len_saturates() {
        let d = desc(u32::MAX, u32::MAX, TextureFormat::Rgba8);
        assert_eq!(d.byte_len(), u64::MAX);
    }

    #[test]
    fn usage_flags_combine() {
        let u = TextureUsage::SAMPLED | TextureUsage::COPY_DST;
        assert!(u.contains(TextureUsage::SAMPLED));
        assert!(u.contains(TextureUsage::COPY_DST));
        assert!(!u.contains(TextureUsage::RENDER_TARGET));
        assert!(!u.contains(TextureUsage::SAMPLED | TextureUsage::RENDER_TARGET));
    }
}
