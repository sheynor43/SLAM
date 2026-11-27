//! The logo's images (ADR-0186), built into the binary from `assets/logo` (rendered from
//! `assets/icon/slam-logo.svg`) and put into an atlas of their own when the draw thread
//! starts.

use slam_render::atlas::Atlas;
use slam_render::image::{ImageLimits, decode_png};
use slam_render::textures::Textures;
use slam_render::{Device, Extent, TextureFormat};
use slam_ui::menu::LogoArt;

const DISC: &[u8] = include_bytes!("../../../assets/logo/disc.png");
const FRAME: &[u8] = include_bytes!("../../../assets/logo/frame.png");
const PLAY: &[u8] = include_bytes!("../../../assets/logo/play.png");
const FACET: &[u8] = include_bytes!("../../../assets/logo/facet.png");
const ACCENT: &[u8] = include_bytes!("../../../assets/logo/accent.png");
const SMALL: &[u8] = include_bytes!("../../../assets/logo/small.png");
const TRIANGLE: &[u8] = include_bytes!("../../../assets/logo/triangle.png");

/// One page holds every image (five 640-pixel layers and two small ones).
const PAGE: Extent = Extent::new(2048, 2048);

/// Decodes the images and adds them to a new atlas; `None` (logged) if one fails, and the
/// menu then goes without its logo. Allocates: call it when the draw thread starts.
pub fn upload<D: Device>(
    device: &mut D,
    textures: &mut Textures<D::Texture>,
) -> Option<(Atlas, LogoArt)> {
    let mut atlas = Atlas::new(PAGE, TextureFormat::Rgba8);
    let mut add = |name: &str, bytes: &[u8]| {
        let image = decode_png(bytes, ImageLimits::default())
            .map_err(|e| tracing::error!("logo image {name}: {e}"))
            .ok()?;
        atlas
            .add(device, textures, image.size, &image.pixels)
            .map_err(|e| tracing::error!("logo image {name}: {e}"))
            .ok()
    };
    let art = LogoArt {
        disc: add("disc", DISC)?,
        frame: add("frame", FRAME)?,
        play: add("play", PLAY)?,
        facet: add("facet", FACET)?,
        accent: add("accent", ACCENT)?,
        small: add("small", SMALL)?,
        triangle: add("triangle", TRIANGLE)?,
    };
    Some((atlas, art))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_images_decode_square() {
        for (bytes, side) in [
            (DISC, 640),
            (FRAME, 640),
            (PLAY, 640),
            (FACET, 640),
            (ACCENT, 640),
            (SMALL, 256),
            (TRIANGLE, 128),
        ] {
            let image = decode_png(bytes, ImageLimits::default()).expect("decodes");
            assert_eq!(image.size, Extent::new(side, side));
        }
    }
}
