//! The logo's images (ADR-0186), built into the binary from `assets/logo` (rendered from
//! `assets/icon/slam-logo.svg`) and put into an atlas of their own when the draw thread
//! starts.

use slam_render::atlas::Atlas;
use slam_render::image::{ImageLimits, decode_png};
use slam_render::textures::Textures;
use slam_render::{Device, Extent, TextureFormat};
use slam_ui::menu::LogoArt;
use slam_ui::menu::view::LogoLayers;

/// The masks, large and small, by name (`assets/logo/<name>.png`, `<name>-small.png`).
macro_rules! masks {
    ($($name:ident),*) => {
        [$((
            stringify!($name),
            include_bytes!(concat!("../../../assets/logo/", stringify!($name), ".png")).as_slice(),
            include_bytes!(concat!("../../../assets/logo/", stringify!($name), "-small.png")).as_slice(),
        )),*]
    };
}
const MASKS: [(&str, &[u8], &[u8]); 7] = masks!(disc, shine, ring, frame, play, facet, accent);
const TRIANGLE: &[u8] = include_bytes!("../../../assets/logo/triangle.png");

/// The first page holds the large masks; the small ones and the triangle take a second.
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
    let mut big = [None; 7];
    let mut small = [None; 7];
    for (i, (name, large, little)) in MASKS.iter().enumerate() {
        big[i] = Some(add(name, large)?);
        small[i] = Some(add(name, little)?);
    }
    let layers = |set: [Option<slam_render::atlas::AtlasRegion>; 7]| {
        let [disc, shine, ring, frame, play, facet, accent] = set.map(|r| r.expect("added"));
        LogoLayers {
            disc,
            shine,
            ring,
            frame,
            play,
            facet,
            accent,
        }
    };
    let art = LogoArt {
        big: layers(big),
        small: layers(small),
        triangle: add("triangle", TRIANGLE)?,
    };
    Some((atlas, art))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_images_decode_square() {
        for (name, large, little) in MASKS {
            for (bytes, side) in [(large, 640), (little, 256)] {
                let image = decode_png(bytes, ImageLimits::default()).expect(name);
                assert_eq!(image.size, Extent::new(side, side), "{name}");
            }
        }
        let triangle = decode_png(TRIANGLE, ImageLimits::default()).expect("triangle");
        assert_eq!(triangle.size, Extent::new(128, 128));
    }
}
