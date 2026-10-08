//! Rendering HAL and backends, sprite batcher, slider meshes, render targets.

pub mod gl;
pub mod hal;

/// Shaders of this crate, translated from `shaders/*.wgsl` at build time (ADR-0019).
pub mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shaders.rs"));
}

pub use hal::{
    Binding, BindingKind, Blend, BufferDesc, BufferKind, BufferUpdate, Color, DescError, Device,
    DeviceError, DrawError, Extent, GlslShader, IndexFormat, Limits, PipelineDesc,
    RenderTargetDesc, Shader, TextureDesc, TextureFormat, TextureUsage, VertexAttribute,
    VertexFormat, VertexLayout,
};
