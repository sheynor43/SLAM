//! Judgement popups and hit lighting: which judgements show, their transforms at key moments,
//! the alive popups and the skin lookup.

mod common;

slam_testkit::install_counting_allocator!();

use std::fs;

use slam_osu::judgement::{HitResult, Judgement, ObjectRef};
use slam_osu::objects::{NestedKind, OsuHitObjectKind};
use slam_osu::{Mod, ModSet, Vec2};
use slam_play::circles::{CircleSettings, SkinSprite};
use slam_play::judgements::{
    JudgementRing, JudgementStyle, JudgementTextures, JudgementView, MAX_POPUPS, PARTICLE_COUNT,
    PieceKind, PieceSprites, PopupPose, PopupTargets, ShownJudgement, particle, piece_sources,
    popup_pose,
};
use slam_render::Blend;
use slam_render::atlas::UvRect;
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_skin::{Skin, SkinChain};

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

const STYLE: JudgementStyle = JudgementStyle {
    miss_falls: true,
    hit_lighting: true,
};

fn shown(result: HitResult, time: f64) -> ShownJudgement {
    ShownJudgement {
        object: ObjectRef {
            index: 3,
            nested: None,
        },
        time,
        result,
        position: Vec2::new(100.0, 100.0),
        scale: 0.5,
    }
}

fn pose(result: HitResult, kind: PieceKind, animated: bool, t: f64) -> PopupPose {
    popup_pose(&shown(result, 1000.0), kind, animated, &STYLE, 1000.0 + t)
}

// ---- Transforms ----

#[test]
fn old_hits_pop_in_then_fade_out() {
    let at = |t| pose(HitResult::Great, PieceKind::Old, false, t).main;
    assert_close!(at(0.0).alpha, 0.0);
    assert_close!(at(0.0).scale, 0.6);
    assert_close!(at(60.0).alpha, 0.5);
    // 0.6 → 1.1 over 96 ms, then 0.9 at 144, forced 0.95, then 1 at 168.
    assert_close!(at(48.0).scale, 0.85);
    assert_close!(at(96.0).scale, 1.1);
    assert_close!(at(120.0).scale, 1.1);
    assert_close!(at(120.0).alpha, 1.0);
    assert_close!(at(132.0).scale, 1.0);
    assert_close!(at(156.0).scale, 0.975);
    assert_close!(at(168.0).scale, 1.0);
    assert_close!(at(500.0).alpha, 1.0);
    assert_close!(at(800.0).alpha, 0.5);
    assert_close!(at(1100.0).alpha, 0.0);
    assert_eq!(at(300.0).rotation, 0.0);
    assert_eq!(at(300.0).offset, Vec2::ZERO);
}

#[test]
fn misses_shrink_turn_and_fall() {
    let at = |t| pose(HitResult::Miss, PieceKind::Old, false, t).main;
    assert_close!(at(0.0).scale, 1.6);
    assert_close!(at(50.0).scale, 1.6 - 0.6 * 0.25);
    assert_close!(at(100.0).scale, 1.0);
    assert_close!(at(0.0).offset.y, -5.0);
    // `Easing.In` over the whole 1100 ms, 80 osu!pixels down.
    assert_close!(at(550.0).offset.y, -5.0 + 80.0 * 0.25);
    assert_close!(at(1100.0).offset.y, 75.0);
    let r = at(120.0).rotation;
    assert!(r.abs() <= 8.6, "{r}");
    assert_close!(at(0.0).rotation, 0.0);
    assert_close!(at(60.0).rotation, r / 2.0);
    assert_close!(at(1100.0).rotation, 2.0 * r);
    assert_close!(at(1100.0).alpha, 0.0);
}

#[test]
fn misses_fall_only_for_skins_above_version_1() {
    let style = JudgementStyle {
        miss_falls: false,
        ..STYLE
    };
    let j = shown(HitResult::Miss, 0.0);
    let p = popup_pose(&j, PieceKind::Old, false, &style, 600.0).main;
    assert_eq!(p.offset, Vec2::ZERO);
    assert!(p.rotation != 0.0);
}

#[test]
fn miss_rotation_is_the_same_every_frame_and_varies_between_misses() {
    let rotation = |time: f64| {
        let j = shown(HitResult::Miss, time);
        popup_pose(&j, PieceKind::Old, false, &STYLE, time + 120.0)
            .main
            .rotation
    };
    assert_eq!(rotation(1000.0), rotation(1000.0));
    let distinct: Vec<f32> = (0..20).map(|i| rotation(1000.0 + f64::from(i))).collect();
    assert!(distinct.iter().any(|&r| r > 0.0) && distinct.iter().any(|&r| r < 0.0));
    assert!(distinct.iter().all(|r| r.abs() <= 8.6));
}

#[test]
fn missed_ticks_fade_out_early() {
    for result in [HitResult::LargeTickMiss, HitResult::IgnoreMiss] {
        let at = |t| pose(result, PieceKind::Old, false, t).main;
        assert_close!(at(0.0).scale, 1.2);
        assert_close!(at(100.0).scale, 1.0);
        assert_close!(at(250.0).alpha, 1.0);
        assert_close!(at(550.0).alpha, 0.5);
        assert_close!(at(850.0).alpha, 0.0);
        assert_eq!(at(300.0).rotation, 0.0);
        assert_eq!(at(300.0).offset, Vec2::ZERO);
    }
}

#[test]
fn animated_old_pieces_only_fade() {
    let at = |t| pose(HitResult::Miss, PieceKind::Old, true, t).main;
    assert_close!(at(0.0).scale, 1.0);
    assert_close!(at(60.0).alpha, 0.5);
    assert_close!(at(600.0).rotation, 0.0);
    assert_eq!(at(600.0).offset, Vec2::ZERO);
    assert_close!(at(800.0).alpha, 0.5);
}

#[test]
fn new_hits_grow_with_a_brief_additive_old_style_and_particles() {
    let at = |t| pose(HitResult::Ok, PieceKind::New, false, t);
    assert_close!(at(0.0).main.scale, 0.9);
    assert_close!(at(550.0).main.scale, 0.975);
    assert_close!(at(1100.0).main.scale, 1.05);
    assert_close!(at(60.0).main.alpha, 0.5);
    assert_close!(at(1100.0).main.alpha, 0.0);
    assert_eq!(at(300.0).main.rotation, 0.0);

    // The temporary old style: from 0 to 0.5 over 16 ms before to 40 ms after, then out by 340,
    // within the piece's own fade-in.
    let temporary = |t| at(t).temporary.unwrap();
    let out = |x: f32| x * (2.0 - x);
    assert_close!(temporary(0.0).alpha, 0.0);
    assert_close!(
        temporary(20.0).alpha,
        0.5 * out(36.0 / 56.0) * (20.0 / 120.0)
    );
    assert_close!(temporary(190.0).alpha, 0.25);
    assert_close!(temporary(340.0).alpha, 0.0);
    // Its scale ends at 1.05.
    assert_close!(temporary(0.0).scale, 0.6);
    assert_close!(temporary(168.0).scale, 1.05);

    // The particles start 100 ms in, fading out over 1600 ms, within the piece's fades.
    assert_close!(at(0.0).particles.0, 0.0);
    assert_eq!(at(0.0).particles.1, 100.0);
    assert_close!(at(300.0).particles.0, 1.0 - 400.0 / 1600.0);
    assert_close!(at(800.0).particles.0, 0.5 * (1.0 - 900.0 / 1600.0));
}

#[test]
fn animated_new_pieces_keep_their_scale_but_not_the_temporary_one() {
    let at = |t| pose(HitResult::Great, PieceKind::New, true, t);
    assert_close!(at(0.0).main.scale, 1.0);
    assert_close!(at(1000.0).main.scale, 1.0);
    assert_close!(at(0.0).temporary.unwrap().scale, 0.6);
}

#[test]
fn hit_ticks_of_old_skins_rise_then_vanish() {
    for result in [HitResult::LargeTickHit, HitResult::SliderTailHit] {
        let at = |t| pose(result, PieceKind::TickHit, false, t).main;
        assert_close!(at(0.0).alpha, 1.0);
        assert_close!(at(0.0).offset.y, 0.0);
        assert_close!(at(150.0).offset.y, -7.5);
        assert_close!(at(300.0).offset.y, -10.0);
        assert_close!(at(330.0).alpha, 0.5);
        assert_close!(at(360.0).alpha, 0.0);
    }
}

#[test]
fn hit_lighting_lights_hits_but_not_ticks_or_misses() {
    let light = |result, t| pose(result, PieceKind::Old, false, t).lighting;
    assert_eq!(light(HitResult::Great, 0.0), (0.0, 0.8));
    assert_close!(light(HitResult::Meh, 100.0).0, 0.5);
    assert_close!(light(HitResult::Ok, 400.0).0, 1.0);
    assert_close!(light(HitResult::Ok, 900.0).0, 0.5);
    assert_close!(light(HitResult::Ok, 1400.0).0, 0.0);
    assert_close!(light(HitResult::Great, 300.0).1, 0.8 + 0.4 * 0.75);
    assert_close!(light(HitResult::Great, 600.0).1, 1.2);
    for result in [
        HitResult::Miss,
        HitResult::LargeTickHit,
        HitResult::SliderTailHit,
        HitResult::LargeTickMiss,
    ] {
        assert_eq!(light(result, 300.0).0, 0.0, "{result:?}");
    }
    let off = JudgementStyle {
        hit_lighting: false,
        ..STYLE
    };
    let j = shown(HitResult::Great, 0.0);
    assert_eq!(
        popup_pose(&j, PieceKind::Old, false, &off, 300.0)
            .lighting
            .0,
        0.0
    );
}

#[test]
fn particles_spread_within_their_area_and_fade_by_their_end() {
    let j = shown(HitResult::Great, 1000.0);
    for i in 0..PARTICLE_COUNT {
        let (start, alpha) = particle(&j, i, 0.0);
        assert_eq!((start, alpha), (Vec2::ZERO, 1.0));
        let (end, alpha) = particle(&j, i, 1600.0);
        assert_eq!(alpha, 0.0);
        assert!(end.length() <= 70.0 + 1e-3, "{end:?}");
        assert_eq!(particle(&j, i, 800.0), particle(&j, i, 800.0));
    }
    // A third of the duration at least: none is done at 1600 / 3.
    assert!((0..PARTICLE_COUNT).all(|i| particle(&j, i, 500.0).1 > 0.0));
    // Spread in every direction.
    let ends: Vec<Vec2> = (0..PARTICLE_COUNT)
        .map(|i| particle(&j, i, 1600.0).0)
        .collect();
    assert!(ends.iter().any(|p| p.x > 10.0) && ends.iter().any(|p| p.x < -10.0));
    assert!(ends.iter().any(|p| p.y > 10.0) && ends.iter().any(|p| p.y < -10.0));
}

// ---- Which judgements show ----

fn judgement(index: u32, nested: Option<u32>, result: HitResult) -> Judgement {
    Judgement {
        object: ObjectRef { index, nested },
        time: 1000.0,
        result,
        max_result: HitResult::Great,
    }
}

#[test]
fn popups_show_where_lazer_shows_them() {
    let map = common::beatmap();
    let targets = PopupTargets::new(&map, &ModSet::default());
    // Circle 0.
    let circle = targets
        .shown(&judgement(0, None, HitResult::Great))
        .unwrap();
    assert_eq!(circle.position, map.hit_objects[0].stacked_position());
    assert_eq!(circle.scale, map.hit_objects[0].defaults.scale);
    // Small ticks and combo breaks show nothing.
    for result in [
        HitResult::SmallTickHit,
        HitResult::SmallTickMiss,
        HitResult::ComboBreak,
    ] {
        assert!(targets.shown(&judgement(0, None, result)).is_none());
    }

    // Slider 1, of two spans: its tail sits back at the path's start.
    let OsuHitObjectKind::Slider(slider) = &map.hit_objects[1].kind else {
        panic!("object 1 is a slider");
    };
    let tail = slider
        .nested
        .iter()
        .position(|n| matches!(n.kind, NestedKind::Tail { .. }))
        .unwrap() as u32;
    let start = map.hit_objects[1].stacked_position();
    let shown = targets
        .shown(&judgement(1, Some(tail), HitResult::SliderTailHit))
        .unwrap();
    assert_eq!(shown.position, start);
    let tick = slider
        .nested
        .iter()
        .position(|n| matches!(n.kind, NestedKind::Tick { .. }))
        .unwrap();
    let shown = targets
        .shown(&judgement(1, Some(tick as u32), HitResult::LargeTickMiss))
        .unwrap();
    assert_eq!(shown.position, slider.nested[tick].stacked_position());
    // The slider's own result shows only with classic slider behaviour.
    assert!(
        targets
            .shown(&judgement(1, None, HitResult::Great))
            .is_none()
    );
    let classic = ModSet::new(vec![Mod::from_acronym("CL")]).unwrap();
    let classic = PopupTargets::new(&map, &classic);
    let whole = classic
        .shown(&judgement(1, None, HitResult::Great))
        .unwrap();
    assert_eq!(whole.position, start);

    // The spinner shows its own result, not its ticks'.
    assert!(targets.shown(&judgement(4, None, HitResult::Ok)).is_some());
    assert!(
        targets
            .shown(&judgement(4, Some(0), HitResult::LargeTickHit))
            .is_none()
    );
    // Beyond the objects.
    assert!(
        targets
            .shown(&judgement(99, None, HitResult::Great))
            .is_none()
    );
}

// ---- Alive popups ----

fn view() -> JudgementView {
    let map = common::beatmap();
    let dir = common::skin::SampleDir::new(&[]);
    let chain = SkinChain::new(None, Skin::load(&dir.0).unwrap());
    JudgementView::new(&map, CircleSettings::default(), None, &chain)
}

fn on(index: u32, time: f64, result: HitResult) -> ShownJudgement {
    ShownJudgement {
        object: ObjectRef {
            index,
            nested: None,
        },
        ..shown(result, time)
    }
}

#[test]
fn a_new_popup_of_an_object_replaces_its_old_ones() {
    let mut view = view();
    let mut ring = JudgementRing::default();
    ring.push(on(0, 1000.0, HitResult::Great));
    ring.push(on(1, 1010.0, HitResult::Ok));
    view.update(&ring, 0, 1010.0);
    assert_eq!(view.popups().len(), 2);
    ring.push(on(0, 1020.0, HitResult::Miss));
    view.update(&ring, 0, 1020.0);
    let results: Vec<_> = view.popups().iter().map(|p| p.result).collect();
    assert_eq!(results, [HitResult::Ok, HitResult::Miss]);
}

#[test]
fn popups_expire_and_restarts_clear_them() {
    let mut view = view();
    let mut ring = JudgementRing::default();
    ring.push(on(0, 1000.0, HitResult::Great));
    ring.push(on(1, 1500.0, HitResult::Great));
    view.update(&ring, 0, 2399.0);
    assert_eq!(view.popups().len(), 2);
    view.update(&ring, 0, 2400.0);
    assert_eq!(view.popups().len(), 1);
    view.update(&ring, 1, 1000.0);
    assert!(view.popups().is_empty());
    // The ring's old entries are not taken again.
    view.update(&ring, 1, 1000.0);
    assert!(view.popups().is_empty());
}

#[test]
fn judgements_of_a_restarted_attempt_are_not_shown() {
    let mut view = view();
    let mut ring = JudgementRing::default();
    ring.push(on(0, 1000.0, HitResult::Great));
    view.update(&ring, 0, 1000.0);
    // Judged after the last frame, then restarted before the next one.
    ring.push(on(1, 1100.0, HitResult::Great));
    ring.restart();
    view.update(&ring, 1, 0.0);
    assert!(view.popups().is_empty());
    view.update(&ring, 1, 1200.0);
    assert!(view.popups().is_empty());
    assert_eq!(view.dropped(), 0);
    ring.push(on(0, 1300.0, HitResult::Ok));
    view.update(&ring, 1, 1300.0);
    assert_eq!(view.popups().len(), 1);
}

#[test]
fn overflows_drop_the_oldest_popups() {
    let mut view = view();
    let mut ring = JudgementRing::default();
    // More than the ring holds between two frames.
    for i in 0..100 {
        ring.push(on(i, 1000.0, HitResult::Great));
    }
    view.update(&ring, 0, 1000.0);
    assert_eq!(view.popups().len(), 64);
    assert_eq!(view.popups()[0].object.index, 36);
    assert_eq!(view.dropped(), 36);
    // More alive than the view holds.
    for i in 100..100 + MAX_POPUPS as u32 {
        ring.push(on(i, 1000.0, HitResult::Great));
        view.update(&ring, 0, 1000.0);
    }
    assert_eq!(view.popups().len(), MAX_POPUPS);
    assert_eq!(view.popups()[0].object.index, 100);
    assert_eq!(view.dropped(), 36 + 64);
}

// ---- Drawing ----

const BODY: TextureId = TextureId(1);
const PARTICLE: TextureId = TextureId(2);
const LIGHTING: TextureId = TextureId(3);

fn sprite(texture: TextureId, size: f32) -> SkinSprite {
    SkinSprite {
        texture,
        uv: UvRect::FULL,
        size: Vec2::new(size, size),
    }
}

fn textures(kind: PieceKind) -> JudgementTextures {
    let mut t = JudgementTextures::default();
    t.frames[0] = Some(sprite(BODY, 64.0));
    t.lighting = Some(sprite(LIGHTING, 128.0));
    for piece in &mut t.pieces {
        *piece = PieceSprites {
            kind,
            frames: (0, 1),
            frame_length: 1000.0 / 60.0,
            particle: (kind == PieceKind::New).then(|| sprite(PARTICLE, 8.0)),
        };
    }
    t
}

/// A recorded sprite, back from its corners.
#[derive(Debug, Clone, Copy)]
struct Quad {
    texture: TextureId,
    blend: Blend,
    alpha: f32,
    centre: [f32; 2],
    width: f32,
}

fn quads(batch: &SpriteBatch) -> Vec<Quad> {
    let v = batch.vertices();
    batch
        .batches()
        .iter()
        .flat_map(|b| {
            (b.first_quad..b.first_quad + b.quads).map(move |q| {
                let c = &v[q as usize * 4..q as usize * 4 + 4];
                let centre = [
                    (c[0].position[0] + c[2].position[0]) / 2.0,
                    (c[0].position[1] + c[2].position[1]) / 2.0,
                ];
                let (dx, dy) = (
                    c[1].position[0] - c[0].position[0],
                    c[1].position[1] - c[0].position[1],
                );
                Quad {
                    texture: b.texture,
                    blend: b.blend,
                    alpha: c[0].colour[3],
                    centre,
                    width: (dx * dx + dy * dy).sqrt(),
                }
            })
        })
        .collect()
}

fn draw(view: &JudgementView, kind: PieceKind, time: f64) -> (Vec<Quad>, Vec<Quad>) {
    let layout = common::layout();
    let textures = textures(kind);
    let mut below = SpriteBatch::with_capacity(1024);
    let mut above = SpriteBatch::with_capacity(1024);
    view.push_below(&mut below, &layout, time, &textures);
    view.push_above(&mut above, &layout, time, &textures);
    (quads(&below), quads(&above))
}

#[test]
fn old_bodies_draw_above_objects_with_the_lighting_below() {
    let mut view = view();
    let mut ring = JudgementRing::default();
    ring.push(on(0, 1000.0, HitResult::Great));
    view.update(&ring, 0, 1300.0);
    let (below, above) = draw(&view, PieceKind::Old, 1300.0);
    assert_eq!(below.len(), 1);
    assert_eq!(below[0].texture, LIGHTING);
    assert_eq!(below[0].blend, Blend::Additive);
    // The combo colour of object 0 (the default skin's first), at full alpha by 300 ms.
    assert_eq!(below[0].alpha, 1.0);
    // 128 × scale 0.5 × lighting scale 0.8 + 0.4 × Out(0.5).
    assert_close!(below[0].width, 128.0 * 0.5 * (0.8 + 0.4 * 0.75));
    assert_eq!(above.len(), 1);
    assert_eq!(above[0].texture, BODY);
    assert_eq!(above[0].blend, Blend::Alpha);
    assert_eq!(above[0].centre, [164.0, 156.0]);
    assert_close!(above[0].width, 32.0);
}

#[test]
fn new_bodies_and_particles_draw_below_objects_and_the_temporary_style_above() {
    let mut view = view();
    let mut ring = JudgementRing::default();
    ring.push(on(0, 1000.0, HitResult::Great));
    view.update(&ring, 0, 1100.0);
    let (below, above) = draw(&view, PieceKind::New, 1100.0);
    assert_eq!(below.len(), 1 + PARTICLE_COUNT + 1);
    assert_eq!(below[0].texture, LIGHTING);
    assert!(
        below[1..=PARTICLE_COUNT]
            .iter()
            .all(|s| s.texture == PARTICLE && s.blend == Blend::Additive)
    );
    assert_eq!(below[PARTICLE_COUNT + 1].texture, BODY);
    assert_eq!(above.len(), 1);
    assert_eq!(above[0].blend, Blend::Additive);
    // Past the temporary style's end, only the body and the lighting remain.
    let (below, above) = draw(&view, PieceKind::New, 1400.0);
    assert!(above.is_empty());
    assert!(below.iter().any(|s| s.texture == BODY));
}

#[test]
fn drawing_the_most_popups_does_not_allocate() {
    let mut view = view();
    let mut ring = JudgementRing::default();
    let textures = textures(PieceKind::New);
    let layout = common::layout();
    let mut batch = SpriteBatch::with_capacity(MAX_POPUPS * (PARTICLE_COUNT + 3));
    for i in 0..MAX_POPUPS as u32 {
        ring.push(on(i, 1000.0, HitResult::Great));
        view.update(&ring, 0, 1000.0);
    }
    slam_testkit::assert_no_alloc(|| {
        view.update(&ring, 0, 1300.0);
        view.push_below(&mut batch, &layout, 1300.0, &textures);
        view.push_above(&mut batch, &layout, 1300.0, &textures);
    });
    assert_eq!(view.popups().len(), MAX_POPUPS);
    assert_eq!(batch.len(), MAX_POPUPS * (PARTICLE_COUNT + 3));
}

#[test]
fn later_judgements_draw_on_top_above_objects() {
    let mut view = view();
    let mut ring = JudgementRing::default();
    // Arriving out of time order: a slider's late judgement before a circle's earlier one.
    ring.push(on(1, 1100.0, HitResult::Miss));
    ring.push(on(0, 1000.0, HitResult::Miss));
    view.update(&ring, 0, 1700.0);
    let (_, above) = draw(&view, PieceKind::Old, 1700.0);
    assert_eq!(above.len(), 2);
    // The second drawn is the later judgement, whose fade-out started later.
    assert!(above[1].alpha > above[0].alpha, "{above:?}");
}

// ---- Skin lookup ----

fn skin_dir(names: &[&str], ini: &str) -> common::skin::SampleDir {
    let dir = common::skin::SampleDir::new(&[]);
    fs::write(dir.0.join("skin.ini"), ini).unwrap();
    for name in names {
        fs::write(dir.0.join(format!("{name}.png")), b"not checked here").unwrap();
    }
    dir
}

fn sources(user: (&[&str], &str), default: (&[&str], &str)) -> Vec<(PieceKind, usize)> {
    let (user, default) = (skin_dir(user.0, user.1), skin_dir(default.0, default.1));
    let chain = SkinChain::new(
        Some(Skin::load(&user.0).unwrap()),
        Skin::load(&default.0).unwrap(),
    );
    piece_sources(&chain)
        .iter()
        .map(|s| (s.kind, s.frames.len()))
        .collect()
}

const DEFAULT_SKIN: (&[&str], &str) = (
    &[
        "hit0",
        "hit50",
        "hit100",
        "hit300",
        "particle300",
        "slidertickmiss",
        "sliderendmiss",
    ],
    "[General]\nVersion: latest\n",
);

#[test]
fn particles_choose_the_new_style() {
    // Slots: Miss, Meh, Ok, Great, LargeTickMiss, IgnoreMiss, LargeTickHit, SliderTailHit.
    let kinds = sources((&[], "[General]\nVersion: 2.5\n"), DEFAULT_SKIN);
    assert_eq!(
        kinds,
        [
            (PieceKind::Old, 1),
            (PieceKind::Old, 1),
            (PieceKind::Old, 1),
            (PieceKind::New, 1),
            (PieceKind::Old, 1),
            (PieceKind::Old, 1),
            (PieceKind::None, 0),
            (PieceKind::None, 0),
        ]
    );
}

#[test]
fn the_first_skin_with_a_judgement_draws_it_with_its_own_particles() {
    // The user's hit300 without particles is old-style, though the default has particles.
    let kinds = sources(
        (
            &["hit300-0", "hit300-1", "particle100"],
            "[General]\nVersion: 2.5\n",
        ),
        DEFAULT_SKIN,
    );
    assert_eq!(kinds[3], (PieceKind::Old, 2));
    // The user's particle100 alone does not make the default's hit100 new-style.
    assert_eq!(kinds[2], (PieceKind::Old, 1));
}

#[test]
fn old_skins_draw_slider_points_and_keep_their_misses() {
    let kinds = sources(
        (&["sliderpoint30", "sliderpoint10"], "[General]\n"),
        DEFAULT_SKIN,
    );
    assert_eq!(kinds[6], (PieceKind::TickHit, 1));
    assert_eq!(kinds[7], (PieceKind::TickHit, 1));
    // The user's skin shows hit ticks, so their misses are its own: none drawn.
    assert_eq!(kinds[4], (PieceKind::None, 0));
    assert_eq!(kinds[5], (PieceKind::None, 0));
    // From version 2 on, score points are not used.
    let kinds = sources(
        (
            &["sliderpoint30", "sliderpoint10"],
            "[General]\nVersion: 2.0\n",
        ),
        DEFAULT_SKIN,
    );
    assert_eq!(kinds[6], (PieceKind::None, 0));
    assert_eq!(kinds[4], (PieceKind::Old, 1));
}
