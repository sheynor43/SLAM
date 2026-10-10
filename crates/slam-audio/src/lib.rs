//! Audio HAL and backends, mixer, decoding, resampling, time-stretch, audio clock.

mod hal;
#[cfg(target_os = "linux")]
mod pipewire;
mod position;
mod tone;

pub use hal::{
    AudioCallback, AudioError, Backend, CallbackInfo, DeviceInfo, OutputStream, StreamConfig,
    StreamCounters, devices, open_output,
};
pub use position::{AudioPosition, PositionSnapshot};
pub use tone::SineTone;
