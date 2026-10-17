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
use slam_play::fail::FailFrame;
use slam_play::slider_body::Snaking;
use slam_play::sliders::{
    AnimationSprites, ArrowStyle, BallPose, BallStyle, HitSliders, LiveSliders, MAX_LIVE_NESTED,
    NestedPiece, NestedVisual, ObjectTextures, SliderBodies, SliderPose, SliderTextures,
    SliderVisual, SliderVisuals, animation_frame, ball_pose, push_approaches, push_objects,
    repeat_pose, slider_pose, tick_pose,
};
use slam_render::Limits;
use slam_render::atlas::UvRect;
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::upload::{TextureLoaderConfig, texture_loader};
use slam_render::{Blend, Color};
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
            arrows: ArrowStyle::new(&chain),
            ball: BallStyle::new(&chain),
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
        press_time: None,
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
    pose_with(&ModSet::default(), Snaking::LAZER, state, time)
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
    // Without snaking out (the default), the body stays with the slider.
    let default = pose_with(&ModSet::default(), Snaking::default(), &hit, END + 20.0);
    assert_eq!(default.body_alpha, 1.0);
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
    let at = |time| pose_with(&hidden, Snaking::LAZER, &idle, time);
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
    let at = |time| pose_with(&approach_only, Snaking::LAZER, &idle, time);
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

fn repeat_slider_pose(mods: &ModSet, state: &LiveSlider, time: f64) -> SliderPose {
    let mut map = Beatmap::decode(REPEAT_MAP.as_bytes()).unwrap();
    for m in mods.mods() {
        m.clone().apply_to_beatmap(&mut map);
    }
    let (circles, sliders) = objects(&map, mods, Snaking::LAZER);
    let slider = sliders.visuals.get(0).unwrap();
    let state = LiveSlider { index: 0, ..*state };
    slider_pose(slider, &state, time, &circles.style, Snaking::LAZER)
}

#[test]
fn a_repeated_tail_appears_at_the_start_of_the_path_after_one_span() {
    let map = Beatmap::decode(REPEAT_MAP.as_bytes()).unwrap();
    let (_, sliders) = objects(&map, &ModSet::default(), Snaking::LAZER);
    let slider = sliders.visuals.get(0).unwrap();
    assert_eq!(slider.tail.repeat_index, 1);
    assert_eq!(slider.tail.position, Vec2::new(200.0, 200.0));
    // Preempt of two spans before its time (2000 ms), no fade-in, no wait for snaking.
    let idle = live(None, None, None);
    assert_eq!(
        repeat_slider_pose(&ModSet::default(), &idle, 999.0)
            .tail
            .object_alpha,
        0.0
    );
    let at = repeat_slider_pose(&ModSet::default(), &idle, 1000.0).tail;
    assert_eq!((at.object_alpha, at.piece_alpha), (1.0, 1.0));
}

#[test]
fn hidden_spares_only_the_head_of_a_first_slider() {
    let hidden = ModSet::new(vec![Mod::Hidden {
        only_fade_approach_circles: false,
    }])
    .unwrap();
    let idle = live(None, None, None);
    let at = |time| repeat_slider_pose(&hidden, &idle, time);
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
const TICK: TextureId = TextureId(9);
const ARROW: TextureId = TextureId(10);

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
        tick: sprite(TICK),
        arrow: sprite(ARROW),
        ..SliderTextures::default()
    }
}

/// A snapshot of one slider without nested judgements: nothing judged yet.
fn snapshot(map: &Beatmap, slider: LiveSlider) -> Box<LiveSliders> {
    let mut live = Box::new(LiveSliders::new());
    let slam_osu::objects::OsuHitObjectKind::Slider(s) =
        &map.hit_objects[slider.index as usize].kind
    else {
        panic!("not a slider");
    };
    live.push(slider, s.nested.iter().map(|_| None));
    live
}

fn order(batch: &SpriteBatch) -> Vec<u32> {
    batch.batches().iter().map(|b| b.texture.0).collect()
}

#[test]
fn objects_draw_in_lazer_order_with_bodies_between_sprites() {
    let map = beatmap(&ModSet::default());
    let (circles, sliders) = objects(&map, &ModSet::default(), Snaking::LAZER);
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
    let live_sliders = snapshot(&map, live(None, None, None));
    push_objects(
        &circles,
        &sliders,
        &mut batch,
        &mut bodies,
        &layout,
        &live_circles,
        &live_sliders,
        1500.0 - 60.0,
        ObjectTextures {
            circles: &circle_textures,
            sliders: &slider_textures,
        },
        None,
    );
    push_approaches(
        &circles,
        &sliders,
        &mut batch,
        &layout,
        &live_circles,
        live_sliders.as_slice(),
        1500.0 - 60.0,
        &circle_textures,
        None,
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
    let (circles, sliders) = objects(&map, &ModSet::default(), Snaking::LAZER);
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
        &snapshot(&map, live(None, None, None)),
        END + 240.0,
        ObjectTextures {
            circles: &circle_textures,
            sliders: &slider_textures,
        },
        None,
    );
    assert!(bodies.as_slice().is_empty());
    assert!(batch.is_empty());
}

#[test]
fn a_frame_does_not_allocate() {
    frame_does_not_allocate(&beatmap(&ModSet::default()));
    frame_does_not_allocate(&nested_map(&ModSet::default()));
}

fn frame_does_not_allocate(map: &Beatmap) {
    let map = map.clone();
    let (circles, sliders) = objects(&map, &ModSet::default(), Snaking::LAZER);
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
    let (circle_textures, slider_textures) = (circle_textures(), ball_textures());
    let mut drawn = 0;
    let mut fell = 0;
    // From halfway on the play is failing: the objects fall.
    let fail = slam_play::fail::FailView {
        time: map.hit_objects[map.hit_objects.len() / 2].start_time,
        at_ns: 0,
        cut: false,
    };
    while sim.step() {
        let time = sim.time();
        let failing = time >= fail.time;
        assert_no_alloc(|| {
            let [width, height] = layout.window_size();
            let fail = FailFrame::new(failing.then_some(fail), time, 0, width, height);
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
                &live_sliders,
                time,
                ObjectTextures {
                    circles: &circle_textures,
                    sliders: &slider_textures,
                },
                fail.as_ref(),
            );
            push_approaches(
                &circles,
                &sliders,
                &mut batch,
                &layout,
                live_circles.as_slice(),
                live_sliders.as_slice(),
                time,
                &circle_textures,
                fail.as_ref(),
            );
        });
        drawn += bodies.as_slice().len();
        fell += bodies
            .as_slice()
            .iter()
            .filter(|b| failing && time > fail.time + 100.0 && !b.transform.is_identity())
            .count();
        if !failing {
            assert!(bodies.as_slice().iter().all(|b| b.transform.is_identity()));
        }
    }
    assert!(drawn > 0);
    assert!(fell > 0, "slider bodies fall with their sliders");
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
    // Room for every request at once: whether the loader thread drained some before the
    // queue filled would otherwise decide the test.
    let config = TextureLoaderConfig {
        request_capacity: 1024,
        ..TextureLoaderConfig::new(LIMITS)
    };
    let (mut requests, _uploads) = texture_loader(&config).unwrap();
    PlayTextures::request(&chain, &mut requests)
        .unwrap()
        .found()
}

#[test]
fn follow_points_are_an_animation_or_a_texture() {
    assert_eq!(
        found(&["followpoint-0", "followpoint-1", "followpoint-2"], &[]),
        3
    );
    assert_eq!(found(&["followpoint"], &[]), 1);
    // The user's frames hide the default skin's texture.
    assert_eq!(
        found(&["followpoint-0", "followpoint-1"], &["followpoint"]),
        2
    );
}

#[test]
fn judgement_animations_share_a_frame_budget() {
    let frames = |name: &'static str, count: usize| (0..count).map(move |i| format!("{name}-{i}"));
    // Four animations of 64 frames (each cut from 70) fill the budget: the next one is left out.
    let names: Vec<String> = ["hit0", "hit50", "hit100", "hit300"]
        .into_iter()
        .flat_map(|name| frames(name, 70))
        .chain(["slidertickmiss".to_owned()])
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(found(&names, &[]), 4 * 64);
    let names: Vec<String> = frames("hit0", 70).chain(["hit300".to_owned()]).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(found(&names, &[]), 64 + 1);
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

// ---- Ticks and repeats ----

/// AR 9 (preempt 600, fade-in 400) and one slider of three 200 osu!pixel spans of 1000 ms from
/// 1000 ms: ticks at 1500, 2500 and 3500, a repeat at the end at 2000 and one at the start at
/// 3000, the tail at 4000.
const NESTED_MAP: &str = "osu file format v14

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
200,200,1000,2,0,L|400:200,3,200
";

fn nested_map(mods: &ModSet) -> Beatmap {
    let mut map = Beatmap::decode(NESTED_MAP.as_bytes()).unwrap();
    for m in mods.mods() {
        m.clone().apply_to_beatmap(&mut map);
    }
    map
}

/// The nested visuals of the slider of [`NESTED_MAP`] played with `mods`, with its visual and
/// the circle style.
fn nested_visuals(mods: &ModSet) -> (SliderVisual, Vec<NestedVisual>, CircleStyle) {
    let map = nested_map(mods);
    let (circles, sliders) = objects(&map, mods, Snaking::LAZER);
    let slider = *sliders.visuals.get(0).unwrap();
    let nested = sliders.visuals.nested(&slider).to_vec();
    (slider, nested, circles.style)
}

fn ticks(nested: &[NestedVisual]) -> Vec<NestedVisual> {
    nested
        .iter()
        .filter(|n| n.piece == NestedPiece::Tick)
        .copied()
        .collect()
}

fn repeats(nested: &[NestedVisual]) -> Vec<NestedVisual> {
    nested
        .iter()
        .filter(|n| matches!(n.piece, NestedPiece::Repeat { .. }))
        .copied()
        .collect()
}

#[test]
fn nested_visuals_follow_the_beatmap() {
    let (_, nested, _) = nested_visuals(&ModSet::default());
    let kinds: Vec<_> = nested.iter().map(|n| (n.start_time, n.piece)).collect();
    assert_eq!(kinds.len(), 7, "{kinds:?}");
    assert_eq!(kinds[0], (1000.0, NestedPiece::Head));
    assert_eq!(kinds[6], (4000.0, NestedPiece::Tail));
    let ticks = ticks(&nested);
    assert_eq!(
        ticks.iter().map(|t| t.start_time).collect::<Vec<_>>(),
        [1500.0, 2500.0, 3500.0]
    );
    assert_eq!(ticks[0].position, Vec2::new(300.0, 200.0));
    // `SliderTick`: half the time from the span start plus 0.66 of the preempt, then 200.
    assert_eq!(ticks[0].time_preempt, 250.0 + 600.0 * f64::from(0.66f32));
    assert_eq!(ticks[1].time_preempt, 450.0);

    let repeats = repeats(&nested);
    // At the end the arrow points back along the path; at the start, forwards.
    assert_eq!(
        repeats[0].piece,
        NestedPiece::Repeat {
            repeat_index: 0,
            rotation: 180.0
        }
    );
    assert_eq!(repeats[0].position, Vec2::new(400.0, 200.0));
    assert_eq!(
        repeats[1].piece,
        NestedPiece::Repeat {
            repeat_index: 1,
            rotation: 0.0
        }
    );
    assert_eq!(repeats[1].position, Vec2::new(200.0, 200.0));
    // `SliderEndCircle`: the first appears with the slider, the next two spans before.
    assert_eq!(repeats[0].time_preempt, 600.0 + 1000.0);
    assert_eq!(
        (repeats[1].time_preempt, repeats[1].time_fade_in),
        (2000.0, 0.0)
    );
}

#[test]
fn ticks_fade_in_with_an_elastic_scale_and_burst_when_hit() {
    let (_, nested, style) = nested_visuals(&ModSet::default());
    let tick = ticks(&nested)[0];
    let appear = tick.start_time - tick.time_preempt;
    let at = |judgement, time| tick_pose(&tick, judgement, time, &style);

    assert_eq!(at(None, appear - 1.0).alpha, 0.0);
    assert_eq!(at(None, appear).alpha, 0.0);
    assert!(close(at(None, appear + 75.0).alpha, 0.5));
    assert_eq!(at(None, appear + 150.0).alpha, 1.0);
    assert_eq!(at(None, appear).scale, 0.5);
    // `OutElasticHalf` overshoots, then settles at 1 after 600 ms.
    assert!(at(None, appear + 150.0).scale > 1.0);
    assert_eq!(at(None, appear + 600.0).scale, 1.0);

    let hit = judged(1500.0, HitResult::LargeTickHit);
    assert_eq!(at(hit, 1500.0).alpha, 1.0);
    // `OutQuint` over 150 ms.
    assert!(close(at(hit, 1575.0).alpha, 1.0 / 32.0));
    assert_eq!(at(hit, 1650.0).alpha, 0.0);
    assert!(close(at(hit, 1575.0).scale, 1.0 + 0.5 * 0.75));
    assert_eq!(at(hit, 1650.0).scale, 1.5);

    let miss = judged(1500.0, HitResult::LargeTickMiss);
    assert!(close(at(miss, 1575.0).alpha, 1.0 / 32.0));
    assert_eq!(at(miss, 1575.0).scale, 1.0);

    // Unjudged, it stays until a preempt after its start.
    assert_eq!(at(None, 1500.0 + tick.time_preempt - 1.0).alpha, 1.0);
    assert_eq!(at(None, 1500.0 + tick.time_preempt).alpha, 0.0);
}

#[test]
fn a_tick_hit_while_growing_bursts_from_its_current_scale() {
    // The second span's tick has a preempt of 450 ms: it is hit 150 ms before its elastic
    // scale settles. osu-framework applies the transforms re-added in the past at once, so
    // `Scale * 1.5` reads the current elastic scale.
    let (_, nested, style) = nested_visuals(&ModSet::default());
    let tick = ticks(&nested)[1];
    let from = tick_pose(&tick, None, tick.start_time, &style).scale;
    assert_ne!(from, 1.0);
    let hit = judged(tick.start_time, HitResult::LargeTickHit);
    let at = |time| tick_pose(&tick, hit, time, &style).scale;
    assert_eq!(at(tick.start_time), from);
    assert_eq!(at(tick.start_time + 150.0), from * 1.5);
}

#[test]
fn hidden_fades_ticks_out_before_their_time() {
    let mods = ModSet::new(vec![Mod::Hidden {
        only_fade_approach_circles: false,
    }])
    .unwrap();
    let (_, nested, style) = nested_visuals(&mods);
    let tick = ticks(&nested)[0];
    let duration = (tick.time_preempt - 150.0).min(1000.0);
    let at = |time| tick_pose(&tick, None, time, &style).alpha;
    assert_eq!(at(1500.0 - duration), 1.0);
    assert!(close(at(1500.0 - duration / 2.0), 0.5));
    assert_eq!(at(1500.0), 0.0);
}

const ROCKING: ArrowStyle = ArrowStyle {
    rotate: true,
    default_texture: false,
};

#[test]
fn the_first_repeat_waits_for_snaking_and_its_arrow_pulses() {
    let (slider, nested, style) = nested_visuals(&ModSet::default());
    let repeat = repeats(&nested)[0];
    let appear = 2000.0 - 1600.0;
    let at =
        |arrows, snaking, time| repeat_pose(&slider, &repeat, None, time, &style, snaking, arrows);
    let snaking = Snaking::LAZER;
    let still = Snaking {
        snaking_in: false,
        snaking_out: false,
    };

    // Snaking in takes a third of the slider's preempt.
    assert_eq!(at(ROCKING, snaking, appear + 200.0).arrow_alpha, 0.0);
    assert!(close(at(ROCKING, snaking, appear + 275.0).arrow_alpha, 0.5));
    assert_eq!(at(ROCKING, snaking, appear + 350.0).arrow_alpha, 1.0);
    assert!(close(at(ROCKING, snaking, appear + 400.0).piece_alpha, 0.5));
    assert!(close(at(ROCKING, still, appear + 75.0).arrow_alpha, 0.5));
    assert!(close(at(ROCKING, still, appear + 200.0).piece_alpha, 0.5));
    assert_eq!(at(ROCKING, snaking, appear).object_alpha, 1.0);
    assert_eq!(at(ROCKING, snaking, appear - 1.0).object_alpha, 0.0);

    // Version 1 skins rock it ±5.625° as it shrinks from 1.3 every 300 ms.
    let p = at(ROCKING, snaking, appear + 600.0);
    assert_eq!((p.arrow_rotation, p.arrow_scale), (180.0 + 5.625, 1.3));
    let p = at(ROCKING, snaking, appear + 750.0);
    assert!(close(p.arrow_rotation, 180.0));
    assert!(close(p.arrow_scale, 1.15));
    // Newer ones only pulse, eased.
    let p = at(ArrowStyle::default(), snaking, appear + 750.0);
    assert_eq!(p.arrow_rotation, 180.0);
    assert!(close(p.arrow_scale, 1.3 - 0.3 * 0.75));
}

#[test]
fn a_hit_repeat_grows_its_arrow_and_fades() {
    let (slider, nested, style) = nested_visuals(&ModSet::default());
    let repeat = repeats(&nested)[0];
    let at = |judgement, time| {
        repeat_pose(
            &slider,
            &repeat,
            judgement,
            time,
            &style,
            Snaking::LAZER,
            ROCKING,
        )
    };
    let hit = judged(2000.0, HitResult::LargeTickHit);
    // The rocking stops where it was: 1600 ms in, a third into the loop.
    let p = at(hit, 2150.0);
    assert!(close(p.arrow_rotation, 180.0 + 5.625 - 11.25 / 3.0));
    assert!(close(p.arrow_scale, 1.0 + 0.4 * 0.75));
    assert!(close(p.object_alpha, 0.25));
    assert_eq!(at(hit, 2300.0).object_alpha, 0.0);
    // The circle's legacy hit animation.
    assert!(at(hit, 2120.0).sprite_scale > 1.0);
    assert!(at(hit, 2120.0).sprite_alpha < 1.0);

    let miss = judged(2000.0, HitResult::LargeTickMiss);
    let p = at(miss, 2150.0);
    assert!(close(p.object_alpha, 0.5));
    assert_eq!(p.sprite_scale, 1.0);
    assert!(p.arrow_scale >= 1.0 && p.arrow_scale <= 1.3);

    // Unjudged, it stays until a preempt after its start.
    assert_eq!(
        at(None, 2000.0 + repeat.time_preempt - 1.0).object_alpha,
        1.0
    );
    assert_eq!(at(None, 2000.0 + repeat.time_preempt).object_alpha, 0.0);
}

#[test]
fn later_repeats_appear_two_spans_before_without_fading_in() {
    let (slider, nested, style) = nested_visuals(&ModSet::default());
    let repeat = repeats(&nested)[1];
    let at = |time| {
        repeat_pose(
            &slider,
            &repeat,
            None,
            time,
            &style,
            Snaking::LAZER,
            ROCKING,
        )
    };
    assert_eq!(at(999.0).object_alpha, 0.0);
    let p = at(1000.0);
    assert_eq!((p.object_alpha, p.piece_alpha), (1.0, 1.0));
    assert!(close(at(1075.0).arrow_alpha, 0.5));
}

#[test]
fn hidden_fades_repeat_circles_with_the_head_but_not_their_arrows() {
    let mods = ModSet::new(vec![Mod::Hidden {
        only_fade_approach_circles: false,
    }])
    .unwrap();
    let (slider, nested, style) = nested_visuals(&mods);
    let repeat = repeats(&nested)[0];
    let at = |judgement, time| {
        repeat_pose(
            &slider,
            &repeat,
            judgement,
            time,
            &style,
            Snaking::LAZER,
            ROCKING,
        )
    };
    let head = &slider.head;
    let fade_end =
        head.start_time - head.time_preempt + head.time_fade_in + head.time_preempt * 0.3;
    let p = at(None, fade_end);
    assert_eq!(p.piece_alpha, 0.0);
    assert_eq!((p.object_alpha, p.arrow_alpha), (1.0, 1.0));
    // Gone at once when hit.
    let hit = judged(2000.0, HitResult::LargeTickHit);
    assert_eq!(at(hit, 2000.0).object_alpha, 0.0);
}

#[test]
fn arrows_from_the_default_skin_turn_black_on_bright_colours() {
    let default = ArrowStyle {
        rotate: false,
        default_texture: true,
    };
    let bright = slam_render::Color::rgb(1.0, 1.0, 0.5);
    let dark = slam_render::Color::rgb(1.0, 0.5, 0.5);
    assert_eq!(default.colour(bright), slam_render::Color::BLACK);
    assert_eq!(default.colour(dark), slam_render::Color::WHITE);
    assert_eq!(
        ArrowStyle::default().colour(bright),
        slam_render::Color::WHITE
    );
}

/// The textures drawn for the slider of [`NESTED_MAP`] at `time` with nested judgements
/// `nested` (all unjudged if `None`), and the number of sprites.
fn nested_order(time: f64, nested: Option<Vec<Option<CircleJudgement>>>) -> (Vec<u32>, usize) {
    let map = nested_map(&ModSet::default());
    let (circles, sliders) = objects(&map, &ModSet::default(), Snaking::LAZER);
    let slider = LiveSlider {
        index: 0,
        ..live(None, None, None)
    };
    let mut live = Box::new(LiveSliders::new());
    match nested {
        Some(nested) => live.push(slider, nested),
        None => live = snapshot(&map, slider),
    }
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
        &live,
        time,
        ObjectTextures {
            circles: &circle_textures,
            sliders: &slider_textures,
        },
        None,
    );
    (order(&batch), batch.len())
}

#[test]
fn ticks_and_repeats_draw_between_the_tail_and_the_head_with_arrows_under_its_overlay() {
    // At 1450: the first tick and both repeats are shown (the second repeat appears at 1000),
    // the tail (appearing at 2000) not yet; both arrows go in one batch, between the head's
    // circle and its overlay.
    assert_eq!(
        nested_order(1450.0, None),
        (
            vec![
                TICK.0,
                TAIL.0,
                TAIL_OVERLAY.0,
                TAIL.0,
                TAIL_OVERLAY.0,
                HEAD.0,
                ARROW.0,
                HEAD_OVERLAY.0,
            ],
            9
        )
    );
    // Nested judgements that did not fit: no ticks or repeats.
    assert_eq!(
        nested_order(1450.0, Some(Vec::new())),
        (vec![HEAD.0, HEAD_OVERLAY.0], 2)
    );
}

#[test]
fn nested_judgements_that_do_not_fit_are_left_out() {
    let slider = live(None, None, None);
    let mut snapshot = Box::new(LiveSliders::new());
    snapshot.push(slider, std::iter::repeat_n(None, MAX_LIVE_NESTED - 1));
    snapshot.push(slider, [None, None]);
    snapshot.push(slider, [None]);
    assert_eq!(snapshot.as_slice().len(), 3);
    assert_eq!(snapshot.nested(0).len(), MAX_LIVE_NESTED - 1);
    assert!(snapshot.nested(1).is_empty());
    assert_eq!(snapshot.nested(2).len(), 1);
    assert_eq!(snapshot.nested_dropped(), 1);
    snapshot.clear();
    assert_eq!(
        (snapshot.as_slice().len(), snapshot.nested_dropped()),
        (0, 0)
    );
}

#[test]
fn ticks_and_arrows_use_the_first_skin_with_their_texture() {
    assert_eq!(found(&["sliderscorepoint"], &["reversearrow"]), 2);
    assert_eq!(found(&[], &["sliderscorepoint", "reversearrow"]), 2);
}

#[test]
fn the_arrow_style_reads_the_version_and_the_arrow_provider() {
    let style = |user_ini: &str, user_arrow: bool| {
        let (user_dir, default_dir) = (
            common::skin::SampleDir::new(&[]),
            common::skin::SampleDir::new(&[]),
        );
        fs::write(user_dir.0.join("skin.ini"), user_ini).unwrap();
        fs::write(default_dir.0.join("reversearrow.png"), b"not checked here").unwrap();
        if user_arrow {
            fs::write(user_dir.0.join("reversearrow.png"), b"not checked here").unwrap();
        }
        let chain = SkinChain::new(
            Some(Skin::load(&user_dir.0).unwrap()),
            Skin::load(&default_dir.0).unwrap(),
        );
        ArrowStyle::new(&chain)
    };
    // No version: 1.0, which rocks.
    assert_eq!(
        style("[General]\n", false),
        ArrowStyle {
            rotate: true,
            default_texture: true,
        }
    );
    assert_eq!(
        style("[General]\nVersion: 2.0\n", true),
        ArrowStyle {
            rotate: false,
            default_texture: false,
        }
    );
}

// ---- Ball and follow circle ----

const BALL: TextureId = TextureId(11);
const BALL_ND: TextureId = TextureId(12);
const BALL_SPEC: TextureId = TextureId(13);
const FOLLOW: TextureId = TextureId(14);
const BALL_1: TextureId = TextureId(15);

/// [`slider_textures`] with a two-frame ball, its layers and a follow circle.
fn ball_textures() -> SliderTextures {
    let frames = |ids: &[TextureId]| ids.iter().map(|&t| sprite(t).unwrap()).collect::<Vec<_>>();
    SliderTextures {
        ball: AnimationSprites::new(frames(&[BALL, BALL_1]), 1000.0 / 60.0),
        ball_nd: sprite(BALL_ND),
        ball_spec: sprite(BALL_SPEC),
        follow: AnimationSprites::new(frames(&[FOLLOW]), 1000.0 / 60.0),
        ..slider_textures()
    }
}

#[test]
fn animation_frames_follow_the_playback_position() {
    let frame = |playback| animation_frame(playback, 10.0, 3, true);
    // Before the start: the first frame; each starts where the previous ends.
    assert_eq!(frame(-5.0), 0);
    assert_eq!(frame(0.0), 0);
    assert_eq!(frame(9.999), 0);
    assert_eq!(frame(10.0), 1);
    assert_eq!(frame(25.0), 2);
    // Looping: from the start again.
    assert_eq!(frame(30.0), 0);
    assert_eq!(frame(41.0), 1);
    // Not looping: the last frame stays.
    assert_eq!(animation_frame(41.0, 10.0, 3, false), 2);
    assert_eq!(animation_frame(41.0, 10.0, 1, true), 0);

    let animation = ball_textures().ball;
    assert_eq!(animation.len(), 2);
    assert_eq!(animation.frame_at(20.0, 16.0).unwrap().texture, BALL_1);
    assert!(AnimationSprites::default().frame_at(0.0, 16.0).is_none());
}

/// The judgement at `time` of every nested object of the slider of [`NESTED_MAP`] in `judged`
/// (by start time, as `(start time, hit)`), the others unjudged.
fn nested_judgements(
    nested: &[NestedVisual],
    judged: &[(f64, bool)],
) -> Vec<Option<CircleJudgement>> {
    nested
        .iter()
        .map(|n| {
            let (_, hit) = judged.iter().find(|(t, _)| *t == n.start_time)?;
            let result = if *hit {
                HitResult::LargeTickHit
            } else {
                HitResult::LargeTickMiss
            };
            judged_at(n.start_time, result)
        })
        .collect()
}

fn judged_at(time: f64, result: HitResult) -> Option<CircleJudgement> {
    Some(CircleJudgement { time, result })
}

/// The ball of the slider of [`NESTED_MAP`] at `time`, pressed at `press`, judged as
/// `judgement`, with nested objects judged as `judged` (see [`nested_judgements`]).
fn ball_at(
    press: Option<f64>,
    judgement: Option<CircleJudgement>,
    judged: &[(f64, bool)],
    time: f64,
) -> BallPose {
    let map = nested_map(&ModSet::default());
    let (_, sliders) = objects(&map, &ModSet::default(), Snaking::LAZER);
    let slider = sliders.visuals.get(0).unwrap();
    let nested = sliders.visuals.nested(slider);
    let live = LiveSlider {
        index: 0,
        press_time: press,
        ..live(None, None, judgement)
    };
    let judgements = nested_judgements(nested, judged);
    ball_pose(
        slider,
        sliders.visuals.path(slider),
        nested,
        &live,
        &judgements,
        time,
    )
}

#[test]
fn the_ball_rolls_along_the_path_from_the_start_time() {
    // Appears at the start time with its container; the slider's animations count from its
    // appearance (preempt 600).
    let before = ball_at(None, None, &[], 999.0);
    assert_eq!((before.container_alpha, before.ball_alpha), (0.0, 0.0));
    assert_eq!(before.playback, 599.0);
    let at = ball_at(None, None, &[], 1000.0);
    assert_eq!((at.container_alpha, at.ball_alpha), (1.0, 1.0));
    assert_eq!(at.position, Vec2::new(200.0, 200.0));
    // Facing right along the first span, left along the second.
    assert!(close(at.rotation, 0.0), "{}", at.rotation);
    let first = ball_at(None, None, &[], 1500.0);
    assert_eq!(first.position, Vec2::new(300.0, 200.0));
    assert!(close(first.rotation, 0.0), "{}", first.rotation);
    let second = ball_at(None, None, &[], 2500.0);
    assert_eq!(second.position, Vec2::new(300.0, 200.0));
    assert!(close(second.rotation, -180.0), "{}", second.rotation);
    // Gone at the slider's hit state time: its end time until judged, then its judgement.
    let end = ball_at(None, None, &[], 4000.0);
    assert_eq!((end.container_alpha, end.ball_alpha), (1.0, 0.0));
    assert_eq!(end.position, Vec2::new(400.0, 200.0));
    let judged = judged_at(4010.0, HitResult::Great);
    assert_eq!(ball_at(None, judged, &[], 4005.0).ball_alpha, 1.0);
    assert_eq!(ball_at(None, judged, &[], 4010.0).ball_alpha, 0.0);
}

#[test]
fn the_ball_animates_slower_on_slow_sliders() {
    let map = nested_map(&ModSet::default());
    let (_, sliders) = objects(&map, &ModSet::default(), Snaking::LAZER);
    let slider = *sliders.visuals.get(0).unwrap();
    // 200 osu!pixels per second: at least a frame at 60 fps.
    assert_eq!(slider.velocity, 0.2);
    let pose = |slider: &SliderVisual| {
        let live = LiveSlider {
            index: 0,
            ..live(None, None, None)
        };
        ball_pose(
            slider,
            sliders.visuals.path(slider),
            &[],
            &live,
            &[],
            1000.0,
        )
    };
    assert_eq!(pose(&slider).frame_length, 1000.0 / 60.0);
    let slow = SliderVisual {
        velocity: 0.05,
        ..slider
    };
    assert!((pose(&slow).frame_length - 50.0).abs() < 1e-9);
}

/// `Easing.Out` at `t`.
fn out(t: f32) -> f32 {
    t * (2.0 - t)
}

#[test]
fn the_follow_circle_grows_in_on_press() {
    let follow = |press, time| {
        let p = ball_at(press, None, &[], time);
        (p.follow_alpha, p.follow_scale)
    };
    // Hidden until pressed.
    assert_eq!(follow(None, 2000.0), (0.0, 1.0));
    // Pressed at 1201: fades in over 60 ms and grows to twice its size over 180 ms.
    let (alpha, scale) = follow(Some(1201.0), 1231.0);
    assert!(
        close(alpha, 0.5) && close(scale, 1.0 + out(30.0 / 180.0)),
        "{alpha} {scale}"
    );
    assert_eq!(follow(Some(1201.0), 1381.0), (1.0, 2.0));
    // Held from before the start: from the start time.
    assert_eq!(follow(Some(900.0), 1000.0), (0.0, 1.0));
    assert_eq!(follow(Some(900.0), 1180.0), (1.0, 2.0));
    // Late in the slider: only the time left.
    assert_eq!(follow(Some(3970.0), 4000.0), (1.0, 2.0));
}

#[test]
fn the_follow_circle_pulses_on_ticks_once_grown() {
    let scale =
        |press, judged: &[(f64, bool)], time| ball_at(press, None, judged, time).follow_scale;
    let tick = [(1500.0, true)];
    // Grown when the tick is hit: back from 2.2 to 2 over 200 ms.
    assert!(close(scale(Some(1001.0), &tick, 1500.0), 2.2));
    assert!(close(scale(Some(1001.0), &tick, 1600.0), 2.1));
    assert!(close(scale(Some(1001.0), &tick, 1700.0), 2.0));
    // Still growing: no pulse.
    let growing = scale(Some(1450.0), &tick, 1500.0);
    assert!(close(growing, 1.0 + out(50.0 / 180.0)), "{growing}");
    // Repeats pulse too.
    let repeat = [(1500.0, true), (2000.0, true)];
    assert!(close(scale(Some(1001.0), &repeat, 2000.0), 2.2));
}

#[test]
fn the_follow_circle_bursts_on_a_break_and_shrinks_at_the_end() {
    let follow = |press, judged: &[(f64, bool)], time| {
        let p = ball_at(press, None, judged, time);
        (p.follow_alpha, p.follow_scale)
    };
    // A missed tick: grows to 4 and fades over 100 ms.
    let missed = [(1500.0, false)];
    let (alpha, scale) = follow(Some(1001.0), &missed, 1550.0);
    assert!(close(alpha, 0.5) && close(scale, 3.0), "{alpha} {scale}");
    assert_eq!(follow(Some(1001.0), &missed, 1600.0), (0.0, 4.0));
    // Pressed again later: grows in again from 1.
    assert_eq!(follow(Some(1700.0), &missed, 1700.0), (0.0, 1.0));
    assert_eq!(follow(Some(1700.0), &missed, 1880.0), (1.0, 2.0));
    // A press before the break does not undo it.
    assert_eq!(follow(Some(1400.0), &missed, 1600.0), (0.0, 4.0));

    // The tail hit: at the slider's end, shrinks to 1.6 and fades out over 200 ms.
    let all: Vec<_> = [1500.0, 2000.0, 2500.0, 3000.0, 3500.0, 4000.0]
        .into_iter()
        .map(|t| (t, true))
        .collect();
    assert_eq!(follow(Some(1001.0), &all, 3999.0), (1.0, 2.0));
    let (alpha, scale) = follow(Some(1001.0), &all, 4100.0);
    assert!(
        close(alpha, 0.75) && close(scale, 2.0 - out(0.5) * 0.4),
        "{alpha} {scale}"
    );
    assert_eq!(follow(Some(1001.0), &all, 4200.0), (0.0, 1.6));
    // The tail missed: bursts when it is judged.
    let mut tail_missed = all.clone();
    tail_missed.last_mut().unwrap().1 = false;
    assert_eq!(follow(Some(1001.0), &tail_missed, 4100.0), (0.0, 4.0));
}

/// The slider of [`NESTED_MAP`] with its ticks and repeats hit and its tail hit at
/// `tail_time` (lazer judges it up to 36 ms before the end).
fn all_hit_with_tail_at(nested: &[NestedVisual], tail_time: f64) -> Vec<Option<CircleJudgement>> {
    nested
        .iter()
        .map(|n| match n.piece {
            NestedPiece::Head => None,
            NestedPiece::Tail => judged_at(tail_time, HitResult::SliderTailHit),
            _ => judged_at(n.start_time, HitResult::LargeTickHit),
        })
        .collect()
}

#[test]
fn a_press_after_the_tail_is_hit_cancels_the_end_animation() {
    let map = nested_map(&ModSet::default());
    let (_, sliders) = objects(&map, &ModSet::default(), Snaking::LAZER);
    let slider = sliders.visuals.get(0).unwrap();
    let nested = sliders.visuals.nested(slider);
    let judgements = all_hit_with_tail_at(nested, 3964.0);
    let follow = |press, time| {
        let live = LiveSlider {
            index: 0,
            press_time: Some(press),
            ..live(None, None, None)
        };
        let p = ball_pose(
            slider,
            sliders.visuals.path(slider),
            nested,
            &live,
            &judgements,
            time,
        );
        (p.follow_alpha, p.follow_scale)
    };
    // The tail judged early: the end animation still starts at the end time.
    assert_eq!(follow(1001.0, 3999.0), (1.0, 2.0));
    assert_eq!(follow(1001.0, 4200.0), (0.0, 1.6));
    // Pressed in the same update as the tail's judgement: the end animation plays.
    assert_eq!(follow(3964.0, 4200.0), (0.0, 1.6));
    // Pressed after it: the press drops the later end animation.
    assert_eq!(follow(3980.0, 4200.0), (1.0, 2.0));
    // At the end time: the later press wins at the same start.
    assert_eq!(follow(4000.0, 4200.0), (1.0, 2.0));
    // After it, the slider not judged yet: back at once, no time left.
    assert_eq!(follow(4005.0, 4005.0), (1.0, 2.0));
}

#[test]
fn a_press_before_a_short_slider_grows_in_the_time_left() {
    let map = nested_map(&ModSet::default());
    let (_, sliders) = objects(&map, &ModSet::default(), Snaking::LAZER);
    let slider = SliderVisual {
        end_time: 1150.0,
        ..*sliders.visuals.get(0).unwrap()
    };
    let live = LiveSlider {
        index: 0,
        press_time: Some(950.0),
        ..live(None, None, None)
    };
    let follow = |time| {
        let p = ball_pose(
            &slider,
            sliders.visuals.path(&slider),
            &[],
            &live,
            &[],
            time,
        );
        (p.follow_alpha, p.follow_scale)
    };
    // From the start time, over the 200 ms left from the press: 180 ms.
    assert_eq!(follow(1000.0), (0.0, 1.0));
    assert_eq!(follow(1180.0), (1.0, 2.0));
    let slider = SliderVisual {
        end_time: 1100.0,
        ..slider
    };
    // 150 ms left.
    let p = ball_pose(
        &slider,
        sliders.visuals.path(&slider),
        &[],
        &live,
        &[],
        1075.0,
    );
    assert!(close(p.follow_scale, 1.0 + out(0.5)), "{}", p.follow_scale);
    let p = ball_pose(
        &slider,
        sliders.visuals.path(&slider),
        &[],
        &live,
        &[],
        1150.0,
    );
    assert_eq!((p.follow_alpha, p.follow_scale), (1.0, 2.0));
}

#[test]
fn a_degenerate_slider_poses_a_finite_ball() {
    let map = nested_map(&ModSet::default());
    let (_, sliders) = objects(&map, &ModSet::default(), Snaking::LAZER);
    let slider = sliders.visuals.get(0).unwrap();
    let live = LiveSlider {
        index: 0,
        ..live(None, None, None)
    };
    // No length to tell the direction: the ball keeps its initial rotation.
    let zero_length = SliderVisual {
        distance: 0.0,
        ..*slider
    };
    let p = ball_pose(
        &zero_length,
        sliders.visuals.path(slider),
        &[],
        &live,
        &[],
        1200.0,
    );
    assert_eq!(p.rotation, 0.0);
    assert!(p.position.x.is_finite() && p.position.y.is_finite());
    // No duration: the progress is not a number exactly at the start time.
    let instant = SliderVisual {
        end_time: slider.timing.start_time,
        ..*slider
    };
    for time in [999.0, 1000.0, 1001.0] {
        let p = ball_pose(
            &instant,
            sliders.visuals.path(slider),
            &[],
            &live,
            &[],
            time,
        );
        assert!(p.rotation.is_finite(), "{time}: {p:?}");
        assert!(p.follow_alpha.is_finite() && p.follow_scale.is_finite());
    }
}

#[test]
fn the_follow_circle_and_ball_draw_above_the_head() {
    let map = nested_map(&ModSet::default());
    let (circles, sliders) = objects(&map, &ModSet::default(), Snaking::LAZER);
    let slider = LiveSlider {
        index: 0,
        press_time: Some(1001.0),
        ..live(None, None, None)
    };
    let mut batch = SpriteBatch::new();
    let mut bodies = SliderBodies::new();
    let (circle_textures, slider_textures) = (circle_textures(), ball_textures());
    let layout = common::layout();
    push_objects(
        &circles,
        &sliders,
        &mut batch,
        &mut bodies,
        &layout,
        &[],
        &snapshot(&map, slider),
        2610.0,
        ObjectTextures {
            circles: &circle_textures,
            sliders: &slider_textures,
        },
        None,
    );
    let batches = batch.batches();
    let tail: Vec<_> = batches[batches.len() - 4..]
        .iter()
        .map(|b| (b.texture, b.blend))
        .collect();
    // Playback 2210 ms at 60 fps: frame 132, the first of two.
    assert_eq!(
        tail,
        [
            (FOLLOW, Blend::Alpha),
            (BALL_ND, Blend::Alpha),
            (BALL, Blend::Alpha),
            (BALL_SPEC, Blend::Additive),
        ]
    );
    // The follow circle at half its texture size, doubled; the ball's layers at its size.
    let scale = layout.scale() * sliders.visuals.get(0).unwrap().head.scale;
    let quads = batch.vertices().len() / 4;
    let width = |quad: usize| {
        let v = &batch.vertices()[quad * 4..quad * 4 + 4];
        let xs = v.iter().map(|v| v.position[0]);
        xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min)
    };
    assert!((width(quads - 4) - 128.0 * scale).abs() < 1e-3);
    assert!((width(quads - 3) - 128.0 * scale).abs() < 1e-3);
    // The ball faces left on the second span: turned half a turn, the same width.
    assert!((width(quads - 2) - 128.0 * scale).abs() < 1e-2);
    // The ball takes the skin's `SliderBall` colour (white in a skin without one).
    let ball = &batch.vertices()[(quads - 2) * 4];
    assert_eq!(ball.colour[0..3], [1.0, 1.0, 1.0]);
    let nd = &batch.vertices()[(quads - 3) * 4];
    assert!((nd.colour[0] - 5.0 / 255.0).abs() < 1e-6);
}

#[test]
fn the_ball_and_its_layers_come_from_one_skin() {
    // A two-frame ball; the layers of a later skin are not used.
    assert_eq!(found(&["sliderb0", "sliderb1"], &["sliderb-nd"]), 2);
    assert_eq!(found(&[], &["sliderb", "sliderb-nd", "sliderb-spec"]), 3);
    // The follow circle is looked up on its own.
    assert_eq!(
        found(
            &["sliderfollowcircle-0", "sliderfollowcircle-1"],
            &["sliderb"]
        ),
        3
    );
}

#[test]
fn the_ball_style_reads_the_skin_that_provides_the_ball() {
    let style = |user_ini: &str, user_ball: bool| {
        let (user_dir, default_dir) = (
            common::skin::SampleDir::new(&[]),
            common::skin::SampleDir::new(&[]),
        );
        fs::write(user_dir.0.join("skin.ini"), user_ini).unwrap();
        let default_ini = "[General]\nAllowSliderBallTint: 1\n[Colours]\nSliderBall: 2,170,255\n";
        fs::write(default_dir.0.join("skin.ini"), default_ini).unwrap();
        fs::write(default_dir.0.join("sliderb.png"), b"not checked here").unwrap();
        if user_ball {
            fs::write(user_dir.0.join("sliderb0.png"), b"not checked here").unwrap();
        }
        let chain = SkinChain::new(
            Some(Skin::load(&user_dir.0).unwrap()),
            Skin::load(&default_dir.0).unwrap(),
        );
        BallStyle::new(&chain)
    };
    let accent = Color::rgb(1.0, 0.0, 0.0);
    // The default skin's ball: tinted with the combo colour.
    let default = style("[Colours]\nSliderBall: 10,20,30\n", false);
    assert!(default.tint);
    assert_eq!(default.ball_colour(accent), accent);
    // The user's ball: its colour, and no tint without the setting.
    let user = style("[Colours]\nSliderBall: 255,0,255\n", true);
    assert!(!user.tint);
    assert_eq!(user.ball_colour(accent), Color::rgb(1.0, 0.0, 1.0));
    assert_eq!(style("", true).ball_colour(accent), Color::WHITE);
}
