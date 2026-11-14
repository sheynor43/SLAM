//! Comparison of a screenshot with its reference, with a tolerance for the small
//! differences another Mesa version may draw (ADR-0111).

use super::Frame;

/// How far a screenshot may be from its reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    /// A pixel differs when a channel is further off than this.
    pub channel: u8,
    /// The share of pixels, 0..=1, that may differ.
    pub pixels: f64,
}

impl Default for Tolerance {
    /// Rounding and filtering differences between Mesa versions, not a moved or missing
    /// element: a sprite of 20×20 pixels is already 0.13 % of a 640×480 frame. The share
    /// leaves room for the ~0.054 % that the intro's text edges differ by between Mesa
    /// 26.2 and CI's older Mesa.
    fn default() -> Self {
        Self {
            channel: 8,
            pixels: 0.001,
        }
    }
}

/// How a screenshot differs from its reference.
#[derive(Clone, Debug, PartialEq)]
pub struct Difference {
    /// Pixels further off than the tolerance allows.
    pub pixels: usize,
    /// All pixels of the frame.
    pub total: usize,
    /// The largest difference of a channel.
    pub max_channel: u8,
    /// The reference greyed out, with the differing pixels in red as bright as the
    /// difference.
    pub image: Frame,
}

impl Difference {
    /// The share of differing pixels.
    pub fn share(&self) -> f64 {
        self.pixels as f64 / self.total.max(1) as f64
    }
}

/// Why a screenshot does not match its reference.
#[derive(Clone, Debug, PartialEq)]
pub enum Mismatch {
    Size {
        expected: (u32, u32),
        actual: (u32, u32),
    },
    Pixels(Difference),
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mismatch::Size { expected, actual } => write!(
                f,
                "size {}x{}, the reference is {}x{}",
                actual.0, actual.1, expected.0, expected.1
            ),
            Mismatch::Pixels(d) => write!(
                f,
                "{} of {} pixels differ ({:.3} %), up to {} in a channel",
                d.pixels,
                d.total,
                d.share() * 100.0,
                d.max_channel
            ),
        }
    }
}

/// Compares `actual` with `expected`: `Ok` with the differing pixels, if any, while they
/// stay within `tolerance`.
pub fn compare(
    expected: &Frame,
    actual: &Frame,
    tolerance: Tolerance,
) -> Result<Option<Difference>, Mismatch> {
    if (expected.width, expected.height) != (actual.width, actual.height) {
        return Err(Mismatch::Size {
            expected: (expected.width, expected.height),
            actual: (actual.width, actual.height),
        });
    }
    let mut image = Vec::with_capacity(expected.pixels.len());
    let (mut pixels, mut max_channel) = (0, 0);
    for (e, a) in expected
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(actual.pixels.as_chunks::<4>().0)
    {
        let delta = e
            .iter()
            .zip(a)
            .map(|(e, a)| e.abs_diff(*a))
            .max()
            .unwrap_or(0);
        max_channel = max_channel.max(delta);
        if delta > tolerance.channel {
            pixels += 1;
            image.extend_from_slice(&[delta.max(128), 0, 0, 255]);
        } else {
            let grey = ((u16::from(e[0]) + u16::from(e[1]) + u16::from(e[2])) / 12) as u8;
            image.extend_from_slice(&[grey, grey, grey, 255]);
        }
    }
    if max_channel == 0 {
        return Ok(None);
    }
    let total = expected.pixels.len() / 4;
    let difference = Difference {
        pixels,
        total,
        max_channel,
        image: Frame {
            width: expected.width,
            height: expected.height,
            pixels: image,
        },
    };
    if difference.share() > tolerance.pixels {
        Err(Mismatch::Pixels(difference))
    } else {
        Ok(Some(difference))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, fill: [u8; 4]) -> Frame {
        Frame {
            width,
            height,
            pixels: fill.repeat((width * height) as usize),
        }
    }

    const TOLERANCE: Tolerance = Tolerance {
        channel: 8,
        pixels: 0.1,
    };

    #[test]
    fn equal_frames_match_without_a_difference() {
        let a = frame(4, 4, [10, 20, 30, 255]);
        assert_eq!(compare(&a, &a.clone(), TOLERANCE), Ok(None));
    }

    #[test]
    fn small_channel_differences_are_within_the_tolerance() {
        let a = frame(4, 4, [10, 20, 30, 255]);
        let b = frame(4, 4, [18, 12, 30, 255]);
        let d = compare(&a, &b, TOLERANCE).unwrap().unwrap();
        assert_eq!((d.pixels, d.max_channel), (0, 8));
    }

    #[test]
    fn a_few_differing_pixels_pass_and_more_fail_with_a_red_map() {
        let a = frame(10, 10, [0, 0, 0, 255]);
        let mut b = a.clone();
        // 10 % of the pixels: at the limit.
        for i in 0..10 {
            b.pixels[i * 4] = 200;
        }
        let d = compare(&a, &b, TOLERANCE).unwrap().unwrap();
        assert_eq!((d.pixels, d.total, d.max_channel), (10, 100, 200));
        b.pixels[10 * 4 + 1] = 100;
        let Err(Mismatch::Pixels(d)) = compare(&a, &b, TOLERANCE) else {
            panic!("11 % should fail");
        };
        assert_eq!(d.pixels, 11);
        assert_eq!(&d.image.pixels[..4], &[200, 0, 0, 255]);
        assert_eq!(&d.image.pixels[10 * 4..11 * 4], &[128, 0, 0, 255]);
        assert_eq!(&d.image.pixels[11 * 4..12 * 4], &[0, 0, 0, 255]);
    }

    #[test]
    fn frames_of_other_sizes_do_not_match() {
        let a = frame(4, 4, [0; 4]);
        let b = frame(4, 5, [0; 4]);
        assert!(matches!(
            compare(&a, &b, TOLERANCE),
            Err(Mismatch::Size { .. })
        ));
    }
}
