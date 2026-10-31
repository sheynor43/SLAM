//! The beatmap background behind gameplay: dimmed by the user's setting and lightened in
//! breaks, as lazer's `BackgroundScreenBeatmap` with its `DimmableBackground`.

use slam_render::atlas::AtlasRegion;
use slam_render::hal::{Color, Extent};
use slam_render::image::Image;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::upload::TextureState;

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

    pub fn settings(&self) -> BackgroundSettings {
        self.settings
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

    /// Pushes the background for a `width`×`height` window (`size`) at `now_ns`. `texture` is the
    /// background's region, `None` while it loads or when there is none (nothing is drawn:
    /// the window's clear colour shows). Fully dimmed it is black, so nothing is drawn
    /// either unless `over_layer` (something drawn under it must be hidden): the caller
    /// clears the frame black, as lazer's renderer does, and saves a pass over the whole
    /// window.
    pub fn push(
        &mut self,
        batch: &mut SpriteBatch,
        [width, height]: [f32; 2],
        snapshot: &PlaySnapshot,
        now_ns: u64,
        texture: Option<AtlasRegion>,
        over_layer: bool,
    ) {
        let grey = self.update(snapshot, now_ns);
        let Some(region) = texture else {
            return;
        };
        let appeared = *self.appeared_ns.get_or_insert(now_ns);
        if grey <= 0.0 && !over_layer {
            return;
        }
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

/// The region to draw from the beatmap's background and SLAM's default (ADR-0076), each
/// `None` without a texture: the beatmap's once ready, nothing while it loads, the default
/// without one or when it failed, as lazer's `BeatmapBackground` falls back to
/// `Backgrounds/bg1` when `GetBackground()` returns `null`.
pub fn shown_region(
    beatmap: Option<TextureState>,
    default: Option<TextureState>,
) -> Option<AtlasRegion> {
    match beatmap {
        Some(TextureState::Ready(region)) => Some(region),
        Some(TextureState::Pending) => None,
        Some(TextureState::Failed(_)) | None => match default {
            Some(TextureState::Ready(region)) => Some(region),
            _ => None,
        },
    }
}

/// The default background's size: small, as the sampler's linear filter smooths it out.
pub const DEFAULT_IMAGE_SIZE: Extent = Extent {
    width: 256,
    height: 144,
};

/// The default background's base, sRGB: a dark indigo.
const DEFAULT_BASE: [f32; 3] = [0.07, 0.06, 0.16];

/// A soft patch of colour on the default background.
struct Glow {
    /// sRGB.
    colour: [f32; 3],
    /// Centre, in fractions of the image's width and height.
    centre: [f32; 2],
    /// Radius along and across the streaks, in fractions of the image's height.
    radius: [f32; 2],
    /// Opacity at the centre.
    strength: f32,
}

/// The patches of the default background, bottom first.
const DEFAULT_GLOWS: [Glow; 6] = [
    Glow {
        colour: [0.13, 0.25, 0.62],
        centre: [0.22, 0.2],
        radius: [0.75, 0.45],
        strength: 0.75,
    },
    Glow {
        colour: [0.0, 0.74, 0.93],
        centre: [0.55, 0.08],
        radius: [0.8, 0.38],
        strength: 0.95,
    },
    Glow {
        colour: [0.55, 0.22, 0.42],
        centre: [0.92, 0.62],
        radius: [0.7, 0.45],
        strength: 0.7,
    },
    Glow {
        colour: [0.62, 0.1, 0.18],
        centre: [0.38, 1.0],
        radius: [0.7, 0.4],
        strength: 0.85,
    },
    Glow {
        colour: [0.93, 0.42, 0.22],
        centre: [0.55, 0.72],
        radius: [0.55, 0.26],
        strength: 0.9,
    },
    Glow {
        colour: [0.95, 0.72, 0.45],
        centre: [0.6, 0.52],
        radius: [0.35, 0.14],
        strength: 0.4,
    },
];

/// Angle of the streaks from the horizontal, clockwise on screen (rising to the right).
const STREAK_ANGLE: f32 = -1.05;

/// SLAM's own background for beatmaps without one (ADR-0076), in place of lazer's
/// `Backgrounds/bg1` (osu-resources, under a non-commercial licence): soft patches of
/// cyan, orange, red and purple streaked diagonally over dark indigo. `Rgba8`, opaque,
/// the same on every call (across platforms up to rounding of `exp` and `powf`). Allocates: build it off the hot paths.
pub fn default_image() -> Image {
    let Extent { width, height } = DEFAULT_IMAGE_SIZE;
    let aspect = width as f32 / height as f32;
    let (sin, cos) = STREAK_ANGLE.sin_cos();
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            // Pixel centres, in heights from the top-left corner.
            let point = [
                (x as f32 + 0.5) / height as f32,
                (y as f32 + 0.5) / height as f32,
            ];
            let mut colour = DEFAULT_BASE.map(to_linear);
            for glow in &DEFAULT_GLOWS {
                let d = [
                    point[0] - glow.centre[0] * aspect,
                    point[1] - glow.centre[1],
                ];
                let along = (d[0] * cos + d[1] * sin) / glow.radius[0];
                let across = (-d[0] * sin + d[1] * cos) / glow.radius[1];
                let weight = glow.strength * (-(along * along + across * across)).exp();
                for (c, g) in colour.iter_mut().zip(glow.colour) {
                    *c += (to_linear(g) - *c) * weight;
                }
            }
            for c in colour {
                pixels.push((to_srgb(c).clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            pixels.push(u8::MAX);
        }
    }
    Image {
        size: DEFAULT_IMAGE_SIZE,
        pixels,
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
pub fn cover(image: [f32; 2], frame: [f32; 2]) -> ([f32; 2], [f32; 2]) {
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

    fn region(texture: u32) -> AtlasRegion {
        AtlasRegion {
            texture: slam_render::textures::TextureId(texture),
            origin: slam_render::hal::Origin { x: 0, y: 0 },
            size: DEFAULT_IMAGE_SIZE,
            uv: slam_render::atlas::UvRect {
                u0: 0.0,
                v0: 0.0,
                u1: 1.0,
                v1: 1.0,
            },
        }
    }

    #[test]
    fn the_default_stands_in_for_a_missing_or_failed_background_only() {
        use slam_render::upload::LoadError;
        let beatmap = Some(TextureState::Ready(region(1)));
        let default = Some(TextureState::Ready(region(2)));
        assert_eq!(shown_region(beatmap, default), Some(region(1)));
        assert_eq!(shown_region(None, default), Some(region(2)));
        let failed = Some(TextureState::Failed(LoadError::Decode));
        assert_eq!(shown_region(failed, default), Some(region(2)));
        // Loading: nothing yet, so the default does not flash before the beatmap's.
        assert_eq!(shown_region(Some(TextureState::Pending), default), None);
        // The default not ready or refused: the window's clear colour, as before.
        assert_eq!(shown_region(None, Some(TextureState::Pending)), None);
        assert_eq!(shown_region(None, None), None);
        let default_failed = Some(TextureState::Failed(LoadError::Decode));
        assert_eq!(shown_region(failed, default_failed), None);
    }

    #[test]
    fn a_fully_dimmed_background_is_not_drawn_but_comes_back_in_breaks() {
        let mut view = BackgroundView::new(BackgroundSettings {
            dim_level: 1.0,
            lighten_during_breaks: true,
        });
        let mut batch = SpriteBatch::with_capacity(4);
        let texture = Some(region(1));
        // The first frame starts from no dim.
        view.push(
            &mut batch,
            [1920.0, 1080.0],
            &snapshot(false),
            0,
            texture,
            false,
        );
        assert_eq!(batch.len(), 1);
        batch.clear();
        view.push(
            &mut batch,
            [1920.0, 1080.0],
            &snapshot(false),
            800 * MS,
            texture,
            false,
        );
        assert_eq!(batch.len(), 0);
        // A break lightens it by 0.3: still black as the fade starts, visible after.
        view.push(
            &mut batch,
            [1920.0, 1080.0],
            &snapshot(true),
            900 * MS,
            texture,
            false,
        );
        assert_eq!(batch.len(), 0);
        view.push(
            &mut batch,
            [1920.0, 1080.0],
            &snapshot(true),
            1700 * MS,
            texture,
            false,
        );
        assert_eq!(batch.len(), 1);
        // Dimmed fully again after the break; over another layer it still covers it.
        batch.clear();
        view.push(
            &mut batch,
            [1920.0, 1080.0],
            &snapshot(false),
            1800 * MS,
            texture,
            false,
        );
        view.push(
            &mut batch,
            [1920.0, 1080.0],
            &snapshot(false),
            2600 * MS,
            texture,
            false,
        );
        assert_eq!(batch.len(), 1);
        view.push(
            &mut batch,
            [1920.0, 1080.0],
            &snapshot(false),
            2700 * MS,
            texture,
            true,
        );
        assert_eq!(batch.len(), 2);
    }

    #[test]
    fn default_image_is_opaque_coloured_and_the_same_each_time() {
        let image = default_image();
        assert_eq!(image.size, DEFAULT_IMAGE_SIZE);
        let Extent { width, height } = image.size;
        assert_eq!(image.pixels.len(), width as usize * height as usize * 4);
        assert!(image.pixels.chunks(4).all(|p| p[3] == u8::MAX));
        assert_eq!(image, default_image());
        let at = |x: u32, y: u32| {
            let i = (y * width + x) as usize * 4;
            [image.pixels[i], image.pixels[i + 1], image.pixels[i + 2]]
        };
        // Not one flat colour: cyan above, orange in the lower middle.
        let [r, g, b] = at(width * 11 / 20, 4);
        assert!(b > r && g > r, "top is not cyan: {r} {g} {b}");
        let [r, g, b] = at(width * 11 / 20, height * 3 / 4);
        assert!(r > g && g > b, "lower middle is not orange: {r} {g} {b}");
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
