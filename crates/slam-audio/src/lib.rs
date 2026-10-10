//! Audio HAL and backends, mixer, decoding, resampling, time-stretch, audio clock.

mod hal;
mod mixer;
#[cfg(target_os = "linux")]
mod pipewire;
mod position;
mod sample;
mod tone;

pub use hal::{
    AudioCallback, AudioError, Backend, CallbackInfo, DeviceInfo, OutputStream, StreamConfig,
    StreamCounters, devices, open_output,
};
pub use mixer::{
    Mixer, MixerConfig, MixerHandle, MixerStats, PlayError, SAMPLE_CONCURRENCY, VoiceStart, mixer,
};
pub use position::{AudioPosition, PositionSnapshot};
pub use sample::{MAX_CHANNELS, MAX_SAMPLE_VALUES, Sample, SampleError};
pub use tone::SineTone;
