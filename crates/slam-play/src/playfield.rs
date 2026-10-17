//! Playfield layout: osu!pixels (512×384) to window coordinates and back.

use slam_osu::Vec2;
use slam_osu::objects::PLAYFIELD_SIZE;

/// Part of the gameplay area the playfield's 4:3 box may take on each axis.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuPlayfieldAdjustmentContainer.cs
// (`playfield_size_adjust`: osu!stable's 512 / 640).
const PLAYFIELD_SIZE_ADJUST: f32 = 0.8;

/// How far the playfield is shifted down to line up with storyboards, in osu!pixels.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuPlayfieldAdjustmentContainer.cs
// (`PlayfieldShift`, which `DrawableOsuRuleset` turns on through `AlignWithStoryboard`).
const STORYBOARD_SHIFT: f32 = 8.0;

/// Where the playfield sits in a window: a uniform scale and an offset.
///
/// Window coordinates are in whatever unit the window size is given in (points for input
/// events, pixels for rendering): the layout is the same up to that scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayfieldLayout {
    /// Window position of the playfield's origin (osu!pixel 0, 0).
    origin: Vec2,
    /// Window units per osu!pixel.
    scale: f32,
    /// The window's size in window units.
    window: [f32; 2],
}

impl PlayfieldLayout {
    /// The layout of a playfield filling a window of `width` × `height`, as lazer lays out
    /// gameplay.
    ///
    /// Lazer scales gameplay uniformly to fit 1024×768 (`ScalingContainer` in gameplay mode,
    /// `DrawSizePreservingFillContainer` with the `Minimum` strategy), takes 0.8 of that on
    /// each axis, fits a 4:3 box into it and scales 512×384 osu!pixels to the box's width. The
    /// uniform 1024×768 scale cancels out, so the box is computed in window units directly.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuPlayfieldAdjustmentContainer.cs
    pub fn new(width: f32, height: f32) -> PlayfieldLayout {
        let area_width = width * PLAYFIELD_SIZE_ADJUST;
        let area_height = height * PLAYFIELD_SIZE_ADJUST;
        // `FillMode.Fit` with `FillAspectRatio = 4f / 3`.
        let box_width = area_width.min(area_height * (4.0 / 3.0));
        let box_height = box_width / (4.0 / 3.0);
        let scale = box_width / PLAYFIELD_SIZE.x;
        // The box is centred; the playfield inside it is shifted down by `8 * Scale`.
        let origin = Vec2::new(
            (width - box_width) / 2.0,
            (height - box_height) / 2.0 + STORYBOARD_SHIFT * scale,
        );
        PlayfieldLayout {
            origin,
            scale,
            window: [width, height],
        }
    }

    /// Window units per osu!pixel.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The window's width and height in window units.
    pub fn window_size(&self) -> [f32; 2] {
        self.window
    }

    /// Window position of osu!pixel (0, 0).
    pub fn origin(&self) -> Vec2 {
        self.origin
    }

    /// Converts a window position to osu!pixels.
    pub fn to_playfield(&self, window: Vec2) -> Vec2 {
        Vec2::new(
            (window.x - self.origin.x) / self.scale,
            (window.y - self.origin.y) / self.scale,
        )
    }

    /// Converts osu!pixels to a window position.
    pub fn to_window(&self, playfield: Vec2) -> Vec2 {
        Vec2::new(
            self.origin.x + playfield.x * self.scale,
            self.origin.y + playfield.y * self.scale,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-3
    }

    #[test]
    fn lazer_reference_size_gives_the_stable_ratio() {
        // At 1024×768, lazer's comment derives a scale of 1.6 and a box of 819.2×614.4.
        let layout = PlayfieldLayout::new(1024.0, 768.0);
        assert!(close(layout.scale(), 1.6));
        assert!(close(layout.origin().x, (1024.0 - 819.2) / 2.0));
        assert!(close(layout.origin().y, (768.0 - 614.4) / 2.0 + 8.0 * 1.6));
    }

    #[test]
    fn stable_window_size_maps_one_to_one() {
        let layout = PlayfieldLayout::new(640.0, 480.0);
        assert_eq!(layout.scale(), 1.0);
        assert_eq!(layout.origin(), Vec2::new(64.0, 56.0));
        assert_eq!(
            layout.to_playfield(Vec2::new(64.0, 56.0)),
            Vec2::new(0.0, 0.0)
        );
        assert_eq!(
            layout.to_window(Vec2::new(512.0, 384.0)),
            Vec2::new(576.0, 440.0)
        );
    }

    #[test]
    fn wide_window_is_limited_by_height() {
        let layout = PlayfieldLayout::new(1920.0, 1080.0);
        // 0.8 × 1080 = 864 tall, 1152 wide.
        assert!(close(layout.scale(), 1152.0 / 512.0));
        assert!(close(layout.origin().x, (1920.0 - 1152.0) / 2.0));
        assert!(close(
            layout.origin().y,
            (1080.0 - 864.0) / 2.0 + 8.0 * 2.25
        ));
        let centre = layout.to_window(Vec2::new(256.0, 192.0));
        assert!(close(centre.x, 960.0));
        assert!(close(centre.y, 540.0 + 8.0 * 2.25));
    }

    #[test]
    fn tall_window_is_limited_by_width() {
        let layout = PlayfieldLayout::new(800.0, 1200.0);
        // 0.8 × 800 = 640 wide, 480 tall.
        assert!(close(layout.scale(), 1.25));
        assert!(close(layout.origin().x, 80.0));
        assert!(close(layout.origin().y, (1200.0 - 480.0) / 2.0 + 10.0));
    }

    #[test]
    fn window_and_playfield_round_trip() {
        let layout = PlayfieldLayout::new(1366.0, 768.0);
        for p in [
            Vec2::new(0.0, 0.0),
            Vec2::new(512.0, 384.0),
            Vec2::new(123.25, 300.5),
            Vec2::new(-40.0, 420.0),
        ] {
            let back = layout.to_playfield(layout.to_window(p));
            assert!(
                close(back.x, p.x) && close(back.y, p.y),
                "{p:?} -> {back:?}"
            );
        }
    }
}
