//! Drawing through the OpenGL backend: buffers, pipelines, blending, render targets,
//! textures and draw-time errors, checked by reading pixels back. Needs a display and
//! a GL 3.3 driver: `cargo test -p slam-engine --test gl_draw -- --ignored`.

use glow::HasContext;
use slam_engine::gl::SdlGlSurface;
use slam_input::InputWindow;
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::{
    Blend, BufferDesc, BufferKind, BufferUpdate, Color, DescError, Device, DeviceError, DrawError,
    Extent, GlslShader, IndexFormat, PipelineDesc, RenderTargetDesc, Shader, TextureDesc,
    TextureFormat, TextureUsage, VertexAttribute, VertexFormat, VertexLayout,
};
use slam_testkit::count_allocs;

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

/// Reads one pixel of the bound framebuffer (render target or back buffer).
fn pixel(device: &GlDevice<SdlGlSurface>, x: i32, y: i32, back_buffer: bool) -> [u8; 4] {
    let mut pixel = [0u8; 4];
    let gl = device.gl();
    // SAFETY: the context is current on this thread; `pixel` holds one RGBA8 pixel.
    unsafe {
        gl.read_buffer(if back_buffer {
            glow::BACK
        } else {
            glow::COLOR_ATTACHMENT0
        });
        gl.read_pixels(
            x,
            y,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixel)),
        );
    }
    pixel
}

fn assert_near(actual: [u8; 4], expected: [u8; 4]) {
    let close = actual
        .iter()
        .zip(expected)
        .all(|(&a, e)| a.abs_diff(e) <= 2);
    assert!(close, "pixel {actual:?}, expected about {expected:?}");
}

fn buffer(kind: BufferKind, size: u64) -> BufferDesc {
    BufferDesc {
        kind,
        size,
        update: BufferUpdate::Static,
    }
}

// One test per binary: SDL is process-global.
#[test]
#[ignore = "needs a display and an OpenGL 3.3 driver"]
fn draws_through_buffers_pipelines_and_targets() {
    let mut window = InputWindow::new_opengl("SLAM GL draw test", 64, 64).expect("window");
    let surface = SdlGlSurface(window.create_gl_context().expect("GL context"));

    let surface = std::thread::spawn(move || {
        let mut d = GlDevice::new(surface, GlDebug::On).expect("GL device");
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

        // Compiler output is reported with the shader name and stage, for each stage
        // (a failing render target variant also cleans up the surface program).
        const BROKEN: &str = "#version 330 core\nvoid main() { undefined_call(); }";
        let glsl = shaders::TEST_COLOR.glsl;
        for (broken, stage) in [
            (
                GlslShader {
                    fragment: BROKEN,
                    ..glsl
                },
                "fragment",
            ),
            (
                GlslShader {
                    vertex_surface: BROKEN,
                    ..glsl
                },
                "surface vertex",
            ),
            (
                GlslShader {
                    vertex_target: BROKEN,
                    ..glsl
                },
                "render target vertex",
            ),
        ] {
            let shader = Shader {
                name: "broken",
                glsl: broken,
            };
            let result = d.create_pipeline(&PipelineDesc {
                shader: &shader,
                ..color_desc(Blend::Replace)
            });
            match result {
                Err(DeviceError::Shader(log)) => {
                    assert!(log.starts_with(&format!("broken: {stage}: ")), "{log}")
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
        assert_near(pixel(&d, 8, 8, false), [0, 0, 0, 255]);
        d.set_pipeline(&opaque);
        d.set_vertex_buffer(&vertices, 0).expect("vertex buffer");
        d.set_uniform_buffer(2, &tint).expect("uniform buffer");
        d.draw(0..3).expect("draw");
        assert_near(pixel(&d, 8, 8, false), [255, 0, 0, 255]);
        d.write_buffer(&tint, 0, &floats(&[0.0, 1.0, 1.0, 1.0]))
            .expect("write");
        d.draw(0..3).expect("draw");
        assert_near(pixel(&d, 8, 8, false), [0, 0, 0, 255]);

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
        assert_near(pixel(&d, 8, 8, false), [128, 0, 127, 192]);
        // The same vertices through a 3-vertex window starting at vertex 3.
        d.begin_pass(Some(&target), Some(Color::rgb(0.0, 0.0, 1.0)));
        d.set_vertex_buffer(&vertices, 0).expect("vertex buffer");
        d.draw(3..6).expect("draw");
        assert_near(pixel(&d, 8, 8, false), [128, 0, 127, 255]);

        // Premultiplied: the tint halves red, so src = (0.5, 0, 0, 0.5) over blue.
        d.write_buffer(&tint, 0, &floats(&[0.5, 1.0, 1.0, 1.0]))
            .expect("write");
        d.begin_pass(Some(&target), Some(quarter_blue));
        d.set_pipeline(&premultiplied);
        d.draw(3..6).expect("draw");
        // rgb: (0.5, 0, 0) + blue * 127/255; alpha: 128/255 + 0.25 * 127/255.
        assert_near(pixel(&d, 8, 8, false), [128, 0, 127, 159]);
        // Additive: red * 128/255 + blue; alpha 128/255 + 0.25.
        d.write_buffer(&tint, 0, &floats(&[1.0, 1.0, 1.0, 1.0]))
            .expect("write");
        d.begin_pass(Some(&target), Some(quarter_blue));
        d.set_pipeline(&additive);
        d.draw(3..6).expect("draw");
        assert_near(pixel(&d, 8, 8, false), [128, 0, 255, 192]);

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
        assert_near(pixel(&d, 8, 8, false), [255, 0, 0, 255]);
        d.destroy_pipeline(scratch_pipeline);
        // Destroying the bound target leaves the cache consistent: the next pass binds
        // `target` for real (its pixel reads back).
        d.begin_pass(Some(&scratch_target), Some(Color::BLACK));
        d.destroy_render_target(scratch_target);
        d.begin_pass(Some(&target), None);
        assert_near(pixel(&d, 8, 8, false), [255, 0, 0, 255]);

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
        assert_near(pixel(&d, 4, 8, false), [0, 255, 0, 255]);
        assert_near(pixel(&d, 12, 8, false), [0, 0, 0, 255]);
        assert_eq!(
            d.draw_indexed(0..7),
            Err(DrawError::OutOfRange { start: 0, end: 7 })
        );

        // The render target sampled into the surface.
        d.set_texture(3, d.render_target_texture(&target))
            .expect("target texture");
        let size = d.begin_frame(Color::rgb(0.0, 0.0, 1.0));
        assert!(!size.is_empty(), "drawable size {size:?}");
        d.draw_indexed(0..6).expect("draw indexed");
        // The left of the surface samples the left half of the target: green.
        assert_near(pixel(&d, 0, 0, true), [0, 255, 0, 255]);
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
        assert_near(pixel(&d, 8, 0, false), RED);
        assert_near(pixel(&d, 8, 15, false), BLUE);
        // GL reads the back buffer bottom-up.
        let top = d.begin_frame(Color::BLACK).height as i32 - 1;
        d.draw_indexed(0..6).expect("draw indexed");
        assert_near(pixel(&d, 0, top, true), RED);
        assert_near(pixel(&d, 0, 0, true), BLUE);
        // The target drawn onto the surface keeps its orientation.
        d.set_texture(3, d.render_target_texture(&target))
            .expect("target texture");
        d.begin_frame(Color::BLACK);
        d.draw_indexed(0..6).expect("draw indexed");
        assert_near(pixel(&d, 0, top, true), RED);
        assert_near(pixel(&d, 0, 0, true), BLUE);
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

        // A frame of draws into a target and onto the surface does not allocate.
        let (result, stats) = count_allocs(|| {
            let mut tint_data = [0u8; 16];
            for i in 0..100u8 {
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
                d.present().expect("present");
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
        d.into_surface().expect("release")
    })
    .join()
    .expect("draw thread");

    drop(surface);
    drop(window);
}
