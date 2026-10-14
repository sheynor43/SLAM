//! The beatmap background behind gameplay: dimmed by the user's setting and lightened in
//! breaks, as lazer's `BackgroundScreenBeatmap` with its `DimmableBackground`.

use slam_render::atlas::AtlasRegion;
use slam_render::hal::Color;
use slam_render::sprite::{Sprite, SpriteBatch};

use crate::circles::{to_linear, to_srgb};
use crate::session::PlaySnapshot;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Containers/UserDimContainer.cs (BREAK_LIGHTEN_AMOUNT)
/// How much less the background dims in breaks.
pub const BREAK_LIGHTEN_AMOUNT: f32 = 0.3;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Containers/UserDimContainer.cs (BACKGROUND_FADE_DURATION)
/// How long the dim takes to change, in milliseconds of real time.
pub const BACKGROUND_FADE_DURATION: f64 = 800.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Backgrounds/BackgroundScreenBeatmap.cs (load)
/// How long a loaded background fades in, in milliseconds of real time.
pub const BACKGROUND_FADE_IN: f64 = 500.0;

const NS_PER_MS: f64 = 1_000_000.0;

/// The user's background settings: lazer's `DimLevel` and `LightenDuringBreaks`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackgroundSettings {
    /// How dark the background is, 0 (as is) ..= 1 (black).
    pub dim_level: f64,
    /// Whether the background dims [`BREAK_LIGHTEN_AMOUNT`] less in breaks.
    pub lighten_during_breaks: bool,
}

impl Default for BackgroundSettings {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Configuration/OsuConfigManager.cs (InitialiseDefaults)
    fn default() -> Self {
        BackgroundSettings {
            dim_level: 0.7,
            lighten_during_breaks: true,
        }
    }
}

impl BackgroundSettings {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Containers/UserDimContainer.cs (DimLevel, breakLightening)
    /// The dim in gameplay, with user settings applied (not ignored, as the player loader
    /// does) and no storyboard replacing the background.
    pub fn dim_level(&self, is_break_time: bool) -> f32 {
        let lightening = if self.lighten_during_breaks && is_break_time {
            BREAK_LIGHTEN_AMOUNT
        } else {
            0.0
        };
        (self.dim_level as f32 - lightening).max(0.0)
    }
}

/// A grey level's change: osu-framework's `FadeColour` with `Easing.OutQuint`, interpolated
/// in linear space.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fade {
    from: f32,
    to: f32,
    start_ns: u64,
}

impl Fade {
    // Ported from osu-framework 2026.921.1: osu.Framework/Utils/Interpolation.cs (ValueAt for Color4)
    fn value_at(&self, now_ns: u64) -> f32 {
        let current = now_ns.saturating_sub(self.start_ns) as f64 / NS_PER_MS;
        if current >= BACKGROUND_FADE_DURATION {
            return self.to;
        }
        if self.from == self.to || current == 0.0 {
            return self.from;
        }
        let t = (out_quint(current / BACKGROUND_FADE_DURATION) as f32).clamp(0.0, 1.0);
        let (a, b) = (to_linear(self.from), to_linear(self.to));
        to_srgb(a + t * (b - a))
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/DefaultEasingFunction.cs (OutQuint)
fn out_quint(time: f64) -> f64 {
    let time = time - 1.0;
    time * time * time * time * time + 1.0
}

/// Draws the background: a sprite covering the window (`FillMode.Fill`, centred), tinted
/// `Gray(1 - dim)` and faded in once the texture is ready.
///
/// The dim follows [`PlaySnapshot::is_break_time`] in real time: every change starts an
/// [`BACKGROUND_FADE_DURATION`] fade from the current grey, as `UserDimContainer.UpdateVisuals`
/// does. The first frame starts from no dim, as the player loader leaves the background
/// (user settings ignored). No blur, parallax or storyboard yet. Does not allocate.
#[derive(Debug, Clone)]
pub struct BackgroundView {
    settings: BackgroundSettings,
    /// The colour fade and the break state it was started for; `None` before the first frame.
    fade: Option<(Fade, bool)>,
    /// When the texture was first seen ready.
    appeared_ns: Option<u64>,
}

impl BackgroundView {
    pub fn new(settings: BackgroundSettings) -> BackgroundView {
        BackgroundView {
            settings,
            fade: None,
            appeared_ns: None,
        }
    }

    /// The background's grey level at `now_ns`, after taking the snapshot's break state.
    pub fn update(&mut self, snapshot: &PlaySnapshot, now_ns: u64) -> f32 {
        let is_break_time = snapshot.is_break_time;
        let current = match self.fade {
            Some((fade, _)) => fade.value_at(now_ns),
            None => 1.0,
        };
        if self
            .fade
            .is_none_or(|(_, was_break)| was_break != is_break_time)
        {
            let fade = Fade {
                from: current,
                to: 1.0 - self.settings.dim_level(is_break_time),
                start_ns: now_ns,
            };
            self.fade = Some((fade, is_break_time));
        }
        // The fade was just started if anything changed: its value now is `current`.
        current
    }

    /// Pushes the background for a `width`×`height` window at `now_ns`. `texture` is the
    /// background's region, `None` while it loads or when there is none (nothing is drawn:
    /// the window's clear colour shows).
    pub fn push(
        &mut self,
        batch: &mut SpriteBatch,
        width: f32,
        height: f32,
        snapshot: &PlaySnapshot,
        now_ns: u64,
        texture: Option<AtlasRegion>,
    ) {
        let grey = self.update(snapshot, now_ns);
        let Some(region) = texture else {
            return;
        };
        let appeared = *self.appeared_ns.get_or_insert(now_ns);
        let alpha = fade_in(now_ns.saturating_sub(appeared) as f64 / NS_PER_MS);
        let (position, size) = cover(
            [region.size.width as f32, region.size.height as f32],
            [width, height],
        );
        let mut sprite = Sprite::new(region.texture, region.uv, position, size);
        sprite.colour = Color {
            r: grey,
            g: grey,
            b: grey,
            a: alpha,
        };
        batch.push(&sprite);
    }

    /// Restarts the fade-in, as for a newly loaded background.
    pub fn reset(&mut self) {
        self.appeared_ns = None;
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Utils/Interpolation.cs (ValueAt for float)
/// `FadeInFromZero(BACKGROUND_FADE_IN, Easing.OutQuint)` after `current` ms.
fn fade_in(current: f64) -> f32 {
    if current >= BACKGROUND_FADE_IN {
        return 1.0;
    }
    out_quint(current / BACKGROUND_FADE_IN) as f32
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Drawable.cs (FillMode.Fill)
/// Position and size of an `image`-sized sprite covering `frame`, centred, keeping its
/// aspect ratio (cropped on one axis).
fn cover(image: [f32; 2], frame: [f32; 2]) -> ([f32; 2], [f32; 2]) {
    if image[0] <= 0.0 || image[1] <= 0.0 {
        return ([0.0, 0.0], frame);
    }
    let scale = (frame[0] / image[0]).max(frame[1] / image[1]);
    let size = [image[0] * scale, image[1] * scale];
    let position = [(frame[0] - size[0]) / 2.0, (frame[1] - size[1]) / 2.0];
    (position, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn snapshot(is_break_time: bool) -> PlaySnapshot {
        PlaySnapshot {
            is_break_time,
            ..PlaySnapshot::default()
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn dim_is_lightened_in_breaks_and_never_negative() {
        let settings = BackgroundSettings::default();
        assert!(close(settings.dim_level(false), 0.7));
        assert!(close(settings.dim_level(true), 0.4));
        let off = BackgroundSettings {
            lighten_during_breaks: false,
            ..settings
        };
        assert!(close(off.dim_level(true), 0.7));
        let low = BackgroundSettings {
            dim_level: 0.2,
            lighten_during_breaks: true,
        };
        assert_eq!(low.dim_level(true), 0.0);
        assert!(close(low.dim_level(false), 0.2));
    }

    #[test]
    fn the_first_frame_fades_from_no_dim_to_the_break_dim() {
        let mut view = BackgroundView::new(BackgroundSettings::default());
        let t0 = 1_000 * MS;
        // The play starts in a break (the intro).
        assert_eq!(view.update(&snapshot(true), t0), 1.0);
        let half = view.update(&snapshot(true), t0 + 400 * MS);
        assert!(half < 1.0 && half > 0.6, "{half}");
        assert!(close(view.update(&snapshot(true), t0 + 800 * MS), 0.6));
        assert!(close(view.update(&snapshot(true), t0 + 5_000 * MS), 0.6));
    }

    #[test]
    fn the_fade_is_out_quint_in_linear_space() {
        let fade = Fade {
            from: 1.0,
            to: 0.3,
            start_ns: 0,
        };
        let t = out_quint(0.25) as f32;
        let expected = to_srgb(1.0 + t * (to_linear(0.3) - 1.0));
        assert!(close(fade.value_at(200 * MS), expected));
        // OutQuint is mostly done after a quarter: 76 % of the way in linear space.
        let value = fade.value_at(200 * MS);
        assert!(value > 0.55 && value < 0.6, "{value}");
    }

    #[test]
    fn a_change_mid_fade_restarts_from_the_current_grey() {
        let mut view = BackgroundView::new(BackgroundSettings::default());
        view.update(&snapshot(true), 0);
        view.update(&snapshot(true), 800 * MS);
        // The break ends: dim back to 0.7.
        assert!(close(view.update(&snapshot(false), 1_000 * MS), 0.6));
        let mid = view.update(&snapshot(false), 1_100 * MS);
        assert!(mid < 0.6 && mid > 0.3, "{mid}");
        // A break again mid-fade: the new fade starts where the old one is.
        assert_eq!(view.update(&snapshot(true), 1_100 * MS), mid);
        let back = view.update(&snapshot(true), 1_200 * MS);
        assert!(back > mid, "{back} after {mid}");
        assert!(close(view.update(&snapshot(true), 1_900 * MS), 0.6));
    }

    #[test]
    fn the_background_covers_the_window_centred() {
        // 16:9 image in a 4:3 window: fits the height, crops the sides.
        let (position, size) = cover([1920.0, 1080.0], [1024.0, 768.0]);
        assert!(close(size[1], 768.0));
        assert!(close(size[0], 768.0 * 16.0 / 9.0));
        assert!(close(position[0], (1024.0 - size[0]) / 2.0));
        assert_eq!(position[1], 0.0);
        // A tall image in a wide window: fits the width.
        let (position, size) = cover([800.0, 1200.0], [1600.0, 900.0]);
        assert!(close(size[0], 1600.0));
        assert!(close(position[1], (900.0 - 2400.0) / 2.0));
    }

    #[test]
    fn a_ready_background_fades_in() {
        assert_eq!(fade_in(0.0), 0.0);
        assert!(fade_in(100.0) > 0.5);
        assert_eq!(fade_in(500.0), 1.0);
    }
}
