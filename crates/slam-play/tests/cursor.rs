//! The legacy cursor and its trail: skin lookups and what a frame draws.

mod common;

use std::fs;

use slam_osu::Vec2;
use slam_play::PlayfieldLayout;
use slam_play::circles::SkinSprite;
use slam_play::cursor::{
    CursorFrame, CursorPath, CursorSkinConfig, CursorTextures, CursorView, ExpandTimes,
};
use slam_render::atlas::UvRect;
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Blend, Color};
use slam_skin::{Skin, SkinChain};

fn skin_dir(names: &[&str], ini: &str) -> common::skin::SampleDir {
    let dir = common::skin::SampleDir::new(&[]);
    fs::write(dir.0.join("skin.ini"), ini).unwrap();
    for name in names {
        fs::write(dir.0.join(format!("{name}.png")), b"not checked here").unwrap();
    }
    dir
}

fn config(user: (&[&str], &str), default: (&[&str], &str)) -> CursorSkinConfig {
    let (user, default) = (skin_dir(user.0, user.1), skin_dir(default.0, default.1));
    let chain = SkinChain::new(
        Some(Skin::load(&user.0).unwrap()),
        Skin::load(&default.0).unwrap(),
    );
    CursorSkinConfig::new(&chain)
}

#[test]
fn the_cursor_and_the_trail_read_the_skins_they_come_from() {
    let default = (
        &["cursor", "cursormiddle", "cursortrail"][..],
        "[General]\nCursorCentre: 0\nCursorRotate: 0\nCursorTrailRotate: 0\n",
    );
    // The user's cursor without a middle: a disjoint trail, the user's settings (defaults).
    let user = config((&["cursor"], "[General]\n"), default);
    assert_eq!(
        user,
        CursorSkinConfig {
            centre: true,
            rotate: true,
            expand: true,
            trail_rotate: false,
            trail_centre: false,
            disjoint: true,
        }
    );
    // No cursor of the user's: the default's, with its middle.
    let fallback = config((&["cursortrail"], "[General]\nCursorExpand: 0\n"), default);
    assert_eq!(
        fallback,
        CursorSkinConfig {
            centre: false,
            rotate: false,
            expand: false,
            trail_rotate: true,
            trail_centre: true,
            disjoint: false,
        }
    );
}

#[test]
fn the_middle_of_another_skin_does_not_make_the_trail_continuous() {
    let disjoint = config(
        (&["cursor"], "[General]\n"),
        (&["cursormiddle", "cursortrail"], "[General]\n"),
    );
    assert!(disjoint.disjoint);
}

const CURSOR: TextureId = TextureId(1);
const MIDDLE: TextureId = TextureId(2);
const TRAIL: TextureId = TextureId(3);

fn sprite(texture: TextureId, size: f32) -> Option<SkinSprite> {
    Some(SkinSprite {
        texture,
        uv: UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
        size: Vec2::new(size, size),
    })
}

fn textures(config: CursorSkinConfig) -> CursorTextures {
    CursorTextures {
        cursor: sprite(CURSOR, 64.0),
        middle: sprite(MIDDLE, 32.0),
        trail: sprite(TRAIL, 16.0),
        config,
    }
}

/// 1024×768: a playfield scale of 1.6, which the cursor's textures undo.
fn layout() -> PlayfieldLayout {
    PlayfieldLayout::new(1024.0, 768.0)
}

/// Bounds of each quad: (min x, min y, max x, max y).
fn quads(batch: &SpriteBatch) -> Vec<[f32; 4]> {
    batch
        .vertices()
        .chunks(4)
        .map(|q| {
            let xs = q.iter().map(|v| v.position[0]);
            let ys = q.iter().map(|v| v.position[1]);
            [
                xs.clone().fold(f32::INFINITY, f32::min),
                ys.clone().fold(f32::INFINITY, f32::min),
                xs.fold(f32::NEG_INFINITY, f32::max),
                ys.fold(f32::NEG_INFINITY, f32::max),
            ]
        })
        .collect()
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
}

fn frame(path: &CursorPath, position: Vec2, expand: ExpandTimes, time: f64) -> CursorFrame<'_> {
    CursorFrame {
        path,
        position,
        expand,
        time,
        restarts: 0,
        now_ns: (time * 1e6) as u64,
        visible: true,
    }
}

#[test]
fn the_cursor_is_hidden_until_it_moves() {
    let mut view = CursorView::default();
    let mut batch = SpriteBatch::new();
    let path = CursorPath::default();
    let textures = textures(CursorSkinConfig::default());
    view.push(
        &mut batch,
        &layout(),
        &frame(&path, Vec2::ZERO, ExpandTimes::default(), 0.0),
        &textures,
    );
    assert!(batch.is_empty());
}

#[test]
fn the_cursor_shows_its_texture_size_at_768_lines_and_expands_without_its_middle() {
    let mut path = CursorPath::default();
    let at = Vec2::new(256.0, 192.0);
    path.push(at);
    let centre = layout().to_window(at);
    let config = CursorSkinConfig {
        rotate: false,
        ..CursorSkinConfig::default()
    };
    let draw = |expand: ExpandTimes, time: f64| {
        let mut view = CursorView::default();
        let mut batch = SpriteBatch::new();
        view.push(
            &mut batch,
            &layout(),
            &frame(&path, at, expand, time),
            &textures(config),
        );
        batch
    };
    let batch = draw(ExpandTimes::default(), 0.0);
    // No trail yet: one position. The cursor, then its middle on top.
    let textures: Vec<TextureId> = batch.batches().iter().map(|b| b.texture).collect();
    assert_eq!(textures, [CURSOR, MIDDLE]);
    let q = quads(&batch);
    assert!(
        close(
            q[0],
            [
                centre.x - 32.0,
                centre.y - 32.0,
                centre.x + 32.0,
                centre.y + 32.0
            ]
        ),
        "{q:?}"
    );
    assert!(close(
        q[1],
        [
            centre.x - 16.0,
            centre.y - 16.0,
            centre.x + 16.0,
            centre.y + 16.0
        ]
    ));
    // Fully expanded: 1.3 times.
    let held = ExpandTimes {
        expanded_at: 0.0,
        contracted_at: f64::NEG_INFINITY,
    };
    let q = quads(&draw(held, 100.0));
    let half = 32.0 * 1.3;
    assert!(close(
        q[0],
        [
            centre.x - half,
            centre.y - half,
            centre.x + half,
            centre.y + half
        ]
    ));
    assert!(close(
        q[1],
        [
            centre.x - 16.0,
            centre.y - 16.0,
            centre.x + 16.0,
            centre.y + 16.0
        ]
    ));
}

#[test]
fn without_centre_the_cursor_hangs_from_its_top_left_corner() {
    let mut path = CursorPath::default();
    let at = Vec2::new(256.0, 192.0);
    path.push(at);
    let corner = layout().to_window(at);
    let config = CursorSkinConfig {
        centre: false,
        rotate: false,
        ..CursorSkinConfig::default()
    };
    let mut view = CursorView::default();
    let mut batch = SpriteBatch::new();
    view.push(
        &mut batch,
        &layout(),
        &frame(&path, at, ExpandTimes::default(), 0.0),
        &textures(config),
    );
    let q = quads(&batch);
    assert!(close(
        q[0],
        [corner.x, corner.y, corner.x + 64.0, corner.y + 64.0]
    ));
}

#[test]
fn the_trail_goes_under_the_cursor_and_starts_over_with_the_play() {
    let mut path = CursorPath::default();
    let mut view = CursorView::default();
    let config = CursorSkinConfig {
        disjoint: false,
        ..CursorSkinConfig::default()
    };
    let textures = textures(config);
    let mut batch = SpriteBatch::new();
    path.push(Vec2::new(100.5, 100.0));
    view.push(
        &mut batch,
        &layout(),
        &frame(&path, Vec2::new(100.5, 100.0), ExpandTimes::default(), 0.0),
        &textures,
    );
    // The trail texture is 10 osu!pixels wide: a part every 4.
    path.push(Vec2::new(200.5, 100.0));
    batch.clear();
    view.push(
        &mut batch,
        &layout(),
        &frame(&path, Vec2::new(200.5, 100.0), ExpandTimes::default(), 1.0),
        &textures,
    );
    let trail = &batch.batches()[0];
    assert_eq!((trail.texture, trail.blend), (TRAIL, Blend::Additive));
    let parts = batch.len() - 2;
    // 4 to 92: the last interval before the cursor stays empty.
    assert_eq!(parts, 23);
    assert_eq!(batch.batches().last().unwrap().texture, MIDDLE);
    // Added a millisecond ago by the previous frame's fade clock.
    let colour = batch.vertices()[0].colour;
    assert_eq!(
        colour[..3],
        [Color::WHITE.r, Color::WHITE.g, Color::WHITE.b]
    );
    assert!((colour[3] - (1.0 - 1.0 / 500.0)).abs() < 1e-5, "{colour:?}");

    // A restart: a new trail, the path read on from where it was.
    path.push(Vec2::new(300.5, 100.0));
    batch.clear();
    let restarted = CursorFrame {
        restarts: 1,
        ..frame(&path, Vec2::new(300.5, 100.0), ExpandTimes::default(), 2.0)
    };
    view.push(&mut batch, &layout(), &restarted, &textures);
    assert_eq!(batch.len(), 2);
}

#[test]
fn a_disjoint_trail_hangs_its_parts_from_the_corner_without_centre() {
    let mut path = CursorPath::default();
    let mut view = CursorView::default();
    let config = CursorSkinConfig {
        trail_centre: false,
        trail_rotate: false,
        ..CursorSkinConfig::default()
    };
    let textures = textures(config);
    let at = Vec2::new(100.0, 100.0);
    path.push(at);
    let mut batch = SpriteBatch::new();
    // The first part a sixtieth of a second into the trail's clock.
    view.push(
        &mut batch,
        &layout(),
        &frame(&path, at, ExpandTimes::default(), 0.0),
        &textures,
    );
    batch.clear();
    view.push(
        &mut batch,
        &layout(),
        &frame(&path, at, ExpandTimes::default(), 17.0),
        &textures,
    );
    let trail = &batch.batches()[0];
    assert_eq!((trail.texture, trail.blend), (TRAIL, Blend::Alpha));
    let corner = layout().to_window(at);
    // 16 pixels: 10 osu!pixels, 16 window pixels at 768 lines.
    let q = quads(&batch);
    assert!(
        close(q[0], [corner.x, corner.y, corner.x + 16.0, corner.y + 16.0]),
        "{q:?}"
    );
}

#[test]
fn trail_parts_take_the_scale_the_cursor_had_a_frame_before() {
    let mut path = CursorPath::default();
    let mut view = CursorView::default();
    let textures = textures(CursorSkinConfig::default());
    let at = Vec2::new(100.0, 100.0);
    path.push(at);
    let held = ExpandTimes {
        expanded_at: 0.0,
        contracted_at: f64::NEG_INFINITY,
    };
    let mut batch = SpriteBatch::new();
    view.push(
        &mut batch,
        &layout(),
        &frame(&path, at, held, 0.0),
        &textures,
    );
    batch.clear();
    // At 17 ms the cursor is past 1; the part still has the scale at 0 ms.
    view.push(
        &mut batch,
        &layout(),
        &frame(&path, at, held, 17.0),
        &textures,
    );
    let q = quads(&batch);
    assert!(
        close(q[0], {
            let c = layout().to_window(at);
            [c.x - 8.0, c.y - 8.0, c.x + 8.0, c.y + 8.0]
        }),
        "{q:?}"
    );
}

#[test]
fn a_jump_leaves_no_trail_on_the_way_and_a_hidden_cursor_draws_nothing() {
    let mut path = CursorPath::default();
    let mut view = CursorView::default();
    let config = CursorSkinConfig {
        disjoint: false,
        ..CursorSkinConfig::default()
    };
    let textures = textures(config);
    let mut batch = SpriteBatch::new();
    path.push(Vec2::new(100.5, 100.0));
    view.push(
        &mut batch,
        &layout(),
        &frame(&path, Vec2::new(100.5, 100.0), ExpandTimes::default(), 0.0),
        &textures,
    );
    // Hidden: the trail still takes the jump, nothing is drawn.
    path.push_jump(Vec2::new(400.5, 100.0));
    batch.clear();
    let hidden = CursorFrame {
        visible: false,
        ..frame(&path, Vec2::new(400.5, 100.0), ExpandTimes::default(), 1.0)
    };
    view.push(&mut batch, &layout(), &hidden, &textures);
    assert_eq!(batch.len(), 0);
    // Shown again: the cursor and its middle, no parts between 100 and 400.
    batch.clear();
    view.push(
        &mut batch,
        &layout(),
        &frame(&path, Vec2::new(400.5, 100.0), ExpandTimes::default(), 2.0),
        &textures,
    );
    assert_eq!(batch.len(), 2);
    // A move after the jump leaves parts from where the cursor jumped to.
    path.push(Vec2::new(500.5, 100.0));
    batch.clear();
    view.push(
        &mut batch,
        &layout(),
        &frame(&path, Vec2::new(500.5, 100.0), ExpandTimes::default(), 3.0),
        &textures,
    );
    assert_eq!(batch.len() - 2, 23);
    let scale = layout().scale();
    let leftmost = quads(&batch)[0][0];
    assert!(leftmost > layout().to_window(Vec2::new(400.0, 100.0)).x - 10.0 * scale);
}
