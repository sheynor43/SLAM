//! Spinners: the legacy style a skin gives, the pose at key moments, the sprites of both
//! styles and a frame written without allocations.

mod common;

use std::fs;

use slam_osu::judgement::HitResult;
use slam_osu::replay::{Actions, InputFrame, ReplayInput, Simulator};
use slam_osu::rules::{CircleJudgement, LiveSpinner, OsuRules};
use slam_osu::{Beatmap, GameplayMod, Mod, ModSet, Vec2};
use slam_play::circles::{CircleMods, SkinSprite};
use slam_play::spinners::{
    HitSpinners, LiveSpinners, SpinnerEntry, SpinnerKind, SpinnerPose, SpinnerStyle,
    SpinnerTextures, SpinnerVisuals, TickFade, spinner_pose,
};
use slam_render::atlas::UvRect;
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Blend, Color};
use slam_skin::{Skin, SkinChain};
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

/// AR 9 (preempt 600, fade-in 400) and a spinner at the centre from 2000 to 4000 ms.
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
256,192,2000,12,0,4000
";

const START: f64 = 2000.0;
const END: f64 = 4000.0;
const APPEAR: f64 = START - 600.0;

fn beatmap(mods: &ModSet) -> Beatmap {
    let mut map = Beatmap::decode(MAP.as_bytes()).unwrap();
    for m in mods.mods() {
        m.clone().apply_to_beatmap(&mut map);
    }
    map
}

fn hidden() -> ModSet {
    ModSet::new(vec![Mod::Hidden {
        only_fade_approach_circles: false,
    }])
    .unwrap()
}

/// A skin directory with empty textures `names` and `ini` as its `skin.ini`.
fn skin_dir(names: &[&str], ini: &str) -> common::skin::SampleDir {
    let dir = common::skin::SampleDir::new(&[]);
    for name in names {
        fs::write(dir.0.join(format!("{name}.png")), b"not checked here").unwrap();
    }
    fs::write(dir.0.join("skin.ini"), ini).unwrap();
    dir
}

/// The style of a user skin with textures `user` (none without a user skin) over a default
/// skin with textures `default`.
fn style_of(user: Option<(&[&str], &str)>, default: &[&str]) -> SpinnerStyle {
    let default_dir = skin_dir(default, "");
    let user_dir = user.map(|(names, ini)| skin_dir(names, ini));
    let chain = SkinChain::new(
        user_dir.as_ref().map(|d| Skin::load(&d.0).unwrap()),
        Skin::load(&default_dir.0).unwrap(),
    );
    SpinnerStyle::new(CircleMods::new(&ModSet::default()), &chain)
}

fn style(kind: SpinnerKind, mods: &ModSet) -> SpinnerStyle {
    SpinnerStyle {
        kind: Some(kind),
        approach: true,
        blink: false,
        background: Color::rgb(0.5, 0.5, 0.5),
        turn_ratio: 0.5,
        mods: CircleMods::new(mods),
    }
}

fn live() -> LiveSpinner {
    LiveSpinner {
        index: 0,
        is_spinning: false,
        progress_unclamped: 0.0,
        bonus_max_spins: 0,
        progress: 0.0,
        rotation: 0.0,
        spun_rotation: 0.0,
        time_completed: None,
        last_bonus_hit: None,
        judgement: None,
    }
}

fn entry(live: LiveSpinner) -> SpinnerEntry {
    SpinnerEntry {
        live,
        tick_fade: None,
    }
}

fn judged(result: HitResult) -> Option<CircleJudgement> {
    Some(CircleJudgement { time: END, result })
}

fn pose_with(mods: &ModSet, entry: &SpinnerEntry, time: f64) -> SpinnerPose {
    let map = beatmap(mods);
    let visuals = SpinnerVisuals::new(&map);
    let spinner = visuals.get(0).unwrap();
    spinner_pose(spinner, entry, time, &style(SpinnerKind::NewStyle, mods))
}

fn pose(entry: &SpinnerEntry, time: f64) -> SpinnerPose {
    pose_with(&ModSet::default(), entry, time)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

// ---- Style ----

#[test]
fn the_first_skin_with_a_legacy_spinner_gives_its_style() {
    let none: &[&str] = &[];
    // The default skin's new style, without its approach circle.
    let s = style_of(None, &["spinner-top"]);
    assert_eq!((s.kind, s.approach), (Some(SpinnerKind::NewStyle), false));
    let s = style_of(Some((none, "")), &["spinner-top"]);
    assert_eq!((s.kind, s.approach), (Some(SpinnerKind::NewStyle), false));
    // The user's new style shows it.
    let s = style_of(Some((&["spinner-top"], "")), &["spinner-top"]);
    assert_eq!((s.kind, s.approach), (Some(SpinnerKind::NewStyle), true));
    // A background makes the old style, ahead of a top in the same skin.
    let s = style_of(
        Some((&["spinner-background", "spinner-top"], "")),
        &["spinner-top"],
    );
    assert_eq!((s.kind, s.approach), (Some(SpinnerKind::OldStyle), true));
    let s = style_of(Some((none, "")), &["spinner-background"]);
    assert_eq!(s.kind, Some(SpinnerKind::OldStyle));
    // Without either, no legacy spinner.
    assert_eq!(style_of(Some((&["spinner-circle"], "")), &[]).kind, None);
}

#[test]
fn spinner_settings_come_through_the_skins() {
    let none: &[&str] = &[];
    let s = style_of(Some((none, "")), &["spinner-top", "spinner-middle2"]);
    assert_eq!(s.turn_ratio, 0.5);
    assert!(s.blink);
    assert_eq!(
        s.background,
        Color::rgb(100.0 / 255.0, 100.0 / 255.0, 100.0 / 255.0)
    );
    let s = style_of(
        Some((
            none,
            "[General]\nSpinnerNoBlink: 1\n[Colours]\nSpinnerBackground: 255,0,51,128\n",
        )),
        &["spinner-top"],
    );
    assert_eq!(s.turn_ratio, 1.0);
    assert!(!s.blink);
    // Skins' colours have no alpha.
    assert_eq!(s.background, Color::rgb(1.0, 0.0, 0.2));
}

// ---- Poses ----

#[test]
fn the_spinner_fades_in_before_its_start() {
    let idle = entry(live());
    assert_eq!(pose(&idle, APPEAR).body_alpha, 0.0);
    assert_eq!(pose(&idle, START - 400.0).body_alpha, 0.0);
    assert!(close(pose(&idle, START - 200.0).body_alpha, 0.5));
    assert_eq!(pose(&idle, START).body_alpha, 1.0);
    assert_eq!(pose(&idle, APPEAR).object_alpha, 1.0);
    // "Spin" fades in over the second half of the fade-in and out before the end.
    assert_eq!(pose(&idle, START - 200.0).spin_alpha, 0.0);
    assert!(close(pose(&idle, START - 100.0).spin_alpha, 0.5));
    assert_eq!(pose(&idle, END - 400.0).spin_alpha, 1.0);
    assert!(close(pose(&idle, END - 200.0).spin_alpha, 0.5));
    assert_eq!(pose(&idle, END).spin_alpha, 0.0);
}

#[test]
fn the_approach_circle_shrinks_over_the_duration() {
    let idle = entry(live());
    assert_eq!(pose(&idle, APPEAR).approach_scale, 1.86);
    assert_eq!(pose(&idle, START).approach_scale, 1.86);
    assert!(close(pose(&idle, START + 1000.0).approach_scale, 0.98));
    assert!(close(pose(&idle, END).approach_scale, 0.1));
    assert_eq!(pose(&idle, START).approach_alpha, 1.0);
}

#[test]
fn the_middle_turns_red_and_the_layers_grow_with_the_progress() {
    let idle = entry(live());
    assert_eq!(pose(&idle, START).middle_colour, Color::WHITE);
    let half = pose(&idle, START + 1000.0).middle_colour;
    assert_eq!(half.r, 1.0);
    // Halfway in linear space.
    assert!(close(half.g, 0.735_356_6) && half.g == half.b, "{half:?}");
    assert_eq!(pose(&idle, END).middle_colour, Color::rgb(1.0, 0.0, 0.0));

    let at = |progress| {
        let state = LiveSpinner {
            progress,
            rotation: 123.0,
            ..live()
        };
        pose(&entry(state), START + 500.0)
    };
    assert_eq!(at(0.0).scale, 0.8);
    assert!(close(at(0.5).scale, 0.95));
    assert_eq!(at(1.0).scale, 1.0);
    assert_eq!((at(0.5).glow_alpha, at(0.5).rotation), (0.5, 123.0));
}

#[test]
fn the_spinner_fades_out_once_judged() {
    let hit = entry(LiveSpinner {
        progress: 0.8,
        judgement: judged(HitResult::Meh),
        ..live()
    });
    assert_eq!(pose(&hit, END).object_alpha, 1.0);
    assert!(close(pose(&hit, END + 120.0).object_alpha, 0.5));
    assert_eq!(pose(&hit, END + 240.0).object_alpha, 0.0);
    // A hit fades the glow out over 300 ms from its progress.
    assert!(close(pose(&hit, END + 150.0).glow_alpha, 0.4));
    assert_eq!(pose(&hit, END + 150.0).spin_alpha, 0.0);
    // A miss keeps the glow.
    let missed = entry(LiveSpinner {
        progress: 0.5,
        judgement: judged(HitResult::Miss),
        ..live()
    });
    assert_eq!(pose(&missed, END + 150.0).glow_alpha, 0.5);
    assert!(close(pose(&missed, END + 120.0).object_alpha, 0.5));
}

#[test]
fn tick_hits_fade_spin_out() {
    // The first hit at 2500 fades from 1; at the second, 150 ms later, half is left, and that
    // half fades out over another 300 ms.
    let ticks = SpinnerEntry {
        live: live(),
        tick_fade: TickFade::new(&[2500.0, 2650.0]),
    };
    assert!(close(pose(&ticks, 2650.0).spin_alpha, 0.5));
    assert!(close(pose(&ticks, 2800.0).spin_alpha, 0.25));
    assert_eq!(pose(&ticks, 2950.0).spin_alpha, 0.0);
    // Hits in the last 400 ms fade from the fade-out before the end.
    let late = SpinnerEntry {
        live: live(),
        tick_fade: TickFade::new(&[END - 200.0]),
    };
    assert!(close(pose(&late, END - 200.0).spin_alpha, 0.5));
    assert!(close(pose(&late, END - 50.0).spin_alpha, 0.25));
    // A judgement replaces the tick fades.
    let judged = SpinnerEntry {
        live: LiveSpinner {
            judgement: judged(HitResult::Great),
            ..live()
        },
        tick_fade: TickFade::new(&[2500.0]),
    };
    assert_eq!(pose(&judged, END).spin_alpha, 0.0);
}

#[test]
fn clear_shows_from_the_completion_to_the_end() {
    let complete = |time_completed| {
        entry(LiveSpinner {
            progress: 1.0,
            time_completed: Some(time_completed),
            ..live()
        })
    };
    let early = complete(2500.0);
    assert_eq!(pose(&early, 2499.0).clear_alpha, 0.0);
    let at = |time| {
        let p = pose(&early, time);
        (p.clear_alpha, p.clear_scale)
    };
    assert_eq!(at(2500.0), (0.0, 2.0));
    // `Easing.Out` halfway.
    assert!(close(at(2700.0).0, 0.75));
    assert!(close(at(2740.0).1, 0.8));
    assert!(close(at(2820.0).1, 0.9));
    assert_eq!(at(2900.0), (1.0, 1.0));
    assert_eq!(at(END - 50.0).0, 1.0);
    assert!(close(at(END - 25.0).0, 0.5));
    // Completed late: the transforms start 400 ms before the end, already under way.
    let late = complete(END - 200.0);
    assert!(close(pose(&late, END - 200.0).clear_alpha, 0.75));
    // Not complete or judged: hidden.
    assert_eq!(pose(&entry(live()), 3000.0).clear_alpha, 0.0);
    let judged = entry(LiveSpinner {
        judgement: judged(HitResult::Great),
        ..complete(2500.0).live
    });
    assert_eq!(pose(&judged, END).clear_alpha, 0.0);
}

#[test]
fn bonus_ticks_flash_the_glow_until_the_judgement() {
    let flashed = |hit, judgement| {
        entry(LiveSpinner {
            last_bonus_hit: Some(hit),
            judgement,
            ..live()
        })
    };
    let idle = flashed(3000.0, None);
    assert_eq!(pose(&idle, 3000.0).glow_flash, 1.0);
    assert_eq!(pose(&idle, 3100.0).glow_flash, 0.5);
    assert_eq!(pose(&idle, 3200.0).glow_flash, 0.0);
    // A judgement rewinds the flash: the glow's own colour from then.
    let judged = flashed(END - 50.0, judged(HitResult::Great));
    assert_eq!(pose(&judged, END - 25.0).glow_flash, 0.875);
    assert_eq!(pose(&judged, END).glow_flash, 0.0);
}

#[test]
fn hidden_hides_the_approach_circle_and_fades_the_spinner_at_the_end() {
    let mods = hidden();
    let idle = entry(live());
    let at = |time| pose_with(&mods, &idle, time);
    assert_eq!(at(APPEAR).approach_alpha, 0.0);
    // The fade-in is 0.4 of the preempt.
    assert!(close(at(START - 120.0).body_alpha, 0.5));
    assert_eq!(at(END).object_alpha, 1.0);
    // Faded out over 0.3 of the preempt from the end, judged or not.
    assert!(close(at(END + 90.0).object_alpha, 0.5));
    let hit = entry(LiveSpinner {
        judgement: judged(HitResult::Great),
        ..live()
    });
    assert!(close(pose_with(&mods, &hit, END + 90.0).object_alpha, 0.5));
    assert!(close(
        pose_with(&mods, &hit, END + 150.0).object_alpha,
        1.0 / 6.0
    ));
}

// ---- Sprites ----

const GLOW: TextureId = TextureId(1);
const BOTTOM: TextureId = TextureId(2);
const TOP: TextureId = TextureId(3);
const MIDDLE2: TextureId = TextureId(4);
const MIDDLE: TextureId = TextureId(5);
const APPROACH: TextureId = TextureId(6);
const SPIN: TextureId = TextureId(7);
const CLEAR: TextureId = TextureId(8);
const BACKGROUND: TextureId = TextureId(9);
const CIRCLE: TextureId = TextureId(10);
const METRE: TextureId = TextureId(11);

fn sprite(texture: TextureId, size: Vec2) -> Option<SkinSprite> {
    Some(SkinSprite {
        texture,
        uv: UvRect::FULL,
        size,
    })
}

fn textures() -> SpinnerTextures {
    let square = Vec2::new(100.0, 100.0);
    SpinnerTextures {
        background: sprite(BACKGROUND, square),
        circle: sprite(CIRCLE, square),
        // Stable's full-window metre.
        metre: sprite(METRE, Vec2::new(1024.0, 768.0)),
        glow: sprite(GLOW, square),
        bottom: sprite(BOTTOM, square),
        top: sprite(TOP, square),
        middle2: sprite(MIDDLE2, square),
        middle: sprite(MIDDLE, square),
        approach: sprite(APPROACH, square),
        spin: sprite(SPIN, square),
        clear: sprite(CLEAR, square),
    }
}

fn spinners(kind: SpinnerKind, map: &Beatmap) -> HitSpinners {
    HitSpinners {
        visuals: SpinnerVisuals::new(map),
        style: style(kind, &ModSet::default()),
    }
}

fn order(batch: &SpriteBatch) -> Vec<(u32, Blend)> {
    batch
        .batches()
        .iter()
        .map(|b| (b.texture.0, b.blend))
        .collect()
}

/// The centre of sprite `i` of `batch`.
fn centre(batch: &SpriteBatch, i: usize) -> [f32; 2] {
    let v = &batch.vertices()[i * 4..i * 4 + 4];
    [
        (v[0].position[0] + v[2].position[0]) / 2.0,
        (v[0].position[1] + v[2].position[1]) / 2.0,
    ]
}

fn close_to(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
}

#[test]
fn the_new_style_draws_its_layers_around_the_centre() {
    let map = beatmap(&ModSet::default());
    let layout = common::layout();
    let mut batch = SpriteBatch::new();
    let state = SpinnerEntry {
        live: LiveSpinner {
            rotation: 90.0,
            ..live()
        },
        tick_fade: None,
    };
    spinners(SpinnerKind::NewStyle, &map).push(&mut batch, &layout, &[state], START, &textures());
    assert_eq!(
        order(&batch),
        [
            (BOTTOM.0, Blend::Alpha),
            (TOP.0, Blend::Alpha),
            (MIDDLE2.0, Blend::Alpha),
            (MIDDLE.0, Blend::Alpha),
            (APPROACH.0, Blend::Alpha),
            (SPIN.0, Blend::Alpha),
        ]
    );
    // The glow is transparent without progress. At 640×480, osu!pixel (256, 192) is window
    // (320, 248).
    for i in 0..5 {
        assert!(close_to(centre(&batch, i), [320.0, 248.0]), "sprite {i}");
    }
    // "Spin" is 116 osu!pixels below the centre.
    assert!(close_to(centre(&batch, 5), [320.0, 364.0]));
    // Without progress the layers are at 0.8 of the sprite scale: 50 wide.
    let v = &batch.vertices()[3 * 4..3 * 4 + 4];
    assert!((v[1].position[0] - v[0].position[0] - 50.0).abs() < 1e-3);
    // The top turns at half the rotation: a quarter turn of the middle, an eighth of the top.
    let edge = |i: usize| {
        let v = &batch.vertices()[i * 4..i * 4 + 4];
        (v[1].position[1] - v[0].position[1]).atan2(v[1].position[0] - v[0].position[0])
    };
    assert!((edge(2) - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    assert!((edge(1) - std::f32::consts::FRAC_PI_4).abs() < 1e-4);
    assert!((edge(0) - std::f32::consts::FRAC_PI_4 / 3.0).abs() < 1e-4);
    // With progress the glow goes first, added.
    let glowing = SpinnerEntry {
        live: LiveSpinner {
            progress: 0.5,
            ..live()
        },
        tick_fade: None,
    };
    batch.clear();
    spinners(SpinnerKind::NewStyle, &map).push(&mut batch, &layout, &[glowing], START, &textures());
    assert_eq!(order(&batch)[0], (GLOW.0, Blend::Additive));
    assert_eq!(batch.vertices()[0].colour[3], 0.5);
}

#[test]
fn the_old_style_draws_its_background_disc_and_metre() {
    let map = beatmap(&ModSet::default());
    let layout = common::layout();
    let mut batch = SpriteBatch::new();
    let state = entry(LiveSpinner {
        progress: 0.3,
        ..live()
    });
    spinners(SpinnerKind::OldStyle, &map).push(&mut batch, &layout, &[state], START, &textures());
    assert_eq!(
        order(&batch).iter().map(|b| b.0).collect::<Vec<_>>(),
        [BACKGROUND.0, CIRCLE.0, METRE.0, APPROACH.0, SPIN.0]
    );
    assert_eq!(batch.vertices()[0].colour, [0.5, 0.5, 0.5, 1.0]);
    // Three bars of 43.25 osu!pixels without blinking: the metre (480 osu!pixels tall) is
    // masked from 432.5 - 129.75 osu!pixels down, its top left at the box's left edge, 29
    // osu!pixels below the box's top; at 640×480 the box is the window.
    let metre = &batch.vertices()[2 * 4..2 * 4 + 4];
    let hidden = 692.0 * 0.625 - 3.0 * 692.0 * 0.625 / 10.0;
    assert!((metre[0].position[0] - 0.0).abs() < 1e-3);
    assert!((metre[0].position[1] - (29.0 + hidden)).abs() < 1e-3);
    assert!((metre[2].position[1] - (29.0 + 480.0)).abs() < 1e-3);
    assert!((metre[2].position[0] - 640.0).abs() < 1e-3);
    assert!((metre[0].uv[1] - hidden / 480.0).abs() < 1e-5);
    assert_eq!(metre[2].uv[1], 1.0);
}

#[test]
fn nothing_is_drawn_without_a_legacy_spinner_or_before_the_fade_in() {
    let map = beatmap(&ModSet::default());
    let layout = common::layout();
    let mut batch = SpriteBatch::new();
    let mut none = spinners(SpinnerKind::NewStyle, &map);
    none.style.kind = None;
    none.push(&mut batch, &layout, &[entry(live())], START, &textures());
    assert_eq!(batch.len(), 0);
    spinners(SpinnerKind::NewStyle, &map).push(
        &mut batch,
        &layout,
        &[entry(live())],
        START - 400.0,
        &textures(),
    );
    assert_eq!(batch.len(), 0);
}

// ---- Snapshot and frames ----

/// Circles around the centre, a turn every `period` ms, the left button held, from `from` to
/// `to`.
fn spin_frames(from: f64, to: f64, period: f64) -> Vec<InputFrame> {
    let mut frames = Vec::new();
    let mut time = from;
    while time <= to {
        let angle = (time - from) / period * std::f64::consts::TAU;
        frames.push(InputFrame {
            time,
            position: Vec2::new(
                256.0 + 50.0 * angle.cos() as f32,
                192.0 + 50.0 * angle.sin() as f32,
            ),
            actions: Actions::LEFT,
        });
        time += 10.0;
    }
    frames
}

#[test]
fn a_frame_does_not_allocate() {
    let map = beatmap(&ModSet::default());
    let layout = common::layout();
    let mut frames = vec![InputFrame {
        time: 0.0,
        position: Vec2::new(256.0, 142.0),
        actions: Actions::NONE,
    }];
    frames.extend(spin_frames(START, END, 150.0));
    let mut sim = Simulator::new(
        &map,
        ReplayInput::new(frames),
        OsuRules::new(&map, &ModSet::default()),
    );
    let mut live = Box::new(LiveSpinners::new());
    let mut batch = SpriteBatch::with_capacity(64);
    let textures = textures();
    let (old, new) = (
        spinners(SpinnerKind::OldStyle, &map),
        spinners(SpinnerKind::NewStyle, &map),
    );
    let mut drawn = 0;
    let mut completed = false;
    while sim.step() {
        let time = sim.time();
        assert_no_alloc(|| {
            live.capture(sim.rules());
            batch.clear();
            old.push(&mut batch, &layout, live.as_slice(), time, &textures);
            new.push(&mut batch, &layout, live.as_slice(), time, &textures);
        });
        drawn += batch.len();
        if let Some(s) = live.as_slice().first() {
            assert_eq!(
                s.tick_fade,
                TickFade::new(sim.rules().spinner_tick_hits(s.live.index))
            );
            completed |= s.live.time_completed.is_some();
        }
    }
    assert!(drawn > 0);
    assert!(completed);
    assert_eq!(live.dropped(), 0);
}

#[test]
fn an_unjudged_spinner_fades_out_from_its_end_time() {
    let idle = entry(live());
    assert_eq!(pose(&idle, END).object_alpha, 1.0);
    assert!(close(pose(&idle, END + 120.0).object_alpha, 0.5));
}

#[test]
fn the_rotation_keeps_following_the_spins_while_fading_out() {
    let state = LiveSpinner {
        rotation: 900.0,
        spun_rotation: 1000.0,
        judgement: judged(HitResult::Great),
        ..live()
    };
    let spun = entry(state);
    assert_eq!(pose(&spun, END).rotation, 900.0);
    // The gap shrinks by 1% per millisecond.
    let gap = 100.0 * 0.99f64.powf(100.0);
    assert!(close(
        pose(&spun, END + 100.0).rotation,
        (1000.0 - gap) as f32
    ));
    // Unjudged, the rules' rotation as is.
    let idle = entry(LiveSpinner {
        judgement: None,
        ..state
    });
    assert_eq!(pose(&idle, END + 100.0).rotation, 900.0);
}

#[test]
fn hidden_fading_only_approach_circles_keeps_the_spinner() {
    let mods = ModSet::new(vec![Mod::Hidden {
        only_fade_approach_circles: true,
    }])
    .unwrap();
    let idle = entry(live());
    assert_eq!(pose_with(&mods, &idle, START).approach_alpha, 0.0);
    assert_eq!(pose_with(&mods, &idle, END).object_alpha, 1.0);
    // Faded out as without Hidden.
    assert!(close(
        pose_with(&mods, &idle, END + 120.0).object_alpha,
        0.5
    ));
}

#[test]
fn a_short_spinner_fades_spin_over_its_duration() {
    let map = beatmap(&ModSet::default());
    let visuals = SpinnerVisuals::new(&map);
    let short = slam_play::spinners::SpinnerVisual {
        end_time: START + 200.0,
        duration: 200.0,
        ..*visuals.get(0).unwrap()
    };
    let at = |time| {
        spinner_pose(
            &short,
            &entry(live()),
            time,
            &style(SpinnerKind::NewStyle, &ModSet::default()),
        )
    };
    // "Spin" fades out from its start, where it has just faded in.
    assert_eq!(at(START).spin_alpha, 1.0);
    assert!(close(at(START + 100.0).spin_alpha, 0.5));
    assert_eq!(at(START + 200.0).spin_alpha, 0.0);
    // A spinner without duration is red from its start.
    let empty = slam_play::spinners::SpinnerVisual {
        end_time: START,
        duration: 0.0,
        ..short
    };
    let pose = spinner_pose(
        &empty,
        &entry(live()),
        START,
        &style(SpinnerKind::NewStyle, &ModSet::default()),
    );
    assert_eq!(pose.middle_colour, Color::rgb(1.0, 0.0, 0.0));
}

#[test]
fn a_late_tick_does_not_bring_back_the_fade_out() {
    // The first hit faded "spin" out for good; a hit in the last 400 ms keeps it out.
    let ticks = SpinnerEntry {
        live: live(),
        tick_fade: TickFade::new(&[2500.0, END - 300.0]),
    };
    assert_eq!(pose(&ticks, END - 350.0).spin_alpha, 0.0);
    assert_eq!(pose(&ticks, END - 100.0).spin_alpha, 0.0);
}
