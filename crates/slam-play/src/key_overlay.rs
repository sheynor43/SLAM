//! The key overlay (ADR-0113): stable's four keys K1, K2, M1, M2 on the skin's
//! `inputoverlay-background` and `inputoverlay-key`, laid out as lazer's
//! `LegacyKeyCounterDisplay`, animated as stable (McOsu) or as lazer's `LegacyKeyCounter`.
//!
//! Update side: [`slot`] names the key of an input; the session counts presses into
//! [`KeyValues`]. Draw side: [`KeyOverlay`] follows the values each frame and draws the keys.
//! Times are the HUD's clock in milliseconds. Allocation-free.

use slam_osu::replay::Actions;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};

use crate::circles::SkinSprite;
use crate::input::Input;

/// Keys of the overlay: K1, K2, M1, M2.
pub const KEYS: usize = 4;
/// The keys' captions before their first press: key names, not translated text.
pub const LABELS: [&str; KEYS] = ["K1", "K2", "M1", "M2"];

/// The overlay key of `input` pressing `actions`: the keyboard's or the mouse's, for the left
/// or right action. Smoke has none.
pub fn slot(input: Input, actions: Actions) -> Option<usize> {
    let side = if actions.contains(Actions::LEFT) {
        0
    } else if actions.contains(Actions::RIGHT) {
        1
    } else {
        return None;
    };
    match input {
        Input::Key(_) => Some(side),
        Input::Mouse(_) => Some(2 + side),
    }
}

/// The keys as gameplay last saw them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyValues {
    /// Presses ever, for the animation.
    pub presses: [u32; KEYS],
    /// Presses counted: during gameplay, outside breaks and the skippable intro.
    pub counts: [u32; KEYS],
    pub held: [bool; KEYS],
}

/// How the keys animate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyAnimation {
    /// McOsu's `animateInputoverlay`: every press shrinks the key from full size, the colour
    /// fades after the release.
    #[default]
    Stable,
    /// Lazer's `LegacyKeyCounter`: the key eases to its pressed size and back, the colour
    /// switches at once.
    Lazer,
}

// Ported from McOsu db2add2: src/App/Osu/OsuHUD.cpp (osu_hud_inputoverlay_anim_*)
const STABLE_SCALE_DURATION: f64 = 160.0;
const STABLE_PRESSED_SCALE: f32 = 0.8;
const STABLE_COLOUR_DURATION: f64 = 100.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyKeyCounter.cs
const LAZER_DURATION: f64 = 160.0;
const LAZER_PRESSED_SCALE: f32 = 0.75;
/// The keys' frame, the longest side of the default skin's key.
pub const KEY_SIZE: f32 = 46.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyKeyCounterDisplay.cs
const KEYS_TOP: f32 = 7.0;
const KEY_SPACING: f32 = 1.2;
const BACKGROUND_SCALE_X: f32 = 1.05;
const BACKGROUND_X: f32 = 0.5;
/// K1 and K2, M1 and M2.
const KEYBOARD_COLOUR: Color = Color {
    r: 1.0,
    g: 222.0 / 255.0,
    b: 0.0,
    a: 1.0,
};
const MOUSE_COLOUR: Color = Color {
    r: 248.0 / 255.0,
    g: 0.0,
    b: 158.0 / 255.0,
    a: 1.0,
};

/// A value moving from `from` to `to` over `duration` from `start`, eased.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tween {
    start: f64,
    duration: f64,
    from: f32,
    to: f32,
    /// Quadratic out (`Easing.Out`, McOsu's `moveQuadOut`), or linear.
    quad_out: bool,
}

impl Tween {
    fn still(value: f32) -> Tween {
        Tween {
            start: f64::NEG_INFINITY,
            duration: 0.0,
            from: value,
            to: value,
            quad_out: false,
        }
    }

    fn at(&self, time: f64) -> f32 {
        if time <= self.start {
            return self.from;
        }
        if self.duration <= 0.0 || time >= self.start + self.duration {
            return self.to;
        }
        let t = (time - self.start) / self.duration;
        let t = if self.quad_out { t * (2.0 - t) } else { t } as f32;
        self.from + (self.to - self.from) * t
    }

    fn end(&self) -> f64 {
        self.start + self.duration
    }
}

/// One key's animation.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    scale: Tween,
    /// A scale tween queued behind `scale`: it takes over from its start (stable's release
    /// waits for the press's shrink).
    next: Option<Tween>,
    /// 0 idle, 1 pressed colour.
    colour: Tween,
    held: bool,
}

impl Key {
    const IDLE: Key = Key {
        scale: Tween {
            start: f64::NEG_INFINITY,
            duration: 0.0,
            from: 1.0,
            to: 1.0,
            quad_out: false,
        },
        next: None,
        colour: Tween {
            start: f64::NEG_INFINITY,
            duration: 0.0,
            from: 0.0,
            to: 0.0,
            quad_out: false,
        },
        held: false,
    };

    // Ported from McOsu db2add2: src/App/Osu/OsuHUD.cpp (animateInputoverlay)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyKeyCounter.cs (Activate, Deactivate)
    fn scale_at(&self, time: f64) -> f32 {
        match self.next {
            Some(next) if time >= next.start => next.at(time),
            _ => self.scale.at(time),
        }
    }

    fn press(&mut self, animation: KeyAnimation, time: f64) {
        self.held = true;
        let from = self.scale_at(time);
        self.next = None;
        match animation {
            KeyAnimation::Stable => {
                self.scale = Tween {
                    start: time,
                    duration: STABLE_SCALE_DURATION,
                    from: 1.0,
                    to: STABLE_PRESSED_SCALE,
                    quad_out: true,
                };
            }
            KeyAnimation::Lazer => {
                self.scale = Tween {
                    start: time,
                    duration: LAZER_DURATION,
                    from,
                    to: LAZER_PRESSED_SCALE,
                    quad_out: true,
                };
            }
        }
        self.colour = Tween::still(1.0);
    }

    fn release(&mut self, animation: KeyAnimation, time: f64) {
        self.held = false;
        match animation {
            KeyAnimation::Stable => {
                // McOsu: the release runs once the press's shrink is over (a delayed
                // `moveQuadOut` that leaves the shrink running), faster the later that is;
                // the colour fades at once.
                let remaining = (self.scale.end() - time).max(0.0);
                let duration = STABLE_SCALE_DURATION - (remaining * 1.4).min(STABLE_SCALE_DURATION);
                let start = time + remaining;
                self.next = Some(Tween {
                    start,
                    duration,
                    from: self.scale.at(start),
                    to: 1.0,
                    quad_out: true,
                });
                self.colour = Tween {
                    start: time,
                    duration: STABLE_COLOUR_DURATION,
                    from: self.colour.at(time),
                    to: 0.0,
                    quad_out: false,
                };
            }
            KeyAnimation::Lazer => {
                self.scale = Tween {
                    start: time,
                    duration: LAZER_DURATION,
                    from: self.scale_at(time),
                    to: 1.0,
                    quad_out: true,
                };
                self.next = None;
                self.colour = Tween::still(0.0);
            }
        }
    }
}

/// The key's pose at a moment: its scale and how far its colour is towards the pressed one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyPose {
    pub scale: f32,
    pub colour: f32,
}

/// The skin textures of the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct KeyOverlayTextures {
    pub background: Option<SkinSprite>,
    pub key: Option<SkinSprite>,
}

/// The draw side of the key overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyOverlay {
    animation: KeyAnimation,
    keys: [Key; KEYS],
    /// The presses last seen; `None` before the first frame.
    presses: Option<[u32; KEYS]>,
    counts: [u32; KEYS],
}

impl KeyOverlay {
    pub fn new(animation: KeyAnimation) -> Self {
        KeyOverlay {
            animation,
            keys: [Key::IDLE; KEYS],
            presses: None,
            counts: [0; KEYS],
        }
    }

    /// Takes `values` at `time`. A press since the last frame animates even when the key is
    /// released again already. Call once a frame, with times that do not go back.
    pub fn update(&mut self, values: &KeyValues, time: f64) {
        let seen = self.presses.unwrap_or([0; KEYS]);
        for (i, key) in self.keys.iter_mut().enumerate() {
            let (now, before) = (values.presses[i], seen[i]);
            // A count going back is a new attempt: pressed if it pressed at all.
            let pressed = if now < before { now > 0 } else { now > before };
            if pressed {
                key.press(self.animation, time);
            }
            if key.held && !values.held[i] {
                key.release(self.animation, time);
            } else if !key.held && values.held[i] {
                key.press(self.animation, time);
            }
        }
        self.presses = Some(values.presses);
        self.counts = values.counts;
    }

    pub fn animation(&self) -> KeyAnimation {
        self.animation
    }

    /// The presses counted per key.
    pub fn counts(&self) -> [u32; KEYS] {
        self.counts
    }

    /// Key `i`'s pose at `time`.
    pub fn pose(&self, i: usize, time: f64) -> KeyPose {
        KeyPose {
            scale: self.keys[i].scale_at(time),
            colour: self.keys[i].colour.at(time).clamp(0.0, 1.0),
        }
    }
}

/// The overlay's box at a scale of 1 with `textures`: the keys' column and the turned
/// background, whichever reaches further.
pub fn size(textures: &KeyOverlayTextures) -> [f32; 2] {
    let keys_height = KEYS_TOP + KEYS as f32 * KEY_SIZE + (KEYS - 1) as f32 * KEY_SPACING;
    let (bg_width, bg_height) = textures
        .background
        .map_or((0.0, 0.0), |b| (b.size.x * BACKGROUND_SCALE_X, b.size.y));
    // Turned a quarter clockwise about its top left, which sits half a pixel right of the
    // box's top right: it reaches its height left of there and its width down.
    [
        KEY_SIZE.max(bg_height - BACKGROUND_X),
        keys_height.max(bg_width),
    ]
}

/// The centre of key `i` in a box `width` wide, at a scale of 1.
pub fn key_centre(width: f32, i: usize) -> [f32; 2] {
    [
        width - KEY_SIZE / 2.0,
        KEYS_TOP + KEY_SIZE / 2.0 + i as f32 * (KEY_SIZE + KEY_SPACING),
    ]
}

/// The pressed colour of key `i`.
fn active_colour(i: usize) -> Color {
    if i < 2 { KEYBOARD_COLOUR } else { MOUSE_COLOUR }
}

impl KeyOverlay {
    /// Writes the background and the keys at `time` with the box's top left at (0, 0), each
    /// pixel of lazer's HUD `scale` window pixels.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyKeyCounterDisplay.cs (load)
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        textures: &KeyOverlayTextures,
        scale: f32,
        time: f64,
    ) {
        let [width, _] = size(textures);
        if let Some(bg) = textures.background {
            let mut sprite = Sprite::new(
                bg.texture,
                bg.uv,
                [(width + BACKGROUND_X) * scale, 0.0],
                [bg.size.x * BACKGROUND_SCALE_X * scale, bg.size.y * scale],
            );
            sprite.rotation = std::f32::consts::FRAC_PI_2;
            batch.push(&sprite);
        }
        let Some(key) = textures.key else {
            return;
        };
        for i in 0..KEYS {
            let pose = self.pose(i, time);
            let [cx, cy] = key_centre(width, i);
            let active = active_colour(i);
            let mix = |idle: f32, on: f32| idle + (on - idle) * pose.colour;
            let w = key.size.x * pose.scale * scale;
            let h = key.size.y * pose.scale * scale;
            let mut sprite = Sprite::new(
                key.texture,
                key.uv,
                [cx * scale - w / 2.0, cy * scale - h / 2.0],
                [w, h],
            );
            sprite.colour = Color {
                r: mix(1.0, active.r),
                g: mix(1.0, active.g),
                b: mix(1.0, active.b),
                a: 1.0,
            };
            sprite.blend = Blend::Alpha;
            batch.push(&sprite);
        }
    }
}

/// The most sprites [`KeyOverlay::push`] writes.
pub const MAX_KEY_OVERLAY_SPRITES: usize = KEYS + 1;

#[cfg(test)]
mod tests {
    use super::*;
    use slam_input::MouseButton;

    #[test]
    fn keys_split_keyboard_and_mouse_left_and_right() {
        assert_eq!(slot(Input::Key(29), Actions::LEFT), Some(0));
        assert_eq!(slot(Input::Key(27), Actions::RIGHT), Some(1));
        assert_eq!(
            slot(Input::Mouse(MouseButton::Left), Actions::LEFT),
            Some(2)
        );
        assert_eq!(
            slot(Input::Mouse(MouseButton::Right), Actions::RIGHT),
            Some(3)
        );
        assert_eq!(slot(Input::Key(6), Actions::SMOKE), None);
    }

    fn values(presses: u32, held: bool) -> KeyValues {
        KeyValues {
            presses: [presses, 0, 0, 0],
            counts: [presses, 0, 0, 0],
            held: [held, false, false, false],
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn stable_shrinks_every_press_and_fades_the_colour_after_the_release() {
        let mut overlay = KeyOverlay::new(KeyAnimation::Stable);
        overlay.update(&values(0, false), 0.0);
        overlay.update(&values(1, true), 100.0);
        assert_eq!(overlay.pose(0, 100.0).scale, 1.0);
        // Quad out to 0.8 over 160 ms: three quarters of the way half way through.
        assert!(close(overlay.pose(0, 180.0).scale, 1.0 - 0.2 * 0.75));
        assert_eq!(overlay.pose(0, 180.0).colour, 1.0);
        // Released 40 ms into the shrink: it goes on shrinking, then grows back over
        // 160 - 1.4 * 120 < 0 → at once.
        overlay.update(&values(1, false), 140.0);
        let shrunk = |t: f32| 1.0 - 0.2 * t * (2.0 - t);
        assert!(close(overlay.pose(0, 141.0).scale, shrunk(41.0 / 160.0)));
        assert!(close(overlay.pose(0, 259.0).scale, shrunk(159.0 / 160.0)));
        assert_eq!(overlay.pose(0, 261.0).scale, 1.0);
        // The colour fades over 100 ms.
        assert!(close(overlay.pose(0, 190.0).colour, 0.5));
        assert_eq!(overlay.pose(0, 240.0).colour, 0.0);
        // A tap between two frames still shows: the whole shrink, then back at once.
        overlay.update(&values(2, false), 1000.0);
        assert_eq!(overlay.pose(0, 1000.0).colour, 1.0);
        assert!(close(overlay.pose(0, 1050.0).colour, 0.5));
        assert!(close(overlay.pose(0, 1080.0).scale, 0.85));
        assert_eq!(overlay.pose(0, 1161.0).scale, 1.0);
        assert_eq!(overlay.counts()[0], 2);
        // A new attempt counts from zero: a press animates.
        overlay.update(&values(1, false), 2000.0);
        assert_eq!(overlay.pose(0, 2000.0).colour, 1.0);
    }

    #[test]
    fn a_press_drops_the_release_waiting_for_the_shrink() {
        let mut overlay = KeyOverlay::new(KeyAnimation::Stable);
        overlay.update(&values(0, false), 0.0);
        overlay.update(&values(1, true), 0.0);
        overlay.update(&values(1, false), 20.0);
        // Pressed again before the release starts: shrinking from full size again.
        overlay.update(&values(2, true), 40.0);
        assert_eq!(overlay.pose(0, 40.0).scale, 1.0);
        assert!(close(overlay.pose(0, 200.0).scale, 0.8));
        assert!(close(overlay.pose(0, 300.0).scale, 0.8));

        // Lazer presses from where the release got to.
        let mut overlay = KeyOverlay::new(KeyAnimation::Lazer);
        overlay.update(&values(0, false), 0.0);
        overlay.update(&values(1, true), 0.0);
        overlay.update(&values(1, false), 200.0);
        let mid = overlay.pose(0, 280.0).scale;
        overlay.update(&values(2, true), 280.0);
        assert!(close(overlay.pose(0, 280.0).scale, mid));
    }

    #[test]
    fn stable_released_after_the_shrink_grows_back_over_the_full_time() {
        let mut overlay = KeyOverlay::new(KeyAnimation::Stable);
        overlay.update(&values(0, false), 0.0);
        overlay.update(&values(1, true), 0.0);
        overlay.update(&values(1, false), 500.0);
        assert!(close(overlay.pose(0, 500.0).scale, 0.8));
        assert!(close(overlay.pose(0, 580.0).scale, 0.8 + 0.2 * 0.75));
        assert_eq!(overlay.pose(0, 660.0).scale, 1.0);
    }

    #[test]
    fn lazer_eases_both_ways_and_switches_the_colour() {
        let mut overlay = KeyOverlay::new(KeyAnimation::Lazer);
        overlay.update(&values(0, false), 0.0);
        overlay.update(&values(1, true), 0.0);
        assert_eq!(overlay.pose(0, 0.0).colour, 1.0);
        assert!(close(overlay.pose(0, 80.0).scale, 1.0 - 0.25 * 0.75));
        overlay.update(&values(1, false), 80.0);
        assert_eq!(overlay.pose(0, 80.0).colour, 0.0);
        // Back from where it was.
        let from = 1.0 - 0.25 * 0.75;
        assert!(close(
            overlay.pose(0, 160.0).scale,
            from + (1.0 - from) * 0.75
        ));
        // A tap between two frames barely shows.
        overlay.update(&values(2, false), 1000.0);
        assert_eq!(overlay.pose(0, 1000.0).colour, 0.0);
        assert_eq!(overlay.pose(0, 1050.0).scale, 1.0);
    }

    #[test]
    fn the_box_fits_the_turned_background_and_the_keys() {
        let textures = KeyOverlayTextures {
            background: Some(SkinSprite {
                texture: slam_render::textures::TextureId(1),
                uv: slam_render::atlas::UvRect::FULL,
                size: slam_osu::Vec2::new(193.0, 55.0),
            }),
            key: None,
        };
        let [w, h] = size(&textures);
        assert!(close(w, 54.5));
        assert!(close(h, 202.65));
        assert_eq!(key_centre(w, 0), [54.5 - 23.0, 30.0]);
        assert!(close(key_centre(w, 3)[1], 30.0 + 3.0 * 47.2));
    }
}
