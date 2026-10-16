//! The hit error meter: which judgements it shows, the lines' places and transforms, the
//! average arrow, restarts and drawing.

mod common;

slam_testkit::install_counting_allocator!();

use slam_osu::hit_windows::HitWindows;
use slam_osu::judgement::{HitResult, Judgement, ObjectRef};
use slam_osu::objects::{NestedKind, OsuHitObjectKind};
use slam_play::hit_error::{
    HitError, HitErrorRing, HitErrorTargets, HitErrorView, MAX_LINES, MAX_METER_SPRITES,
    MeterTextures, colour_for, relative_position,
};
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Blend, Color, Extent, Origin};

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

macro_rules! assert_close {
    ($a:expr, $b:expr) => {
        assert!(
            close($a, $b),
            "{} = {}, expected {}",
            stringify!($a),
            $a,
            $b
        )
    };
}

// ---- Which judgements show ----

fn judgement(index: u32, nested: Option<u32>, time: f64, result: HitResult) -> Judgement {
    Judgement {
        object: ObjectRef { index, nested },
        time,
        result,
        max_result: HitResult::Great,
    }
}

#[test]
fn only_hits_of_circles_and_slider_heads_show() {
    let map = common::beatmap();
    let targets = HitErrorTargets::new(&map);
    let circle = map.hit_objects[0].start_time;
    let error = targets
        .error(&judgement(0, None, circle - 12.5, HitResult::Great))
        .unwrap();
    assert_eq!(
        error,
        HitError {
            time: circle - 12.5,
            offset: -12.5,
            result: HitResult::Great,
        }
    );
    // Misses do not show.
    assert!(
        targets
            .error(&judgement(0, None, circle + 400.0, HitResult::Miss))
            .is_none()
    );

    let OsuHitObjectKind::Slider(slider) = &map.hit_objects[1].kind else {
        panic!("object 1 is a slider");
    };
    let nested = |kind: fn(&NestedKind) -> bool| {
        slider.nested.iter().position(|n| kind(&n.kind)).unwrap() as u32
    };
    let head = nested(|k| matches!(k, NestedKind::Head));
    let tick = nested(|k| matches!(k, NestedKind::Tick { .. }));
    let tail = nested(|k| matches!(k, NestedKind::Tail { .. }));
    let head_time = slider.nested[head as usize].start_time;
    let error = targets
        .error(&judgement(1, Some(head), head_time + 30.0, HitResult::Ok))
        .unwrap();
    assert_eq!(error.offset, 30.0);
    // A classic slider head judges as a tick; it still has hit windows and shows.
    assert!(
        targets
            .error(&judgement(
                1,
                Some(head),
                head_time,
                HitResult::LargeTickHit
            ))
            .is_some()
    );
    // Ticks, tails, the slider itself and spinners have no hit windows.
    for (nested, result) in [
        (Some(tick), HitResult::LargeTickHit),
        (Some(tail), HitResult::SliderTailHit),
        (None, HitResult::Great),
    ] {
        assert!(
            targets
                .error(&judgement(1, nested, 2000.0, result))
                .is_none()
        );
    }
    assert!(matches!(
        map.hit_objects[4].kind,
        OsuHitObjectKind::Spinner(_)
    ));
    for (nested, result) in [
        (None, HitResult::Great),
        (Some(0), HitResult::LargeBonus),
        (Some(0), HitResult::SmallBonus),
    ] {
        assert!(
            targets
                .error(&judgement(4, nested, 5000.0, result))
                .is_none()
        );
    }
    assert!(
        targets
            .error(&judgement(99, None, 0.0, HitResult::Great))
            .is_none()
    );
}

#[test]
fn offsets_stop_at_the_miss_window() {
    let map = common::beatmap();
    let targets = HitErrorTargets::new(&map);
    let circle = map.hit_objects[0].start_time;
    let error = targets
        .error(&judgement(0, None, circle + 1000.0, HitResult::Meh))
        .unwrap();
    assert_eq!(error.offset, 400.0);
}

// ---- Lines and the arrow ----

/// OD 5: Great 49.5, Ok 99.5, Meh 149.5.
fn view() -> HitErrorView {
    HitErrorView::new(&HitWindows::new(5.0))
}

fn hit(time: f64, offset: f64, result: HitResult) -> HitError {
    HitError {
        time,
        offset,
        result,
    }
}

#[test]
fn lines_land_by_their_offset_scaled_to_the_meh_window() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    ring.push(hit(1000.0, 0.0, HitResult::Great));
    ring.push(hit(1100.0, -149.5, HitResult::Meh));
    ring.push(hit(1200.0, 74.75, HitResult::Ok));
    view.update(&ring, 0, 1200.0);
    let positions: Vec<_> = view.lines().iter().map(|l| l.position).collect();
    assert_eq!(positions, [0.5, 0.0, 0.75]);
    assert_eq!(relative_position(-1000.0, 149.5), 0.0);
    assert_eq!(relative_position(1000.0, 149.5), 1.0);
}

#[test]
fn lines_grow_in_then_fade_and_narrow_out() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    ring.push(hit(1000.0, 0.0, HitResult::Great));
    view.update(&ring, 0, 1000.0);
    let line = view.lines()[0];
    let at = |t: f64| line.pose(1000.0 + t);
    assert_close!(at(0.0).alpha, 0.0);
    assert_close!(at(0.0).width, 0.0);
    // OutQuint over 100 ms: half way is 1 - 0.5^5.
    assert_close!(at(50.0).alpha, 0.6 * (1.0 - 0.5f32.powi(5)));
    assert_close!(at(50.0).width, 1.0 - 0.5f32.powi(5));
    assert_close!(at(100.0).alpha, 0.6);
    assert_close!(at(100.0).width, 1.0);
    // Then linear fade and InQuint narrowing over 5 s.
    assert_close!(at(2600.0).alpha, 0.3);
    assert_close!(at(2600.0).width, 1.0 - 0.5f32.powi(5));
    assert_close!(at(5100.0).alpha, 0.0);
    assert_close!(at(5100.0).width, 0.0);
    assert_eq!(line.end(), 6100.0);
    view.update(&ring, 0, 6099.0);
    assert_eq!(view.lines().len(), 1);
    view.update(&ring, 0, 6100.0);
    assert!(view.lines().is_empty());
}

#[test]
fn over_fifty_lines_fade_the_oldest_quickly() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    for i in 0..52 {
        ring.push(hit(1000.0 + f64::from(i), 0.0, HitResult::Great));
        view.update(&ring, 0, 1000.0 + f64::from(i));
    }
    // The 52nd arrived with 51 alive: the first is cut, as lazer checks `Count > 50`.
    let lines = view.lines();
    assert_eq!(lines.len(), 52);
    assert_eq!(lines[0].cut, Some(1051.0));
    assert!(lines[1..].iter().all(|l| l.cut.is_none()));
    // It fades from where it was over 100 ms, keeping its width (still growing at 51 ms).
    let reached = lines[0].pose(1051.0);
    assert!(reached.width > 0.9 && reached.width < 1.0);
    assert_close!(lines[0].pose(1101.0).alpha, reached.alpha / 2.0);
    assert_close!(lines[0].pose(1101.0).width, reached.width);
    // The next one cuts the second: the first, fading already, ends too soon to count.
    ring.push(hit(1060.0, 0.0, HitResult::Great));
    view.update(&ring, 0, 1060.0);
    assert_eq!(view.lines()[1].cut, Some(1060.0));
    view.update(&ring, 0, 1151.0);
    assert_eq!(view.lines().len(), 52);
    assert_eq!(view.dropped(), 0);
}

#[test]
fn no_line_is_cut_when_all_end_within_the_lenience() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    for _ in 0..51 {
        ring.push(hit(1000.0, 0.0, HitResult::Great));
    }
    view.update(&ring, 0, 1000.0);
    // 51 alive, all ending exactly 110 ms after the next one: it cuts none.
    let now = view.lines()[0].end() - 110.0;
    ring.push(hit(now, 0.0, HitResult::Great));
    view.update(&ring, 0, now);
    assert_eq!(view.lines().len(), 52);
    assert!(view.lines().iter().all(|l| l.cut.is_none()));
    // A millisecond earlier, the first is cut.
    let mut view = self::view();
    let mut ring = HitErrorRing::default();
    for _ in 0..51 {
        ring.push(hit(1000.0, 0.0, HitResult::Great));
    }
    ring.push(hit(now - 1.0, 0.0, HitResult::Great));
    view.update(&ring, 0, now - 1.0);
    assert_eq!(view.lines()[0].cut, Some(now - 1.0));
}

#[test]
fn the_most_lines_drop_the_oldest() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    // Many in one moment: the cut ones are still alive when the next arrive.
    for i in 0..MAX_LINES as u32 + 3 {
        ring.push(hit(1000.0, f64::from(i), HitResult::Great));
        view.update(&ring, 0, 1000.0);
    }
    assert_eq!(view.lines().len(), MAX_LINES);
    assert_eq!(view.dropped(), 3);
    assert_eq!(view.lines()[0].position, relative_position(3.0, 149.5));
}

#[test]
fn the_arrow_follows_a_moving_average() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    assert_eq!(view.arrow_position(0.0), 0.5);
    ring.push(hit(1000.0, 100.0, HitResult::Ok));
    view.update(&ring, 0, 1000.0);
    assert_eq!(view.floating_average(), 10.0);
    let target = relative_position(10.0, 149.5);
    assert_eq!(view.arrow_position(1000.0), 0.5);
    assert_close!(
        view.arrow_position(1400.0),
        0.5 + (target - 0.5) * (1.0 - 0.5f32.powi(5))
    );
    assert_eq!(view.arrow_position(1800.0), target);
    // A new judgement moves on from where the arrow is.
    ring.push(hit(1400.0, -50.0, HitResult::Ok));
    view.update(&ring, 0, 1400.0);
    assert_close!(view.floating_average() as f32, 4.0);
    let from = view.arrow_position(1400.0);
    assert!(from > 0.5 && from < target);
    assert_close!(view.arrow_position(2200.0), relative_position(4.0, 149.5));
}

#[test]
fn a_restart_clears_the_meter() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    ring.push(hit(1000.0, 100.0, HitResult::Ok));
    view.update(&ring, 0, 1000.0);
    assert_eq!(view.lines().len(), 1);
    ring.restart();
    view.update(&ring, 1, 0.0);
    assert!(view.lines().is_empty());
    assert_eq!(view.floating_average(), 0.0);
    assert_eq!(view.arrow_position(0.0), 0.5);
    // Judgements of the previous attempt the view had not taken stay unseen.
    let mut ring = HitErrorRing::default();
    let mut view = self::view();
    ring.push(hit(1000.0, 100.0, HitResult::Ok));
    ring.restart();
    view.update(&ring, 1, 1000.0);
    assert!(view.lines().is_empty());
}

// ---- Drawing ----

const WHITE: TextureId = TextureId(1);
const DISC: TextureId = TextureId(2);
const GRADIENT: TextureId = TextureId(3);

fn region(texture: TextureId) -> AtlasRegion {
    AtlasRegion {
        texture,
        origin: Origin { x: 0, y: 0 },
        size: Extent {
            width: 4,
            height: 4,
        },
        uv: UvRect::FULL,
    }
}

fn textures() -> MeterTextures {
    MeterTextures {
        white: region(WHITE),
        disc: region(DISC),
        gradient: region(GRADIENT),
    }
}

/// A recorded sprite's texture, blend, colour and bounds.
#[derive(Debug, Clone, Copy)]
struct Quad {
    texture: TextureId,
    blend: Blend,
    colour: [f32; 4],
    min: [f32; 2],
    max: [f32; 2],
}

fn quads(batch: &SpriteBatch) -> Vec<Quad> {
    let v = batch.vertices();
    batch
        .batches()
        .iter()
        .flat_map(|b| {
            (b.first_quad..b.first_quad + b.quads).map(move |q| {
                let c = &v[q as usize * 4..q as usize * 4 + 4];
                let xs = c.iter().map(|v| v.position[0]);
                let ys = c.iter().map(|v| v.position[1]);
                Quad {
                    texture: b.texture,
                    blend: b.blend,
                    colour: c[0].colour,
                    min: [
                        xs.clone().fold(f32::MAX, f32::min),
                        ys.clone().fold(f32::MAX, f32::min),
                    ],
                    max: [xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max)],
                }
            })
        })
        .collect()
}

fn same_colour(quad: &Quad, colour: Color) -> bool {
    close(quad.colour[0], colour.r)
        && close(quad.colour[1], colour.g)
        && close(quad.colour[2], colour.b)
}

#[test]
fn the_meter_is_a_bar_at_the_bottom_centre_with_zones_lines_and_arrow() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    ring.push(hit(1000.0, -149.5, HitResult::Meh));
    view.update(&ring, 0, 1100.0);
    let mut batch = SpriteBatch::with_capacity(MAX_METER_SPRITES);
    // 1024 × 768: one virtual unit per pixel.
    view.push(&mut batch, 1024.0, 768.0, 1100.0, &textures());
    let quads = quads(&batch);
    // The centre marker behind, two Meh parts, Ok and Great each side, a line (two caps and
    // the straight part), the marker in front and the chevron's two strokes.
    assert_eq!(quads.len(), 1 + 2 * 4 + 3 + 1 + 2);
    let (meh, ok, great) = (
        colour_for(HitResult::Meh),
        colour_for(HitResult::Ok),
        colour_for(HitResult::Great),
    );
    assert!(same_colour(&quads[0], great) && quads[0].texture == DISC);
    // Bars are 200 long, centred at x = 512, on the axis 15 below the meter's top edge.
    let bars = &quads[1..9];
    let left = 512.0 - 100.0;
    let axis = 768.0 - 10.0 - 24.0 + 15.0;
    for bar in bars {
        assert_close!((bar.min[1] + bar.max[1]) / 2.0, axis);
        assert_close!(bar.max[1] - bar.min[1], 2.0);
    }
    // The early Meh bar: solid from the middle out to 80 %, then the gradient to the end.
    assert!(same_colour(&bars[0], meh) && bars[0].texture == WHITE);
    assert_close!(bars[0].max[0], 512.0);
    assert_close!(bars[0].min[0], 512.0 - 80.0);
    assert!(bars[1].texture == GRADIENT && same_colour(&bars[1], meh));
    assert_close!(bars[1].min[0], left);
    assert_close!(bars[1].max[0], 512.0 - 80.0);
    // Ok and Great by their windows' share of Meh's.
    assert!(same_colour(&bars[2], ok));
    assert_close!(bars[2].min[0], 512.0 - 100.0 * 99.5 / 149.5);
    assert!(same_colour(&bars[3], great));
    assert_close!(bars[3].min[0], 512.0 - 100.0 * 49.5 / 149.5);
    // The late side mirrors it.
    assert_close!(bars[4].min[0], 512.0);
    assert_close!(bars[5].max[0], 512.0 + 100.0);
    // The line at the early end: 4 wide, 14 tall, additive, Meh's colour at 0.6.
    let line = &quads[9..12];
    assert!(
        line.iter()
            .all(|q| q.blend == Blend::Additive && same_colour(q, meh))
    );
    assert_close!(line[0].colour[3], 0.6);
    assert_close!(line[0].min[0], left - 2.0);
    assert_close!(line[2].max[0] - line[0].min[0], 4.0);
    assert_close!(line[2].max[1] - line[0].min[1], 14.0);
    // The arrow sits above the bars, still near the middle (it moves for 800 ms).
    let arrow = &quads[13..];
    for stroke in arrow {
        assert!(stroke.max[1] < axis - 7.0 + 0.5, "{stroke:?}");
        assert!(stroke.min[0] > 400.0 && stroke.max[0] < 520.0, "{stroke:?}");
    }
}

#[test]
fn the_meter_scales_with_the_window_by_its_smaller_ratio() {
    let view = view();
    let mut batch = SpriteBatch::with_capacity(MAX_METER_SPRITES);
    view.push(&mut batch, 3840.0, 1080.0, 0.0, &textures());
    let quads = quads(&batch);
    let scale = 1080.0 / 768.0;
    let bars = &quads[1..9];
    let min = bars.iter().map(|q| q.min[0]).fold(f32::MAX, f32::min);
    let max = bars.iter().map(|q| q.max[0]).fold(f32::MIN, f32::max);
    assert_close!(min, 1920.0 - 100.0 * scale);
    assert_close!(max, 1920.0 + 100.0 * scale);
}

#[test]
fn drawing_the_most_lines_does_not_allocate() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    for i in 0..MAX_LINES as u32 {
        ring.push(hit(1000.0, f64::from(i) - 64.0, HitResult::Great));
        view.update(&ring, 0, 1000.0);
    }
    let textures = textures();
    let mut batch = SpriteBatch::with_capacity(MAX_METER_SPRITES);
    slam_testkit::assert_no_alloc(|| {
        view.update(&ring, 0, 1050.0);
        view.push(&mut batch, 1920.0, 1080.0, 1050.0, &textures);
    });
    assert_eq!(view.lines().len(), MAX_LINES);
    assert!(batch.len() <= MAX_METER_SPRITES);
}
