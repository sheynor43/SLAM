//! Follow points between consecutive objects of a combo, as lazer draws them.
//!
//! Built once at load from the beatmap ([`FollowPoints::new`]); every frame each point's
//! transforms are evaluated at the game time ([`follow_point_pose`]) without allocations.
//! Nothing goes through the snapshot: follow points depend only on the beatmap and the time.
//!
//! Not done: lazer's default (non-legacy) follow point and argon's, when no skin has
//! `followpoint`; hiding them under Depth, Magnetised and Repel; textures from the beatmap's
//! skin.

use slam_osu::objects::OsuHitObject;
use slam_osu::{Beatmap, Vec2};
use slam_render::Color;
use slam_render::sprite::SpriteBatch;

use crate::circles::{Easing, Kind, Track, centred};
use crate::playfield::PlayfieldLayout;
use crate::sliders::AnimationSprites;

/// `FollowPointConnection.SPACING`: the distance between two points, in osu!pixels.
const SPACING: i32 = 32;

/// `FollowPointConnection.PREEMPT`: how long before its time a point fades in, at most.
const PREEMPT: f64 = 800.0;

/// `OsuHitObject.PREEMPT_MIN`: the preempt of AR 10.
const PREEMPT_MIN: f64 = 450.0;

/// How long a point takes to fade, scale and move in, and to fade out (stable's
/// `HitObjectManager.TimeFadeIn`, unaffected by Hidden).
const FADE_DURATION: f64 = 400.0;

/// The largest display size of a follow point texture, in osu!pixels (`OBJECT_RADIUS * 2` by
/// `OBJECT_RADIUS`).
pub const MAX_FOLLOW_POINT_SIZE: Vec2 = Vec2::new(128.0, 64.0);

/// One follow point's static drawing data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FollowPoint {
    /// Where the point starts and where it moves to, in osu!pixels.
    pub from: Vec2,
    pub to: Vec2,
    /// Degrees clockwise on screen: the direction of the connection.
    pub rotation: f32,
    /// The scale of the object the connection ends at (`OsuHitObject.Scale`).
    pub scale: f32,
    /// When the point starts fading in (also its animation's start) and out.
    pub fade_in: f64,
    pub fade_out: f64,
}

/// A follow point's appearance at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FollowPointPose {
    /// In osu!pixels.
    pub position: Vec2,
    pub scale: f32,
    pub alpha: f32,
}

/// The transforms of a point at `time`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/Connections/FollowPointConnection.cs (scheduleRefresh)
pub fn follow_point_pose(point: &FollowPoint, time: f64) -> FollowPointPose {
    let mut alpha = Track::new(Kind::Alpha);
    alpha.add(point.fade_in, FADE_DURATION, 1.0, Easing::None);
    alpha.fade_out(point.fade_out, FADE_DURATION);
    let mut scale = Track::new(Kind::Scale);
    scale.add(point.fade_in, FADE_DURATION, point.scale, Easing::Out);
    // A `Vector2` is interpolated per component in `float`, as a scale.
    let mut x = Track::new(Kind::Scale);
    x.add(point.fade_in, FADE_DURATION, point.to.x, Easing::Out);
    let mut y = Track::new(Kind::Scale);
    y.add(point.fade_in, FADE_DURATION, point.to.y, Easing::Out);
    FollowPointPose {
        position: Vec2::new(x.value(point.from.x, time), y.value(point.from.y, time)),
        scale: scale.value(1.5 * point.scale, time),
        alpha: alpha.value(0.0, time),
    }
}

/// The points between two objects, alive from `lifetime.0` until before `lifetime.1`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Connection {
    /// Range of [`FollowPoints::points`].
    points: (u32, u32),
    lifetime: (f64, f64),
}

/// The follow points of a beatmap.
#[derive(Debug, Clone, Default)]
pub struct FollowPoints {
    points: Vec<FollowPoint>,
    /// By lifetime start.
    connections: Vec<Connection>,
    /// The longest lifetime of a connection.
    longest: f64,
}

impl FollowPoints {
    /// The follow points of `beatmap`, with its mods applied.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/Connections/FollowPointRenderer.cs (addEntry)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/Connections/FollowPointLifetimeEntry.cs (refreshLifetimes)
    pub fn new(beatmap: &Beatmap) -> Self {
        // Each object connects to the next by start time; equal ones keep their order.
        let mut objects: Vec<&OsuHitObject> = beatmap.hit_objects.iter().collect();
        objects.sort_by(|a, b| a.start_time.total_cmp(&b.start_time));
        let mut follow = FollowPoints::default();
        for pair in objects.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            if end.new_combo || start.is_spinner() || end.is_spinner() {
                continue;
            }
            follow.connect(start, end);
        }
        follow
            .connections
            .sort_by(|a, b| a.lifetime.0.total_cmp(&b.lifetime.0));
        follow.longest = follow
            .connections
            .iter()
            .map(|c| c.lifetime.1 - c.lifetime.0)
            .fold(0.0, f64::max);
        follow
    }

    /// Adds the points from `start` to `end`.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/Connections/FollowPointConnection.cs (scheduleRefresh)
    fn connect(&mut self, start: &OsuHitObject, end: &OsuHitObject) {
        let start_position = start.stacked_end_position();
        let vector = end.stacked_position() - start_position;
        let length = vector.length();
        let distance = length as i32;
        let rotation = (f64::from(vector.y).atan2(f64::from(vector.x))
            * (180.0 / std::f64::consts::PI)) as f32;
        let first = self.points.len() as u32;
        let mut d = SPACING * 3 / 2;
        while d < distance - SPACING {
            let fraction = d as f32 / distance as f32;
            let (fade_in, fade_out) = fade_times(start, end, fraction);
            self.points.push(FollowPoint {
                from: start_position + (fraction - 0.1) * vector,
                to: start_position + fraction * vector,
                rotation,
                scale: end.defaults.scale,
                fade_in,
                fade_out,
            });
            d += SPACING;
        }
        let last = self.points.len() as u32;
        if first == last {
            // Without points the connection ends where it starts.
            return;
        }
        // The lifetime starts as the first point would fade in at the exact length.
        let (lifetime_start, _) = fade_times(start, end, (SPACING * 3 / 2) as f32 / length);
        // `Expire` ends a point's lifetime when its fade out does.
        let lifetime_end = self.points[last as usize - 1].fade_out + FADE_DURATION;
        self.connections.push(Connection {
            points: (first, last),
            lifetime: (lifetime_start, lifetime_end),
        });
    }

    /// Every point, connection by connection.
    pub fn points(&self) -> &[FollowPoint] {
        &self.points
    }

    /// The points of the connections alive at `time`, in drawing order (later on top).
    pub fn alive(&self, time: f64) -> impl Iterator<Item = &FollowPoint> {
        let end = self.connections.partition_point(|c| c.lifetime.0 <= time);
        let begin = self.connections[..end].partition_point(|c| c.lifetime.0 < time - self.longest);
        self.connections[begin..end]
            .iter()
            .filter(move |c| time < c.lifetime.1)
            .flat_map(|c| &self.points[c.points.0 as usize..c.points.1 as usize])
    }

    /// Writes the points visible at `time` drawn with `texture`; nothing without one.
    /// Allocation-free once `batch` has room.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/Connections/FollowPoint.cs
    pub fn push(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        time: f64,
        texture: &AnimationSprites,
    ) {
        if texture.is_empty() {
            return;
        }
        for point in self.alive(time) {
            let pose = follow_point_pose(point, time);
            if pose.alpha <= 0.0 {
                continue;
            }
            // The animation starts as the point fades in (`AnimationStartTime`).
            let Some(frame) = texture.frame_at(time - point.fade_in, texture.frame_length) else {
                continue;
            };
            let scale = pose.scale * layout.scale();
            let size = Vec2::new(frame.size.x * scale, frame.size.y * scale);
            let colour = Color {
                a: pose.alpha,
                ..Color::WHITE
            };
            let mut sprite = centred(frame, layout.to_window(pose.position), size, colour);
            sprite.rotation = point.rotation.to_radians();
            batch.push(&sprite);
        }
    }
}

/// When the point at `fraction` of the way from `start` to `end` fades in and out.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/Connections/FollowPointConnection.cs (GetFadeTimes)
fn fade_times(start: &OsuHitObject, end: &OsuHitObject, fraction: f32) -> (f64, f64) {
    let start_time = start.end_time();
    let duration = end.start_time - start_time;
    // Shorter preempts than AR 10's (mods) shorten the fade in.
    let preempt = PREEMPT * (start.defaults.time_preempt / PREEMPT_MIN).min(1.0);
    let fade_out = start_time + f64::from(fraction) * duration;
    (fade_out - preempt, fade_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(objects: &str, approach_rate: f32) -> Beatmap {
        let text = format!(
            "osu file format v14

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:5
ApproachRate:{approach_rate}
SliderMultiplier:1
SliderTickRate:1

[TimingPoints]
0,500,4,2,0,100,1,0

[HitObjects]
{objects}"
        );
        Beatmap::decode(text.as_bytes()).unwrap()
    }

    /// Two circles 200 osu!pixels apart along x, 1000 ms apart.
    fn pair() -> FollowPoints {
        FollowPoints::new(&map("100,100,1000,1,0\n300,100,2000,1,0\n", 9.0))
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-3,
            "{actual} is not {expected}"
        );
    }

    #[test]
    fn places_points_every_32_pixels_from_48() {
        let follow = pair();
        // d = 48, 80, 112, 144 (< 200 - 32).
        let points = follow.points();
        assert_eq!(points.len(), 4);
        for (point, d) in points.iter().zip([48.0f32, 80.0, 112.0, 144.0]) {
            let fraction = d / 200.0;
            assert_eq!(point.to, Vec2::new(100.0 + fraction * 200.0, 100.0));
            assert_eq!(
                point.from,
                Vec2::new(100.0 + (fraction - 0.1) * 200.0, 100.0)
            );
            assert_eq!(point.rotation, 0.0);
        }
    }

    #[test]
    fn fades_by_the_fraction_of_the_way() {
        let follow = pair();
        let first = follow.points()[0];
        // AR 9: preempt 600 ms, so 800 × 600 / 450 is capped at 800.
        assert_close(first.fade_out, 1000.0 + 0.24 * 1000.0);
        assert_close(first.fade_in, first.fade_out - 800.0);
    }

    #[test]
    fn preempts_under_ar_10_shorten_the_fade_in() {
        // AR 10.5 (a mod): preempt 375 ms.
        let mut beatmap = map("100,100,1000,1,0\n300,100,2000,1,0\n", 10.0);
        for object in &mut beatmap.hit_objects {
            object.defaults.time_preempt = 375.0;
        }
        let first = FollowPoints::new(&beatmap).points()[0];
        assert_close(first.fade_out - first.fade_in, 800.0 * 375.0 / 450.0);
    }

    #[test]
    fn short_connections_have_no_points() {
        // 80 pixels: 48 is not under 80 - 32.
        let follow = FollowPoints::new(&map("100,100,1000,1,0\n180,100,2000,1,0\n", 9.0));
        assert!(follow.points().is_empty());
        assert_eq!(follow.alive(1500.0).count(), 0);
        // 81 pixels: one point.
        let follow = FollowPoints::new(&map("100,100,1000,1,0\n181,100,2000,1,0\n", 9.0));
        assert_eq!(follow.points().len(), 1);
    }

    #[test]
    fn truncates_the_distance() {
        // Length 201.0025 is cut to 201.
        let follow = FollowPoints::new(&map("100,100,1000,1,0\n301,101,2000,1,0\n", 9.0));
        let vector = Vec2::new(201.0, 1.0);
        let point = follow.points()[0];
        assert_eq!(
            point.to,
            Vec2::new(100.0, 100.0) + (48.0f32 / 201.0) * vector
        );
        let rotation = (f64::from(vector.y).atan2(f64::from(vector.x))
            * (180.0 / std::f64::consts::PI)) as f32;
        assert_eq!(point.rotation, rotation);
    }

    #[test]
    fn new_combos_and_spinners_break_the_connection() {
        let follow = FollowPoints::new(&map(
            "100,100,1000,1,0\n300,100,2000,5,0\n500,100,3000,1,0\n256,192,4000,8,0,5000\n100,100,6000,1,0\n300,100,7000,1,0\n",
            9.0,
        ));
        // Only 2 → 3 (300 → 500) and 6 → 7 connect: 2 starts a combo, the spinner breaks
        // the connections on both sides.
        let fades: Vec<f64> = follow.points().iter().map(|p| p.fade_out).collect();
        assert_eq!(follow.points().len(), 8);
        assert!(fades[..4].iter().all(|&t| (2000.0..3000.0).contains(&t)));
        assert!(fades[4..].iter().all(|&t| (6000.0..7000.0).contains(&t)));
    }

    #[test]
    fn slider_connections_start_at_its_end() {
        // A slider from (100, 100) to (200, 100), one span of 100 pixels: one 500 ms beat.
        let follow = FollowPoints::new(&map(
            "100,100,1000,2,0,L|200:100,1,100\n400,100,2000,1,0\n",
            9.0,
        ));
        let point = follow.points()[0];
        // From the slider's end, 200 pixels: d = 48.
        assert_eq!(point.to, Vec2::new(200.0 + 0.24 * 200.0, 100.0));
        assert_close(point.fade_out, 1500.0 + 0.24 * 500.0);
    }

    #[test]
    fn objects_starting_before_the_previous_ends_fade_backwards() {
        // The slider ends at 1500, the circle 300 pixels on starts at 1200.
        let follow = FollowPoints::new(&map(
            "100,100,1000,2,0,L|200:100,1,100\n500,100,1200,1,0\n",
            9.0,
        ));
        let points = follow.points();
        assert!(points.windows(2).all(|p| p[1].fade_out < p[0].fade_out));
        // The lifetime ends with the last point, the earliest: d = 240 of 300.
        let end = 1500.0 - 0.8 * 300.0 + 400.0;
        assert_close(points.last().unwrap().fade_out + 400.0, end);
        assert!(follow.alive(end - 1.0).count() > 0);
        assert_eq!(follow.alive(end).count(), 0);
    }

    #[test]
    fn connects_stacked_positions() {
        let beatmap = map(
            "100,100,1000,1,0\n100,100,1100,1,0\n300,100,2000,1,0\n",
            9.0,
        );
        let start = &beatmap.hit_objects[1];
        assert_ne!(beatmap.hit_objects[0].stack_height, start.stack_height);
        let from = start.stacked_end_position();
        let vector = beatmap.hit_objects[2].stacked_position() - from;
        let follow = FollowPoints::new(&beatmap);
        let last = follow.points().last().unwrap();
        let distance = vector.length() as i32;
        let d = (48..distance - 32).step_by(32).next_back().unwrap();
        assert_eq!(last.to, from + (d as f32 / distance as f32) * vector);
    }

    #[test]
    fn equal_start_times_connect_in_beatmap_order() {
        let follow = FollowPoints::new(&map(
            "100,100,1000,1,0\n300,100,1000,1,0\n100,300,1000,1,0\n",
            9.0,
        ));
        // 1 → 2 along x, 2 → 3 along the diagonal back.
        let rotations: Vec<f32> = follow.points().iter().map(|p| p.rotation).collect();
        assert_eq!(rotations[0], 0.0);
        assert_eq!(*rotations.last().unwrap(), 135.0);
    }

    #[test]
    fn fades_scales_and_moves_in_then_fades_out() {
        let point = pair().points()[0];
        let at = |t: f64| follow_point_pose(&point, point.fade_in + t);
        let before = at(-1.0);
        assert_eq!(before.alpha, 0.0);
        assert_eq!(before.position, point.from);
        assert_eq!(before.scale, 1.5 * point.scale);

        let half = at(200.0);
        assert_close(f64::from(half.alpha), 0.5);
        // `Easing.Out` at 0.5: 0.75 of the way.
        assert_close(
            f64::from(half.scale),
            f64::from(1.5 * point.scale - 0.75 * 0.5 * point.scale),
        );
        assert_close(
            f64::from(half.position.x),
            f64::from(point.from.x + 0.75 * (point.to.x - point.from.x)),
        );

        let shown = at(400.0);
        assert_eq!(shown.alpha, 1.0);
        assert_eq!(shown.scale, point.scale);
        assert_eq!(shown.position, point.to);

        let out = follow_point_pose(&point, point.fade_out + 100.0);
        assert_close(f64::from(out.alpha), 0.75);
        let gone = follow_point_pose(&point, point.fade_out + 400.0);
        assert_eq!(gone.alpha, 0.0);
    }

    #[test]
    fn a_fade_out_during_the_fade_in_starts_from_its_alpha() {
        let point = FollowPoint {
            from: Vec2::ZERO,
            to: Vec2::ZERO,
            rotation: 0.0,
            scale: 1.0,
            fade_in: 0.0,
            fade_out: 200.0,
        };
        // Half faded in at 200, then half of the way out at 400.
        assert_close(f64::from(follow_point_pose(&point, 400.0).alpha), 0.25);
    }

    #[test]
    fn connections_live_from_the_first_fade_in_to_the_last_fade_out() {
        let follow = pair();
        let points = follow.points();
        // The lifetime starts at the fade in of d = 48 over the exact length (200 here).
        let start = points[0].fade_in;
        let end = points[3].fade_out + 400.0;
        assert_eq!(follow.alive(start - 0.01).count(), 0);
        assert_eq!(follow.alive(start).count(), 4);
        assert_eq!(follow.alive(end - 0.01).count(), 4);
        assert_eq!(follow.alive(end).count(), 0);
    }

    #[test]
    fn finds_alive_connections_behind_long_ones() {
        // A long gap first, then a short connection alive while the long one still is.
        let follow = FollowPoints::new(&map(
            "0,0,1000,1,0\n512,384,5000,1,0\n100,100,5000,5,0\n300,100,5500,1,0\n",
            9.0,
        ));
        let time = 4800.0;
        let alive: Vec<&FollowPoint> = follow.alive(time).collect();
        let long = alive.iter().filter(|p| p.fade_out < 5000.0).count();
        let short = alive.iter().filter(|p| p.fade_out > 5000.0).count();
        assert!(long > 0);
        assert_eq!(short, 4);
    }

    fn frame(texture: u32) -> crate::circles::SkinSprite {
        crate::circles::SkinSprite {
            texture: slam_render::textures::TextureId(texture),
            uv: slam_render::atlas::UvRect::FULL,
            size: Vec2::new(16.0, 16.0),
        }
    }

    #[test]
    fn draws_the_frame_since_the_fade_in() {
        let follow = pair();
        let point = follow.points()[0];
        // Three frames of a second each third (no `AnimationFramerate`).
        let texture = AnimationSprites::new([frame(1), frame(2), frame(3)], 1000.0 / 3.0);
        let layout = PlayfieldLayout::new(640.0, 480.0);
        let mut batch = SpriteBatch::with_capacity(16);
        let textures_at = |batch: &mut SpriteBatch, time: f64| {
            batch.clear();
            follow.push(batch, &layout, time, &texture);
            let textures: Vec<u32> = batch.batches().iter().map(|b| b.texture.0).collect();
            (batch.len(), textures)
        };
        // Not before the first point fades in.
        assert_eq!(textures_at(&mut batch, point.fade_in).0, 0);
        // The first point is 400 ms into its animation: the second frame. The next fade in
        // 160 ms apart: the last one not yet.
        let (drawn, textures) = textures_at(&mut batch, point.fade_in + 400.0);
        assert_eq!(drawn, 3);
        assert_eq!(textures[0], 2);
        // Nothing without a texture.
        batch.clear();
        follow.push(
            &mut batch,
            &layout,
            point.fade_in + 400.0,
            &AnimationSprites::default(),
        );
        assert!(batch.is_empty());
    }
}
