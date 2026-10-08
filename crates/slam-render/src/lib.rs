//! Rendering HAL and backends, sprite batcher, slider meshes, render targets.

pub mod gl;
pub mod hal;

pub use hal::{
    Binding, BindingKind, Blend, BufferDesc, BufferKind, BufferUpdate, Color, DescError, Device,
    DeviceError, DrawError, Extent, IndexFormat, Limits, PipelineDesc, RenderTargetDesc,
    TextureDesc, TextureFormat, TextureUsage, VertexAttribute, VertexFormat, VertexLayout,
};
