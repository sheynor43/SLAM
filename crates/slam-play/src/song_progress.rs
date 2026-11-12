//! The legacy song progress (`LegacySongProgress`): a pie filling clockwise from the top as
//! the beatmap's objects go by, inside a white ring with a dot in the middle. Before the
//! first object it is green and empties counter-clockwise through the intro.
//!
//! Draw side. Allocation-free once warm.

use slam_osu::Beatmap;
use slam_osu::lead_in::earliest_start_time;
use slam_osu::statistics::playable_bounds;
use slam_render::atlas::AtlasRegion;
use slam_render::image::Image;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::{Blend, Color, Extent};

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySongProgress.cs
/// The side of the component, in lazer's HUD pixels: the ring's.
pub const SIZE: f32 = 33.0;
/// The pie's diameter as a fraction of [`SIZE`].
const PIE: f32 = 0.92;
const RING_BORDER: f32 = 2.0;
const DOT: f32 = 4.0;
const INTRO_COLOUR: Color = Color {
    r: 199.0 / 255.0,
    g: 1.0,
    b: 47.0 / 255.0,
    a: 153.0 / 255.0,
};
const COLOUR: Color = Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 153.0 / 255.0,
};
/// Slices of a whole pie.
const SLICES: usize = 64;
/// The most sprites [`SongProgress::push`] writes.
pub const MAX_SONG_PROGRESS_SPRITES: usize = SLICES + 2;

/// Side of [`ring_image`] in pixels.
const RING_SIDE: u32 = 128;

/// A white ring on transparent, its border [`RING_BORDER`] of [`SIZE`] wide, anti-aliased
/// over a pixel.
pub fn ring_image() -> Image {
    let side = RING_SIDE as usize;
    let outer = RING_SIDE as f32 / 2.0;
    let inner = outer * (1.0 - 2.0 * RING_BORDER / SIZE);
    let mut pixels = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            let (dx, dy) = (x as f32 + 0.5 - outer, y as f32 + 0.5 - outer);
            let d = (dx * dx + dy * dy).sqrt();
            let coverage = (outer - d + 0.5).clamp(0.0, 1.0) * (d - inner + 0.5).clamp(0.0, 1.0);
            pixels.extend_from_slice(&[
                u8::MAX,
                u8::MAX,
                u8::MAX,
                (coverage * 255.0).round() as u8,
            ]);
        }
    }
    Image {
        size: Extent::new(RING_SIDE, RING_SIDE),
        pixels,
    }
}

/// The times the progress goes by; the default stands for a beatmap without objects.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SongProgress {
    /// When the clock starts (`GameplayClock.StartTime`).
    start: f64,
    /// `FirstHitTime` and `LastHitTime`.
    first: f64,
    last: f64,
}

impl SongProgress {
    /// The progress of `beatmap`'s play, whose clock starts where the session's does.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/SongProgress.cs (Objects)
    pub fn new(beatmap: &Beatmap) -> Self {
        let (first, last) = playable_bounds(&beatmap.hit_objects);
        SongProgress {
            start: earliest_start_time(beatmap, None),
            first,
            last,
        }
    }

    /// The progress at gameplay `time` and whether it is the intro.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/SongProgress.cs (Update)
    pub fn at(&self, time: f64) -> (f64, bool) {
        let current = time.min(self.last);
        if current < self.first {
            let intro = self.first - self.start;
            ((current - self.start) / intro, true)
        } else {
            let duration = self.last - self.first;
            if duration == 0.0 {
                (0.0, false)
            } else {
                ((current - self.first) / duration, false)
            }
        }
    }

    /// Writes the progress at gameplay `time` with the box's top left at (0, 0), each lazer
    /// pixel `scale` window pixels: the pie with `disc` (a white disc), the ring with `ring`
    /// ([`ring_image`]) and the dot.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySongProgress.cs (load, UpdateProgress)
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        time: f64,
        scale: f32,
        disc: AtlasRegion,
        ring: AtlasRegion,
    ) {
        let (progress, intro) = self.at(time);
        // The intro's pie is mirrored and shows what is left of it.
        let (fill, colour, mirror) = if intro {
            (1.0 - progress, INTRO_COLOUR, -1.0)
        } else {
            (progress, COLOUR, 1.0)
        };
        let side = SIZE * scale;
        let centre = [side / 2.0, side / 2.0];
        push_pie(batch, centre, PIE * side / 2.0, fill, mirror, colour, disc);
        batch.push(&Sprite::new(
            ring.texture,
            ring.uv,
            [0.0, 0.0],
            [side, side],
        ));
        let dot = DOT * scale;
        batch.push(&Sprite::new(
            disc.texture,
            disc.uv,
            [centre[0] - dot / 2.0, centre[1] - dot / 2.0],
            [dot, dot],
        ));
    }
}

/// A pie of `radius` around `centre` filled `fill` of the way (`CircularProgress`):
/// clockwise from the top, counter-clockwise when `mirror` is -1. Slices of `disc`, whose
/// edge anti-aliases the arc.
fn push_pie(
    batch: &mut SpriteBatch,
    centre: [f32; 2],
    radius: f32,
    fill: f64,
    mirror: f32,
    colour: Color,
    disc: AtlasRegion,
) {
    // `Math.Abs((float)progress)`; nothing at 0.
    let fill = (fill.abs() as f32).min(1.0);
    if fill.is_nan() || fill <= 0.0 || colour.a <= 0.0 {
        return;
    }
    let slices = ((fill * SLICES as f32).ceil() as usize).clamp(1, SLICES);
    let step = std::f32::consts::TAU * fill / slices as f32;
    // The slices' outer corners reach past the circle so that their chords stay outside
    // it: the disc's edge draws the arc.
    let reach = 1.0 / (std::f32::consts::PI / SLICES as f32).cos();
    let uv = disc.uv;
    let to_uv = |dx: f32, dy: f32| {
        [
            uv.u0 + (uv.u1 - uv.u0) * (0.5 + dx / 2.0),
            uv.v0 + (uv.v1 - uv.v0) * (0.5 + dy / 2.0),
        ]
    };
    let corner = |i: usize| {
        let (sin, cos) = (step * i as f32).sin_cos();
        let (dx, dy) = (mirror * sin * reach, -cos * reach);
        (
            [centre[0] + dx * radius, centre[1] + dy * radius],
            to_uv(dx, dy),
        )
    };
    let middle = to_uv(0.0, 0.0);
    let (mut a, mut a_uv) = corner(0);
    for i in 0..slices {
        let (b, b_uv) = corner(i + 1);
        batch.push_quad(
            disc.texture,
            Blend::Alpha,
            [centre, a, b, centre],
            [middle, a_uv, b_uv, middle],
            colour,
        );
        (a, a_uv) = (b, b_uv);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slam_render::atlas::UvRect;
    use slam_render::textures::TextureId;

    fn progress() -> SongProgress {
        SongProgress {
            start: -1000.0,
            first: 1000.0,
            last: 5000.0,
        }
    }

    #[test]
    fn the_intro_runs_from_the_start_to_the_first_object_then_the_objects() {
        let p = progress();
        assert_eq!(p.at(-1000.0), (0.0, true));
        assert_eq!(p.at(0.0), (0.5, true));
        assert_eq!(p.at(1000.0), (0.0, false));
        assert_eq!(p.at(3000.0), (0.5, false));
        assert_eq!(p.at(9000.0), (1.0, false));
        let one = SongProgress {
            start: 0.0,
            first: 500.0,
            last: 500.0,
        };
        assert_eq!(one.at(800.0), (0.0, false));
    }

    fn region(id: u32) -> AtlasRegion {
        AtlasRegion {
            texture: TextureId(id),
            origin: slam_render::Origin { x: 0, y: 0 },
            size: Extent::new(64, 64),
            uv: UvRect::FULL,
        }
    }

    #[test]
    fn the_pie_fills_clockwise_from_the_top_inside_the_ring() {
        let p = progress();
        let mut batch = SpriteBatch::new();
        // A quarter of the objects: 16 slices, the ring, the dot.
        p.push(&mut batch, 2000.0, 2.0, region(1), region(2));
        assert_eq!(batch.len(), 16 + 2);
        let v = batch.vertices();
        let centre = [33.0, 33.0];
        assert_eq!(v[0].position, centre);
        // The first slice starts straight up, the last ends straight right.
        let radius = 0.92 * 33.0;
        let reach = radius / (std::f32::consts::PI / 64.0).cos();
        assert!((v[1].position[0] - 33.0).abs() < 1e-3);
        assert!((v[1].position[1] - (33.0 - reach)).abs() < 1e-3);
        let last = &v[15 * 4..16 * 4];
        assert!((last[2].position[0] - (33.0 + reach)).abs() < 1e-3);
        assert!((last[2].position[1] - 33.0).abs() < 1e-3);
        assert!((v[0].colour[3] - 0.6).abs() < 1e-6);
        // The ring covers the box.
        assert_eq!(v[16 * 4].position, [0.0, 0.0]);
        assert_eq!(v[16 * 4 + 2].position, [66.0, 66.0]);

        // In the intro, half way: half a pie left, green, to the left of the top.
        batch.clear();
        p.push(&mut batch, 0.0, 2.0, region(1), region(2));
        assert_eq!(batch.len(), 32 + 2);
        let v = batch.vertices();
        assert!(v[2].position[0] < 33.0);
        assert!((v[0].colour[0] - 199.0 / 255.0).abs() < 1e-6);
        // At the first object it is empty.
        batch.clear();
        p.push(&mut batch, 1000.0, 2.0, region(1), region(2));
        assert_eq!(batch.len(), 2);
    }

    #[test]
    fn the_ring_is_a_ring() {
        let image = ring_image();
        let side = RING_SIDE as usize;
        let alpha = |x: usize, y: usize| image.pixels[(y * side + x) * 4 + 3];
        assert_eq!(alpha(side / 2, side / 2), 0);
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(side / 2, 2), u8::MAX);
    }
}
