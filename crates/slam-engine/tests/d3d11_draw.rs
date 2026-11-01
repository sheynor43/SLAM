//! Drawing through the Direct3D 11 backend: buffers, pipelines, blending, render
//! targets, textures and draw-time errors, checked by reading pixels back. Runs on the
//! WARP software adapter, so it needs neither a display nor a GPU.
#![cfg(windows)]

use slam_render::{
    Blend, BufferDesc, BufferKind, BufferUpdate, Color, DescError, Device, DeviceError, DrawError,
    Extent, HlslShader, IndexFormat, PipelineDesc, RenderTargetDesc, Shader, TextureDesc,
    TextureFormat, TextureUsage, VertexAttribute, VertexFormat, VertexLayout,
};
use slam_testkit::count_allocs;

#[path = "common/d3d11.rs"]
mod common;

use common::{assert_near, assert_no_debug_messages, surface_pixel, target_pixel};

slam_testkit::install_counting_allocator!();

// Generated from `shaders/*.wgsl` by the build script; holds the example shaders too.
#[allow(dead_code)]
mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shaders.rs"));
}

const COLOR_ATTRIBUTES: [VertexAttribute; 2] = [
    VertexAttribute {
        location: 0,
        format: VertexFormat::Float32x2,
        offset: 0,
    },
    VertexAttribute {
        location: 1,
        format: VertexFormat::Unorm8x4,
        offset: 8,
    },
];

const TEXTURED_ATTRIBUTES: [VertexAttribute; 2] = [
    VertexAttribute {
        location: 0,
        format: VertexFormat::Float32x2,
        offset: 0,
    },
    VertexAttribute {
        location: 1,
        format: VertexFormat::Float32x2,
        offset: 8,
    },
];

/// Vertices of `(x, y, rgba)`, 12 bytes each.
fn color_vertices(vertices: &[(f32, f32, [u8; 4])]) -> Vec<u8> {
    let mut out = Vec::new();
    for &(x, y, rgba) in vertices {
        out.extend_from_slice(&x.to_ne_bytes());
        out.extend_from_slice(&y.to_ne_bytes());
        out.extend_from_slice(&rgba);
    }
    out
}

fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

/// A triangle covering the whole target.
const FULL: [(f32, f32); 3] = [(-1.0, -1.0), (3.0, -1.0), (-1.0, 3.0)];

fn buffer(kind: BufferKind, size: u64) -> BufferDesc {
    BufferDesc {
        kind,
        size,
        update: BufferUpdate::Static,
    }
}

// Coordinates: probes are from the top left, as the HAL defines the origin of both
// the surface and render targets (ADR-0019). The GL test read render targets with
// the same numbers (its target vertex stage flips y) and the surface bottom-up
// (`height - 1 - y`); here both are read top first.
#[test]
fn draws_through_buffers_pipelines_and_targets() {
    const SURFACE: u32 = 64;
    let mut d = common::device(SURFACE, SURFACE);
    let limits = d.limits();
    assert!(limits.max_uniform_block_size >= 16384);
    assert_eq!(limits.max_uniform_buffer_slots, 12);
    assert!(limits.max_texture_slots >= 15);

    // Nothing set yet.
    assert_eq!(d.draw(0..3), Err(DrawError::NoPipeline));
    assert_eq!(d.draw_indexed(0..3), Err(DrawError::NoIndexBuffer));

    let color_desc = |blend| PipelineDesc {
        shader: &shaders::TEST_COLOR,
        layout: VertexLayout {
            stride: 12,
            attributes: &COLOR_ATTRIBUTES,
        },
        blend,
    };
    let opaque = d
        .create_pipeline(&color_desc(Blend::Replace))
        .expect("pipeline");
    let alpha = d
        .create_pipeline(&color_desc(Blend::Alpha))
        .expect("pipeline");
    let premultiplied = d
        .create_pipeline(&color_desc(Blend::Premultiplied))
        .expect("pipeline");
    let additive = d
        .create_pipeline(&color_desc(Blend::Additive))
        .expect("pipeline");
    let textured = d
        .create_pipeline(&PipelineDesc {
            shader: &shaders::TEST_TEXTURED,
            layout: VertexLayout {
                stride: 16,
                attributes: &TEXTURED_ATTRIBUTES,
            },
            blend: Blend::Replace,
        })
        .expect("textured pipeline");

    // Compiler output is reported with the shader name and target, for each stage.
    // (Unlike GLSL there is one HLSL source: a missing entry point fails one
    // stage's compile, a broken source fails the first.)
    let hlsl = shaders::TEST_COLOR.hlsl;
    for (broken, stage) in [
        (
            HlslShader {
                fragment_entry: "missing_entry",
                ..hlsl
            },
            "ps_5_0",
        ),
        (
            HlslShader {
                vertex_entry: "missing_entry",
                ..hlsl
            },
            "vs_5_0",
        ),
        (
            HlslShader {
                source: "void main() { undefined_call(); }",
                vertex_entry: "main",
                fragment_entry: "main",
            },
            "vs_5_0",
        ),
    ] {
        let shader = Shader {
            name: "broken",
            glsl: shaders::TEST_COLOR.glsl,
            hlsl: broken,
        };
        let result = d.create_pipeline(&PipelineDesc {
            shader: &shader,
            ..color_desc(Blend::Replace)
        });
        match result {
            Err(DeviceError::Shader(log)) => {
                assert!(log.starts_with(&format!("broken ({stage}): ")), "{log}")
            }
            other => panic!("expected a shader error, got {:?}", other.err()),
        }
    }

    let tint = d
        .create_buffer(
            &buffer(BufferKind::Uniform, 16),
            Some(&floats(&[1.0, 1.0, 1.0, 1.0])),
        )
        .expect("uniform buffer");
    let red = color_vertices(&FULL.map(|(x, y)| (x, y, [255, 0, 0, 255])));
    let half_red = color_vertices(&FULL.map(|(x, y)| (x, y, [255, 0, 0, 128])));
    // Two triangles side by side in one buffer: the second starts at byte 36.
    let mut vertex_data = red.clone();
    vertex_data.extend_from_slice(&half_red);
    let vertices = d
        .create_buffer(&buffer(BufferKind::Vertex, 72), Some(&vertex_data))
        .expect("vertex buffer");

    let target = d
        .create_render_target(&RenderTargetDesc {
            size: Extent::new(16, 16),
            format: TextureFormat::Rgba8,
        })
        .expect("render target");
    assert_eq!(
        d.render_target_texture(&target).desc().usage,
        TextureUsage::SAMPLED | TextureUsage::RENDER_TARGET
    );

    // Opaque draw, tinted through the uniform buffer.
    assert_eq!(
        d.begin_pass(Some(&target), Some(Color::BLACK)),
        Extent::new(16, 16)
    );
    assert_near(target_pixel(&mut d, &target, 8, 8), [0, 0, 0, 255]);
    d.set_pipeline(&opaque);
    d.set_vertex_buffer(&vertices, 0).expect("vertex buffer");
    d.set_uniform_buffer(2, &tint).expect("uniform buffer");
    d.draw(0..3).expect("draw");
    assert_near(target_pixel(&mut d, &target, 8, 8), [255, 0, 0, 255]);
    d.write_buffer(&tint, 0, &floats(&[0.0, 1.0, 1.0, 1.0]))
        .expect("write");
    d.draw(0..3).expect("draw");
    assert_near(target_pixel(&mut d, &target, 8, 8), [0, 0, 0, 255]);

    // Straight alpha over blue, from the second triangle (vertex buffer offset).
    d.write_buffer(&tint, 0, &floats(&[1.0, 1.0, 1.0, 1.0]))
        .expect("write");
    let quarter_blue = Color {
        a: 0.25,
        ..Color::rgb(0.0, 0.0, 1.0)
    };
    d.begin_pass(Some(&target), Some(quarter_blue));
    d.set_pipeline(&alpha);
    d.set_vertex_buffer(&vertices, 36).expect("vertex buffer");
    d.draw(0..3).expect("draw");
    // rgb: red * 128/255 + blue * 127/255; alpha (osu-framework's Mixture):
    // 128/255 + 0.25.
    assert_near(target_pixel(&mut d, &target, 8, 8), [128, 0, 127, 192]);
    // The same vertices through a 3-vertex window starting at vertex 3.
    d.begin_pass(Some(&target), Some(Color::rgb(0.0, 0.0, 1.0)));
    d.set_vertex_buffer(&vertices, 0).expect("vertex buffer");
    d.draw(3..6).expect("draw");
    assert_near(target_pixel(&mut d, &target, 8, 8), [128, 0, 127, 255]);

    // Premultiplied: the tint halves red, so src = (0.5, 0, 0, 0.5) over blue.
    d.write_buffer(&tint, 0, &floats(&[0.5, 1.0, 1.0, 1.0]))
        .expect("write");
    d.begin_pass(Some(&target), Some(quarter_blue));
    d.set_pipeline(&premultiplied);
    d.draw(3..6).expect("draw");
    // rgb: (0.5, 0, 0) + blue * 127/255; alpha: 128/255 + 0.25 * 127/255.
    assert_near(target_pixel(&mut d, &target, 8, 8), [128, 0, 127, 159]);
    // Additive: red * 128/255 + blue; alpha 128/255 + 0.25.
    d.write_buffer(&tint, 0, &floats(&[1.0, 1.0, 1.0, 1.0]))
        .expect("write");
    d.begin_pass(Some(&target), Some(quarter_blue));
    d.set_pipeline(&additive);
    d.draw(3..6).expect("draw");
    assert_near(target_pixel(&mut d, &target, 8, 8), [128, 0, 255, 192]);

    // Creating resources mid-pass leaves the bound target and pipeline alone.
    d.begin_pass(Some(&target), Some(Color::BLACK));
    d.set_pipeline(&opaque);
    let scratch_target = d
        .create_render_target(&RenderTargetDesc {
            size: Extent::new(4, 4),
            format: TextureFormat::Rgba8,
        })
        .expect("render target");
    // Writes green: drawing with it instead of `opaque` would show.
    let scratch_pipeline = d
        .create_pipeline(&PipelineDesc {
            shader: &shaders::TEST_GREEN,
            ..color_desc(Blend::Replace)
        })
        .expect("pipeline");
    d.draw(0..3).expect("draw");
    assert_near(target_pixel(&mut d, &target, 8, 8), [255, 0, 0, 255]);
    d.destroy_pipeline(scratch_pipeline);
    // Destroying the bound target leaves the cache consistent: the next pass binds
    // `target` for real (its pixel reads back).
    d.begin_pass(Some(&scratch_target), Some(Color::BLACK));
    d.destroy_render_target(scratch_target);
    d.begin_pass(Some(&target), None);
    assert_near(target_pixel(&mut d, &target, 8, 8), [255, 0, 0, 255]);

    // Indexed, textured quad on the left half of the target.
    let green = d
        .create_texture(
            &TextureDesc {
                size: Extent::new(1, 1),
                format: TextureFormat::Rgba8,
                usage: TextureUsage::SAMPLED,
            },
            Some(&[0, 255, 0, 255]),
        )
        .expect("texture");
    let quad = d
        .create_buffer(
            &buffer(BufferKind::Vertex, 64),
            Some(&floats(&[
                -1.0, -1.0, 0.0, 0.0, //
                0.0, -1.0, 1.0, 0.0, //
                0.0, 1.0, 1.0, 1.0, //
                -1.0, 1.0, 0.0, 1.0,
            ])),
        )
        .expect("quad");
    let index_data: Vec<u8> = [0u16, 1, 2, 0, 2, 3]
        .iter()
        .flat_map(|i| i.to_ne_bytes())
        .collect();
    let indices = d
        .create_buffer(&buffer(BufferKind::Index, 12), Some(&index_data))
        .expect("index buffer");
    d.begin_pass(Some(&target), Some(Color::BLACK));
    d.set_pipeline(&textured);
    d.set_vertex_buffer(&quad, 0).expect("vertex buffer");
    d.set_index_buffer(&indices, IndexFormat::U16)
        .expect("index buffer");
    d.set_texture(3, &green).expect("texture");
    // Creating a texture does not disturb the one bound to slot 3.
    let other = d
        .create_texture(
            &TextureDesc {
                size: Extent::new(1, 1),
                format: TextureFormat::Rgba8,
                usage: TextureUsage::SAMPLED,
            },
            Some(&[255, 255, 255, 255]),
        )
        .expect("texture");
    d.draw_indexed(0..6).expect("draw indexed");
    assert_near(target_pixel(&mut d, &target, 4, 8), [0, 255, 0, 255]);
    assert_near(target_pixel(&mut d, &target, 12, 8), [0, 0, 0, 255]);
    assert_eq!(
        d.draw_indexed(0..7),
        Err(DrawError::OutOfRange { start: 0, end: 7 })
    );

    // The render target sampled into the surface.
    d.set_texture(3, d.render_target_texture(&target))
        .expect("target texture");
    let size = d.begin_frame(Color::rgb(0.0, 0.0, 1.0));
    assert_eq!(size, Extent::new(SURFACE, SURFACE));
    d.draw_indexed(0..6).expect("draw indexed");
    // The left of the surface samples the left half of the target: green.
    // (The quad covers the left half at every height, so any row would do.)
    assert_near(surface_pixel(&mut d, SURFACE, 0, 0), [0, 255, 0, 255]);
    assert_no_debug_messages(&mut d);
    d.present().expect("present");

    // Coordinates (ADR-0019): v = 0 is the first uploaded row and the top of the
    // picture, on the surface and in render targets alike. A texture with a red
    // top row and a blue bottom row, drawn over the whole target (y up, v down).
    let stripes = d
        .create_texture(
            &TextureDesc {
                size: Extent::new(1, 2),
                format: TextureFormat::Rgba8,
                usage: TextureUsage::SAMPLED,
            },
            Some(&[255, 0, 0, 255, 0, 0, 255, 255]),
        )
        .expect("texture");
    let screen = d
        .create_buffer(
            &buffer(BufferKind::Vertex, 64),
            Some(&floats(&[
                -1.0, 1.0, 0.0, 0.0, //
                1.0, 1.0, 1.0, 0.0, //
                1.0, -1.0, 1.0, 1.0, //
                -1.0, -1.0, 0.0, 1.0,
            ])),
        )
        .expect("screen quad");
    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    // The pipeline stays set across the switch between surface and target.
    d.set_vertex_buffer(&screen, 0).expect("vertex buffer");
    d.set_texture(3, &stripes).expect("texture");
    d.begin_pass(Some(&target), Some(Color::BLACK));
    d.draw_indexed(0..6).expect("draw indexed");
    // The target's first row (read back at y = 0) is the top of the picture.
    assert_near(target_pixel(&mut d, &target, 8, 0), RED);
    assert_near(target_pixel(&mut d, &target, 8, 15), BLUE);
    // GL read the back buffer bottom-up (red at `height - 1`, blue at 0): that is
    // the top row (0) and the bottom row here.
    let bottom = d.begin_frame(Color::BLACK).height - 1;
    d.draw_indexed(0..6).expect("draw indexed");
    assert_near(surface_pixel(&mut d, SURFACE, 0, 0), RED);
    assert_near(surface_pixel(&mut d, SURFACE, 0, bottom), BLUE);
    // The target drawn onto the surface keeps its orientation.
    d.set_texture(3, d.render_target_texture(&target))
        .expect("target texture");
    d.begin_frame(Color::BLACK);
    d.draw_indexed(0..6).expect("draw indexed");
    assert_near(surface_pixel(&mut d, SURFACE, 0, 0), RED);
    assert_near(surface_pixel(&mut d, SURFACE, 0, bottom), BLUE);
    assert_no_debug_messages(&mut d);
    d.present().expect("present");
    d.destroy_buffer(screen);
    d.destroy_texture(stripes);

    // Draw-time mistakes.
    assert_eq!(
        d.set_vertex_buffer(&tint, 0),
        Err(DrawError::BufferKind {
            expected: BufferKind::Vertex,
            actual: BufferKind::Uniform
        })
    );
    assert_eq!(
        d.set_index_buffer(&quad, IndexFormat::U16),
        Err(DrawError::BufferKind {
            expected: BufferKind::Index,
            actual: BufferKind::Vertex
        })
    );
    assert_eq!(
        d.set_uniform_buffer(0, &quad),
        Err(DrawError::BufferKind {
            expected: BufferKind::Uniform,
            actual: BufferKind::Vertex
        })
    );
    assert_eq!(
        d.set_vertex_buffer(&quad, 2),
        Err(DrawError::Unaligned { offset: 2 })
    );
    assert_eq!(
        d.set_vertex_buffer(&quad, 68),
        Err(DrawError::OffsetPastEnd {
            offset: 68,
            size: 64
        })
    );
    assert_eq!(
        d.set_vertex_buffer(&quad, u64::MAX - 3),
        Err(DrawError::OffsetPastEnd {
            offset: u64::MAX - 3,
            size: 64
        })
    );
    // An offset at the very end is accepted but leaves no room for a vertex.
    d.set_vertex_buffer(&quad, 64).expect("vertex buffer");
    assert_eq!(
        d.draw(0..1),
        Err(DrawError::OutOfRange { start: 0, end: 1 })
    );
    d.set_vertex_buffer(&quad, 0).expect("vertex buffer");
    let max = limits.max_texture_slots;
    assert_eq!(
        d.set_texture(max, &green),
        Err(DrawError::Slot { slot: max, max })
    );
    assert_eq!(
        d.write_buffer(&quad, 60, &[0; 8]),
        Err(DescError::WriteOutOfBounds {
            offset: 60,
            len: 8,
            size: 64
        })
    );
    assert_eq!(
        d.draw(0..5),
        Err(DrawError::OutOfRange { start: 0, end: 5 })
    );
    assert_eq!(
        d.draw(std::ops::Range { start: 3, end: 2 }),
        Err(DrawError::OutOfRange { start: 3, end: 2 })
    );
    d.destroy_buffer(quad);
    assert_eq!(d.draw(0..3), Err(DrawError::NoVertexBuffer));
    d.destroy_buffer(indices);
    assert_eq!(d.draw_indexed(0..3), Err(DrawError::NoIndexBuffer));
    d.destroy_pipeline(textured);
    d.set_vertex_buffer(&vertices, 0).expect("vertex buffer");
    assert_eq!(d.draw(0..3), Err(DrawError::NoPipeline));

    assert_no_debug_messages(&mut d);

    // A frame of draws into a target and onto the surface does not allocate.
    let frame = |d: &mut common::TestDevice, i: u8, check: bool| {
        let mut tint_data = [0u8; 16];
        tint_data[0] = i;
        d.begin_pass(Some(&target), Some(Color::BLACK));
        d.set_pipeline(&opaque);
        d.set_vertex_buffer(&vertices, 0)?;
        d.set_uniform_buffer(2, &tint)?;
        d.write_buffer(&tint, 0, &tint_data).expect("write");
        d.draw(0..3)?;
        d.begin_frame(Color::BLACK);
        d.set_pipeline(&alpha);
        d.draw(3..6)?;
        if check {
            assert_no_debug_messages(d);
        }
        d.present().expect("present");
        Ok::<_, DrawError>(())
    };
    // One warm-up frame outside the count, checked for debug-layer hazards before
    // `present` drains them.
    frame(&mut d, 0, true).expect("warm-up frame");
    let (result, stats) = count_allocs(|| {
        for i in 0..100u8 {
            frame(&mut d, i, false)?;
        }
        Ok::<_, DrawError>(())
    });
    result.expect("frame loop");
    assert_eq!(stats.total(), 0, "frame loop allocated: {stats}");

    d.destroy_texture(green);
    d.destroy_texture(other);
    d.destroy_render_target(target);
    d.destroy_buffer(vertices);
    d.destroy_buffer(tint);
    d.destroy_pipeline(opaque);
    d.destroy_pipeline(alpha);
    d.destroy_pipeline(premultiplied);
    d.destroy_pipeline(additive);
    assert_no_debug_messages(&mut d);
}

fn color_pipeline(d: &mut common::TestDevice) -> <common::TestDevice as Device>::Pipeline {
    d.create_pipeline(&PipelineDesc {
        shader: &shaders::TEST_COLOR,
        layout: VertexLayout {
            stride: 12,
            attributes: &COLOR_ATTRIBUTES,
        },
        blend: Blend::Replace,
    })
    .expect("pipeline")
}

#[test]
fn partial_uniform_write_keeps_the_rest_of_the_buffer() {
    let mut d = common::device(16, 16);
    let pipeline = color_pipeline(&mut d);
    let white = color_vertices(&FULL.map(|(x, y)| (x, y, [255; 4])));
    let vertices = d
        .create_buffer(&buffer(BufferKind::Vertex, 36), Some(&white))
        .expect("vertex buffer");
    let uniform = d
        .create_buffer(&buffer(BufferKind::Uniform, 32), None)
        .expect("uniform buffer");
    d.write_buffer(&uniform, 0, &floats(&[1.0, 0.0, 0.0, 1.0]))
        .expect("write tint");
    // Only the green component, bytes 4..8: the shadow copy keeps the others.
    d.write_buffer(&uniform, 4, &1.0f32.to_ne_bytes())
        .expect("write green");
    d.begin_frame(Color::BLACK);
    d.set_pipeline(&pipeline);
    d.set_vertex_buffer(&vertices, 0).expect("vertex buffer");
    d.set_uniform_buffer(2, &uniform).expect("uniform buffer");
    d.draw(0..3).expect("draw");
    assert_near(surface_pixel(&mut d, 16, 8, 8), [255, 255, 0, 255]);
    assert_no_debug_messages(&mut d);
}

#[test]
fn texture_state_survives_target_switches() {
    let mut d = common::device(SURFACE_SIZE, SURFACE_SIZE);
    let green_pipeline = d
        .create_pipeline(&PipelineDesc {
            shader: &shaders::TEST_GREEN,
            layout: VertexLayout {
                stride: 12,
                attributes: &COLOR_ATTRIBUTES,
            },
            blend: Blend::Replace,
        })
        .expect("green pipeline");
    let textured = d
        .create_pipeline(&PipelineDesc {
            shader: &shaders::TEST_TEXTURED,
            layout: VertexLayout {
                stride: 16,
                attributes: &TEXTURED_ATTRIBUTES,
            },
            blend: Blend::Replace,
        })
        .expect("textured pipeline");
    let make_target = |d: &mut common::TestDevice| {
        d.create_render_target(&RenderTargetDesc {
            size: Extent::new(16, 16),
            format: TextureFormat::Rgba8,
        })
        .expect("render target")
    };
    let a = make_target(&mut d);
    let b = make_target(&mut d);
    let vertices = d
        .create_buffer(
            &buffer(BufferKind::Vertex, 36),
            Some(&color_vertices(&FULL.map(|(x, y)| (x, y, [255; 4])))),
        )
        .expect("vertex buffer");
    // A full-target quad with UVs, as two triangles.
    let quad = d
        .create_buffer(
            &buffer(BufferKind::Vertex, 96),
            Some(&floats(&[
                -1.0, 1.0, 0.0, 0.0, //
                1.0, 1.0, 1.0, 0.0, //
                1.0, -1.0, 1.0, 1.0, //
                -1.0, 1.0, 0.0, 0.0, //
                1.0, -1.0, 1.0, 1.0, //
                -1.0, -1.0, 0.0, 1.0,
            ])),
        )
        .expect("quad");

    // Distinct colours by clearing.
    d.begin_pass(Some(&a), Some(Color::rgb(1.0, 0.0, 0.0)));
    d.begin_pass(Some(&b), Some(Color::rgb(0.0, 0.0, 1.0)));

    d.begin_pass(None, None);
    d.set_texture(3, d.render_target_texture(&a)).expect("a");
    d.begin_pass(Some(&b), None);
    d.set_texture(3, d.render_target_texture(&b)).expect("b");
    // Slot 3 holds B's texture while B is the target: D3D11 unbinds it.
    d.begin_pass(Some(&a), None);
    d.set_pipeline(&green_pipeline);
    d.set_vertex_buffer(&vertices, 0).expect("vertex buffer");
    d.draw(0..3).expect("draw");
    assert_no_debug_messages(&mut d);

    // Leaving B's role as the target again binds its texture to slot 3 without a
    // new `set_texture`.
    d.begin_frame(Color::BLACK);
    d.set_pipeline(&textured);
    d.set_vertex_buffer(&quad, 0).expect("quad");
    d.draw(0..6).expect("draw");
    assert_no_debug_messages(&mut d);
    assert_near(surface_pixel(&mut d, SURFACE_SIZE, 8, 8), [0, 0, 255, 255]);
    assert_near(target_pixel(&mut d, &a, 8, 8), [0, 255, 0, 255]);
    assert_no_debug_messages(&mut d);
}

#[test]
fn hidden_surface_skips_drawing() {
    let mut d = common::device(SURFACE_SIZE, SURFACE_SIZE);
    let pipeline = color_pipeline(&mut d);
    let red = color_vertices(&FULL.map(|(x, y)| (x, y, [255, 0, 0, 255])));
    let vertices = d
        .create_buffer(&buffer(BufferKind::Vertex, 36), Some(&red))
        .expect("vertex buffer");
    let tint = d
        .create_buffer(&buffer(BufferKind::Uniform, 16), Some(&floats(&[1.0; 4])))
        .expect("uniform buffer");

    d.surface_mut().set_hidden(true);
    assert!(d.begin_frame(Color::BLACK).is_empty());
    d.set_pipeline(&pipeline);
    d.set_vertex_buffer(&vertices, 0).expect("vertex buffer");
    d.set_uniform_buffer(2, &tint).expect("uniform buffer");
    d.draw(0..3).expect("draw while hidden");
    // `present` drains the debug messages, so check before it.
    assert_no_debug_messages(&mut d);
    d.present().expect("present while hidden");

    d.surface_mut().set_hidden(false);
    assert_eq!(
        d.begin_frame(Color::BLACK),
        Extent::new(SURFACE_SIZE, SURFACE_SIZE)
    );
    d.draw(0..3).expect("draw");
    assert_near(surface_pixel(&mut d, SURFACE_SIZE, 8, 8), [255, 0, 0, 255]);
    assert_no_debug_messages(&mut d);
}

const SURFACE_SIZE: u32 = 64;
