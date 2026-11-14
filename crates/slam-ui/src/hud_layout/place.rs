//! Where a layout element lands in a window: its anchor, origin, offset, scale and
//! rotation as one affine map from the element's own pixels to the window's.
//!
//! An element draws itself with its box's top-left corner at (0, 0), already at the
//! window's size ([`unit_scale`] pixels per screen unit), then [`LayoutElement::transform`]
//! moves it into place. The editor (M5) uses the same map.

use slam_render::sprite::Affine;

use super::{Anchor, LayoutElement};

/// The screen the offsets are given in, as stable's HUD and lazer's HUD JSON.
pub const SCREEN_UNITS: [f32; 2] = [640.0, 480.0];

/// Window pixels per screen unit: the 640x480 screen fitted inside the window.
pub fn unit_scale(width: f32, height: f32) -> f32 {
    (width / SCREEN_UNITS[0]).min(height / SCREEN_UNITS[1])
}

impl Anchor {
    /// The point as a fraction of a rectangle's size: `[0, 0]` top left, `[1, 1]` bottom
    /// right.
    pub fn fraction(self) -> [f32; 2] {
        match self {
            Self::TopLeft => [0.0, 0.0],
            Self::TopCentre => [0.5, 0.0],
            Self::TopRight => [1.0, 0.0],
            Self::CentreLeft => [0.0, 0.5],
            Self::Centre => [0.5, 0.5],
            Self::CentreRight => [1.0, 0.5],
            Self::BottomLeft => [0.0, 1.0],
            Self::BottomCentre => [0.5, 1.0],
            Self::BottomRight => [1.0, 1.0],
        }
    }
}

impl LayoutElement {
    /// The element at the in-game interface scale `factor` (ADR-0118): bigger or smaller
    /// about the screen point it is placed from, so an element at an edge stays there.
    /// Its scale and offset are multiplied; an attached element follows the one it is
    /// attached to (ADR-0112).
    pub fn scaled_by(&self, factor: f32) -> LayoutElement {
        let mut element = self.clone();
        element.scale = self.scale.map(|s| s * factor);
        element.position = self.position.map(|p| p * factor);
        element
    }

    /// The map from the element's own pixels (its box of `size` pixels with the top-left
    /// corner at (0, 0)) to a window of `width` × `height` pixels: the element's origin
    /// lands on the window's anchor point plus the offset; the scale (negative mirrors) and
    /// the rotation (degrees, clockwise on screen) turn the element about its origin, as a
    /// drawable's in osu-framework.
    pub fn transform(&self, width: f32, height: f32, size: [f32; 2]) -> Affine {
        let anchor = self.anchor.fraction();
        self.transform_from([anchor[0] * width, anchor[1] * height], width, height, size)
    }

    /// [`Self::transform`] with the origin placed from `point` (window pixels) instead of
    /// the screen's anchor point: for an element attached to another one (ADR-0112).
    pub fn transform_from(
        &self,
        point: [f32; 2],
        width: f32,
        height: f32,
        size: [f32; 2],
    ) -> Affine {
        let unit = unit_scale(width, height);
        let origin = self.origin.fraction();
        let at = [
            point[0] + self.position[0] * unit,
            point[1] + self.position[1] * unit,
        ];
        let pivot = [origin[0] * size[0], origin[1] * size[1]];
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let [sx, sy] = self.scale;
        let (a, b, c, d) = (cos * sx, sin * sx, -sin * sy, cos * sy);
        Affine {
            a,
            b,
            c,
            d,
            tx: at[0] - (a * pivot[0] + c * pivot[1]),
            ty: at[1] - (b * pivot[0] + d * pivot[1]),
        }
    }
}
