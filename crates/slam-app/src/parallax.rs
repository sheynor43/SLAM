//! Parallax of the menus (ADR-0132): the background and the main menu's buttons shift with
//! the pointer, the background by more than the buttons. Draw thread only: the pointer is
//! read from a [`PointerWatch`], without locks, and nothing here allocates.
//!
//! Hit-testing stays on the unshifted layout: the shift is applied to the quads after the
//! screens laid themselves out.

use slam_input::{PointerWatch, WindowScale};
use slam_render::sprite::Affine;

/// Longest frame a smoothing step counts, in seconds.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Containers/ParallaxContainer.cs
// (`Math.Clamp(Clock.ElapsedFrameTime, 0, parallax_duration)`, 100 ms)
const MAX_STEP_SECONDS: f64 = 0.1;

/// Where the pointer is, relative to the window's middle: `-1` at the left and top edges,
/// `1` at the right and bottom edges, clamped; `[0, 0]` without a pointer in the window.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Containers/ParallaxContainer.cs
// (`ToLocalSpace(mouse) - DrawSize / 2`, relative to half the size). Lazer squashes the
// distance with `Damp(0, 1, 0.999, px)`, which depends on the window's pixel size; here the
// distance is linear, so the corners reach the full shift.
pub fn pointer_target(pointer: Option<[f32; 2]>, size: [f32; 2]) -> [f32; 2] {
    let Some([x, y]) = pointer else {
        return [0.0, 0.0];
    };
    let axis = |p: f32, extent: f32| {
        if extent > 0.0 && p.is_finite() {
            ((p / extent - 0.5) * 2.0).clamp(-1.0, 1.0)
        } else {
            0.0
        }
    };
    [axis(x, size[0]), axis(y, size[1])]
}

/// How far a layer shifts and grows for a smoothed pointer `pointer` (from
/// [`Parallax::update`]) on a surface of `size`, with the layer's `amount` (a part of the
/// half size at full strength) and the setting's `strength`, 0 to 1 (0 is off, and so is
/// effects being off: the caller passes 0 then). The layer shifts with the pointer and is
/// scaled up to cover the edges the shift would show when `cover` is set.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Containers/ParallaxContainer.cs
// (`offset = relativeAmount * sizeDiv2 * amount`, `scale = 1 + |amount|`)
pub fn layer_map(
    pointer: [f32; 2],
    size: [f32; 2],
    amount: f32,
    strength: f32,
    cover: bool,
) -> Affine {
    let amount = amount * strength.clamp(0.0, 1.0);
    if amount == 0.0 {
        return Affine::IDENTITY;
    }
    let offset = [
        pointer[0] * size[0] * 0.5 * amount,
        pointer[1] * size[1] * 0.5 * amount,
    ];
    let scale = if cover { 1.0 + amount.abs() } else { 1.0 };
    Affine::about([size[0] * 0.5, size[1] * 0.5], scale, 0.0, offset)
}

/// `map` with its shift rounded to whole pixels, for text: a glyph moved by a fraction of a
/// pixel is resampled and blurs, while shapes may move smoothly. Only the shift changes.
pub fn snap_to_pixels(map: &Affine) -> Affine {
    Affine {
        tx: map.tx.round(),
        ty: map.ty.round(),
        ..*map
    }
}

/// The smoothed pointer: eases towards the pointer's place in the window and back to the
/// middle while the pointer is outside.
pub struct Parallax {
    pointer: PointerWatch,
    scale: WindowScale,
    current: [f32; 2],
    last_ns: Option<u64>,
}

impl Parallax {
    pub fn new(pointer: PointerWatch, scale: WindowScale) -> Self {
        Parallax {
            pointer,
            scale,
            current: [0.0, 0.0],
            last_ns: None,
        }
    }

    /// The smoothed pointer at `now_ns` on a surface of `size` pixels, eased by the frame's
    /// time: the same curve at any frame rate. `half_life` is in seconds.
    // Ported from osu!lazer 2026.1005.0-lazer: osu-framework/osu.Framework/Utils/Interpolation.cs
    // (`DampContinuously`: `Damp(current, target, 0.5, elapsed / halfTime)`), in place of
    // lazer's per-frame OutQuint step, which depends on the frame rate.
    pub fn update(&mut self, now_ns: u64, size: [f32; 2], half_life: f32) -> [f32; 2] {
        let ppp = self.scale.pixels_per_point();
        let pointer = self.pointer.position().map(|[x, y]| [x * ppp, y * ppp]);
        let target = pointer_target(pointer, size);
        let dt = self.last_ns.map_or(0.0, |last| {
            (now_ns.saturating_sub(last) as f64 / 1e9).min(MAX_STEP_SECONDS)
        });
        self.last_ns = Some(now_ns);
        self.current = ease(self.current, target, dt, f64::from(half_life));
        self.current
    }
}

/// `current` moved towards `target` by `dt` seconds of a curve that halves the distance
/// every `half_life` seconds.
pub fn ease(current: [f32; 2], target: [f32; 2], dt: f64, half_life: f64) -> [f32; 2] {
    let keep = if half_life > 0.0 {
        0.5_f64.powf(dt / half_life) as f32
    } else {
        0.0
    };
    [
        target[0] + (current[0] - target[0]) * keep,
        target[1] + (current[1] - target[1]) * keep,
    ]
}

#[cfg(test)]
mod tests;
