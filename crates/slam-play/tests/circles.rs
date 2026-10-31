//! Hit circles: combo colours and numbers from the beatmap and the skins, sprite order, and a
//! frame written without allocations.
//!
//! Colour cases follow lazer's `TestSceneLegacyBeatmapSkin` (osu!lazer 2026.1005.0-lazer).

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use slam_osu::replay::{InputFrame, ReplayInput, Simulator};
use slam_osu::rules::{CircleJudgement, LiveCircle, OsuRules};
use slam_osu::{Beatmap, ModSet, Vec2};
use slam_play::circles::{
    CircleMods, CircleSettings, CircleSkinConfig, CircleStyle, CircleTextures, CircleVisuals,
    HitCircles, LiveCircles, SkinSprite,
};
use slam_render::Color;
use slam_render::atlas::UvRect;
use slam_render::sprite::{RING_QUADS, SpriteBatch};
use slam_render::textures::TextureId;
use slam_skin::{Skin, SkinChain};
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

/// Three new combos; the second skips one colour.
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
100,100,1000,5,0
150,100,1200,1,0
200,100,1400,21,0
250,100,1600,5,0
";

const BEATMAP_COLOURS: &str = "[Colours]\nCombo1: 50,100,150\nCombo2: 40,80,120\n";

/// A temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str, skin_ini: Option<&str>) -> TempDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "slam-circles-{}-{}-{name}",
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

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    }
}

/// Combo colours and numbers of the map's circles.
fn colours(beatmap_colours: bool, user_ini: Option<&str>, setting: bool) -> Vec<(Color, u32)> {
    let osu = MAP.replace(
        "COLOURS",
        if beatmap_colours { BEATMAP_COLOURS } else { "" },
    );
    let map = Beatmap::decode(osu.as_bytes()).unwrap();
    let beatmap_dir = TempDir::new("beatmap", None);
    let user_dir = TempDir::new("user", user_ini);
    let default_dir = TempDir::new("default", None);
    let beatmap_skin = Skin::load_beatmap(&beatmap_dir.0, osu.as_bytes()).unwrap();
    let chain = SkinChain::new(
        Some(Skin::load(&user_dir.0).unwrap()),
        Skin::load(&default_dir.0).unwrap(),
    );
    let settings = CircleSettings {
        beatmap_colours: setting,
        ..CircleSettings::default()
    };
    let visuals = CircleVisuals::new(
        &map,
        CircleMods::new(&ModSet::default()),
        settings,
        Some(&beatmap_skin),
        &chain,
    );
    (0..map.hit_objects.len() as u32)
        .map(|i| {
            let c = visuals.get(i).unwrap();
            (c.colour, c.number)
        })
        .collect()
}

const USER_COLOURS: &str = "[Colours]\nCombo1: 150,100,50\nCombo2: 20,20,20\nCombo3: 75,50,25\n";

#[test]
fn beatmap_colours_use_the_index_with_offsets() {
    // Combo indices 1, 1, 2, 3; with offsets 1, 1, 3, 4.
    let (a, b) = (rgb(50, 100, 150), rgb(40, 80, 120));
    assert_eq!(
        colours(true, Some(USER_COLOURS), true),
        vec![(b, 1), (b, 2), (b, 1), (a, 1)]
    );
}

#[test]
fn skin_colours_use_the_combo_index() {
    let user = [rgb(150, 100, 50), rgb(20, 20, 20), rgb(75, 50, 25)];
    let expected = vec![(user[1], 1), (user[1], 2), (user[2], 1), (user[0], 1)];
    // Beatmap colours turned off, or a beatmap without any.
    assert_eq!(colours(true, Some(USER_COLOURS), false), expected);
    assert_eq!(colours(false, Some(USER_COLOURS), true), expected);
}

#[test]
fn a_skin_without_colours_uses_the_default_ones() {
    // `SkinConfiguration.DefaultComboColours`, not the default skin's.
    let defaults = [
        rgb(255, 192, 0),
        rgb(0, 202, 0),
        rgb(18, 124, 255),
        rgb(242, 24, 57),
    ];
    let got = colours(false, None, true);
    assert_eq!(got[0].0, defaults[1]);
    assert_eq!(got[2].0, defaults[2]);
    assert_eq!(got[3].0, defaults[3]);
}

// ---- Sprites ----

const CIRCLE: TextureId = TextureId(1);
const OVERLAY: TextureId = TextureId(2);
const APPROACH: TextureId = TextureId(3);
const DIGITS: TextureId = TextureId(4);

fn sprite(texture: TextureId, size: f32) -> SkinSprite {
    SkinSprite {
        texture,
        uv: UvRect::FULL,
        size: Vec2::new(size, size),
    }
}

fn textures() -> CircleTextures {
    CircleTextures {
        circle: Some(sprite(CIRCLE, 128.0)),
        overlay: Some(sprite(OVERLAY, 128.0)),
        approach: Some(sprite(APPROACH, 126.0)),
        approach_hole: [0.0; 2],
        digits: [Some(sprite(DIGITS, 40.0)); 10],
    }
}

fn circles(map: &Beatmap, overlay_above_number: bool) -> HitCircles {
    let default_dir = TempDir::new("default", None);
    let chain = SkinChain::new(None, Skin::load(&default_dir.0).unwrap());
    let mods = CircleMods::new(&ModSet::default());
    let windows = slam_osu::hit_windows::HitWindows::new(map.difficulty.overall_difficulty);
    HitCircles {
        visuals: CircleVisuals::new(map, mods, CircleSettings::default(), None, &chain),
        style: CircleStyle::new(mods, &windows, &chain),
        config: CircleSkinConfig {
            overlay_above_number,
            ..CircleSkinConfig::new(&chain)
        },
    }
}

fn live(index: u32, judgement: Option<CircleJudgement>) -> LiveCircle {
    LiveCircle { index, judgement }
}

/// The textures of the written batches, in order.
fn order(batch: &SpriteBatch) -> Vec<u32> {
    batch.batches().iter().map(|b| b.texture.0).collect()
}

#[test]
fn earlier_circles_draw_on_top_and_approach_circles_above_all() {
    let map = Beatmap::decode(MAP.replace("COLOURS", "").as_bytes()).unwrap();
    let layout = common::layout();
    let mut batch = SpriteBatch::new();
    let live = [live(0, None), live(1, None)];

    circles(&map, true).push(&mut batch, &layout, &live, 1000.0, &textures());
    // Circle 1, then circle 0 over it; number below the overlay; approach circles last.
    let piece = [CIRCLE.0, DIGITS.0, OVERLAY.0];
    let mut expected = [piece, piece].concat();
    expected.push(APPROACH.0);
    assert_eq!(order(&batch), expected);
    assert_eq!(batch.len(), 2 * 3 + 2);

    batch.clear();
    circles(&map, false).push(&mut batch, &layout, &live, 1000.0, &textures());
    assert_eq!(
        order(&batch)[..3],
        [CIRCLE.0, OVERLAY.0, DIGITS.0],
        "number above the overlay"
    );
}

#[test]
fn circles_are_placed_and_sized_on_the_playfield() {
    let map = Beatmap::decode(MAP.replace("COLOURS", "").as_bytes()).unwrap();
    // Two window units per osu!pixel.
    let layout = slam_play::PlayfieldLayout::new(1280.0, 960.0);
    assert_eq!(layout.scale(), 2.0);
    let mut batch = SpriteBatch::new();
    let hit = CircleJudgement {
        time: 1000.0,
        result: slam_osu::judgement::HitResult::Great,
    };
    // At the start time the approach circle has its texture's size.
    let c = circles(&map, true);
    let scale = c.visuals.get(0).unwrap().scale;
    c.push(&mut batch, &layout, &[live(0, None)], 1000.0, &textures());
    let vertices = batch.vertices();
    let approach = &vertices[vertices.len() - 4..];
    let width = approach[1].position[0] - approach[0].position[0];
    let expected = 126.0 * scale * 2.0;
    assert!(
        (width.abs() - expected).abs() < 1e-3,
        "{width} vs {expected}"
    );
    // Centred on the object.
    let centre = layout.to_window(Vec2::new(100.0, 100.0));
    let mid_x = (approach[0].position[0] + approach[1].position[0]) / 2.0;
    assert!((mid_x - centre.x).abs() < 1e-3);

    // Once hit, the approach circle is gone.
    batch.clear();
    c.push(
        &mut batch,
        &layout,
        &[live(0, Some(hit))],
        1240.0,
        &textures(),
    );
    assert!(!order(&batch).contains(&APPROACH.0));
}

#[test]
fn an_approach_circle_with_a_clear_middle_is_drawn_as_a_ring() {
    let map = Beatmap::decode(MAP.replace("COLOURS", "").as_bytes()).unwrap();
    let layout = common::layout();
    let c = circles(&map, true);
    let live = [live(0, None)];
    let mut whole = SpriteBatch::new();
    c.push(&mut whole, &layout, &live, 900.0, &textures());
    let mut ring = SpriteBatch::new();
    let holed = CircleTextures {
        approach_hole: [0.45, 0.45],
        ..textures()
    };
    c.push(&mut ring, &layout, &live, 900.0, &holed);
    // The pieces are unchanged; the approach circle takes RING_QUADS quads with the same
    // outer corners.
    assert_eq!(ring.len(), whole.len() - 1 + RING_QUADS);
    let n = whole.len() - 1;
    assert_eq!(ring.vertices()[..n * 4], whole.vertices()[..n * 4]);
    assert_eq!(order(&ring), order(&whole));
    for corner in &whole.vertices()[n * 4..] {
        assert!(ring.vertices()[n * 4..].contains(corner));
    }
}

#[test]
fn the_hole_follows_the_crop_and_the_texture_scale() {
    let sprite = SkinSprite {
        texture: APPROACH,
        uv: UvRect::FULL,
        size: Vec2::new(126.0, 128.0),
    };
    // An @2x image of 252 × 256 pixels, clear for 112 of them around the centre.
    let [hx, hy] = sprite.hole(112.0, 2.0);
    assert!((hx - 112.0 / 252.0).abs() < 1e-6 && (hy - 112.0 / 256.0).abs() < 1e-6);
    let empty = SkinSprite {
        size: Vec2::ZERO,
        ..sprite
    };
    assert_eq!(empty.hole(10.0, 1.0), [0.0, 0.0]);
}

#[test]
fn a_frame_does_not_allocate() {
    let map = Beatmap::decode(MAP.replace("COLOURS", "").as_bytes()).unwrap();
    let layout = common::layout();
    let circles = circles(&map, true);
    let frames = map.hit_objects.iter().map(|o| InputFrame {
        time: o.start_time,
        position: o.stacked_position(),
        actions: slam_osu::replay::Actions::LEFT,
    });
    let mut sim = Simulator::new(
        &map,
        ReplayInput::new(frames),
        OsuRules::new(&map, &ModSet::default()),
    );
    let mut snapshot = Box::new(LiveCircles::new());
    let mut batch = SpriteBatch::with_capacity(4096);
    // Approach circles as rings.
    let textures = CircleTextures {
        approach_hole: [0.45, 0.45],
        ..textures()
    };
    let mut drawn = 0;
    while sim.step() {
        let time = sim.time();
        assert_no_alloc(|| {
            snapshot.capture(sim.rules());
            batch.clear();
            circles.push(&mut batch, &layout, snapshot.as_slice(), time, &textures);
        });
        drawn = drawn.max(batch.len());
        assert_eq!(snapshot.dropped(), 0);
    }
    assert!(drawn > 0);
}

/// The quads of the combo number of circle `index`, as `(left, top, width)` in window units.
fn digit_quads(batch: &SpriteBatch) -> Vec<(f32, f32, f32)> {
    let digits = batch
        .batches()
        .iter()
        .find(|b| b.texture == DIGITS)
        .unwrap();
    let vertices = batch.vertices();
    (digits.first_quad..digits.first_quad + digits.quads)
        .map(|q| {
            let v = &vertices[q as usize * 4..q as usize * 4 + 4];
            let left = v.iter().map(|v| v.position[0]).fold(f32::MAX, f32::min);
            let right = v.iter().map(|v| v.position[0]).fold(f32::MIN, f32::max);
            let top = v.iter().map(|v| v.position[1]).fold(f32::MAX, f32::min);
            (left, top, right - left)
        })
        .collect()
}

#[test]
fn numbers_are_centred_with_the_overlap_between_digits() {
    // A combo of twelve circles: the last shows 12.
    let mut osu = String::from(
        "osu file format v14\n\n[Difficulty]\nCircleSize:4\nOverallDifficulty:5\nApproachRate:9\n\n\
         [TimingPoints]\n0,500,4,2,0,100,1,0\n\n[HitObjects]\n",
    );
    for i in 0..12 {
        osu.push_str(&format!(
            "256,192,{},{},0\n",
            1000 + i * 10,
            if i == 0 { 5 } else { 1 }
        ));
    }
    let map = Beatmap::decode(osu.as_bytes()).unwrap();
    let mut c = circles(&map, true);
    c.config.overlap = 6.0;
    let layout = common::layout();
    let mut batch = SpriteBatch::new();
    c.push(&mut batch, &layout, &[live(11, None)], 1000.0, &textures());
    let quads = digit_quads(&batch);
    assert_eq!(quads.len(), 2);

    // Digits 40 wide at 0.8 of the object scale, 6 closer together than side by side.
    let scale = c.visuals.get(11).unwrap().scale * 0.8;
    let (first, second) = (quads[0], quads[1]);
    assert!((first.2 - 40.0 * scale).abs() < 1e-3);
    assert!((second.0 - (first.0 + (40.0 - 6.0) * scale)).abs() < 1e-3);
    let centre = layout.to_window(c.visuals.get(11).unwrap().position);
    let middle = (first.0 + second.0 + second.2) / 2.0;
    assert!((middle - centre.x).abs() < 1e-3);
    assert!((first.1 - (centre.y - 20.0 * scale)).abs() < 1e-3);
}

#[test]
fn only_skins_after_version_1_fade_the_number_briefly() {
    let short_fade = |ini: &str| {
        let user = TempDir::new("user", Some(ini));
        let default_dir = TempDir::new("default", None);
        let chain = SkinChain::new(
            Some(Skin::load(&user.0).unwrap()),
            Skin::load(&default_dir.0).unwrap(),
        );
        let windows = slam_osu::hit_windows::HitWindows::new(5.0);
        CircleStyle::new(CircleMods::new(&ModSet::default()), &windows, &chain).number_short_fade
    };
    assert!(!short_fade("[General]\nVersion: 1.0\n"));
    assert!(short_fade("[General]\nVersion: 2.0\n"));
    assert!(short_fade("[General]\nVersion: latest\n"));
}
