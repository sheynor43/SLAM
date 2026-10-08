//! Draws a spinning triangle from the draw thread through the OpenGL backend: vertex
//! buffer, uniform buffer rewritten every frame, GLSL 3.30 pipeline. Prints the draw
//! rate once per second.
//!
//! Run: `cargo run --release -p slam-engine --example triangle [draw_hz]` (default 0 =
//! unlimited). Close the window to exit. GL debug output (debug builds) is logged;
//! filter with `RUST_LOG`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use slam_engine::clock::now_ns;
use slam_engine::gl::SdlGlSurface;
use slam_engine::limiter::LimiterMode;
use slam_engine::{Draw, Engine, EngineConfig, FrameInfo, TickInfo, Update};
use slam_input::{InputEvent, InputWindow, TickMapper, event_ring, sdl_ticks_ns};
use slam_render::gl::{GlBuffer, GlDebug, GlDevice, GlPipeline};
use slam_render::{
    Binding, BindingKind, Blend, BufferDesc, BufferKind, BufferUpdate, Color, Device, PipelineDesc,
    VertexAttribute, VertexFormat, VertexLayout,
};

const VERTEX_SHADER: &str = r"#version 330 core
layout(location = 0) in vec2 position;
layout(location = 1) in vec4 color;
layout(std140) uniform Frame { vec4 rotation_aspect; };
out vec4 v_color;
void main() {
    float c = rotation_aspect.x;
    float s = rotation_aspect.y;
    vec2 p = vec2(c * position.x - s * position.y, s * position.x + c * position.y);
    gl_Position = vec4(p.x * rotation_aspect.z, p.y, 0.0, 1.0);
    v_color = color;
}
";

const FRAGMENT_SHADER: &str = r"#version 330 core
in vec4 v_color;
out vec4 frag;
void main() { frag = v_color; }
";

const ATTRIBUTES: [VertexAttribute; 2] = [
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

/// Nothing to simulate: the triangle's angle comes from the clock on the draw thread.
struct Idle;

impl Update for Idle {
    type Snapshot = ();

    fn tick(&mut self, _: &TickInfo, _: &[InputEvent], _: &mut ()) {}
}

struct Resources {
    device: GlDevice<SdlGlSurface>,
    pipeline: GlPipeline,
    vertices: GlBuffer,
    uniforms: GlBuffer,
}

/// Owns the GL surface; the device and its resources exist between `start` and
/// `stop`, on the draw thread.
struct Triangle {
    frames: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
    surface: Option<SdlGlSurface>,
    resources: Option<Resources>,
    start_ns: u64,
}

impl Draw<()> for Triangle {
    fn start(&mut self) {
        let surface = self.surface.take().expect("surface is set before start");
        let mut device = GlDevice::new(surface, GlDebug::for_build()).expect("GL device");
        let pipeline = device
            .create_pipeline(&PipelineDesc {
                vertex_shader: VERTEX_SHADER,
                fragment_shader: FRAGMENT_SHADER,
                layout: VertexLayout {
                    stride: 12,
                    attributes: &ATTRIBUTES,
                },
                bindings: &[Binding {
                    name: "Frame",
                    kind: BindingKind::UniformBuffer,
                    slot: 0,
                }],
                blend: Blend::Replace,
            })
            .expect("pipeline");
        let mut data = Vec::new();
        for (x, y, rgba) in [
            (0.0f32, 0.8f32, [255u8, 64, 64, 255]),
            (-0.7, -0.45, [64, 255, 64, 255]),
            (0.7, -0.45, [64, 64, 255, 255]),
        ] {
            data.extend_from_slice(&x.to_ne_bytes());
            data.extend_from_slice(&y.to_ne_bytes());
            data.extend_from_slice(&rgba);
        }
        let vertices = device
            .create_buffer(
                &BufferDesc {
                    kind: BufferKind::Vertex,
                    size: data.len() as u64,
                    update: BufferUpdate::Static,
                },
                Some(&data),
            )
            .expect("vertex buffer");
        let uniforms = device
            .create_buffer(
                &BufferDesc {
                    kind: BufferKind::Uniform,
                    size: 16,
                    update: BufferUpdate::Dynamic,
                },
                None,
            )
            .expect("uniform buffer");
        self.start_ns = now_ns();
        self.resources = Some(Resources {
            device,
            pipeline,
            vertices,
            uniforms,
        });
    }

    fn frame(&mut self, _: &FrameInfo, _: &()) {
        let r = self.resources.as_mut().expect("started");
        let size = r.device.begin_frame(Color::rgb(0.05, 0.05, 0.08));
        let seconds = now_ns().saturating_sub(self.start_ns) as f64 / 1e9;
        let angle = (seconds * 1.5) as f32;
        let aspect = if size.is_empty() {
            1.0
        } else {
            size.height as f32 / size.width as f32
        };
        let mut uniforms = [0u8; 16];
        for (chunk, value) in
            uniforms
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip([angle.cos(), angle.sin(), aspect, 0.0])
        {
            chunk.copy_from_slice(&value.to_ne_bytes());
        }
        let drawn = r.device.write_buffer(&r.uniforms, 0, &uniforms).is_ok() && {
            r.device.set_pipeline(&r.pipeline);
            r.device.set_vertex_buffer(&r.vertices, 0).is_ok()
                && r.device.set_uniform_buffer(0, &r.uniforms).is_ok()
                && r.device.draw(0..3).is_ok()
        };
        if !drawn || r.device.present().is_err() {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        self.frames.fetch_add(1, Ordering::Relaxed);
    }

    fn stop(&mut self) {
        let Resources {
            mut device,
            pipeline,
            vertices,
            uniforms,
        } = self.resources.take().expect("started");
        device.destroy_pipeline(pipeline);
        device.destroy_buffer(vertices);
        device.destroy_buffer(uniforms);
        self.surface = Some(device.into_surface().expect("release GL context"));
    }
}

fn main() {
    let draw_hz: f64 = std::env::args()
        .nth(1)
        .map_or(0.0, |a| a.parse().expect("draw_hz"));
    let draw = if draw_hz == 0.0 {
        LimiterMode::Unlimited
    } else {
        LimiterMode::Hz(draw_hz)
    };

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // The window and its GL context are created on the main thread; the draw thread
    // takes the context over (ADR-0018).
    let mut window = InputWindow::new_opengl("SLAM triangle", 1280, 720).expect("window");
    let surface = SdlGlSurface(window.create_gl_context().expect("GL context"));

    let frames = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let (mut sink, source) = event_ring(1024);
    let engine = Engine::spawn(
        EngineConfig {
            draw,
            ..EngineConfig::default()
        },
        Idle,
        Triangle {
            frames: Arc::clone(&frames),
            errors: Arc::clone(&errors),
            surface: Some(surface),
            resources: None,
            start_ns: 0,
        },
        (),
        source,
    )
    .expect("engine");
    sink.set_notify(engine.waker().as_notify());

    let done = Arc::new(AtomicBool::new(false));
    let reporter = {
        let (frames, errors, done) = (Arc::clone(&frames), Arc::clone(&errors), Arc::clone(&done));
        std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_secs(1));
                println!(
                    "draw {:>6}/s  errors {}",
                    frames.swap(0, Ordering::Relaxed),
                    errors.swap(0, Ordering::Relaxed),
                );
            }
        })
    };

    let mut mapper = TickMapper::new(now_ns, sdl_ticks_ns);
    window.run(&mut sink, &mut mapper).expect("event loop");

    // Stop drawing and destroy the GL context before the window.
    let stopped = engine.shutdown();
    drop(stopped.draw);
    drop(window);
    done.store(true, Ordering::Release);
    reporter.join().unwrap();
    println!("frames {}", stopped.frames);
}
