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
    /// WASAPI, Windows, in the given mode (ADR-0027). An unsupported mode falls back to
    /// the next less demanding one; [`OutputStream::backend`] reports the mode opened.
    #[cfg(windows)]
    Wasapi(WasapiMode),
}

impl Backend {
    /// Backends compiled into this build, preferred first.
    pub const fn available() -> &'static [Backend] {
        &[
            #[cfg(target_os = "linux")]
            Backend::PipeWire,
            #[cfg(windows)]
            Backend::Wasapi(WasapiMode::LowLatency),
            #[cfg(windows)]
            Backend::Wasapi(WasapiMode::Exclusive),
            #[cfg(windows)]
            Backend::Wasapi(WasapiMode::Shared),
        ]
    }

    /// Stable name for settings and command lines: `pipewire`, `wasapi-shared`,
    /// `wasapi-low-latency`, `wasapi-exclusive`.
    pub const fn name(self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Backend::PipeWire => "pipewire",
            #[cfg(windows)]
            Backend::Wasapi(WasapiMode::Shared) => "wasapi-shared",
            #[cfg(windows)]
            Backend::Wasapi(WasapiMode::LowLatency) => "wasapi-low-latency",
            #[cfg(windows)]
            Backend::Wasapi(WasapiMode::Exclusive) => "wasapi-exclusive",
        }
    }

    /// The available backend called `name` (see [`Backend::name`]).
    pub fn from_name(name: &str) -> Option<Backend> {
        Self::available().iter().copied().find(|b| b.name() == name)
    }
}

/// WASAPI stream mode (ADR-0027), from most to least compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WasapiMode {
    /// Shared through the system mixer with the engine's default period (about 10 ms).
    /// [`StreamConfig::buffer_frames`] is ignored.
    Shared,
    /// Shared with the smallest engine period the driver allows near
    /// [`StreamConfig::buffer_frames`] (`IAudioClient3`). Needs the stream format to
    /// match the system mix format; otherwise falls back to [`WasapiMode::Shared`].
    LowLatency,
    /// Exclusive: bypasses the system mixer, other applications are silent on the
    /// device. Falls back to [`WasapiMode::LowLatency`] if the device or format is not
    /// available exclusively.
    Exclusive,
}

/// An output device as reported by the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Backend-specific stable identifier, passed back in [`StreamConfig::device`]
    /// (PipeWire: the node name; WASAPI: the endpoint id).
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
    /// Desired frames per callback (PipeWire: the quantum; WASAPI: the period). The
    /// backend may deliver other sizes; the callback must handle any. The opened stream
    /// reports the period it got in [`OutputStream::config`] where the backend knows it.
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
    backend: Backend,
    position: Arc<AudioPosition>,
    counters: Arc<StreamCounters>,
    config: StreamConfig,
    // Dropped after `position` is no longer written: the handle owns the writer.
    _handle: Box<dyn StreamHandle>,
}

impl OutputStream {
    /// Backend the stream was opened with; for WASAPI, the mode actually used after
    /// fallback.
    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// Playback position for the update thread; clone the `Arc` to keep it.
    pub fn position(&self) -> &Arc<AudioPosition> {
        &self.position
    }

    /// Health counters of the stream.
    pub fn counters(&self) -> &Arc<StreamCounters> {
        &self.counters
    }

    /// Format the stream was opened with; `buffer_frames` is the period the backend
    /// granted where it knows it (WASAPI).
    pub fn config(&self) -> &StreamConfig {
        &self.config
    }
}

impl std::fmt::Debug for OutputStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputStream")
            .field("backend", &self.backend)
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
        #[cfg(windows)]
        Backend::Wasapi(_) => crate::wasapi::devices(),
    }
}

/// Opens an output stream on `backend` that pulls audio from `callback`.
///
/// Returns once the stream is connected; the first callbacks follow shortly after.
// Without a backend for the target the match below is empty and diverges.
#[cfg_attr(
    not(any(target_os = "linux", windows)),
    allow(unreachable_code, unused_variables)
)]
// Only WASAPI reports the granted period back into the config.
#[cfg_attr(not(windows), allow(unused_mut))]
pub fn open_output(
    backend: Backend,
    mut config: StreamConfig,
    callback: Box<dyn AudioCallback>,
) -> Result<OutputStream, AudioError> {
    validate(&config)?;
    let position = Arc::new(AudioPosition::new());
    let counters = Arc::new(StreamCounters::default());
    let (handle, backend): (Box<dyn StreamHandle>, Backend) = match backend {
        #[cfg(target_os = "linux")]
        Backend::PipeWire => (
            Box::new(crate::pipewire::open_output(
                &config,
                callback,
                Arc::clone(&position),
                Arc::clone(&counters),
            )?),
            backend,
        ),
        #[cfg(windows)]
        Backend::Wasapi(mode) => {
            let opened = crate::wasapi::open_output(
                mode,
                &config,
                callback,
                Arc::clone(&position),
                Arc::clone(&counters),
            )?;
            config.buffer_frames = opened.period_frames;
            let mode = opened.mode;
            (Box::new(opened.stream), Backend::Wasapi(mode))
        }
    };
    Ok(OutputStream {
        backend,
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
    fn backend_names_round_trip() {
        for &backend in Backend::available() {
            assert_eq!(Backend::from_name(backend.name()), Some(backend));
        }
        assert_eq!(Backend::from_name("asio"), None);
    }

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
