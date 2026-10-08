//! A [`Device`] for unit tests: validates like a backend, records frame commands
//! into a log (pre-allocate it for allocation tests), tracks live resources and can
//! fail resource creation on demand.

use std::ops::Range;

use crate::hal::{
    Blend, BufferDesc, BufferKind, Color, DescError, Device, DeviceError, DrawError, Extent,
    IndexFormat, Limits, Origin, PipelineDesc, RenderTargetDesc, TextureDesc,
};

#[derive(Default)]
pub struct Fake {
    next_id: u32,
    pub log: Vec<Call>,
    pub live_buffers: Vec<(u32, u64)>,
    pub live_textures: Vec<(u32, TextureDesc)>,
    pub live_pipelines: Vec<u32>,
    /// Resource creations left before one fails; `None` never fails.
    pub fail_after: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Write {
        buffer: u32,
        len: usize,
    },
    WriteTexture {
        texture: u32,
        origin: Origin,
        size: Extent,
    },
    Pipeline(Blend),
    Texture(u32),
    Vertices {
        buffer: u32,
        offset: u64,
    },
    DrawIndexed(u32),
}

pub struct FakeBuffer {
    pub id: u32,
    desc: BufferDesc,
}

pub struct FakePipeline {
    id: u32,
    blend: Blend,
}

pub const LIMITS: Limits = Limits {
    max_texture_size: 4096,
    max_uniform_block_size: 16384,
    max_uniform_buffer_slots: 12,
    max_texture_slots: 16,
};

impl Fake {
    /// Fails resource creation after `n` successful ones.
    pub fn failing_after(n: u32) -> Self {
        Self {
            fail_after: Some(n),
            ..Self::default()
        }
    }

    fn create(&mut self) -> Result<u32, DeviceError> {
        if let Some(left) = &mut self.fail_after {
            if *left == 0 {
                return Err(DeviceError::Backend("injected failure".into()));
            }
            *left -= 1;
        }
        self.next_id += 1;
        Ok(self.next_id)
    }

    /// The log without buffer and texture writes.
    pub fn draws(&self) -> Vec<Call> {
        self.log
            .iter()
            .copied()
            .filter(|c| !matches!(c, Call::Write { .. } | Call::WriteTexture { .. }))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.live_buffers.is_empty()
            && self.live_textures.is_empty()
            && self.live_pipelines.is_empty()
    }
}

impl Device for Fake {
    type Texture = u32;
    type Buffer = FakeBuffer;
    type Pipeline = FakePipeline;
    type RenderTarget = u32;

    fn limits(&self) -> Limits {
        LIMITS
    }

    fn create_texture(
        &mut self,
        desc: &TextureDesc,
        data: Option<&[u8]>,
    ) -> Result<u32, DeviceError> {
        desc.validate(&LIMITS, data)?;
        let id = self.create()?;
        self.live_textures.push((id, *desc));
        Ok(id)
    }

    fn destroy_texture(&mut self, texture: u32) {
        self.live_textures.retain(|(id, _)| *id != texture);
    }

    fn write_texture(
        &mut self,
        texture: &u32,
        origin: Origin,
        size: Extent,
        data: &[u8],
    ) -> Result<(), DescError> {
        let (_, desc) = self
            .live_textures
            .iter()
            .find(|(id, _)| id == texture)
            .expect("live texture");
        desc.check_write(origin, size, data.len())?;
        self.log.push(Call::WriteTexture {
            texture: *texture,
            origin,
            size,
        });
        Ok(())
    }

    fn create_buffer(
        &mut self,
        desc: &BufferDesc,
        data: Option<&[u8]>,
    ) -> Result<FakeBuffer, DeviceError> {
        desc.validate(&LIMITS, data)?;
        let id = self.create()?;
        self.live_buffers.push((id, desc.size));
        Ok(FakeBuffer { id, desc: *desc })
    }

    fn destroy_buffer(&mut self, buffer: FakeBuffer) {
        self.live_buffers.retain(|(id, _)| *id != buffer.id);
    }

    fn write_buffer(
        &mut self,
        buffer: &FakeBuffer,
        offset: u64,
        data: &[u8],
    ) -> Result<(), DescError> {
        buffer.desc.check_write(offset, data.len())?;
        self.log.push(Call::Write {
            buffer: buffer.id,
            len: data.len(),
        });
        Ok(())
    }

    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<FakePipeline, DeviceError> {
        desc.validate(&LIMITS)?;
        let id = self.create()?;
        self.live_pipelines.push(id);
        Ok(FakePipeline {
            id,
            blend: desc.blend,
        })
    }

    fn destroy_pipeline(&mut self, pipeline: FakePipeline) {
        self.live_pipelines.retain(|id| *id != pipeline.id);
    }

    fn create_render_target(&mut self, _: &RenderTargetDesc) -> Result<u32, DeviceError> {
        self.create()
    }

    fn destroy_render_target(&mut self, _: u32) {}

    fn render_target_texture<'a>(&self, target: &'a u32) -> &'a u32 {
        target
    }

    fn begin_frame(&mut self, _: Color) -> Extent {
        Extent::new(100, 100)
    }

    fn begin_pass(&mut self, _: Option<&u32>, _: Option<Color>) -> Extent {
        Extent::new(100, 100)
    }

    fn set_pipeline(&mut self, pipeline: &FakePipeline) {
        self.log.push(Call::Pipeline(pipeline.blend));
    }

    fn set_vertex_buffer(&mut self, buffer: &FakeBuffer, offset: u64) -> Result<(), DrawError> {
        assert_eq!(buffer.desc.kind, BufferKind::Vertex);
        self.log.push(Call::Vertices {
            buffer: buffer.id,
            offset,
        });
        Ok(())
    }

    fn set_index_buffer(
        &mut self,
        buffer: &FakeBuffer,
        format: IndexFormat,
    ) -> Result<(), DrawError> {
        assert_eq!(buffer.desc.kind, BufferKind::Index);
        assert_eq!(format, IndexFormat::U16);
        Ok(())
    }

    fn set_uniform_buffer(&mut self, slot: u32, buffer: &FakeBuffer) -> Result<(), DrawError> {
        assert_eq!((slot, buffer.desc.kind), (0, BufferKind::Uniform));
        Ok(())
    }

    fn set_texture(&mut self, slot: u32, texture: &u32) -> Result<(), DrawError> {
        assert_eq!(slot, 0);
        self.log.push(Call::Texture(*texture));
        Ok(())
    }

    fn draw(&mut self, _: Range<u32>) -> Result<(), DrawError> {
        unreachable!("not used by the tests")
    }

    fn draw_indexed(&mut self, indices: Range<u32>) -> Result<(), DrawError> {
        assert_eq!(indices.start, 0);
        self.log.push(Call::DrawIndexed(indices.end));
        Ok(())
    }

    fn present(&mut self) -> Result<(), DeviceError> {
        Ok(())
    }
}
