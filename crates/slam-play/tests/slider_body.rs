//! Slider bodies: the classic skin's colours and gradient, snaking ranges, and the
//! mesh clipped like lazer's `SliderPath.GetPathToProgress`.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use slam_osu::objects::{OsuHitObject, OsuHitObjectKind, Slider};
use slam_osu::{Beatmap, Vec2};
use slam_play::slider_body::{
    LegacyBodyColours, Snaking, body_mesh, range_distances, snaking_range,
};
use slam_skin::{Skin, SkinChain};

/// AR 9 (preempt 600 ms); a Bézier slider with one repeat (two spans of 1500 ms)
/// and a straight one without.
const MAP: &str = "osu file format v14

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:5
ApproachRate:9
SliderMultiplier:1
SliderTickRate:1

[TimingPoints]
0,500,4,2,0,100,1,0

COLOURS
[HitObjects]
100,100,1000,6,0,B|200:0|300:200|150:250,2,300
100,300,5000,2,0,L|300:300,1,200
100,200,7000,2,0,L|200:200|200:200,1,150
300,300,9000,2,0,L|300:300,1,0
100,100,10000,2,0,L|200:100,3,100
";

fn beatmap(colours: &str) -> (Beatmap, String) {
    let osu = MAP.replace("COLOURS", colours);
    (Beatmap::decode(osu.as_bytes()).unwrap(), osu)
}

fn slider(object: &OsuHitObject) -> &Slider {
    match &object.kind {
        OsuHitObjectKind::Slider(s) => s,
        _ => panic!("not a slider"),
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str, skin_ini: Option<&str>) -> TempDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "slam-slider-body-{}-{}-{name}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        if let Some(ini) = skin_ini {
            fs::write(dir.join("skin.ini"), ini).unwrap();
        }
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[track_caller]
fn assert_colour(actual: [f32; 4], expected: [f32; 4]) {
    let close = actual
        .iter()
        .zip(expected)
        .all(|(a, e)| (a - e).abs() < 1e-6);
    assert!(close, "colour {actual:?}, expected {expected:?}");
}

/// Body colours with optional `[Colours]` in the beatmap and the user's skin.ini.
fn colours(beatmap_colours: &str, user_ini: Option<&str>) -> LegacyBodyColours {
    let (_, osu) = beatmap(beatmap_colours);
    let beatmap_dir = TempDir::new("beatmap", None);
    let user_dir = TempDir::new("user", user_ini);
    let default_dir = TempDir::new("default", None);
    let beatmap_skin = Skin::load_beatmap(&beatmap_dir.0, osu.as_bytes()).unwrap();
    let chain = SkinChain::new(
        Some(Skin::load(&user_dir.0).unwrap()),
        Skin::load(&default_dir.0).unwrap(),
    );
    LegacyBodyColours::new([1.0, 192.0 / 255.0, 0.0, 1.0], Some(&beatmap_skin), &chain)
}

#[test]
fn combo_colour_at_track_alpha_and_a_white_border_by_default() {
    let c = colours("", None);
    assert_colour(c.accent, [1.0, 192.0 / 255.0, 0.0, 0.7]);
    assert_colour(c.border, [1.0; 4]);
}

#[test]
fn skin_overrides_track_and_border_and_the_beatmap_comes_first() {
    let user = "[Colours]\nSliderTrackOverride: 0,53,69\nSliderBorder: 0,255,255\n";
    let c = colours("", Some(user));
    assert_colour(c.accent, [0.0, 53.0 / 255.0, 69.0 / 255.0, 0.7]);
    assert_colour(c.border, [0.0, 1.0, 1.0, 1.0]);

    let beatmap = "[Colours]\nSliderBorder: 80,80,80\n";
    let c = colours(beatmap, Some(user));
    assert_colour(c.accent, [0.0, 53.0 / 255.0, 69.0 / 255.0, 0.7]);
    let grey = 80.0 / 255.0;
    assert_colour(c.border, [grey, grey, grey, 1.0]);
}

#[test]
fn classic_colour_runs_shadow_border_then_track() {
    let c = LegacyBodyColours {
        accent: [1.0, 192.0 / 255.0, 0.0, 0.7],
        border: [0.5, 0.5, 0.5, 1.0],
    };
    // Shadow: transparent black to black at 0.25 over the outer 5/64.
    assert_colour(c.colour_at(0.0), [0.0; 4]);
    assert_colour(c.colour_at(0.078125 / 2.0), [0.0, 0.0, 0.0, 0.125]);
    assert_colour(c.colour_at(0.078125), [0.0, 0.0, 0.0, 0.25]);
    // Border up to 0.1875.
    assert_colour(c.colour_at(0.1), c.border);
    assert_colour(c.colour_at(0.1875), c.border);
    // Track: the accent darkened by 0.1 at the border, lightened at the centre.
    let outer = [1.0 / 1.1, 192.0 / 255.0 / 1.1, 0.0, 0.7];
    let inner = [1.0, 1.0, 0.25, 0.7];
    let just_inside = c.colour_at(0.1875 + 1e-6);
    assert!(
        just_inside
            .iter()
            .zip(outer)
            .all(|(a, e)| (a - e).abs() < 1e-5)
    );
    assert_colour(c.colour_at(1.0), inner);
    let mid = c.colour_at((0.1875 + 1.0) / 2.0);
    for i in 0..4 {
        assert!(
            (mid[i] - (outer[i] + inner[i]) / 2.0).abs() < 1e-5,
            "channel {i}"
        );
    }
}

#[test]
fn classic_gradient_has_the_texture_width_of_the_radius() {
    let (map, _) = beatmap("");
    let radius = slam_osu::objects::OBJECT_RADIUS * map.hit_objects[0].defaults.scale;
    let c = colours("", None);
    let g = c.gradient(radius);
    // CS 4: radius 36.48 → 72 texels.
    assert_eq!(g.width(), 72);
    // Edge texel: transparent; centre texel: the lightened track.
    assert_eq!(g.texel(0), [0, 0, 0, 0]);
    assert_eq!(g.texel(71), [255, 255, 64, 179]);
}

fn range(map: &Beatmap, index: usize, time: f64, head_hit: bool, snaking: Snaking) -> (f64, f64) {
    let object = &map.hit_objects[index];
    snaking_range(
        slider(object),
        object.start_time,
        object.defaults.time_preempt,
        time,
        head_hit,
        snaking,
    )
}

#[test]
fn snaking_in_grows_over_a_third_of_the_preempt() {
    let (map, _) = beatmap("");
    assert_eq!(map.hit_objects[0].defaults.time_preempt, 600.0);
    let r = |t| range(&map, 0, t, false, Snaking::default());
    assert_eq!(r(300.0), (0.0, 0.0));
    assert_eq!(r(400.0), (0.0, 0.0));
    assert_eq!(r(500.0), (0.0, 0.5));
    assert_eq!(r(600.0), (0.0, 1.0));
    assert_eq!(r(1500.0), (0.0, 1.0));
}

#[test]
fn snaking_out_follows_the_ball_on_the_last_span() {
    let (map, _) = beatmap("");
    let r = |t, hit| range(&map, 0, t, hit, Snaking::default());
    // First of two spans: the whole body.
    assert_eq!(r(1500.0, true), (0.0, 1.0));
    // Second span runs back to the head: the end shrinks towards it.
    assert_eq!(r(2500.0, true), (0.0, 1.0));
    assert_eq!(r(3250.0, true), (0.0, 0.5));
    assert_eq!(r(4000.0, true), (0.0, 0.0));
    // Until the head is hit, progress stays 0.
    assert_eq!(r(3250.0, false), (0.0, 1.0));

    // One span: the start follows the ball to the end.
    let r = |t| range(&map, 1, t, true, Snaking::default());
    assert_eq!(r(5000.0), (0.0, 1.0));
    assert_eq!(r(5500.0), (0.5, 1.0));
    assert_eq!(r(6000.0), (1.0, 1.0));
}

#[test]
fn three_spans_snake_out_only_on_the_last() {
    let (map, _) = beatmap("");
    // Three spans of 500 ms from 10000: out, back, out.
    let r = |t| range(&map, 4, t, true, Snaking::default());
    assert_eq!(r(10250.0), (0.0, 1.0));
    assert_eq!(r(10750.0), (0.0, 1.0));
    // The last span runs out again: the start follows the ball.
    assert_eq!(r(11250.0), (0.5, 1.0));
    assert_eq!(r(11500.0), (1.0, 1.0));
}

#[test]
fn a_zero_length_slider_is_a_circle_at_its_head() {
    let (map, _) = beatmap("");
    let object = &map.hit_objects[3];
    let path = &slider(object).path;
    // Zero duration: progress is NaN at the start time, as in lazer.
    let p = range(&map, 3, object.start_time, true, Snaking::default());
    let mesh = body_mesh(path);
    let (d0, d1) = range_distances(path, (p.0, p.1));
    let drawn: Vec<_> = mesh.clipped(d0, d1).collect();
    assert!(!drawn.is_empty());
    let head = path.calculated_path()[0];
    for (a, b) in drawn {
        assert_eq!(a, b);
        assert_eq!(a, [head.x, head.y]);
    }
}

#[test]
fn a_repeated_end_point_keeps_one_length_per_point() {
    // The last two points coincide and the expected length is longer, so lazer skips
    // the extension and keeps an extra length.
    let (map, _) = beatmap("");
    let path = &slider(&map.hit_objects[2]).path;
    assert_eq!(
        path.cumulative_length().len(),
        path.calculated_path().len() + 1
    );
    let mesh = body_mesh(path);
    assert_eq!(mesh.segments(), path.calculated_path().len() - 1);
    assert_eq!(mesh.clipped(0.0, mesh.length()).count(), mesh.segments());
}

#[test]
fn snaking_can_be_turned_off() {
    let (map, _) = beatmap("");
    let off = Snaking {
        snaking_in: false,
        snaking_out: false,
    };
    assert_eq!(range(&map, 0, 400.0, false, off), (0.0, 1.0));
    assert_eq!(range(&map, 0, 3250.0, true, off), (0.0, 1.0));
    assert_eq!(range(&map, 1, 5500.0, true, off), (0.0, 1.0));
}

/// The polyline a mesh draws for a range, without zero-length steps.
fn mesh_polyline(segments: &[([f32; 2], [f32; 2])]) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = Vec::new();
    for &(a, b) in segments {
        for p in [a, b] {
            if out
                .last()
                .is_none_or(|q| (q[0] - p[0]).abs() + (q[1] - p[1]).abs() > 1e-3)
            {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn the_mesh_clips_like_get_path_to_progress() {
    let (map, _) = beatmap("");
    let path = &slider(&map.hit_objects[0]).path;
    let mesh = body_mesh(path);
    assert_eq!(mesh.segments(), path.calculated_path().len() - 1);
    let mut lazer: Vec<Vec2> = Vec::with_capacity(path.calculated_path().len() + 2);
    let vertex = path.cumulative_length()[3] / path.distance();
    for progress in [
        (0.0, 1.0),
        (0.1, 0.7),
        (0.5, 1.0),
        (0.0, 0.25),
        (0.3, 0.3),
        (1.0, 1.0),
        (vertex, 1.0),
        (0.0, vertex),
    ] {
        let (d0, d1) = range_distances(path, progress);
        let ours = mesh_polyline(&mesh.clipped(d0, d1).collect::<Vec<_>>());
        path.path_to_progress(&mut lazer, progress.0, progress.1);
        let theirs = mesh_polyline(
            &lazer
                .windows(2)
                .map(|w| ([w[0].x, w[0].y], [w[1].x, w[1].y]))
                .collect::<Vec<_>>(),
        );
        assert!(!ours.is_empty());
        let theirs = if theirs.is_empty() {
            vec![[lazer[0].x, lazer[0].y]]
        } else {
            theirs
        };
        assert_eq!(ours.len(), theirs.len(), "{progress:?}");
        for (a, b) in ours.iter().zip(&theirs) {
            assert!(
                (a[0] - b[0]).abs() < 1e-2 && (a[1] - b[1]).abs() < 1e-2,
                "{progress:?}: {a:?} vs {b:?}"
            );
        }
    }
}
