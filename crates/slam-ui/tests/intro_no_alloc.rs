//! The intro does not touch the allocator once warm: its state on the update side and
//! `IntroView::push` on the draw side (ADR-0114).

use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slam_render::Extent;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::textures::Textures;
use slam_render::{
    BufferDesc, Color, DescError, Device, DeviceError, DrawError, IndexFormat, Limits, Origin,
    PipelineDesc, RenderTargetDesc, TextureDesc,
};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::intro::{Intro, IntroView, LENGTH_NS};
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

const FRAME_NS: u64 = 16_000_000;

/// Takes texture uploads, so that glyphs get ready; nothing else is reached.
#[derive(Default)]
struct UploadDevice {
    next: u32,
}

impl Device for UploadDevice {
    type Texture = u32;
    type Buffer = ();
    type Pipeline = ();
    type RenderTarget = ();

    fn limits(&self) -> Limits {
        LIMITS
    }
    fn create_texture(&mut self, _: &TextureDesc, _: Option<&[u8]>) -> Result<u32, DeviceError> {
        self.next += 1;
        Ok(self.next)
    }
    fn destroy_texture(&mut self, _: u32) {}
    fn write_texture(&mut self, _: &u32, _: Origin, _: Extent, _: &[u8]) -> Result<(), DescError> {
        Ok(())
    }
    fn create_buffer(&mut self, _: &BufferDesc, _: Option<&[u8]>) -> Result<(), DeviceError> {
        unimplemented!()
    }
    fn destroy_buffer(&mut self, _: ()) {
        unimplemented!()
    }
    fn write_buffer(&mut self, _: &(), _: u64, _: &[u8]) -> Result<(), DescError> {
        unimplemented!()
    }
    fn create_pipeline(&mut self, _: &PipelineDesc<'_>) -> Result<(), DeviceError> {
        unimplemented!()
    }
    fn destroy_pipeline(&mut self, _: ()) {
        unimplemented!()
    }
    fn create_render_target(&mut self, _: &RenderTargetDesc) -> Result<(), DeviceError> {
        unimplemented!()
    }
    fn destroy_render_target(&mut self, _: ()) {
        unimplemented!()
    }
    fn render_target_texture<'a>(&self, _: &'a ()) -> &'a u32 {
        unimplemented!()
    }
    fn begin_frame(&mut self, _: Color) -> Extent {
        unimplemented!()
    }
    fn begin_pass(&mut self, _: Option<&()>, _: Option<Color>) -> Extent {
        unimplemented!()
    }
    fn set_pipeline(&mut self, _: &()) {
        unimplemented!()
    }
    fn set_vertex_buffer(&mut self, _: &(), _: u64) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn set_index_buffer(&mut self, _: &(), _: IndexFormat) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn set_uniform_buffer(&mut self, _: u32, _: &()) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn set_texture(&mut self, _: u32, _: &u32) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn draw(&mut self, _: Range<u32>) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn draw_indexed(&mut self, _: Range<u32>) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn present(&mut self) -> Result<(), DeviceError> {
        unimplemented!()
    }
}

const LIMITS: Limits = Limits {
    max_texture_size: 4096,
    max_uniform_block_size: 16384,
    max_uniform_buffer_slots: 12,
    max_texture_slots: 16,
};

#[test]
fn intro_state_does_not_allocate() {
    let mut intro = Intro::new();
    assert_no_alloc(|| {
        intro.start(0);
        let mut now = 0;
        while now < LENGTH_NS + FRAME_NS {
            std::hint::black_box(intro.snapshot(now));
            std::hint::black_box(intro.music_due(now));
            now += FRAME_NS;
        }
        intro.skip(now);
    });
}

#[test]
fn view_pushes_without_allocating_once_warm() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
    let (mut device, mut textures) = (UploadDevice::default(), Textures::with_capacity(16));
    let mut view = IntroView::new(registry, Theme::default(), "SLAM");
    let white = AtlasRegion {
        texture: TextureId(0),
        origin: Origin::new(0, 0),
        size: Extent::new(1, 1),
        uv: UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
    };
    let (mut shapes, mut text, mut outlines) = (
        ShapeBatch::with_capacity(16),
        SpriteBatch::with_capacity(16),
        SpriteBatch::with_capacity(16),
    );
    let mut intro = Intro::new();
    intro.start(0);
    let mut frame = |now: u64, width: f32, height: f32, glyphs: &mut _| {
        shapes.clear();
        text.clear();
        outlines.clear();
        if let Some(snapshot) = intro.snapshot(now) {
            view.push(
                &mut shapes,
                &mut text,
                &mut outlines,
                glyphs,
                width,
                height,
                &snapshot,
                white,
            );
        }
        // Letters drawn: outlined, filled.
        (outlines.len(), text.len())
    };
    // Warm: the first frames at each size lay out and request glyphs, which then get
    // ready, so the letters are drawn below.
    frame(0, 800.0, 600.0, &mut glyphs);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (outlined, _) = frame(1_500_000_000, 1280.0, 720.0, &mut glyphs);
        glyphs.process(&mut device, &mut textures);
        if glyphs.all_ready() && outlined == 4 {
            break;
        }
        assert!(Instant::now() < deadline, "glyphs did not get ready");
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut last = (0, 0);
    assert_no_alloc(|| {
        let mut now = 0;
        while now < LENGTH_NS {
            last = frame(now, 1280.0, 720.0, &mut glyphs);
            now += FRAME_NS;
        }
    });
    // The letters were drawn, filled at the end.
    assert_eq!(last.1, 4);
}
