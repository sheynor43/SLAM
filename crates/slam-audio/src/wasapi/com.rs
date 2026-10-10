//! WASAPI through COM: device listing, stream setup per mode and the render loop.
//!
//! Each stream owns a thread (`slam-audio-wasapi`) in the COM multithreaded apartment,
//! registered with MMCSS as "Pro Audio". It sets the stream up, reports the result to
//! `open_output`, then waits on the stream event and fills one buffer per wake-up.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_INVALIDATED,
    AUDCLNT_SHAREMODE_EXCLUSIVE, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    DEVICE_STATE_ACTIVE, IAudioClient, IAudioClient3, IAudioClock, IAudioRenderClient, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    WAVEFORMATEXTENSIBLE_0, eConsole, eRender,
};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
    WaitForSingleObject,
};
use windows::core::{HSTRING, Interface, PWSTR, w};

use super::{
    EnginePeriods, SampleFormat, aligned_period_hns, channel_mask, convert, exclusive_period_hns,
    fallback_chain, frames_from_hns, latency_ns, low_latency_period, qpc_hns_to_ns,
    shared_frames_to_write,
};
use crate::hal::{AudioCallback, AudioError, CallbackInfo, DeviceInfo, StreamConfig};
use crate::hal::{StreamCounters, StreamHandle, WasapiMode};
use crate::position::{AudioPosition, PositionSnapshot};

/// How long `open_output` waits for the stream thread to set the stream up.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest wait for a buffer event before the thread checks for shutdown and probes the
/// device.
const EVENT_TIMEOUT_MS: u32 = 100;

/// Initialises COM (multithreaded apartment) on the current thread for its lifetime.
/// Declare it before any COM object so it drops last.
struct Com;

impl Com {
    fn init() -> windows::core::Result<Self> {
        // SAFETY: no reserved pointer; balanced by `CoUninitialize` in `Drop`.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
        Ok(Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: paired with the successful `CoInitializeEx` in `init` on this thread.
        unsafe { CoUninitialize() };
    }
}

/// Registers the current thread with MMCSS as "Pro Audio" until dropped.
struct Mmcss(HANDLE);

impl Mmcss {
    fn join() -> windows::core::Result<Self> {
        let mut task_index = 0;
        // SAFETY: a valid task name and out pointer.
        let handle = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index) }?;
        Ok(Self(handle))
    }
}

impl Drop for Mmcss {
    fn drop(&mut self) {
        // SAFETY: the handle came from `AvSetMmThreadCharacteristicsW` on this thread.
        let _ = unsafe { AvRevertMmThreadCharacteristics(self.0) };
    }
}

/// Auto-reset event signalled by the audio engine when a buffer is due.
struct Event(HANDLE);

impl Event {
    fn new() -> windows::core::Result<Self> {
        // SAFETY: default security, auto-reset, initially unsignalled, unnamed.
        let handle = unsafe { CreateEventW(None, false, false, None) }?;
        Ok(Self(handle))
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: we own the handle; the audio client that signals it is dropped first.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn unavailable(err: windows::core::Error) -> AudioError {
    AudioError::Unavailable(format!("WASAPI: {err}"))
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    // SAFETY: plain COM activation on a thread with COM initialised.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

/// Copies a COM-allocated wide string and frees it.
fn take_pwstr(s: PWSTR) -> String {
    // SAFETY: `s` is a NUL-terminated string allocated by COM, freed exactly once here.
    unsafe {
        let text = s.to_string().unwrap_or_default();
        CoTaskMemFree(Some(s.0 as *const _));
        text
    }
}

/// Lists active render endpoints. Runs COM on a short-lived thread so the caller's
/// apartment (an SDL window thread, say) is not touched.
pub(crate) fn devices() -> Result<Vec<DeviceInfo>, AudioError> {
    thread::scope(|scope| {
        scope
            .spawn(|| {
                let _com = Com::init().map_err(unavailable)?;
                list_devices().map_err(unavailable)
            })
            .join()
            .unwrap_or_else(|_| {
                Err(AudioError::Unavailable(
                    "WASAPI: device listing panicked".to_owned(),
                ))
            })
    })
}

fn list_devices() -> windows::core::Result<Vec<DeviceInfo>> {
    let enumerator = enumerator()?;
    // SAFETY: COM calls on valid interfaces; every out value is owned by the wrappers.
    unsafe {
        let collection = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let count = collection.GetCount()?;
        let mut devices = Vec::with_capacity(count as usize);
        for i in 0..count {
            let device = collection.Item(i)?;
            let id = take_pwstr(device.GetId()?);
            let name = friendly_name(&device).unwrap_or_else(|_| id.clone());
            devices.push(DeviceInfo { id, name });
        }
        Ok(devices)
    }
}

fn friendly_name(device: &IMMDevice) -> windows::core::Result<String> {
    // SAFETY: COM calls on a valid device; the PROPVARIANT is cleared once, the string
    // freed once.
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ)?;
        let mut value = store.GetValue(&PKEY_Device_FriendlyName)?;
        let text = PropVariantToStringAlloc(&value).map(take_pwstr);
        let _ = PropVariantClear(&mut value);
        text
    }
}

/// An open WASAPI stream: its thread. Dropping it stops the stream and joins the thread.
pub(crate) struct WasapiStream {
    quit: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl StreamHandle for WasapiStream {}

impl Drop for WasapiStream {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Result of [`open_output`]: the stream, the mode it runs in after fallback and the
/// period granted, in frames.
pub(crate) struct Opened {
    pub stream: WasapiStream,
    pub mode: WasapiMode,
    pub period_frames: u32,
}

pub(crate) fn open_output(
    mode: WasapiMode,
    config: &StreamConfig,
    callback: Box<dyn AudioCallback>,
    position: Arc<AudioPosition>,
    counters: Arc<StreamCounters>,
) -> Result<Opened, AudioError> {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let quit = Arc::new(AtomicBool::new(false));
    let config = config.clone();
    let thread = thread::Builder::new()
        .name("slam-audio-wasapi".to_owned())
        .spawn({
            let quit = Arc::clone(&quit);
            move || {
                stream_thread(
                    mode, &config, callback, &position, &counters, &quit, ready_tx,
                )
            }
        })
        .map_err(|err| AudioError::Stream(format!("spawning WASAPI thread: {err}")))?;

    let mut stream = WasapiStream {
        quit,
        thread: Some(thread),
    };
    match ready_rx.recv_timeout(OPEN_TIMEOUT) {
        Ok(Ok((mode, period_frames))) => Ok(Opened {
            stream,
            mode,
            period_frames,
        }),
        Ok(Err(err)) => Err(err),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // A driver stuck in `Activate`/`Initialize` would block a join: detach the
            // thread instead. It sees `quit` once setup returns and exits by itself.
            stream.quit.store(true, Ordering::Relaxed);
            drop(stream.thread.take());
            Err(AudioError::Stream(
                "timed out opening the stream".to_owned(),
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let panicked = stream.thread.take().map(|t| t.join().is_err());
            Err(AudioError::Stream(format!(
                "WASAPI thread exited (panicked: {panicked:?})"
            )))
        }
    }
}

/// Marks the stream failed if the thread unwinds (a panicking callback).
struct FailOnPanic<'a>(&'a StreamCounters);

impl Drop for FailOnPanic<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            self.0.failed.store(true, Ordering::Relaxed);
        }
    }
}

type Ready = mpsc::SyncSender<Result<(WasapiMode, u32), AudioError>>;

fn stream_thread(
    mode: WasapiMode,
    config: &StreamConfig,
    callback: Box<dyn AudioCallback>,
    position: &AudioPosition,
    counters: &StreamCounters,
    quit: &AtomicBool,
    ready: Ready,
) {
    let _fail_on_panic = FailOnPanic(counters);
    let _com = match Com::init() {
        Ok(com) => com,
        Err(err) => {
            let _ = ready.send(Err(unavailable(err)));
            return;
        }
    };
    let _mmcss = Mmcss::join()
        .inspect_err(|err| tracing::warn!(%err, "MMCSS \"Pro Audio\" unavailable"))
        .ok();
    let session = match open_session(mode, config) {
        Ok(session) => session,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    let mut render = Render {
        callback,
        position,
        counters,
        scratch: Vec::new(),
        written: 0,
        rate: config.sample_rate,
        channels: config.channels,
    };
    if session.format != SampleFormat::F32 {
        render.scratch = vec![0.0; session.buffer_frames as usize * usize::from(config.channels)];
    }
    if let Err(err) = session.start(&mut render) {
        let _ = ready.send(Err(AudioError::Stream(format!("WASAPI: starting: {err}"))));
        return;
    }
    let _ = ready.send(Ok((session.mode, session.period_frames)));

    let result = session.run(&mut render, quit);
    // SAFETY: COM call on a valid client.
    let _ = unsafe { session.client.Stop() };
    if let Err(err) = result {
        counters.failed.store(true, Ordering::Relaxed);
        tracing::error!(%err, "WASAPI stream failed");
    }
}

/// An initialised audio client with its services.
struct Session {
    mode: WasapiMode,
    format: SampleFormat,
    /// Endpoint buffer size, frames.
    buffer_frames: u32,
    /// Frames the engine consumes per event.
    period_frames: u32,
    clock_frequency: u64,
    render: IAudioRenderClient,
    clock: IAudioClock,
    client: IAudioClient,
    // Dropped after the client that signals it.
    event: Event,
}

/// Opens the device and initialises the first mode of the fallback chain that works.
fn open_session(mode: WasapiMode, config: &StreamConfig) -> Result<Session, AudioError> {
    let enumerator = enumerator().map_err(unavailable)?;
    // SAFETY: COM calls on a valid enumerator; the id string outlives the call.
    let device = unsafe {
        match &config.device {
            Some(id) => enumerator.GetDevice(&HSTRING::from(id.as_str())),
            None => enumerator.GetDefaultAudioEndpoint(eRender, eConsole),
        }
    }
    .map_err(unavailable)?;

    let mut failures = Vec::new();
    for &candidate in fallback_chain(mode) {
        match init_mode(&device, candidate, config) {
            Ok(session) => return Ok(session),
            Err(err) => {
                tracing::warn!(mode = ?candidate, %err, "WASAPI mode unavailable");
                failures.push(format!("{candidate:?}: {err}"));
            }
        }
    }
    Err(AudioError::Unavailable(format!(
        "WASAPI: {}",
        failures.join("; ")
    )))
}

/// `WAVEFORMATEXTENSIBLE` for interleaved `format` samples.
fn wave_format(format: SampleFormat, rate: u32, channels: u16) -> WAVEFORMATEXTENSIBLE {
    let container = format.container_bytes() as u16;
    let block_align = channels * container;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * u32::from(block_align),
            nBlockAlign: block_align,
            wBitsPerSample: container * 8,
            cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: format.valid_bits(),
        },
        dwChannelMask: channel_mask(channels),
        SubFormat: if format.is_float() {
            KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            KSDATAFORMAT_SUBTYPE_PCM
        },
    }
}

fn as_wave_format(format: &WAVEFORMATEXTENSIBLE) -> *const WAVEFORMATEX {
    (format as *const WAVEFORMATEXTENSIBLE).cast()
}

/// Error for a mode that cannot serve the request (the chain moves on).
fn mode_error(message: String) -> windows::core::Error {
    windows::core::Error::new(windows::core::HRESULT(0x8000_4005_u32 as i32), message)
}

fn init_mode(
    device: &IMMDevice,
    mode: WasapiMode,
    config: &StreamConfig,
) -> windows::core::Result<Session> {
    let rate = config.sample_rate;
    let channels = config.channels;
    // SAFETY (whole block): COM calls on valid interfaces with pointers to live locals.
    unsafe {
        let (client, format, period_frames) = match mode {
            WasapiMode::Shared => {
                let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
                let wave = wave_format(SampleFormat::F32, rate, channels);
                let mut default_hns = 0;
                client.GetDevicePeriod(Some(&mut default_hns), None)?;
                // The engine converts rate and channels to its mix format.
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    0,
                    0,
                    as_wave_format(&wave),
                    None,
                )?;
                (
                    client,
                    SampleFormat::F32,
                    frames_from_hns(default_hns, rate),
                )
            }
            WasapiMode::LowLatency => {
                let client: IAudioClient3 = device.Activate(CLSCTX_ALL, None)?;
                // Small periods need the engine's own format: no conversion in between.
                let mix = client.GetMixFormat()?;
                let (mix_rate, mix_channels) = {
                    let mix = mix.read_unaligned();
                    (mix.nSamplesPerSec, mix.nChannels)
                };
                CoTaskMemFree(Some(mix as *const _));
                if (mix_rate, mix_channels) != (rate, channels) {
                    return Err(mode_error(format!(
                        "mix format is {mix_rate} Hz × {mix_channels} ch, stream wants \
                         {rate} Hz × {channels} ch"
                    )));
                }
                let wave = wave_format(SampleFormat::F32, rate, channels);
                let mut engine = EnginePeriods {
                    default: 0,
                    fundamental: 0,
                    min: 0,
                    max: 0,
                };
                client.GetSharedModeEnginePeriod(
                    as_wave_format(&wave),
                    &mut engine.default,
                    &mut engine.fundamental,
                    &mut engine.min,
                    &mut engine.max,
                )?;
                let period = low_latency_period(config.buffer_frames, engine);
                // Fails (and the chain falls back to Shared) if the mix format is not
                // float or has another channel mask: only rate and channels are checked.
                client.InitializeSharedAudioStream(
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                    period,
                    as_wave_format(&wave),
                    None,
                )?;
                (client.cast::<IAudioClient>()?, SampleFormat::F32, period)
            }
            WasapiMode::Exclusive => {
                let mut client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
                let format = SampleFormat::EXCLUSIVE_PREFERENCE
                    .into_iter()
                    .find(|&f| {
                        let wave = wave_format(f, rate, channels);
                        client
                            .IsFormatSupported(
                                AUDCLNT_SHAREMODE_EXCLUSIVE,
                                as_wave_format(&wave),
                                None,
                            )
                            .is_ok()
                    })
                    .ok_or_else(|| {
                        mode_error(format!("no exclusive format for {rate} Hz × {channels} ch"))
                    })?;
                let wave = wave_format(format, rate, channels);
                let mut min_hns = 0;
                client.GetDevicePeriod(None, Some(&mut min_hns))?;
                let period_hns = exclusive_period_hns(config.buffer_frames, rate, min_hns);
                let init = |client: &IAudioClient, period_hns| {
                    client.Initialize(
                        AUDCLNT_SHAREMODE_EXCLUSIVE,
                        AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                        period_hns,
                        period_hns,
                        as_wave_format(&wave),
                        None,
                    )
                };
                if let Err(err) = init(&client, period_hns) {
                    if err.code() != AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED {
                        return Err(err);
                    }
                    // The size the driver can do; retry on a fresh client.
                    let aligned = client.GetBufferSize()?;
                    client = device.Activate(CLSCTX_ALL, None)?;
                    init(&client, aligned_period_hns(aligned, rate))?;
                }
                // Event-driven exclusive streams consume the whole buffer per event.
                let period = client.GetBufferSize()?;
                (client, format, period)
            }
        };

        let event = Event::new()?;
        client.SetEventHandle(event.0)?;
        let render: IAudioRenderClient = client.GetService()?;
        let clock: IAudioClock = client.GetService()?;
        let clock_frequency = clock.GetFrequency()?;
        let buffer_frames = client.GetBufferSize()?;
        Ok(Session {
            mode,
            format,
            buffer_frames,
            period_frames: period_frames.clamp(1, buffer_frames.max(1)),
            clock_frequency,
            render,
            clock,
            client,
            event,
        })
    }
}

/// State of the render loop: the callback and its bookkeeping.
struct Render<'a> {
    callback: Box<dyn AudioCallback>,
    position: &'a AudioPosition,
    counters: &'a StreamCounters,
    /// `f32` staging for integer device formats, sized for the whole buffer up front.
    scratch: Vec<f32>,
    /// Stream frames handed to the device so far.
    written: u64,
    rate: u32,
    channels: u16,
}

impl Session {
    /// Frames to write now.
    fn frames_due(&self) -> windows::core::Result<u32> {
        if self.mode == WasapiMode::Exclusive {
            return Ok(self.buffer_frames);
        }
        // SAFETY: COM call on a valid client.
        let padding = unsafe { self.client.GetCurrentPadding() }?;
        Ok(shared_frames_to_write(
            self.buffer_frames,
            padding,
            self.period_frames,
        ))
    }

    /// Pre-fills the buffer with silence (the engine reads it right after `Start`) and
    /// starts the stream.
    fn start(&self, render: &mut Render) -> windows::core::Result<()> {
        let frames = self.frames_due()?;
        // SAFETY: COM calls on valid interfaces; the buffer is released unwritten with
        // the silent flag.
        unsafe {
            if frames > 0 {
                self.render.GetBuffer(frames)?;
                self.render
                    .ReleaseBuffer(frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
            }
            render.written += u64::from(frames);
            self.client.Start()
        }
    }

    /// Render loop: one buffer per event until `quit` is set or the device fails.
    fn run(&self, render: &mut Render, quit: &AtomicBool) -> windows::core::Result<()> {
        while !quit.load(Ordering::Relaxed) {
            // SAFETY: a valid event handle owned by `self`.
            let woke = unsafe { WaitForSingleObject(self.event.0, EVENT_TIMEOUT_MS) };
            if woke == WAIT_FAILED {
                return Err(windows::core::Error::from_thread());
            }
            if woke != WAIT_OBJECT_0 {
                // No event for a while: an invalidated device stops signalling, so ask.
                // SAFETY: COM call on a valid client.
                unsafe { self.client.GetCurrentPadding() }?;
                continue;
            }
            render.counters.callbacks.fetch_add(1, Ordering::Relaxed);
            self.fill(render)?;
        }
        Ok(())
    }

    /// Fills one buffer. No allocations, locks or logging; only WASAPI calls. (Error
    /// paths build a `windows::core::Error`, which queries `GetErrorInfo`: a COM call,
    /// no Rust allocation, and never on the steady path.)
    fn fill(&self, render: &mut Render) -> windows::core::Result<()> {
        let frames = self.frames_due()?;
        if frames == 0 {
            return Ok(());
        }
        // SAFETY: COM call on a valid render client.
        let data = match unsafe { self.render.GetBuffer(frames) } {
            Ok(data) => data,
            Err(err) if err.code() == AUDCLNT_E_DEVICE_INVALIDATED => return Err(err),
            Err(_) => {
                render
                    .counters
                    .missed_buffers
                    .fetch_add(1, Ordering::Relaxed);
                // Exclusive: the device plays the stale buffer anyway and its position
                // moves on; count those frames so the latency stays right.
                if self.mode == WasapiMode::Exclusive {
                    render.written += u64::from(frames);
                }
                return Ok(());
            }
        };

        let (timestamp_ns, latency_ns) = self.timing(render.written, render.rate);
        let info = CallbackInfo {
            frame: render.written,
            timestamp_ns,
            latency_ns,
            sample_rate: render.rate,
            channels: render.channels,
        };
        let samples = frames as usize * usize::from(render.channels);
        let delivered = if self.format == SampleFormat::F32 {
            if data.align_offset(align_of::<f32>()) == 0 {
                // SAFETY: WASAPI hands out `frames` whole frames of `channels` `f32`
                // samples, writable until `ReleaseBuffer`; alignment checked above.
                let out = unsafe { std::slice::from_raw_parts_mut(data.cast::<f32>(), samples) };
                render.callback.process(out, &info);
                true
            } else {
                false
            }
        } else {
            let staged = &mut render.scratch[..samples];
            render.callback.process(staged, &info);
            // SAFETY: the buffer holds `frames` frames of the negotiated format, writable
            // until `ReleaseBuffer`.
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(data, samples * self.format.container_bytes())
            };
            convert(staged, bytes, self.format);
            true
        };
        let flags = if delivered {
            0
        } else {
            render
                .counters
                .missed_buffers
                .fetch_add(1, Ordering::Relaxed);
            AUDCLNT_BUFFERFLAGS_SILENT.0 as u32
        };
        // SAFETY: releases exactly the frames obtained above.
        unsafe { self.render.ReleaseBuffer(frames, flags) }?;

        if delivered && timestamp_ns != 0 {
            render.position.publish(PositionSnapshot {
                frame: render.written,
                timestamp_ns,
                latency_ns,
                sample_rate: render.rate,
            });
        }
        render.written += u64::from(frames);
        Ok(())
    }

    /// `(timestamp_ns, latency_ns)` for stream frame `frame` from the audio clock; zeros
    /// if the clock has no report.
    fn timing(&self, frame: u64, rate: u32) -> (u64, u64) {
        let mut device_position = 0;
        let mut qpc_hns = 0;
        // SAFETY: COM call with valid out pointers.
        let ok = unsafe {
            self.clock
                .GetPosition(&mut device_position, Some(&mut qpc_hns))
        }
        .is_ok();
        if !ok || qpc_hns == 0 {
            return (0, 0);
        }
        (
            qpc_hns_to_ns(qpc_hns),
            latency_ns(frame, device_position, self.clock_frequency, rate),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_format_fields() {
        let f32_stereo = wave_format(SampleFormat::F32, 48_000, 2);
        let (tag, block, avg, bits, cb) = {
            let f = f32_stereo.Format;
            (
                f.wFormatTag,
                f.nBlockAlign,
                f.nAvgBytesPerSec,
                f.wBitsPerSample,
                f.cbSize,
            )
        };
        assert_eq!(tag, WAVE_FORMAT_EXTENSIBLE as u16);
        assert_eq!((block, avg, bits, cb), (8, 384_000, 32, 22));
        let sub = f32_stereo.SubFormat;
        assert_eq!(sub, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
        let mask = f32_stereo.dwChannelMask;
        assert_eq!(mask, 0x3);

        let i24 = wave_format(SampleFormat::I24In32, 44_100, 6);
        let (block, avg, bits) = {
            let f = i24.Format;
            (f.nBlockAlign, f.nAvgBytesPerSec, f.wBitsPerSample)
        };
        // SAFETY: the union was written as `wValidBitsPerSample`.
        let valid = unsafe { i24.Samples.wValidBitsPerSample };
        assert_eq!((block, avg, bits, valid), (24, 44_100 * 24, 32, 24));
        let sub = i24.SubFormat;
        assert_eq!(sub, KSDATAFORMAT_SUBTYPE_PCM);

        let packed = wave_format(SampleFormat::I24, 48_000, 2);
        let (block, bits) = {
            let f = packed.Format;
            (f.nBlockAlign, f.wBitsPerSample)
        };
        assert_eq!((block, bits), (6, 24));
    }
}
