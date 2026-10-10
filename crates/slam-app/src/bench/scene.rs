//! The benchmark scene: rotating squares from one atlas page, nothing else.

use slam_engine::gl::SdlGlSurface;
use slam_engine::{Draw, FrameInfo};
use slam_input::QuitHandle;
use slam_render::atlas::{Atlas, AtlasRegion};
use slam_render::gl::{GlDebug, GlDevice, GlTexture};
use slam_render::sprite::{DEFAULT_SLOTS, Projection, Sprite, SpriteBatch, SpriteRenderer};
use slam_render::textures::Textures;
use slam_render::{Color, Device, Extent, TextureFormat};

use slam_engine::clock::now_ns;

use super::record::{FrameWork, Recorder};
use super::thread::ThreadSched;

const SPRITE_SIZE: f32 = 24.0;

/// Fills `batch` with `count` squares moving over a `[w, h]` surface at time `t`
/// seconds. Does not allocate once `batch` has room for `count` sprites.
pub fn fill(batch: &mut SpriteBatch, region: &AtlasRegion, count: usize, t: f32, [w, h]: [f32; 2]) {
    batch.clear();
    for i in 0..count {
        let phase = i as f32 * 0.37;
        let x = (0.5 + 0.45 * (t * 0.7 + phase).sin()) * w;
        let y = (0.5 + 0.45 * (t * 1.1 + phase * 1.7).cos()) * h;
        let mut sprite = Sprite::new(region.texture, region.uv, [x, y], [SPRITE_SIZE; 2]);
        sprite.origin = [SPRITE_SIZE / 2.0; 2];
        sprite.rotation = t + phase;
        sprite.colour = Color::rgb(0.4 + 0.3 * (phase.sin() + 1.0), 0.5, 0.9);
        batch.push(&sprite);
    }
}

/// Closes the window when dropped. The draw thread drops it while unwinding from a
/// panic, so a failed start does not leave the main thread waiting for the timer.
pub(crate) struct QuitOnDrop(pub(crate) QuitHandle);

impl Drop for QuitOnDrop {
    fn drop(&mut self) {
        self.0.request();
    }
}

struct Resources {
    device: GlDevice<SdlGlSurface>,
    textures: Textures<GlTexture>,
    renderer: SpriteRenderer<GlDevice<SdlGlSurface>>,
    square: AtlasRegion,
}

/// Owns the GL surface; the device and its resources exist between `start` and
/// `stop`, on the draw thread.
pub struct Scene {
    surface: Option<SdlGlSurface>,
    sprites: usize,
    /// Engine time of `t = 0` for the animation.
    epoch_ns: u64,
    batch: SpriteBatch,
    resources: Option<Resources>,
    pub recorder: Recorder,
    /// Filled by `start`.
    pub driver: String,
    /// Wait for the GPU (`glFinish`) before `present`; the wait counts as draw time.
    finish: bool,
    sched: ThreadSched,
    /// Outcome of applying `sched` to the draw thread, set by `start`.
    pub sched_result: Result<(), String>,
    _quit: QuitOnDrop,
}

impl Scene {
    pub fn new(
        surface: SdlGlSurface,
        sprites: usize,
        epoch_ns: u64,
        recorder: Recorder,
        quit: QuitHandle,
        finish: bool,
        sched: ThreadSched,
    ) -> Self {
        Self {
            surface: Some(surface),
            sprites,
            epoch_ns,
            batch: SpriteBatch::with_capacity(sprites),
            resources: None,
            recorder,
            driver: String::new(),
            finish,
            sched,
            sched_result: Ok(()),
            _quit: QuitOnDrop(quit),
        }
    }
}

impl Draw<()> for Scene {
    fn start(&mut self) {
        let surface = self.surface.take().expect("surface is set before start");
        let mut device = GlDevice::new(surface, GlDebug::for_build()).expect("GL device");
        self.driver = device.driver_info();
        let mut textures = Textures::new();
        let mut atlas = Atlas::new(Extent::new(64, 64), TextureFormat::Rgba8);
        let square = atlas
            .add(
                &mut device,
                &mut textures,
                Extent::new(8, 8),
                &[255; 8 * 8 * 4],
            )
            .expect("square");
        let renderer = SpriteRenderer::new(&mut device, DEFAULT_SLOTS).expect("renderer");
        // After the driver has set up its worker threads, so they do not inherit
        // the draw thread's affinity.
        self.sched_result = self.sched.apply();
        self.resources = Some(Resources {
            device,
            textures,
            renderer,
            square,
        });
    }

    fn frame(&mut self, info: &FrameInfo, _: &()) {
        let r = self.resources.as_mut().expect("started");
        let size = r.device.begin_frame(Color::rgb(0.05, 0.05, 0.08));
        let t = info.wait.woke_ns.saturating_sub(self.epoch_ns) as f32 / 1e9;
        let surface = [size.width as f32, size.height as f32];
        fill(&mut self.batch, &r.square, self.sprites, t, surface);

        r.renderer.begin_frame();
        let textures = &r.textures;
        let stats = r
            .renderer
            .draw(
                &mut r.device,
                &self.batch,
                &Projection::pixels(size),
                |id| textures.get(id),
            )
            .ok();
        if self.finish {
            r.device.finish();
        }
        let drawn_ns = now_ns();
        let presented = r.device.present().is_ok();
        let work = FrameWork {
            drawn_ns,
            presented_ns: now_ns(),
        };
        self.recorder
            .record(&info.wait, work, stats.filter(|_| presented));
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
