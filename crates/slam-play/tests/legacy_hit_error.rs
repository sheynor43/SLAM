//! The legacy hit error meter: geometry, colours, the ticks' fade, the average arrow,
//! restarts and drawing without allocations.

slam_testkit::install_counting_allocator!();

use slam_osu::hit_windows::HitWindows;
use slam_osu::judgement::HitResult;
use slam_play::hit_error::{
    HitError, HitErrorMeter, HitErrorRing, HitErrorStyle, MAX_HIT_ERROR_SPRITES, MeterTextures,
    colour_for,
};
use slam_play::legacy_hit_error::{LegacyHitErrorView, MAX_LEGACY_METER_SPRITES, MAX_TICKS};
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Blend, Color, Extent, Origin};

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
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

/// OD 5: Great 49.5, Ok 99.5, Meh 149.5.
fn view() -> LegacyHitErrorView {
    LegacyHitErrorView::new(&HitWindows::new(5.0))
}

fn hit(time: f64, offset: f64, result: HitResult) -> HitError {
    HitError {
        time,
        offset,
        result,
    }
}

const WHITE: TextureId = TextureId(1);
const TRIANGLE: TextureId = TextureId(4);

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
        disc: region(TextureId(2)),
        gradient: region(TextureId(3)),
        triangle: region(TRIANGLE),
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

impl Quad {
    fn centre(&self) -> [f32; 2] {
        [
            (self.min[0] + self.max[0]) / 2.0,
            (self.min[1] + self.max[1]) / 2.0,
        ]
    }

    fn size(&self) -> [f32; 2] {
        [self.max[0] - self.min[0], self.max[1] - self.min[1]]
    }
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

fn same_rgb(quad: &Quad, colour: Color) -> bool {
    close(quad.colour[0], colour.r)
        && close(quad.colour[1], colour.g)
        && close(quad.colour[2], colour.b)
}

#[test]
fn the_default_style_is_legacy() {
    let windows = HitWindows::new(5.0);
    assert_eq!(HitErrorStyle::default(), HitErrorStyle::Legacy);
    let meter = HitErrorMeter::new(HitErrorStyle::default(), &windows, 1.0);
    assert_eq!(meter.style(), HitErrorStyle::Legacy);
    assert_eq!(
        HitErrorMeter::new(HitErrorStyle::Bar, &windows, 1.0).style(),
        HitErrorStyle::Bar
    );
    const { assert!(MAX_HIT_ERROR_SPRITES >= MAX_LEGACY_METER_SPRITES) };
}

#[test]
fn the_meter_is_stables_bar_at_the_bottom_centre() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    ring.push(hit(1000.0, -149.5, HitResult::Meh));
    view.update(&ring, 0, 1000.0);
    let mut batch = SpriteBatch::with_capacity(MAX_LEGACY_METER_SPRITES);
    // 1024 × 768: one virtual unit per pixel.
    view.push(&mut batch, 1024.0, 768.0, 1000.0, &textures());
    let quads = quads(&batch);
    // The background, Meh, Ok and Great bars, the centre line, a tick and the arrow.
    assert_eq!(quads.len(), 7);
    let middle = 768.0 - 19.2 / 2.0;

    // The background: 1.6 × the Meh window wide, 3 × 4 × 1.6 tall, black at 0.6.
    let background = &quads[0];
    assert!(background.texture == WHITE && background.blend == Blend::Alpha);
    assert_close!(background.colour[3], 0.6);
    assert!(same_rgb(background, Color::BLACK));
    assert_close!(background.centre()[0], 512.0);
    assert_close!(background.max[1], 768.0);
    assert_close!(background.size()[0], 149.5 * 1.6);
    assert_close!(background.size()[1], 19.2);

    // The bars, widest first, 3 × 1.6 tall across the middle.
    for (quad, (result, window)) in quads[1..4].iter().zip([
        (HitResult::Meh, 149.5),
        (HitResult::Ok, 99.5),
        (HitResult::Great, 49.5),
    ]) {
        assert!(same_rgb(quad, colour_for(result)), "{result:?}");
        assert_close!(quad.colour[3], 1.0);
        assert_close!(quad.size()[0], window * 1.6);
        assert_close!(quad.size()[1], 4.8);
        assert_close!(quad.centre()[0], 512.0);
        assert_close!(quad.centre()[1], middle);
    }
    assert!(same_rgb(&quads[3], Color::rgb(0.4, 0.8, 1.0)));

    // The white centre line.
    let line = &quads[4];
    assert!(same_rgb(line, Color::WHITE));
    assert_close!(line.size()[0], 2.4);
    assert_close!(line.size()[1], 19.2);
    assert_close!(line.centre()[0], 512.0);

    // The tick at the early end: 3 wide, the meter's height, additive, Meh at 0.4.
    let tick = &quads[5];
    assert!(tick.blend == Blend::Additive && same_rgb(tick, colour_for(HitResult::Meh)));
    assert_close!(tick.colour[3], 0.4);
    assert_close!(tick.centre()[0], 512.0 - 149.5 * 0.8);
    assert_close!(tick.size()[0], 3.0);
    assert_close!(tick.size()[1], 19.2);

    // The arrow hangs from the top edge, 10.2 × 4.8, still at the centre when it starts.
    let arrow = &quads[6];
    assert_eq!(arrow.texture, TRIANGLE);
    assert_close!(arrow.min[1], 768.0 - 19.2);
    assert_close!(arrow.size()[0], 10.2);
    assert_close!(arrow.size()[1], 4.8);
    assert_close!(arrow.centre()[0], 512.0);
}

#[test]
fn the_meter_scales_with_the_window_and_its_size_around_the_bottom_centre() {
    let view = view().sized(2.0);
    let mut batch = SpriteBatch::with_capacity(MAX_LEGACY_METER_SPRITES);
    view.push(&mut batch, 1920.0, 1080.0, 0.0, &textures());
    let quads = quads(&batch);
    let scale = 1080.0 / 768.0 * 2.0;
    let background = &quads[0];
    assert_close!(background.centre()[0], 960.0);
    assert_close!(background.max[1], 1080.0);
    assert_close!(background.size()[0], 149.5 * 1.6 * scale);
    assert_close!(background.size()[1], 19.2 * scale);
    // The arrow scales too.
    let arrow = quads.last().unwrap();
    assert_close!(arrow.size()[0], 10.2 * scale);
}

#[test]
fn ticks_start_at_0_4_and_fade_out_linearly_over_ten_seconds() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    ring.push(hit(1000.0, 30.0, HitResult::Great));
    view.update(&ring, 0, 1000.0);
    let tick = view.ticks()[0];
    assert_close!(tick.position, 30.0 / 149.5 / 2.0);
    assert_eq!(tick.alpha(999.0), 0.0);
    assert_close!(tick.alpha(1000.0), 0.4);
    assert_close!(tick.alpha(6000.0), 0.2);
    assert_close!(tick.alpha(10_999.0), 0.4 / 10_000.0);
    view.update(&ring, 0, 10_999.0);
    assert_eq!(view.ticks().len(), 1);
    view.update(&ring, 0, 11_000.0);
    assert!(view.ticks().is_empty());
}

#[test]
fn offsets_beyond_the_windows_clamp_to_the_ends() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    ring.push(hit(1000.0, 400.0, HitResult::Meh));
    ring.push(hit(1001.0, -400.0, HitResult::Meh));
    view.update(&ring, 0, 1001.0);
    let positions: Vec<f32> = view.ticks().iter().map(|t| t.position).collect();
    assert_eq!(positions, [0.5, -0.5]);
}

#[test]
fn the_arrow_eases_out_to_the_floating_error_in_800_ms() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    // A late Meh at the end: position 0.5, error 0.5 × 0.2.
    ring.push(hit(1000.0, 149.5, HitResult::Meh));
    view.update(&ring, 0, 1000.0);
    assert_close!(view.floating_error(), 0.1);
    assert_close!(view.arrow_position(1000.0), 0.0);
    // `Easing.Out` halfway: 1 - (1 - 0.5)² = 0.75.
    assert_close!(view.arrow_position(1400.0), 0.075);
    assert_close!(view.arrow_position(1800.0), 0.1);
    // An early hit 200 ms later moves on from where the arrow is.
    ring.push(hit(1200.0, -74.75, HitResult::Ok));
    view.update(&ring, 0, 1200.0);
    let from = 0.1 * (1.0 - 0.75 * 0.75);
    let error = 0.1 * 0.8 + -0.25 * 0.2;
    assert_close!(view.floating_error(), error);
    assert_close!(view.arrow_position(1200.0), from);
    assert_close!(view.arrow_position(2000.0), error);
}

#[test]
fn a_restart_clears_everything_and_a_clear_keeps_the_arrow() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    ring.push(hit(1000.0, 149.5, HitResult::Meh));
    view.update(&ring, 0, 2000.0);
    view.clear();
    assert!(view.ticks().is_empty());
    assert_close!(view.arrow_position(2000.0), 0.1);

    ring.restart();
    view.update(&ring, 1, 0.0);
    assert!(view.ticks().is_empty());
    assert_eq!(view.floating_error(), 0.0);
    assert_eq!(view.arrow_position(0.0), 0.0);
}

#[test]
fn beyond_the_most_ticks_the_oldest_goes() {
    let mut view = view();
    let mut ring = HitErrorRing::default();
    for i in 0..=MAX_TICKS as u32 {
        ring.push(hit(1000.0 + f64::from(i), 0.0, HitResult::Great));
        view.update(&ring, 0, 1000.0 + f64::from(i));
    }
    assert_eq!(view.ticks().len(), MAX_TICKS);
    assert_eq!(view.ticks()[0].time, 1001.0);
    assert_eq!(view.dropped(), 1);
}

#[test]
fn drawing_the_most_ticks_does_not_allocate() {
    let windows = HitWindows::new(5.0);
    let mut meter = HitErrorMeter::new(HitErrorStyle::Legacy, &windows, 1.0);
    let mut ring = HitErrorRing::default();
    for i in 0..MAX_TICKS as u32 {
        ring.push(hit(1000.0, f64::from(i) - 128.0, HitResult::Great));
        meter.update(&ring, 0, 1000.0);
    }
    let textures = textures();
    let mut batch = SpriteBatch::with_capacity(MAX_HIT_ERROR_SPRITES);
    slam_testkit::assert_no_alloc(|| {
        meter.update(&ring, 0, 1050.0);
        meter.push(&mut batch, 1920.0, 1080.0, 1050.0, &textures);
    });
    assert_eq!(batch.len(), MAX_LEGACY_METER_SPRITES);
}

#[test]
fn a_clear_empties_either_style() {
    let windows = HitWindows::new(5.0);
    let mut ring = HitErrorRing::default();
    ring.push(hit(1000.0, 10.0, HitResult::Great));
    for style in [HitErrorStyle::Legacy, HitErrorStyle::Bar] {
        let mut meter = HitErrorMeter::new(style, &windows, 1.0);
        meter.update(&ring, 0, 1000.0);
        meter.clear();
        let mut batch = SpriteBatch::with_capacity(MAX_HIT_ERROR_SPRITES);
        meter.push(&mut batch, 1024.0, 768.0, 1000.0, &textures());
        // No additive sprites: the ticks and lines are the only ones.
        assert!(
            quads(&batch).iter().all(|q| q.blend != Blend::Additive),
            "{style:?}"
        );
    }
}
