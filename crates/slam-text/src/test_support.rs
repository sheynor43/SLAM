//! Shared by unit tests: a minimal device, keys of real glyphs and a rig that runs
//! the generator by hand.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use skrifa::{FontRef, MetadataProvider};
use slam_render::textures::Textures;
use slam_render::upload::TextureRequests;
use slam_render::{
    BufferDesc, Color, DescError, Device, DeviceError, DrawError, Extent, IndexFormat, Limits,
    Origin, PipelineDesc, RenderTargetDesc, TextureDesc,
};

use crate::glyphs::{Generator, Step, parts};
use crate::{FontRegistry, Glyph, GlyphAtlasConfig, GlyphCache, GlyphKey, UI_FAMILY};

slam_testkit::install_counting_allocator!();

pub const LIMITS: Limits = Limits {
    max_texture_size: 4096,
    max_uniform_block_size: 16384,
    max_uniform_buffer_slots: 12,
    max_texture_slots: 16,
};

/// Implements what texture uploads call; everything else is not reached.
#[derive(Default)]
pub struct TestDevice {
    next: u32,
}

impl Device for TestDevice {
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

/// Key of `c` in the UI family at `weight`.
pub fn key_of(registry: &FontRegistry, weight: u16, c: char) -> GlyphKey {
    let face = registry
        .resolve(registry.family(UI_FAMILY).unwrap(), weight)
        .unwrap();
    let (data, index) = registry.face_data(face.font).unwrap();
    let font = FontRef::from_index(data, index).unwrap();
    GlyphKey {
        font: face.font,
        weight: face.weight,
        glyph_id: font.charmap().map(c).unwrap().to_u32() as u16,
    }
}

pub fn config(max_glyphs: u32, request_capacity: usize) -> GlyphAtlasConfig {
    let mut config = GlyphAtlasConfig::new(LIMITS);
    config.max_glyphs = max_glyphs;
    config.request_capacity = request_capacity;
    config.loader.max_textures = max_glyphs;
    config
}

/// A cache with its generator stepped by the test (the loader thread is real).
pub struct Rig {
    pub registry: Arc<FontRegistry>,
    pub cache: GlyphCache,
    pub generator: Generator<TextureRequests>,
    pub device: TestDevice,
    pub textures: Textures<u32>,
}

impl Rig {
    pub fn new(max_glyphs: u32, request_capacity: usize) -> Self {
        let registry = Arc::new(FontRegistry::with_builtin());
        let (cache, generator) =
            parts(Arc::clone(&registry), &config(max_glyphs, request_capacity)).unwrap();
        Self {
            registry,
            cache,
            generator,
            device: TestDevice::default(),
            textures: Textures::with_capacity(64),
        }
    }

    /// Runs the generator until it waits.
    pub fn pump(&mut self) -> Step {
        loop {
            match self.generator.step() {
                Step::Busy => {}
                other => return other,
            }
        }
    }

    /// Frames until `key` is no longer pending (bounded).
    pub fn settle(&mut self, key: GlyphKey) -> Glyph {
        for _ in 0..3000 {
            self.pump();
            self.cache.process(&mut self.device, &mut self.textures);
            match self.cache.glyph(key) {
                Glyph::Pending => std::thread::sleep(Duration::from_millis(1)),
                other => return other,
            }
        }
        panic!("{key:?} stayed pending");
    }
}
