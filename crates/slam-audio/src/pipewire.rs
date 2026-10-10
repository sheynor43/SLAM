//! PipeWire backend (ADR-0022).
//!
//! The PipeWire main loop runs on its own thread (`slam-audio-pw`). The stream uses
//! `RT_PROCESS`, so [`process`] runs on PipeWire's real-time data thread, whose priority
//! PipeWire itself raises (module-rt / rtkit).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::param::audio::{AudioFormat, AudioInfoRaw, MAX_CHANNELS};
use pw::spa::pod::{Object, Pod, Value, serialize::PodSerializer};
use pw::stream::{StreamFlags, StreamState};

use crate::hal::{AudioCallback, AudioError, CallbackInfo, DeviceInfo, StreamConfig};
use crate::hal::{StreamCounters, StreamHandle};
use crate::position::{AudioPosition, PositionSnapshot};

/// How long `open_output` waits for the stream to connect and `devices` waits for the
/// registry.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

const SAMPLE_BYTES: usize = size_of::<f32>();

fn unavailable(err: pw::Error) -> AudioError {
    AudioError::Unavailable(format!("PipeWire: {err}"))
}

/// Lists `Audio/Sink` nodes. Connects, waits for one registry round trip, disconnects.
pub(crate) fn devices() -> Result<Vec<DeviceInfo>, AudioError> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(unavailable)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(unavailable)?;
    let core = context.connect_rc(None).map_err(unavailable)?;
    let registry = core.get_registry().map_err(unavailable)?;

    let found = Rc::new(RefCell::new(Vec::new()));
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let found = Rc::clone(&found);
            move |global| {
                if global.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else { return };
                if props.get(*pw::keys::MEDIA_CLASS) != Some("Audio/Sink") {
                    return;
                }
                let Some(id) = props.get(*pw::keys::NODE_NAME) else {
                    return;
                };
                let name = props.get(*pw::keys::NODE_DESCRIPTION).unwrap_or(id);
                found.borrow_mut().push(DeviceInfo {
                    id: id.to_owned(),
                    name: name.to_owned(),
                });
            }
        })
        .register();

    let error = Rc::new(RefCell::new(None));
    let pending = core.sync(0).map_err(unavailable)?;
    let _core_listener = core
        .add_listener_local()
        .done({
            let mainloop = mainloop.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    mainloop.quit();
                }
            }
        })
        .error({
            let mainloop = mainloop.clone();
            let error = Rc::clone(&error);
            move |_id, _seq, _res, message| {
                *error.borrow_mut() = Some(message.to_owned());
                mainloop.quit();
            }
        })
        .register();
    let timeout = mainloop.loop_().add_timer({
        let mainloop = mainloop.clone();
        let error = Rc::clone(&error);
        move |_| {
            *error.borrow_mut() = Some("timed out listing devices".to_owned());
            mainloop.quit();
        }
    });
    let _ = timeout.update_timer(Some(CONNECT_TIMEOUT), None);
    mainloop.run();

    if let Some(message) = error.take() {
        return Err(AudioError::Unavailable(format!("PipeWire: {message}")));
    }
    Ok(found.take())
}

/// Open PipeWire stream: the thread running its main loop.
pub(crate) struct PwStream {
    quit: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl StreamHandle for PwStream {}

impl Drop for PwStream {
    fn drop(&mut self) {
        // Fails only if the loop thread already exited.
        let _ = self.quit.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(crate) fn open_output(
    config: &StreamConfig,
    callback: Box<dyn AudioCallback>,
    position: Arc<AudioPosition>,
    counters: Arc<StreamCounters>,
) -> Result<PwStream, AudioError> {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (quit_tx, quit_rx) = pw::channel::channel();
    let config = config.clone();
    let thread = thread::Builder::new()
        .name("slam-audio-pw".to_owned())
        .spawn(move || {
            let state = Process {
                callback,
                position,
                counters: Arc::clone(&counters),
                frame: 0,
                sample_rate: config.sample_rate,
                channels: config.channels,
                quantum: config.buffer_frames,
            };
            if let Err(err) = run(&config, state, counters, quit_rx, ready_tx.clone()) {
                let _ = ready_tx.send(Err(err));
            }
        })
        .map_err(|err| AudioError::Stream(format!("spawning PipeWire thread: {err}")))?;

    let mut stream = PwStream {
        quit: quit_tx,
        thread: Some(thread),
    };
    match ready_rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(Ok(())) => Ok(stream),
        Ok(Err(err)) => Err(err),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(AudioError::Stream(
            "timed out connecting the stream".to_owned(),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // The thread died without reporting (a panic); surface it.
            let panicked = stream.thread.take().map(|t| t.join().is_err());
            Err(AudioError::Stream(format!(
                "PipeWire thread exited (panicked: {panicked:?})"
            )))
        }
    }
}

/// Runs the main loop until `quit` fires. Reports the connect result on `ready` once.
fn run(
    config: &StreamConfig,
    state: Process,
    counters: Arc<StreamCounters>,
    quit: pw::channel::Receiver<()>,
    ready: mpsc::SyncSender<Result<(), AudioError>>,
) -> Result<(), AudioError> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(unavailable)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(unavailable)?;
    let core = context.connect_rc(None).map_err(unavailable)?;

    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Game",
        *pw::keys::NODE_LATENCY => format!("{}/{}", config.buffer_frames, config.sample_rate),
    };
    if let Some(device) = &config.device {
        props.insert(*pw::keys::TARGET_OBJECT, device.as_str());
    }
    let stream = pw::stream::StreamBox::new(&core, &config.name, props).map_err(unavailable)?;

    // Two listeners: `process` runs on the data thread and `state_changed` on this one,
    // and the wrapper hands each callback `&mut` to its listener's whole state.
    let _process_listener = stream
        .add_local_listener_with_user_data(state)
        .process(process)
        .register()
        .map_err(unavailable)?;
    let mut ready = Some(ready);
    let state_listener = stream
        .add_local_listener::<()>()
        .state_changed(move |_, _, _old, new| match new {
            StreamState::Paused | StreamState::Streaming => {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Ok(()));
                }
            }
            StreamState::Error(message) => {
                counters.failed.store(true, Ordering::Relaxed);
                match ready.take() {
                    Some(ready) => {
                        let _ = ready.send(Err(AudioError::Stream(message)));
                    }
                    None => tracing::error!(%message, "PipeWire stream failed"),
                }
            }
            StreamState::Unconnected => {
                // After a successful connect this means the server dropped the stream
                // (or we are closing it).
                if ready.is_none() {
                    counters.failed.store(true, Ordering::Relaxed);
                }
            }
            StreamState::Connecting => {}
        })
        .register()
        .map_err(unavailable)?;

    let format = format_pod(config)?;
    let mut params = [Pod::from_bytes(&format)
        .ok_or(AudioError::InvalidConfig("unrepresentable stream format"))?];
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(unavailable)?;

    let _quit = quit.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |()| mainloop.quit()
    });
    mainloop.run();
    // Our own disconnect is not a failure: stop watching the state first.
    drop(state_listener);
    // Disconnecting stops the data thread before the callback state is dropped.
    let _ = stream.disconnect();
    Ok(())
}

/// `EnumFormat` pod offering exactly the configured format; PipeWire converts to the
/// device format if they differ.
fn format_pod(config: &StreamConfig) -> Result<Vec<u8>, AudioError> {
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32LE);
    info.set_rate(config.sample_rate);
    info.set_channels(u32::from(config.channels));
    let mut position = [0; MAX_CHANNELS];
    let layout = channel_layout(config.channels);
    position[..layout.len()].copy_from_slice(layout);
    info.set_position(position);
    let value = Value::Object(Object {
        type_: spa::sys::SPA_TYPE_OBJECT_Format,
        id: spa::sys::SPA_PARAM_EnumFormat,
        properties: info.into(),
    });
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &value)
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|_| AudioError::InvalidConfig("unrepresentable stream format"))
}

/// Standard speaker positions for 1..=8 channels (mono, stereo, ..., 7.1).
fn channel_layout(channels: u16) -> &'static [u32] {
    use spa::sys::*;
    const LAYOUT: [u32; 8] = [
        SPA_AUDIO_CHANNEL_FL,
        SPA_AUDIO_CHANNEL_FR,
        SPA_AUDIO_CHANNEL_FC,
        SPA_AUDIO_CHANNEL_LFE,
        SPA_AUDIO_CHANNEL_RL,
        SPA_AUDIO_CHANNEL_RR,
        SPA_AUDIO_CHANNEL_SL,
        SPA_AUDIO_CHANNEL_SR,
    ];
    const MONO: [u32; 1] = [SPA_AUDIO_CHANNEL_MONO];
    match channels {
        1 => &MONO,
        n => &LAYOUT[..usize::from(n).min(LAYOUT.len())],
    }
}

/// State owned by the real-time process callback.
struct Process {
    callback: Box<dyn AudioCallback>,
    position: Arc<AudioPosition>,
    counters: Arc<StreamCounters>,
    /// Stream frames written so far.
    frame: u64,
    sample_rate: u32,
    channels: u16,
    /// Requested frames per callback; used when PipeWire does not say how many it wants.
    quantum: u32,
}

/// Frames to write into a buffer of `capacity_bytes`: what PipeWire requested, or the
/// configured quantum if it did not say (never the whole buffer, which can hold hundreds
/// of milliseconds of unreported latency), bounded by the buffer.
fn frames_to_write(requested: usize, quantum: u32, capacity_bytes: usize, stride: usize) -> usize {
    let capacity = capacity_bytes / stride;
    let wanted = if requested == 0 {
        quantum as usize
    } else {
        requested
    };
    wanted.min(capacity)
}

/// Real-time process callback: fill one buffer. No allocations, locks or system calls.
fn process(stream: &pw::stream::Stream, st: &mut Process) {
    st.counters.callbacks.fetch_add(1, Ordering::Relaxed);
    let Some(mut buffer) = stream.dequeue_buffer() else {
        st.counters.missed_buffers.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let requested = buffer.requested() as usize;
    let Some(data) = buffer.datas_mut().first_mut() else {
        st.counters.missed_buffers.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let stride = SAMPLE_BYTES * usize::from(st.channels);
    let timing = timing(stream, st.sample_rate);

    let frames = match data.data() {
        Some(bytes) => {
            let frames = frames_to_write(requested, st.quantum, bytes.len(), stride);
            // SAFETY: every bit pattern is a valid `f32`; `align_to_mut` only reinterprets
            // the aligned middle part.
            let (head, samples, _) = unsafe { bytes[..frames * stride].align_to_mut::<f32>() };
            // PipeWire maps buffers page-aligned, so this always holds in practice; the
            // check keeps the whole-frames contract if `align_to_mut` ever returns less.
            if head.is_empty() && samples.len() == frames * usize::from(st.channels) {
                let (timestamp_ns, latency_ns) = timing.unwrap_or((0, 0));
                let info = CallbackInfo {
                    frame: st.frame,
                    timestamp_ns,
                    latency_ns,
                    sample_rate: st.sample_rate,
                    channels: st.channels,
                };
                st.callback.process(samples, &info);
                frames
            } else {
                0
            }
        }
        None => 0,
    };
    if frames == 0 {
        st.counters.missed_buffers.fetch_add(1, Ordering::Relaxed);
    }

    // Cannot panic: PipeWire always attaches a chunk to mapped buffer data.
    let chunk = data.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.stride_mut() = stride as i32;
    *chunk.size_mut() = (frames * stride) as u32;

    if frames > 0
        && let Some((timestamp_ns, latency_ns)) = timing
    {
        st.position.publish(PositionSnapshot {
            frame: st.frame,
            timestamp_ns,
            latency_ns,
            sample_rate: st.sample_rate,
        });
    }
    st.frame += frames as u64;
    // `buffer` drops here and is queued back to the stream.
}

/// `(timestamp_ns, latency_ns)` of the buffer being filled: when the report was taken
/// (`CLOCK_MONOTONIC`) and how long until its first frame leaves the device. `None`
/// until the graph reports a valid time.
fn timing(stream: &pw::stream::Stream, sample_rate: u32) -> Option<(u64, u64)> {
    let time = stream.time().ok()?;
    let rate = time.rate();
    if time.now() <= 0 || rate.denom == 0 {
        return None;
    }
    Some((
        time.now() as u64,
        latency_ns(
            time.delay(),
            rate.num,
            rate.denom,
            time.buffered(),
            sample_rate,
        ),
    ))
}

/// Graph delay (`delay · num / denom` seconds) plus frames buffered in the stream's
/// converter (`buffered / sample_rate` seconds), in nanoseconds. A negative delay (user
/// offset) is clamped to zero.
fn latency_ns(delay: i64, num: u32, denom: u32, buffered: u64, sample_rate: u32) -> u64 {
    let graph = i128::from(delay.max(0)) * i128::from(num) * 1_000_000_000 / i128::from(denom);
    let converter = i128::from(buffered) * 1_000_000_000 / i128::from(sample_rate.max(1));
    u64::try_from(graph + converter).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_combines_graph_and_converter() {
        // 240 graph ticks at 1/48000 plus 48 buffered frames at 48 kHz = 6 ms.
        assert_eq!(latency_ns(240, 1, 48_000, 48, 48_000), 6_000_000);
        // Graph and stream rates may differ.
        assert_eq!(latency_ns(441, 1, 44_100, 0, 48_000), 10_000_000);
    }

    #[test]
    fn latency_with_coarse_graph_rate() {
        // 3 ticks of 1/1000 s = 3 ms.
        assert_eq!(latency_ns(3, 1, 1_000, 0, 48_000), 3_000_000);
        // 2 ticks of 3/1000 s = 6 ms.
        assert_eq!(latency_ns(2, 3, 1_000, 0, 48_000), 6_000_000);
    }

    #[test]
    fn huge_latency_saturates() {
        assert_eq!(latency_ns(i64::MAX, u32::MAX, 1, 0, 48_000), u64::MAX);
    }

    #[test]
    fn frames_follow_request_then_quantum_bounded_by_buffer() {
        let stride = 8; // stereo f32
        // Requested wins over the quantum.
        assert_eq!(frames_to_write(64, 128, 8192 * stride, stride), 64);
        // Nothing requested: the quantum, not the whole buffer.
        assert_eq!(frames_to_write(0, 128, 8192 * stride, stride), 128);
        // Both bounded by the buffer.
        assert_eq!(frames_to_write(4096, 128, 100 * stride, stride), 100);
        assert_eq!(frames_to_write(0, 128, 100 * stride, stride), 100);
        // Less than one frame of space.
        assert_eq!(frames_to_write(64, 128, stride - 1, stride), 0);
    }

    #[test]
    fn negative_delay_is_clamped() {
        assert_eq!(latency_ns(-1_000, 1, 48_000, 48, 48_000), 1_000_000);
    }

    #[test]
    fn channel_layouts() {
        assert_eq!(channel_layout(1), &[spa::sys::SPA_AUDIO_CHANNEL_MONO]);
        assert_eq!(
            channel_layout(2),
            &[
                spa::sys::SPA_AUDIO_CHANNEL_FL,
                spa::sys::SPA_AUDIO_CHANNEL_FR
            ]
        );
        assert_eq!(channel_layout(6).len(), 6);
    }

    #[test]
    fn format_pod_parses() {
        let bytes = format_pod(&StreamConfig::default()).unwrap();
        assert!(Pod::from_bytes(&bytes).is_some());
    }
}
