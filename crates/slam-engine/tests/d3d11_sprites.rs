//! Textures, the atlas and the sprite batcher through the Direct3D 11 backend, checked
//! by reading pixels back. Runs on the WARP software adapter, so it needs neither a
//! display nor a GPU.
#![cfg(windows)]

use slam_render::atlas::{Atlas, AtlasRegion};
use slam_render::sprite::{
    DEFAULT_SLOTS, Projection, Sprite, SpriteBatch, SpriteError, SpriteRenderer, SpriteStats,
};
use slam_render::textures::Textures;
use slam_render::{
    Blend, Color, DescError, Device, Extent, Origin, RenderTargetDesc, TextureDesc, TextureFormat,
    TextureUsage,
};
use slam_testkit::count_allocs;

#[path = "common/d3d11.rs"]
mod common;

use common::{TestDevice, assert_near, assert_no_debug_messages, surface_pixel, target_pixel};

slam_testkit::install_counting_allocator!();

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const WHITE: [u8; 4] = [255; 4];
const BLACK: [u8; 4] = [0, 0, 0, 255];

fn image(width: u32, height: u32, rgba: [u8; 4]) -> (Extent, Vec<u8>) {
    let pixels = rgba.repeat((width * height) as usize);
    (Extent::new(width, height), pixels)
}

// Coordinates: probes are from the top left (ADR-0019). The GL test read render
// targets with the same numbers and the surface bottom-up (`height - 1 - y`); both
// are read top first here.
#[test]
fn draws_sprites_from_an_atlas() {
    const SURFACE: u32 = 64;
    let mut d = common::device(SURFACE, SURFACE);
    let mut textures = Textures::new();

    // Partial texture writes need COPY_DST and stay inside the texture.
    let plain = d
        .create_texture(
            &TextureDesc {
                size: Extent::new(4, 4),
                format: TextureFormat::Rgba8,
                usage: TextureUsage::SAMPLED,
            },
            None,
        )
        .expect("texture");
    assert_eq!(
        d.write_texture(&plain, Origin::new(0, 0), Extent::new(1, 1), &RED),
        Err(DescError::NotCopyDst)
    );
    d.destroy_texture(plain);

    // A small page so that a large image needs a page of its own.
    let mut atlas = Atlas::new(Extent::new(64, 64), TextureFormat::Rgba8);
    let mut add = |d: &mut TestDevice, (size, pixels): (Extent, Vec<u8>)| {
        atlas
            .add(d, &mut textures, size, &pixels)
            .expect("atlas add")
    };
    let red = add(&mut d, image(8, 8, RED));
    let green = add(&mut d, image(8, 8, GREEN));
    let white = add(&mut d, image(1, 1, WHITE));
    let gradient = add(&mut d, (Extent::new(2, 1), [BLUE, YELLOW].concat()));
    let big = add(&mut d, image(100, 10, BLUE));
    assert_eq!(red.texture, white.texture);
    assert_eq!(red.texture, gradient.texture);
    assert_ne!(red.texture, big.texture);
    assert_eq!(atlas.pages().count(), 2);

    let target = d
        .create_render_target(&RenderTargetDesc {
            size: Extent::new(64, 64),
            format: TextureFormat::Rgba8,
        })
        .expect("render target");
    let mut renderer = SpriteRenderer::new(&mut d, DEFAULT_SLOTS).expect("sprite renderer");
    let mut batch = SpriteBatch::with_capacity(16);

    let record = |batch: &mut SpriteBatch| {
        batch.clear();
        let at = |region: &AtlasRegion, x, y, w, h| {
            Sprite::new(region.texture, region.uv, [x, y], [w, h])
        };
        batch.push(&at(&red, 0.0, 0.0, 16.0, 16.0));
        batch.push(&Sprite {
            blend: Blend::Additive,
            ..at(&green, 16.0, 0.0, 16.0, 16.0)
        });
        // Solid colour from the white pixel, in the same batch as the next two.
        batch.push(&Sprite {
            colour: Color::rgb(0.0, 0.0, 1.0),
            ..at(&white, 32.0, 0.0, 16.0, 16.0)
        });
        batch.push(&Sprite {
            colour: Color {
                a: 0.5,
                ..Color::WHITE
            },
            ..at(&red, 48.0, 0.0, 16.0, 16.0)
        });
        // 16x8, a quarter turn around its centre: covers x 4..12, y 32..48.
        batch.push(&Sprite {
            origin: [8.0, 4.0],
            rotation: std::f32::consts::FRAC_PI_2,
            ..at(&red, 8.0, 40.0, 16.0, 8.0)
        });
        batch.push(&at(&big, 0.0, 56.0, 64.0, 8.0));
        batch.push(&at(&gradient, 32.0, 32.0, 16.0, 8.0));
    };
    record(&mut batch);
    let expected = SpriteStats {
        sprites: 7,
        batches: 5,
        draw_calls: 5,
    };

    renderer.begin_frame();
    let size = d.begin_pass(Some(&target), Some(Color::BLACK));
    let stats = renderer
        .draw(&mut d, &batch, &Projection::pixels(size), |id| {
            textures.get(id)
        })
        .expect("draw");
    assert_eq!(stats, expected);
    // Render target rows read back top first (ADR-0019).
    assert_near(target_pixel(&mut d, &target, 8, 8), RED);
    assert_near(target_pixel(&mut d, &target, 24, 8), GREEN);
    assert_near(target_pixel(&mut d, &target, 40, 8), BLUE);
    assert_near(target_pixel(&mut d, &target, 56, 8), [128, 0, 0, 255]);
    assert_near(target_pixel(&mut d, &target, 8, 34), RED);
    assert_near(target_pixel(&mut d, &target, 8, 46), RED);
    assert_near(target_pixel(&mut d, &target, 2, 40), BLACK);
    assert_near(target_pixel(&mut d, &target, 13, 40), BLACK);
    assert_near(target_pixel(&mut d, &target, 30, 60), BLUE);
    // The border keeps the image's edges free of its neighbours.
    assert_near(target_pixel(&mut d, &target, 32, 36), BLUE);
    assert_near(target_pixel(&mut d, &target, 47, 36), YELLOW);
    assert_near(target_pixel(&mut d, &target, 40, 24), BLACK);

    // The surface variant: the top-left sprite is at the top left of the surface.
    renderer.begin_frame();
    let size = d.begin_frame(Color::BLACK);
    assert_eq!(size, Extent::new(SURFACE, SURFACE));
    renderer
        .draw(&mut d, &batch, &Projection::pixels(size), |id| {
            textures.get(id)
        })
        .expect("draw");
    // GL probed (1, top - 1) for red and (1, 1) for blue, bottom-up: rows 1 and
    // height - 2 from the top (the big blue sprite fills rows 56..64).
    assert_near(surface_pixel(&mut d, SURFACE, 1, 1), RED);
    assert_near(surface_pixel(&mut d, SURFACE, 1, SURFACE - 2), BLUE);
    assert_no_debug_messages(&mut d);
    d.present().expect("present");

    // Steady frames: no allocations.
    let (result, allocs) = count_allocs(|| {
        let mut total = SpriteStats::default();
        for _ in 0..100 {
            record(&mut batch);
            renderer.begin_frame();
            let size = d.begin_pass(Some(&target), Some(Color::BLACK));
            total += renderer.draw(&mut d, &batch, &Projection::pixels(size), |id| {
                textures.get(id)
            })?;
        }
        Ok::<_, SpriteError>(total)
    });
    let total = result.expect("frame loop");
    assert_eq!(allocs.total(), 0, "frame loop allocated: {allocs}");
    assert_eq!(total.draw_calls, expected.draw_calls * 100);

    renderer.destroy(&mut d);
    for texture in textures.drain() {
        d.destroy_texture(texture);
    }
    d.destroy_render_target(target);
    assert_no_debug_messages(&mut d);
}
