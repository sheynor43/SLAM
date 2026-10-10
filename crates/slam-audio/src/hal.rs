//! Backend-neutral audio HAL (ADR-0022): device listing, output streams, the callback
//! contract and the playback position.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};

use crate::position::AudioPosition;

/// Audio API used to talk to the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Backend {
    /// PipeWire, Linux. The quantum is requested through `node.latency`.
    #[cfg(target_os = "linux")]
    PipeWire,
}

impl Backend {
    /// Backends compiled into this build, preferred first.
    pub const fn available() -> &'static [Backend] {
        &[
            #[cfg(target_os = "linux")]
            Backend::PipeWire,
        ]
    }
}

/// An output device as reported by the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Backend-specific stable identifier, passed back in [`StreamConfig::device`]
    /// (PipeWire: the node name).
    pub id: String,
    /// Human-readable name for the settings screen.
    pub name: String,
}

/// Requested output stream parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamConfig {
    /// Target device id from [`DeviceInfo::id`]; `None` follows the system default.
    pub device: Option<String>,
    /// Frames per second.
    pub sample_rate: u32,
    /// Interleaved channels per frame.
    pub channels: u16,
    /// Desired frames per callback (PipeWire: the quantum). The backend may deliver other
    /// sizes; the callback must handle any.
    pub buffer_frames: u32,
    /// Stream name shown by the system mixer.
    pub name: String,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            device: None,
            sample_rate: 48_000,
            channels: 2,
            buffer_frames: 128,
            name: "SLAM".to_owned(),
        }
    }
}

/// Timing of the buffer handed to [`AudioCallback::process`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallbackInfo {
    /// Stream frame index of the first frame in the buffer (frames written before it).
    pub frame: u64,
    /// When the backend took the timing report, engine clock base, nanoseconds. Zero
    /// when the backend has no timing yet (the first buffers of a stream); `latency_ns`
    /// is then zero too and meaningless.
    pub timestamp_ns: u64,
    /// Time from `timestamp_ns` until the first frame of the buffer leaves the device.
    /// Valid only when `timestamp_ns` is non-zero.
    pub latency_ns: u64,
    /// Frames per second.
    pub sample_rate: u32,
    /// Interleaved channels per frame.
    pub channels: u16,
}

/// Fills output buffers. Runs on the backend's real-time thread.
///
/// `process` must not allocate, lock, log, do I/O or make system calls. A panic aborts
/// the process (it crosses the C callback boundary).
pub trait AudioCallback: Send + 'static {
    /// Fills `out` completely with interleaved `f32` samples: `out.len()` is a whole
    /// number of frames, possibly different on every call.
    fn process(&mut self, out: &mut [f32], info: &CallbackInfo);
}

/// Audio HAL errors. Never produced on the audio thread.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("audio backend unavailable: {0}")]
    Unavailable(String),
    #[error("invalid stream config: {0}")]
    InvalidConfig(&'static str),
    #[error("audio stream failed: {0}")]
    Stream(String),
}

/// Health counters of an open stream, written by the audio thread (relaxed), read by
/// anyone. The audio thread cannot log; this is how it reports trouble.
#[derive(Debug, Default)]
pub struct StreamCounters {
    /// Callbacks invoked by the backend.
    pub callbacks: AtomicU64,
    /// Callbacks that could not deliver audio (no buffer available, unmapped or
    /// misaligned memory). Each is an audible gap.
    pub missed_buffers: AtomicU64,
    /// The backend dropped or failed the stream after it opened; callbacks have stopped
    /// and the position is stale. Reopen the stream.
    pub failed: AtomicBool,
}

/// Backend side of an open stream; dropping it closes the stream.
pub(crate) trait StreamHandle: Send {}

/// An open output stream. Closes on drop (joins the backend thread; the callback is
/// dropped there).
pub struct OutputStream {
    position: Arc<AudioPosition>,
    counters: Arc<StreamCounters>,
    config: StreamConfig,
    // Dropped after `position` is no longer written: the handle owns the writer.
    _handle: Box<dyn StreamHandle>,
}

impl OutputStream {
    /// Playback position for the update thread; clone the `Arc` to keep it.
    pub fn position(&self) -> &Arc<AudioPosition> {
        &self.position
    }

    /// Health counters of the stream.
    pub fn counters(&self) -> &Arc<StreamCounters> {
        &self.counters
    }

    /// Format the stream was opened with.
    pub fn config(&self) -> &StreamConfig {
        &self.config
    }
}

impl std::fmt::Debug for OutputStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputStream")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

fn validate(config: &StreamConfig) -> Result<(), AudioError> {
    if !(8_000..=384_000).contains(&config.sample_rate) {
        return Err(AudioError::InvalidConfig(
            "sample rate out of 8000..=384000",
        ));
    }
    if !(1..=8).contains(&config.channels) {
        return Err(AudioError::InvalidConfig("channels out of 1..=8"));
    }
    if !(16..=8192).contains(&config.buffer_frames) {
        return Err(AudioError::InvalidConfig("buffer frames out of 16..=8192"));
    }
    Ok(())
}

/// Lists output devices of `backend`.
pub fn devices(backend: Backend) -> Result<Vec<DeviceInfo>, AudioError> {
    match backend {
        #[cfg(target_os = "linux")]
        Backend::PipeWire => crate::pipewire::devices(),
    }
}

/// Opens an output stream on `backend` that pulls audio from `callback`.
///
/// Returns once the stream is connected; the first callbacks follow shortly after.
// Without a backend for the target the match below is empty and diverges.
#[cfg_attr(not(target_os = "linux"), allow(unreachable_code, unused_variables))]
pub fn open_output(
    backend: Backend,
    config: StreamConfig,
    callback: Box<dyn AudioCallback>,
) -> Result<OutputStream, AudioError> {
    validate(&config)?;
    let position = Arc::new(AudioPosition::new());
    let counters = Arc::new(StreamCounters::default());
    let handle: Box<dyn StreamHandle> = match backend {
        #[cfg(target_os = "linux")]
        Backend::PipeWire => Box::new(crate::pipewire::open_output(
            &config,
            callback,
            Arc::clone(&position),
            Arc::clone(&counters),
        )?),
    };
    Ok(OutputStream {
        position,
        counters,
        config,
        _handle: handle,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        validate(&StreamConfig::default()).unwrap();
    }

    #[test]
    fn accepts_range_bounds() {
        for (sample_rate, channels, buffer_frames) in [(8_000, 1, 16), (384_000, 8, 8192)] {
            validate(&StreamConfig {
                sample_rate,
                channels,
                buffer_frames,
                ..Default::default()
            })
            .unwrap();
        }
    }

    #[test]
    fn rejects_out_of_range_config() {
        let bad = [
            StreamConfig {
                sample_rate: 0,
                ..Default::default()
            },
            StreamConfig {
                channels: 0,
                ..Default::default()
            },
            StreamConfig {
                buffer_frames: 1 << 20,
                ..Default::default()
            },
            StreamConfig {
                sample_rate: 7_999,
                ..Default::default()
            },
            StreamConfig {
                channels: 9,
                ..Default::default()
            },
            StreamConfig {
                buffer_frames: 15,
                ..Default::default()
            },
        ];
        for config in bad {
            assert!(matches!(
                validate(&config),
                Err(AudioError::InvalidConfig(_))
            ));
        }
    }
}
