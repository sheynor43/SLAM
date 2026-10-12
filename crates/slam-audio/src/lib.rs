//! Audio HAL and backends, mixer, decoding, resampling, time-stretch, audio clock.

#[cfg(target_os = "linux")]
mod alsa_delay;
mod game_clock;
mod hal;
mod mixer;
mod music;
#[cfg(target_os = "linux")]
mod pipewire;
mod position;
mod sample;
mod seqlock;
mod tone;
mod wasapi;

pub use game_clock::{ClockError, ClockSnapshot, GameClock, TrackControl};
pub use hal::{
    AudioCallback, AudioError, Backend, CallbackInfo, DeviceInfo, OutputStream, StreamConfig,
    StreamCounters, WasapiMode, devices, open_output,
};
pub use mixer::{
    Mixer, MixerConfig, MixerHandle, MixerStats, PlayError, SAMPLE_CONCURRENCY, VoiceStart, mixer,
};
pub use music::{
    MAX_MUSIC_RATE, MIN_MUSIC_RATE, MUSIC_FADE_MS, MusicAnchor, MusicConfig, MusicError,
    MusicEvent, MusicPlayer, MusicPosition, MusicSnapshot, MusicSource, MusicStats, TrackSource,
    music,
};
pub use position::{AudioPosition, PositionSnapshot};
pub use sample::{MAX_CHANNELS, MAX_SAMPLE_VALUES, Sample, SampleError};
pub use tone::SineTone;
