//! Rendering HAL and backends, sprite batcher, slider meshes, render targets.

pub mod gl;
pub mod hal;

pub use hal::{
    Color, DescError, Device, DeviceError, Extent, Limits, TextureDesc, TextureFormat, TextureUsage,
};
