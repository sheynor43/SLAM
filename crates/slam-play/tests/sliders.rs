//! Sliders: the body, head and tail at key moments, drawing order among circles, the head and
//! tail textures a skin gives, and a frame written without allocations.

mod common;

use std::fs;

use slam_osu::judgement::HitResult;
use slam_osu::replay::{InputFrame, ReplayInput, Simulator};
use slam_osu::rules::{CircleJudgement, LiveCircle, LiveSlider, OsuRules};
use slam_osu::{Beatmap, GameplayMod, Mod, ModSet, Vec2};
use slam_play::PlayTextures;
use slam_play::circles::{
    CircleMods, CircleSettings, CircleSkinConfig, CircleStyle, CircleTextures, CircleVisuals,
    HitCircles, LiveCircles, PieceTextures, SkinSprite,
};
use slam_play::slider_body::Snaking;
use slam_play::sliders::{
    HitSliders, LiveSliders, ObjectTextures, SliderBodies, SliderPose, SliderTextures,
    SliderVisuals, push_objects, slider_pose,
};
use slam_render::Limits;
use slam_render::atlas::UvRect;
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::upload::{TextureLoaderConfig, texture_loader};
use slam_skin::{Skin, SkinChain};
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

/// AR 9 (preempt 600, fade-in 400), a circle, a slider from 1000 to 1500 ms (100 osu!pixels at
/// 200 osu!pixels per second) and a circle.
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

[HitObjects]
100,100,900,5,0
200,200,1000,2,0,L|300:200,1,100
400,300,2000,1,0
";

const START: f64 = 1000.0;
const END: f64 = 1500.0;
const APPEAR: f64 = START - 600.0;
const DIM: f32 = 195.0 / 255.0;

fn beatmap(mods: &ModSet) -> Beatmap {
    // The mods used here only change objects (Hidden's fade-ins), not the difficulty.
    let mut map = Beatmap::decode(MAP.as_bytes()).unwrap();
    for m in mods.mods() {
        m.clone().apply_to_beatmap(&mut map);
    }
    map
}

fn chain() -> (common::skin::SampleDir, SkinChain) {
    let dir = common::skin::SampleDir::new(&[]);
    let chain = SkinChain::new(None, Skin::load(&dir.0).unwrap());
    (dir, chain)
}

fn objects(map: &Beatmap, mods: &ModSet, snaking: Snaking) -> (HitCircles, HitSliders) {
    let (_dir, chain) = chain();
    let circle_mods = CircleMods::new(mods);
    let windows = slam_osu::hit_windows::HitWindows::new(map.difficulty.overall_difficulty);
    let settings = CircleSettings::default();
    (
        HitCircles {
            visuals: CircleVisuals::new(map, circle_mods, settings, None, &chain),
            style: CircleStyle::new(circle_mods, &windows, &chain),
            config: CircleSkinConfig::new(&chain),
        },
        HitSliders {
            visuals: SliderVisuals::new(map, circle_mods, settings, None, &chain),
            snaking,
        },
    )
}

fn judged(time: f64, result: HitResult) -> Option<CircleJudgement> {
    Some(CircleJudgement { time, result })
}

fn live(
    head: Option<CircleJudgement>,
    tail: Option<CircleJudgement>,
    judgement: Option<CircleJudgement>,
) -> LiveSlider {
    LiveSlider {
        index: 1,
        tracking: false,
        head,
        tail,
        judgement,
    }
}

/// The pose of the map's slider at `time`, played with `mods`.
fn pose_with(mods: &ModSet, snaking: Snaking, state: &LiveSlider, time: f64) -> SliderPose {
    let map = beatmap(mods);
    let (circles, sliders) = objects(&map, mods, snaking);
    let slider = sliders.visuals.get(1).unwrap();
    slider_pose(slider, state, time, &circles.style, snaking)
}

fn pose(state: &LiveSlider, time: f64) -> SliderPose {
    pose_with(&ModSet::default(), Snaking::default(), state, time)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn the_body_fades_in_dimmed_and_snakes_in() {
    let idle = live(None, None, None);
    assert_eq!(pose(&idle, APPEAR).body_alpha, 0.0);
    assert!(close(pose(&idle, APPEAR + 200.0).body_alpha, 0.5));
    assert_eq!(pose(&idle, APPEAR + 400.0).body_alpha, 1.0);
    assert_eq!(pose(&idle, APPEAR).object_alpha, 1.0);

    // Dimmed until shortly before it can be hit.
    assert_eq!(pose(&idle, APPEAR + 199.0).body_brightness, DIM);
    assert_eq!(pose(&idle, START - 300.0).body_brightness, 1.0);

    // Snaking in over a third of the preempt.
    assert_eq!(pose(&idle, APPEAR).range, (0.0, 0.0));
    assert_eq!(pose(&idle, APPEAR + 100.0).range, (0.0, 0.5));
    assert_eq!(pose(&idle, APPEAR + 200.0).range, (0.0, 1.0));
}

#[test]
fn the_body_snakes_out_only_once_the_head_is_hit() {
    let hit = live(judged(START, HitResult::Great), None, None);
    assert_eq!(pose(&hit, START + 250.0).range, (0.5, 1.0));
    let missed = live(judged(START + 150.0, HitResult::Miss), None, None);
    assert_eq!(pose(&missed, START + 250.0).range, (0.0, 1.0));
    let off = Snaking {
        snaking_in: true,
        snaking_out: false,
    };
    assert_eq!(
        pose_with(&ModSet::default(), off, &hit, START + 250.0).range,
        (0.0, 1.0)
    );
}

#[test]
fn the_slider_fades_out_from_its_end_time_or_judgement() {
    // Unjudged, its hit state time is its end time.
    let idle = live(None, None, None);
    assert_eq!(pose(&idle, END).object_alpha, 1.0);
    assert!(close(pose(&idle, END + 120.0).object_alpha, 0.5));
    assert_eq!(pose(&idle, END + 240.0).object_alpha, 0.0);

    // Hit after a hit head with snaking out: the body also fades over 40 ms.
    let head = judged(START, HitResult::Great);
    let tail = judged(END - 20.0, HitResult::SliderTailHit);
    let hit = live(head, tail, judged(END, HitResult::IgnoreHit));
    assert!(close(pose(&hit, END + 20.0).body_alpha, 0.5));
    assert!(close(pose(&hit, END + 120.0).object_alpha, 0.5));
    // A missed head keeps the body until the slider is gone.
    let missed = live(
        judged(START + 150.0, HitResult::Miss),
        tail,
        judged(END, HitResult::IgnoreHit),
    );
    assert_eq!(pose(&missed, END + 20.0).body_alpha, 1.0);
}

#[test]
fn the_head_is_drawn_as_a_hit_circle() {
    let hit = live(judged(START, HitResult::Great), None, None);
    let at = |time| pose(&hit, time).head;
    assert_eq!(at(APPEAR + 200.0).piece_alpha, 0.5);
    assert!(close(at(START + 120.0).sprite_scale, 1.0 + 0.4 * 0.75));
    assert_eq!(at(START).approach_alpha, 0.0);
    let idle = live(None, None, None);
    assert!(close(pose(&idle, APPEAR + 100.0).head.approach_scale, 3.5));
}

#[test]
fn the_first_tail_fades_in_after_snaking_in() {
    let idle = live(None, None, None);
    // The tail appears with the slider, its piece a third of the preempt later.
    assert_eq!(pose(&idle, APPEAR).tail.object_alpha, 1.0);
    assert_eq!(pose(&idle, APPEAR + 200.0).tail.piece_alpha, 0.0);
    assert!(close(pose(&idle, APPEAR + 400.0).tail.piece_alpha, 0.5));
    let no_snaking = Snaking {
        snaking_in: false,
        snaking_out: true,
    };
    let early = pose_with(&ModSet::default(), no_snaking, &idle, APPEAR + 200.0);
    assert!(close(early.tail.piece_alpha, 0.5));
    assert_eq!(pose(&idle, APPEAR + 199.0).tail.brightness, DIM);
}

#[test]
fn a_hit_tail_plays_the_legacy_hit_animation_and_dims_again() {
    let tail_time = END - 20.0;
    let hit = live(
        judged(START, HitResult::Great),
        judged(tail_time, HitResult::SliderTailHit),
        None,
    );
    let at = |time| pose(&hit, time).tail;
    assert!(close(at(tail_time + 120.0).sprite_alpha, 0.5));
    assert_eq!(at(tail_time + 240.0).sprite_scale, 1.4);
    assert_eq!(at(tail_time + 240.0).object_alpha, 1.0);
    // The slider dims its tail again on the judgement...
    assert_eq!(at(tail_time).brightness, DIM);
    assert_eq!(at(tail_time + 19.0).brightness, DIM);
    // ...until its own judgement dims it from its appearance again, which ends long before.
    let judged_slider = live(
        judged(START, HitResult::Great),
        judged(tail_time, HitResult::SliderTailHit),
        judged(END, HitResult::IgnoreHit),
    );
    assert_eq!(pose(&judged_slider, END - 1.0).tail.brightness, DIM);
    assert_eq!(pose(&judged_slider, END).tail.brightness, 1.0);
    assert_eq!(pose(&judged_slider, END + 100.0).tail.brightness, 1.0);

    let missed = live(None, judged(END, HitResult::IgnoreMiss), None);
    assert!(close(pose(&missed, END + 50.0).tail.object_alpha, 0.5));
    assert_eq!(pose(&missed, END + 50.0).tail.sprite_alpha, 1.0);
}

#[test]
fn hidden_fades_the_body_and_the_tail() {
    let hidden = ModSet::new(vec![Mod::Hidden {
        only_fade_approach_circles: false,
    }])
    .unwrap();
    let map = beatmap(&hidden);
    let slider = &map.hit_objects[1];
    // The slider keeps its fade-in; its head and tail get Hidden's.
    assert_eq!(slider.defaults.time_fade_in, 400.0);
    let idle = live(None, None, None);
    let at = |time| pose_with(&hidden, Snaking::default(), &idle, time);
    // The body: from the end of its fade-in to the end time, easing out.
    let fade_start = APPEAR + 400.0;
    assert_eq!(at(fade_start).body_alpha, 1.0);
    let half = fade_start + (END - fade_start) / 2.0;
    assert!(close(at(half).body_alpha, 0.25), "{}", at(half).body_alpha);
    assert_eq!(at(END).body_alpha, 0.0);
    // The tail fades as the head: from the head's fade-in (240 ms) over 180 ms.
    let tail_fade = APPEAR + 240.0;
    assert!(close(at(tail_fade + 90.0).tail.object_alpha, 0.5));
    // The first object of the map is the circle: the head's approach circle is hidden.
    assert_eq!(at(APPEAR + 300.0).head.approach_alpha, 0.0);

    let approach_only = ModSet::new(vec![Mod::Hidden {
        only_fade_approach_circles: true,
    }])
    .unwrap();
    let at = |time| pose_with(&approach_only, Snaking::default(), &idle, time);
    assert_eq!(at(half).body_alpha, 1.0);
    assert_eq!(at(tail_fade + 90.0).tail.object_alpha, 1.0);
}

/// A slider first, going there and back: two spans of 500 ms from 1000 ms.
const REPEAT_MAP: &str = "osu file format v14

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:5
ApproachRate:9
SliderMultiplier:1
SliderTickRate:1

[TimingPoints]
0,500,4,2,0,100,1,0

[HitObjects]
200,200,1000,6,0,L|300:200,2,100
400,300,2500,1,0
";

fn repeat_pose(mods: &ModSet, state: &LiveSlider, time: f64) -> SliderPose {
    let mut map = Beatmap::decode(REPEAT_MAP.as_bytes()).unwrap();
    for m in mods.mods() {
        m.clone().apply_to_beatmap(&mut map);
    }
    let (circles, sliders) = objects(&map, mods, Snaking::default());
    let slider = sliders.visuals.get(0).unwrap();
    let state = LiveSlider { index: 0, ..*state };
    slider_pose(slider, &state, time, &circles.style, Snaking::default())
}

#[test]
fn a_repeated_tail_appears_at_the_start_of_the_path_after_one_span() {
    let map = Beatmap::decode(REPEAT_MAP.as_bytes()).unwrap();
    let (_, sliders) = objects(&map, &ModSet::default(), Snaking::default());
    let slider = sliders.visuals.get(0).unwrap();
    assert_eq!(slider.tail.repeat_index, 1);
    assert_eq!(slider.tail.position, Vec2::new(200.0, 200.0));
    // Preempt of two spans before its time (2000 ms), no fade-in, no wait for snaking.
    let idle = live(None, None, None);
    assert_eq!(
        repeat_pose(&ModSet::default(), &idle, 999.0)
            .tail
            .object_alpha,
        0.0
    );
    let at = repeat_pose(&ModSet::default(), &idle, 1000.0).tail;
    assert_eq!((at.object_alpha, at.piece_alpha), (1.0, 1.0));
}

#[test]
fn hidden_spares_only_the_head_of_a_first_slider() {
    let hidden = ModSet::new(vec![Mod::Hidden {
        only_fade_approach_circles: false,
    }])
    .unwrap();
    let idle = live(None, None, None);
    let at = |time| repeat_pose(&hidden, &idle, time);
    // The head keeps its approach circle; its piece fades instead of the whole circle.
    assert!(at(APPEAR + 300.0).head.approach_alpha > 0.0);
    assert_eq!(at(APPEAR + 300.0).head.object_alpha, 1.0);
    assert!(at(APPEAR + 300.0).head.piece_alpha < 1.0);
    // The body and the tail fade all the same.
    assert!(at(START).body_alpha < 1.0);
    assert_eq!(at(APPEAR + 240.0 + 180.0).tail.object_alpha, 0.0);
}

// ---- Sprites ----

const CIRCLE: TextureId = TextureId(1);
const OVERLAY: TextureId = TextureId(2);
const APPROACH: TextureId = TextureId(3);
const HEAD: TextureId = TextureId(5);
const HEAD_OVERLAY: TextureId = TextureId(6);
const TAIL: TextureId = TextureId(7);
const TAIL_OVERLAY: TextureId = TextureId(8);

fn sprite(texture: TextureId) -> Option<SkinSprite> {
    Some(SkinSprite {
        texture,
        uv: UvRect::FULL,
        size: Vec2::new(128.0, 128.0),
    })
}

fn circle_textures() -> CircleTextures {
    CircleTextures {
        circle: sprite(CIRCLE),
        overlay: sprite(OVERLAY),
        approach: sprite(APPROACH),
        // No numbers.
        digits: [None; 10],
    }
}

fn slider_textures() -> SliderTextures {
    SliderTextures {
        head: PieceTextures {
            circle: sprite(HEAD),
            overlay: sprite(HEAD_OVERLAY),
        },
        tail: PieceTextures {
            circle: sprite(TAIL),
            overlay: sprite(TAIL_OVERLAY),
        },
    }
}

fn order(batch: &SpriteBatch) -> Vec<u32> {
    batch.batches().iter().map(|b| b.texture.0).collect()
}

#[test]
fn objects_draw_in_lazer_order_with_bodies_between_sprites() {
    let map = beatmap(&ModSet::default());
    let (circles, sliders) = objects(&map, &ModSet::default(), Snaking::default());
    let layout = common::layout();
    let mut batch = SpriteBatch::new();
    let mut bodies = SliderBodies::new();
    let live_circles = [
        LiveCircle {
            index: 0,
            judgement: None,
        },
        LiveCircle {
            index: 2,
            judgement: None,
        },
    ];
    let (circle_textures, slider_textures) = (circle_textures(), slider_textures());
    push_objects(
        &circles,
        &sliders,
        &mut batch,
        &mut bodies,
        &layout,
        &live_circles,
        &[live(None, None, None)],
        1500.0 - 60.0,
        ObjectTextures {
            circles: &circle_textures,
            sliders: &slider_textures,
        },
    );
    // Circle 2, the slider (body, tail, head), circle 0 on top; then the approach circles
    // of the head and circle 2 (circle 0's has faded) in start time order.
    assert_eq!(
        order(&batch),
        [
            CIRCLE.0,
            OVERLAY.0,
            TAIL.0,
            TAIL_OVERLAY.0,
            HEAD.0,
            HEAD_OVERLAY.0,
            CIRCLE.0,
            OVERLAY.0,
            APPROACH.0,
        ]
    );
    let [body] = bodies.as_slice() else {
        panic!("{:?}", bodies.as_slice())
    };
    assert_eq!(body.index, 1);
    assert_eq!(body.before_sprite, 2);
    assert_eq!(body.position, Vec2::new(200.0, 200.0));
    // Fully snaked in; with the head unjudged it does not snake out.
    assert_eq!(body.range, (0.0, 100.0));
    assert_eq!(body.tint, [1.0; 4]);
    // The tail sits at the end of the path.
    let tail = &batch.vertices()[2 * 4..3 * 4];
    let centre_x = (tail[0].position[0] + tail[1].position[0]) / 2.0;
    assert!((centre_x - layout.to_window(Vec2::new(300.0, 200.0)).x).abs() < 1e-3);
}

#[test]
fn a_faded_slider_records_no_body() {
    let map = beatmap(&ModSet::default());
    let (circles, sliders) = objects(&map, &ModSet::default(), Snaking::default());
    let mut batch = SpriteBatch::new();
    let mut bodies = SliderBodies::new();
    let (circle_textures, slider_textures) = (circle_textures(), slider_textures());
    push_objects(
        &circles,
        &sliders,
        &mut batch,
        &mut bodies,
        &common::layout(),
        &[],
        &[live(None, None, None)],
        END + 240.0,
        ObjectTextures {
            circles: &circle_textures,
            sliders: &slider_textures,
        },
    );
    assert!(bodies.as_slice().is_empty());
    assert!(batch.is_empty());
}

#[test]
fn a_frame_does_not_allocate() {
    let map = beatmap(&ModSet::default());
    let (circles, sliders) = objects(&map, &ModSet::default(), Snaking::default());
    let layout = common::layout();
    let frames = map.hit_objects.iter().flat_map(|o| {
        [
            InputFrame {
                time: o.start_time,
                position: o.stacked_position(),
                actions: slam_osu::replay::Actions::LEFT,
            },
            InputFrame {
                time: o.end_time() + 50.0,
                position: o.stacked_end_position(),
                actions: slam_osu::replay::Actions::NONE,
            },
        ]
    });
    let mut sim = Simulator::new(
        &map,
        ReplayInput::new(frames),
        OsuRules::new(&map, &ModSet::default()),
    );
    let mut live_circles = Box::new(LiveCircles::new());
    let mut live_sliders = Box::new(LiveSliders::new());
    let mut batch = SpriteBatch::with_capacity(1024);
    let mut bodies = SliderBodies::new();
    let (circle_textures, slider_textures) = (circle_textures(), slider_textures());
    let mut drawn = 0;
    while sim.step() {
        let time = sim.time();
        assert_no_alloc(|| {
            live_circles.capture(sim.rules());
            live_sliders.capture(sim.rules());
            batch.clear();
            bodies.clear();
            push_objects(
                &circles,
                &sliders,
                &mut batch,
                &mut bodies,
                &layout,
                live_circles.as_slice(),
                live_sliders.as_slice(),
                time,
                ObjectTextures {
                    circles: &circle_textures,
                    sliders: &slider_textures,
                },
            );
        });
        drawn += bodies.as_slice().len();
    }
    assert!(drawn > 0);
}

// ---- Textures ----

const LIMITS: Limits = Limits {
    max_texture_size: 4096,
    max_uniform_block_size: 16384,
    max_uniform_buffer_slots: 8,
    max_texture_slots: 8,
};

/// The number of skin textures requested from a user skin and a default skin with files
/// `user` and `default`.
fn found(user: &[&str], default: &[&str]) -> usize {
    let (user_dir, default_dir) = (
        common::skin::SampleDir::new(&[]),
        common::skin::SampleDir::new(&[]),
    );
    for (dir, names) in [(&user_dir, user), (&default_dir, default)] {
        for name in names {
            fs::write(dir.0.join(format!("{name}.png")), b"not checked here").unwrap();
        }
    }
    let chain = SkinChain::new(
        Some(Skin::load(&user_dir.0).unwrap()),
        Skin::load(&default_dir.0).unwrap(),
    );
    let (mut requests, _uploads) = texture_loader(&TextureLoaderConfig::new(LIMITS)).unwrap();
    PlayTextures::request(&chain, &mut requests)
        .unwrap()
        .found()
}

#[test]
fn slider_pieces_use_their_textures_only_from_the_hit_circle_provider() {
    // The hit circle's textures alone: heads and tails use them.
    assert_eq!(found(&["hitcircle"], &[]), 1);
    // Their own, overlays looked up through the chain.
    assert_eq!(
        found(&["hitcircle", "sliderstartcircle", "sliderendcircle"], &[]),
        3
    );
    assert_eq!(
        found(
            &["hitcircle", "sliderstartcircle"],
            &["sliderstartcircleoverlay"]
        ),
        3
    );
    // A `sliderstartcircle` in a skin after the one with `hitcircle` is not used.
    assert_eq!(
        found(&["hitcircle"], &["hitcircle", "sliderstartcircle"]),
        1
    );
}
