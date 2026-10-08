//! Moving sprites with the frame-time overlay on top: frame-time graph, p50/p99/p99.9
//! and the draw, update and input (main-thread poll) rates. Move the mouse over the
//! window to see the input rate.
//!
//! Run: `cargo run --release -p slam-engine --example overlay [draw_hz] [sprites]`
//! (defaults: 0 = unlimited, 100 sprites). Close the window to exit. With
//! `--features tracy`, connect the Tracy viewer to see zones and frame marks.

use std::sync::Arc;

use slam_engine::clock::now_ns;
use slam_engine::gl::SdlGlSurface;
use slam_engine::limiter::LimiterMode;
use slam_engine::overlay::{FrameOverlay, OverlayConfig, font_sheet};
use slam_engine::{Draw, Engine, EngineConfig, EngineCounters, FrameInfo, TickInfo, Update};
use slam_input::{InputCounters, InputEvent, InputWindow, TickMapper, event_ring, sdl_ticks_ns};
use slam_render::atlas::{Atlas, AtlasRegion};
use slam_render::gl::{GlDebug, GlDevice, GlTexture};
use slam_render::sprite::{DEFAULT_SLOTS, Projection, Sprite, SpriteBatch, SpriteRenderer};
use slam_render::textures::Textures;
use slam_render::{Color, Device, Extent, TextureFormat};

struct Idle;

impl Update for Idle {
    type Snapshot = ();

    fn tick(&mut self, _: &TickInfo, _: &[InputEvent], _: &mut ()) {}
}

struct Resources {
    device: GlDevice<SdlGlSurface>,
    textures: Textures<GlTexture>,
    renderer: SpriteRenderer<GlDevice<SdlGlSurface>>,
    square: AtlasRegion,
    overlay: FrameOverlay,
}

/// Owns the GL surface; the device and its resources exist between `start` and
/// `stop`, on the draw thread.
struct Scene {
    surface: Option<SdlGlSurface>,
    engine: Arc<EngineCounters>,
    input: Arc<InputCounters>,
    sprites: usize,
    batch: SpriteBatch,
    resources: Option<Resources>,
    start_ns: u64,
    /// Frames whose draw or present failed.
    errors: u64,
}

impl Draw<()> for Scene {
    fn start(&mut self) {
        let surface = self.surface.take().expect("surface is set before start");
        let mut device = GlDevice::new(surface, GlDebug::for_build()).expect("GL device");
        let mut textures = Textures::new();
        let mut atlas = Atlas::new(Extent::new(512, 512), TextureFormat::Rgba8);
        let config = OverlayConfig::default();
        let (size, pixels) = font_sheet(config.scale);
        let font = atlas
            .add(&mut device, &mut textures, size, &pixels)
            .expect("font");
        let square = atlas
            .add(
                &mut device,
                &mut textures,
                Extent::new(8, 8),
                &[255; 8 * 8 * 4],
            )
            .expect("square");
        let renderer = SpriteRenderer::new(&mut device, DEFAULT_SLOTS).expect("renderer");
        let overlay = FrameOverlay::new(
            config,
            font,
            Arc::clone(&self.engine),
            Some(Arc::clone(&self.input)),
        );
        self.start_ns = now_ns();
        self.resources = Some(Resources {
            device,
            textures,
            renderer,
            square,
            overlay,
        });
    }

    fn frame(&mut self, info: &FrameInfo, _: &()) {
        let r = self.resources.as_mut().expect("started");
        r.overlay.record(info);
        let size = r.device.begin_frame(Color::rgb(0.05, 0.05, 0.08));
        let t = now_ns().saturating_sub(self.start_ns) as f32 / 1e9;
        let [w, h] = [size.width as f32, size.height as f32];

        self.batch.clear();
        for i in 0..self.sprites {
            let phase = i as f32 * 0.37;
            let x = (0.5 + 0.45 * (t * 0.7 + phase).sin()) * w;
            let y = (0.5 + 0.45 * (t * 1.1 + phase * 1.7).cos()) * h;
            let mut sprite = Sprite::new(r.square.texture, r.square.uv, [x, y], [24.0, 24.0]);
            sprite.origin = [12.0, 12.0];
            sprite.rotation = t + phase;
            sprite.colour = Color::rgb(0.4 + 0.6 * (phase.sin() * 0.5 + 0.5), 0.5, 0.9);
            self.batch.push(&sprite);
        }
        r.overlay.push(&mut self.batch, [8.0, 8.0]);

        r.renderer.begin_frame();
        let textures = &r.textures;
        let drawn = r
            .renderer
            .draw(
                &mut r.device,
                &self.batch,
                &Projection::pixels(size),
                |id| textures.get(id),
            )
            .is_ok();
        if !drawn || r.device.present().is_err() {
            self.errors += 1;
        }
    }

    fn stop(&mut self) {
        let Resources {
            mut device,
            mut textures,
            renderer,
            ..
        } = self.resources.take().expect("started");
        renderer.destroy(&mut device);
        for texture in textures.drain() {
            device.destroy_texture(texture);
        }
        self.surface = Some(device.into_surface().expect("release GL context"));
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let draw_hz: f64 = args.next().map_or(0.0, |a| a.parse().expect("draw_hz"));
    let sprites: usize = args.next().map_or(100, |a| a.parse().expect("sprites"));
    let draw = if draw_hz == 0.0 {
        LimiterMode::Unlimited
    } else {
        LimiterMode::Hz(draw_hz)
    };

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    slam_engine::profiling::start();

    // The window and its GL context are created on the main thread; the draw thread
    // takes the context over (ADR-0018).
    let mut window = InputWindow::new_opengl("SLAM overlay", 1280, 720).expect("window");
    let surface = SdlGlSurface(window.create_gl_context().expect("GL context"));

    let counters = Arc::new(EngineCounters::default());
    let (mut sink, source) = event_ring(1024);
    let engine = Engine::spawn_with_counters(
        EngineConfig {
            draw,
            ..EngineConfig::default()
        },
        Idle,
        Scene {
            surface: Some(surface),
            engine: Arc::clone(&counters),
            input: Arc::clone(window.counters()),
            sprites,
            batch: SpriteBatch::with_capacity(sprites + 1024),
            resources: None,
            start_ns: 0,
            errors: 0,
        },
        (),
        source,
        counters,
    )
    .expect("engine");
    sink.set_notify(engine.waker().as_notify());

    let mut mapper = TickMapper::new(now_ns, sdl_ticks_ns);
    window.run(&mut sink, &mut mapper).expect("event loop");

    // Stop drawing and destroy the GL context before the window.
    let stopped = engine.shutdown();
    println!(
        "{} frames, {} failed to draw or present",
        stopped.frames, stopped.draw.errors
    );
    drop(stopped.draw);
    drop(window);
}
