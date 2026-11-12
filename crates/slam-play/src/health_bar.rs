//! The legacy health bar (`LegacyHealthDisplay`): `scorebar-bg`, the `scorebar-colour` fill
//! (animated with frames) cut to the health, and a marker at the fill's end. A skin with
//! `scorebar-marker` draws the new style (the marker tinted with the fill's colour), otherwise
//! the old one (`scorebar-ki`, `-kidanger` below half health, `-kidanger2` below a fifth). The
//! skin is the first one of the chain with `scorebar-bg`, and every texture comes from it.
//! The style does not depend on the skin's version.
//!
//! Draw side: [`HealthBar::update`] takes the health once a frame and eases the fill towards
//! it; the marker bulges as health rises and flashes on hits. Times are the HUD's clock in
//! milliseconds. Allocation-free.

use slam_render::atlas::UvRect;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color};

use crate::circles::{Easing, SkinSprite};

/// The most `scorebar-colour` frames drawn; later ones are left out.
pub const MAX_FILL_FRAMES: usize = 64;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyHealthDisplay.cs
const EPIC_CUTOFF: f64 = 0.5;
const DANGER_CUTOFF: f64 = 0.2;
/// `LegacySkin.STABLE_MAGIC_SCALE_FACTOR`: stable's 480-high positions in the 768-high HUD.
const STABLE_MAGIC_SCALE_FACTOR: f32 = 1.6;
const OLD_FILL_POSITION: [f32; 2] = [
    3.0 * STABLE_MAGIC_SCALE_FACTOR,
    10.0 * STABLE_MAGIC_SCALE_FACTOR,
];
const NEW_FILL_POSITION: [f32; 2] = [
    7.5 * STABLE_MAGIC_SCALE_FACTOR,
    7.8 * STABLE_MAGIC_SCALE_FACTOR,
];
/// The fill eases towards the health over this long, from where it is each frame.
const FILL_EASE: f64 = 200.0;
const BULGE_SCALE: f32 = 1.2;
const BULGE_REST_SCALE: f32 = 0.8;
const BULGE_DURATION: f64 = 150.0;
const FLASH_DURATION: f64 = 120.0;
const FLASH_SCALE: f32 = 1.6;
const EPIC_FLASH_SCALE: f32 = 2.0;
/// `Precision.AlmostEquals(lastValue, Current.Value, 0.001f)`
const CHANGE_PRECISION: f64 = 0.001f32 as f64;
/// The initial increase's step and its interval.
const INCREASE_STEP: f32 = 0.05;
const INCREASE_DELAY: f64 = 150.0;

/// The marker's textures: one for the new style, three (normal, danger, super danger) for the
/// old.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum MarkerTextures {
    #[default]
    None,
    /// `scorebar-marker`.
    New(Option<SkinSprite>),
    /// `scorebar-ki`, `scorebar-kidanger`, `scorebar-kidanger2`.
    Old([Option<SkinSprite>; 3]),
}

/// The health bar's textures for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HealthTextures {
    pub background: Option<SkinSprite>,
    /// The fill's frames: `scorebar-colour-{n}`, or `scorebar-colour` alone.
    pub fill: [Option<SkinSprite>; MAX_FILL_FRAMES],
    pub fill_frames: usize,
    /// How long each fill frame shows, in milliseconds.
    pub frame_length: f64,
    pub marker: MarkerTextures,
}

impl Default for HealthTextures {
    fn default() -> Self {
        HealthTextures {
            background: None,
            fill: [None; MAX_FILL_FRAMES],
            fill_frames: 0,
            frame_length: 1000.0,
            marker: MarkerTextures::None,
        }
    }
}

impl HealthTextures {
    /// Whether there is anything to draw: otherwise the HUD draws a plain bar.
    pub fn is_drawn(&self) -> bool {
        self.background.is_some() || self.fill_frames > 0
    }

    fn new_style(&self) -> bool {
        matches!(self.marker, MarkerTextures::New(_))
    }

    /// The fill's size: its first frame's (`LegacyFill`).
    fn fill_size(&self) -> [f32; 2] {
        self.fill[0].map_or([0.0, 0.0], |s| [s.size.x, s.size.y])
    }

    fn fill_position(&self) -> [f32; 2] {
        if self.new_style() {
            NEW_FILL_POSITION
        } else {
            OLD_FILL_POSITION
        }
    }

    /// The bar's size at a scale of 1: the background and the fill, whichever reaches
    /// further (`AutoSizeAxes`; the marker is a point).
    pub fn size(&self) -> [f32; 2] {
        let background = self.background.map_or([0.0, 0.0], |s| [s.size.x, s.size.y]);
        let [x, y] = self.fill_position();
        let [w, h] = self.fill_size();
        if self.fill_frames == 0 {
            return background;
        }
        [background[0].max(x + w), background[1].max(y + h)]
    }

    /// The marker texture for `health`, and its index for a flash.
    fn marker(&self, index: usize) -> Option<SkinSprite> {
        match self.marker {
            MarkerTextures::None => None,
            MarkerTextures::New(marker) => marker,
            MarkerTextures::Old(ki) => ki[index],
        }
    }
}

/// The old-style marker for `health`: normal, danger, super danger.
fn marker_index(health: f64) -> usize {
    if health < DANGER_CUTOFF {
        2
    } else if health < EPIC_CUTOFF {
        1
    } else {
        0
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyHealthDisplay.cs (getFillColour)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/LegacyUtils.cs (InterpolateNonLinear)
/// The new style's colour at `health`: red fading to black up to a fifth, then black to
/// white up to half.
pub fn fill_colour(health: f64) -> Color {
    let lerp = |from: f32, to: f32, t: f64| from + (to - from) * t.clamp(0.0, 1.0) as f32;
    if health < DANGER_CUTOFF {
        let t = (DANGER_CUTOFF - health) / DANGER_CUTOFF;
        Color {
            r: lerp(0.0, 1.0, t),
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
    } else if health < EPIC_CUTOFF {
        let v = lerp(1.0, 0.0, (EPIC_CUTOFF - health) / EPIC_CUTOFF);
        Color {
            r: v,
            g: v,
            b: v,
            a: 1.0,
        }
    } else {
        Color::WHITE
    }
}

/// A flash of the marker: when, whether epic (at half health or more), which marker.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Flash {
    start: f64,
    epic: bool,
    marker: usize,
}

/// `Current` moving linearly towards `to` (`TransformBindableTo`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rise {
    start: f64,
    from: f64,
    to: f64,
}

impl Rise {
    fn at(&self, time: f64) -> f64 {
        let t = ((time - self.start) / INCREASE_DELAY).clamp(0.0, 1.0);
        self.from + t * (self.to - self.from)
    }
}

/// The initial increase: from empty to the health a step at a time, flashing each step.
#[derive(Debug, Clone, Copy, PartialEq)]
struct InitialIncrease {
    /// The health when the bar loaded: once it changes, the increase stops.
    initial: f64,
    /// When the next step runs.
    next: f64,
}

/// The state of a legacy health bar.
#[derive(Debug, Clone, PartialEq)]
pub struct HealthBar {
    /// `Current`: the health the bar shows.
    current: f64,
    /// The value when the last change was noticed (`lastValue`).
    last_value: f64,
    /// The fill's width as a fraction of its full width (`fill.Width / maxFillWidth`).
    fill: f64,
    /// The time of the last update; `None` before the first.
    last_time: Option<f64>,
    /// When the fill animation started.
    start: f64,
    /// Hits taken so far: a new one flashes the marker.
    hits: u32,
    increase: Option<InitialIncrease>,
    rise: Option<Rise>,
    bulge: Option<f64>,
    flash: Option<Flash>,
    /// Off: the bar shows the health at once, without the fill-up, the easing, the bulge
    /// and the flash.
    animated: bool,
}

impl Default for HealthBar {
    fn default() -> Self {
        HealthBar::new(true)
    }
}

impl HealthBar {
    /// A bar with an empty fill, as loaded; `animated` or showing the health at once.
    pub fn new(animated: bool) -> HealthBar {
        HealthBar {
            current: 0.0,
            last_value: 0.0,
            fill: 0.0,
            last_time: None,
            start: 0.0,
            hits: 0,
            increase: None,
            rise: None,
            bulge: None,
            flash: None,
            animated,
        }
    }

    /// Takes `health` (0..=1) and the count of `hits` (results that are hits, ignored ones
    /// aside) at `time`. The first call loads the bar: it fills up from empty in steps of
    /// 0.05 every 150 ms until it reaches the health or the health changes. Call once a
    /// frame, with times that do not go back.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HealthDisplay.cs (LoadComplete, Update, startInitialAnimation, FinishInitialAnimation, onNewJudgement)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyHealthDisplay.cs (Update, HealthChanged, Flash)
    pub fn update(&mut self, health: f64, hits: u32, time: f64) {
        let health = if health.is_nan() {
            0.0
        } else {
            health.clamp(0.0, 1.0)
        };
        if !self.animated {
            self.last_time = Some(time);
            self.current = health;
            self.last_value = health;
            self.fill = health;
            self.hits = hits;
            return;
        }
        let Some(last_time) = self.last_time else {
            self.last_time = Some(time);
            self.start = time;
            self.hits = hits;
            if self.current < health {
                self.increase = Some(InitialIncrease {
                    initial: health,
                    next: time + INCREASE_DELAY,
                });
            }
            self.update_current(health, time);
            return;
        };
        self.last_time = Some(time);
        // The HUD's transforms, then its scheduler: the increase's steps and the flashes
        // judgements asked for, with the health as last shown.
        if let Some(rise) = self.rise {
            self.current = rise.at(time);
        }
        let mut flash = false;
        if let Some(increase) = self.increase
            && time >= increase.next
        {
            let to = (self.current + f64::from(INCREASE_STEP)).min(health);
            self.rise = Some(Rise {
                start: time,
                from: self.current,
                to,
            });
            flash = true;
            self.increase = Some(InitialIncrease {
                next: increase.next + INCREASE_DELAY,
                ..increase
            });
            if to >= health {
                self.finish_increase();
            }
        }
        if hits != self.hits {
            // A count going back is a new attempt: nothing to flash.
            flash |= hits > self.hits;
            self.hits = hits;
        }
        if flash {
            // `Flash` bulges too.
            self.bulge = Some(time);
            self.flash = Some(Flash {
                start: time,
                epic: self.current >= EPIC_CUTOFF,
                marker: marker_index(self.current),
            });
        }
        self.update_current(health, time);
        // `Interpolation.ValueAt(Math.Clamp(elapsed, 0, 200), fill.Width, target, 0, 200,
        // Easing.OutQuint)`
        let elapsed = (time - last_time).clamp(0.0, FILL_EASE);
        if elapsed > 0.0 {
            let t = Easing::OutQuint.apply(elapsed / FILL_EASE);
            self.fill += t * (self.current - self.fill);
        }
    }

    /// `HealthDisplay.Update`: the health shows once the increase is over or the health
    /// changed; a rise notices a change.
    fn update_current(&mut self, health: f64, time: f64) {
        if self.increase.is_none_or(|i| health != i.initial) {
            self.current = health;
            if self.increase.is_some() {
                self.finish_increase();
            }
        }
        if (self.last_value - self.current).abs() > CHANGE_PRECISION {
            if self.current > self.last_value {
                self.bulge = Some(time);
            }
            self.last_value = self.current;
        }
    }

    /// Stops the increase; its rise plays out at once.
    fn finish_increase(&mut self) {
        self.increase = None;
        if let Some(rise) = self.rise.take() {
            self.current = rise.to;
        }
    }

    /// The fill's width as a fraction of its full width.
    pub fn fill(&self) -> f64 {
        self.fill
    }

    /// The marker's scale at `time`: 1 until the first bulge, then from 1.2 down to 0.8,
    /// where it stays.
    fn marker_scale(&self, time: f64) -> f32 {
        self.bulge.map_or(1.0, |start| {
            let t = ((time - start) / BULGE_DURATION).clamp(0.0, 1.0) as f32;
            BULGE_SCALE + (BULGE_REST_SCALE - BULGE_SCALE) * t
        })
    }

    /// Writes the bar for `time` with its top left at (0, 0), each display unit `scale`
    /// window units.
    pub fn push(&self, batch: &mut SpriteBatch, textures: &HealthTextures, scale: f32, time: f64) {
        if let Some(bg) = textures.background {
            batch.push(&Sprite::new(
                bg.texture,
                bg.uv,
                [0.0, 0.0],
                [bg.size.x * scale, bg.size.y * scale],
            ));
        }
        let new_style = textures.new_style();
        let [fx, fy] = textures.fill_position();
        let [full_width, fill_height] = textures.fill_size();
        let width = self.fill as f32 * full_width;
        let colour = if new_style {
            fill_colour(self.current)
        } else {
            Color::WHITE
        };
        if textures.fill_frames > 0 {
            let elapsed = (time - self.start).max(0.0);
            let frame = if textures.frame_length > 0.0 {
                (elapsed / textures.frame_length) as usize % textures.fill_frames
            } else {
                0
            };
            if let Some(f) = textures.fill[frame]
                && width > 0.0
            {
                // Masked to the fill's width, and to the first frame's height (`Size` of
                // `LegacyFill`).
                let cut = |shown: f32, full: f32| if full > 0.0 { shown / full } else { 0.0 };
                let shown = [width.min(f.size.x), fill_height.min(f.size.y)];
                let uv = UvRect {
                    u1: f.uv.u0 + (f.uv.u1 - f.uv.u0) * cut(shown[0], f.size.x),
                    v1: f.uv.v0 + (f.uv.v1 - f.uv.v0) * cut(shown[1], f.size.y),
                    ..f.uv
                };
                let mut sprite = Sprite::new(
                    f.texture,
                    uv,
                    [fx * scale, fy * scale],
                    [shown[0] * scale, shown[1] * scale],
                );
                sprite.colour = colour;
                batch.push(&sprite);
            }
        }
        let at = [
            (fx + width) * scale,
            (fy + if new_style { fill_height / 2.0 } else { 0.0 }) * scale,
        ];
        let centred = |batch: &mut SpriteBatch, s: SkinSprite, size: f32, colour, blend| {
            let w = s.size.x * size * scale;
            let h = s.size.y * size * scale;
            let mut sprite =
                Sprite::new(s.texture, s.uv, [at[0] - w / 2.0, at[1] - h / 2.0], [w, h]);
            sprite.colour = colour;
            sprite.blend = blend;
            batch.push(&sprite);
        };
        if let Some(main) = textures.marker(marker_index(self.current)) {
            let blend = if new_style && self.current >= EPIC_CUTOFF {
                Blend::Additive
            } else {
                Blend::Alpha
            };
            centred(batch, main, self.marker_scale(time), colour, blend);
        }
        if let Some(flash) = self.flash
            && time < flash.start + FLASH_DURATION
            && let Some(explode) = textures.marker(flash.marker)
        {
            let t = Easing::Out.apply(((time - flash.start) / FLASH_DURATION).max(0.0)) as f32;
            let to = if flash.epic {
                EPIC_FLASH_SCALE
            } else {
                FLASH_SCALE
            };
            let blend = if flash.epic {
                Blend::Additive
            } else {
                Blend::Alpha
            };
            let white = Color {
                a: 1.0 - t,
                ..Color::WHITE
            };
            centred(batch, explode, 1.0 + (to - 1.0) * t, white, blend);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slam_osu::Vec2;

    fn sprite(id: u32, w: f32, h: f32) -> Option<SkinSprite> {
        Some(SkinSprite {
            texture: slam_render::textures::TextureId(id),
            uv: UvRect::FULL,
            size: Vec2::new(w, h),
        })
    }

    fn textures(marker: MarkerTextures) -> HealthTextures {
        let mut fill = [None; MAX_FILL_FRAMES];
        fill[0] = sprite(2, 600.0, 10.0);
        fill[1] = sprite(3, 600.0, 10.0);
        HealthTextures {
            background: sprite(1, 700.0, 40.0),
            fill,
            fill_frames: 2,
            frame_length: 100.0,
            marker,
        }
    }

    fn old() -> MarkerTextures {
        MarkerTextures::Old([
            sprite(10, 50.0, 50.0),
            sprite(11, 50.0, 50.0),
            sprite(12, 50.0, 50.0),
        ])
    }

    fn new() -> MarkerTextures {
        MarkerTextures::New(sprite(20, 24.0, 24.0))
    }

    /// Each sprite of `batch`: texture, top left, bottom right.
    fn sprites(batch: &SpriteBatch) -> Vec<(u32, [f32; 2], [f32; 2])> {
        let textures = batch
            .batches()
            .iter()
            .flat_map(|b| std::iter::repeat_n(b.texture.0, b.quads as usize));
        batch
            .vertices()
            .chunks(4)
            .zip(textures)
            .map(|(v, t)| (t, v[0].position, v[2].position))
            .collect()
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    /// A bar loaded empty (no increase) that took `health` at `time`.
    fn loaded(health: f64, time: f64) -> HealthBar {
        let mut bar = HealthBar::new(true);
        bar.update(0.0, 0, time);
        bar.update(health, 0, time);
        bar
    }

    #[test]
    fn the_style_follows_the_marker_and_picks_the_ki_by_health() {
        assert_eq!(marker_index(1.0), 0);
        assert_eq!(marker_index(0.5), 0);
        assert_eq!(marker_index(0.49), 1);
        assert_eq!(marker_index(0.2), 1);
        assert_eq!(marker_index(0.19), 2);

        let bar = loaded(0.3, 0.0);
        let mut batch = SpriteBatch::new();
        bar.push(&mut batch, &textures(old()), 1.0, 0.0);
        let ids: Vec<u32> = sprites(&batch).iter().map(|s| s.0).collect();
        // The fill is still empty: background and kidanger.
        assert_eq!(ids, [1, 11]);
        // The old-style ki sits on the fill's top edge, at its end (bulging: 1.2).
        let (_, min, max) = sprites(&batch)[1];
        assert!(close(min, [4.8 - 30.0, 16.0 - 30.0]) && close(max, [4.8 + 30.0, 16.0 + 30.0]));

        let mut batch = SpriteBatch::new();
        bar.push(&mut batch, &textures(new()), 1.0, 1000.0);
        let (id, min, _) = sprites(&batch)[1];
        // The new style's marker sits on the fill's middle (bulged: 0.8).
        assert_eq!(id, 20);
        assert!(close(min, [12.0 - 9.6, 12.48 + 5.0 - 9.6]), "{min:?}");
        assert_eq!(textures(new()).size(), [700.0, 40.0]);
    }

    #[test]
    fn the_fill_eases_towards_the_health_and_is_cut_to_its_width() {
        let mut bar = loaded(1.0, 0.0);
        assert_eq!(bar.fill(), 0.0);
        // Half of 200 ms: 1 - 0.5^5 of the way.
        bar.update(1.0, 0, 100.0);
        assert!((bar.fill() - (1.0 - 0.5f64.powi(5))).abs() < 1e-9);
        // A long frame counts as 200 ms: all the way.
        bar.update(0.5, 0, 1000.0);
        assert_eq!(bar.fill(), 0.5);

        let mut batch = SpriteBatch::new();
        bar.push(&mut batch, &textures(old()), 2.0, 1150.0);
        // The background, the second frame (100 ms each) at half its width, the ki.
        let all = sprites(&batch);
        assert_eq!(all[1].0, 3);
        assert!(close(all[1].1, [9.6, 32.0]) && close(all[1].2, [9.6 + 600.0, 52.0]));
        let uv = batch.vertices()[4 + 2].uv;
        assert!((uv[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_bar_fills_up_in_steps_when_loaded() {
        let mut bar = HealthBar::new(true);
        bar.update(0.2, 0, 0.0);
        assert_eq!(bar.current, 0.0);
        bar.update(0.2, 0, 150.0);
        assert_eq!(bar.current, 0.0);
        assert_eq!(bar.flash.unwrap().start, 150.0);
        // Half way through the first step of 0.05 (in single precision).
        bar.update(0.2, 0, 225.0);
        assert!((bar.current - f64::from(0.05f32) / 2.0).abs() < 1e-12);
        for time in [300.0, 450.0] {
            bar.update(0.2, 0, time);
        }
        assert!(bar.increase.is_some());
        // The fourth step reaches the health and ends the increase at once.
        bar.update(0.2, 0, 600.0);
        assert_eq!(bar.current, 0.2);
        assert_eq!(bar.increase, None);
        assert_eq!(bar.flash.unwrap().start, 600.0);
        bar.update(0.2, 0, 750.0);
        assert_eq!(bar.flash.unwrap().start, 600.0);

        // A change of the health ends it: the step under way finishes first.
        let mut bar = HealthBar::new(true);
        bar.update(1.0, 0, 0.0);
        bar.update(1.0, 0, 150.0);
        bar.update(0.9, 0, 200.0);
        assert_eq!(bar.current, f64::from(0.05f32));
        assert_eq!(bar.increase, None);
        bar.update(0.9, 0, 210.0);
        assert_eq!(bar.current, 0.9);
    }

    #[test]
    fn rising_health_bulges_and_hits_flash() {
        let mut bar = loaded(0.5, 0.0);
        assert!((bar.marker_scale(500.0) - 0.8).abs() < 1e-6);
        // A drop does not bulge, nor a rise within the precision.
        bar.update(0.4, 0, 1000.0);
        bar.update(0.4005, 0, 1010.0);
        assert!((bar.marker_scale(1010.0) - 0.8).abs() < 1e-6);
        bar.update(0.45, 0, 1020.0);
        assert!((bar.marker_scale(1020.0) - 1.2).abs() < 1e-6);
        assert!((bar.marker_scale(1095.0) - 1.0).abs() < 1e-6);

        // A hit flashes with the health shown before it: at half or more big and additive.
        bar.update(0.6, 0, 1900.0);
        bar.update(0.4, 1, 2000.0);
        let flash = bar.flash.unwrap();
        assert!(flash.epic);
        assert_eq!(flash.marker, 0);
        let mut batch = SpriteBatch::new();
        bar.push(&mut batch, &textures(new()), 1.0, 2000.0);
        assert_eq!(batch.len(), 4);
        // The explosion fades in 120 ms.
        batch.clear();
        bar.push(&mut batch, &textures(new()), 1.0, 2120.0);
        assert_eq!(batch.len(), 3);
        // Below half it is smaller and not additive.
        bar.update(0.3, 2, 3000.0);
        assert!(!bar.flash.unwrap().epic);
        // A new attempt counts from zero again without a flash.
        bar.update(1.0, 0, 4000.0);
        assert_eq!(bar.flash.unwrap().start, 3000.0);
    }

    #[test]
    fn without_animations_the_bar_shows_the_health_at_once() {
        let mut bar = HealthBar::new(false);
        bar.update(0.7, 0, 0.0);
        assert_eq!((bar.current, bar.fill()), (0.7, 0.7));
        bar.update(0.9, 1, 10.0);
        assert_eq!(bar.fill(), 0.9);
        assert_eq!((bar.bulge, bar.flash), (None, None));
    }

    #[test]
    fn the_new_style_colour_darkens_below_half_and_reddens_below_a_fifth() {
        assert_eq!(fill_colour(0.5), Color::WHITE);
        let grey = fill_colour(0.25);
        assert!((grey.r - 0.5).abs() < 1e-6 && grey.r == grey.g && grey.g == grey.b);
        let dark = fill_colour(0.2);
        assert!((dark.r - 0.4).abs() < 1e-6);
        let red = fill_colour(0.1);
        assert!((red.r - 0.5).abs() < 1e-6 && red.g == 0.0);
        assert_eq!(fill_colour(0.0).r, 1.0);
    }
}
