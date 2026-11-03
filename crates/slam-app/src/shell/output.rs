//! The audio output's thread (ADR-0089). It owns the open output: when the output settings
//! change, it closes the old output and opens the new one, and it lists the output devices
//! for the settings screen. Opening, closing and listing block, so the update thread only
//! trades requests and results with it through rings.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use slam_audio::{DeviceInfo, MusicSnapshot, PositionSnapshot};
use slam_ui::widgets::DropdownView;

use crate::settings::Audio as AudioSettings;

/// Requests and results in flight each way: the update thread keeps at most one open and
/// one list in flight, and retires one output per switch.
const RING: usize = 4;

/// How often the thread looks for requests.
const POLL: Duration = Duration::from_millis(10);

/// Opens outputs and lists devices; generic for tests.
pub trait Opener: Send + 'static {
    /// The update thread's handles of an open output.
    type Audio: Send + 'static;
    /// What keeps the output open until it drops.
    type Stream: Send + 'static;

    fn open(&mut self, settings: &AudioSettings) -> Result<(Self::Audio, Self::Stream), String>;

    /// The output devices of the backend `settings` pick.
    fn devices(&mut self, settings: &AudioSettings) -> Vec<DeviceInfo>;

    /// The settings screen's device dropdown over `devices` with `saved` the chosen device;
    /// `None` in tests.
    fn view(&mut self, devices: &[DeviceInfo], saved: Option<&str>) -> Option<DropdownView>;
}

/// What the update thread asks for.
pub enum Request<A> {
    /// Close the output and open one with these settings.
    Open(Box<AudioSettings>),
    /// List the devices of the backend these settings pick; the device they name is the
    /// chosen one.
    List(Box<AudioSettings>),
    /// Free an output's update-side handles.
    Retire(Box<A>),
}

/// The answer to [`Request::Open`].
pub enum Opened<A> {
    /// The output the settings asked for.
    Asked(Box<A>),
    /// It did not open: the one before it again.
    Previous(Box<A>),
    /// Neither opened: no output until the next change.
    Lost,
}

/// The answer to [`Request::List`].
pub struct DeviceList {
    pub devices: Vec<DeviceInfo>,
    /// The chosen device the list was made for: the dropdown has it as an option even when
    /// it is not listed.
    pub saved: Option<String>,
}

/// The update thread's end.
pub struct OutputLink<A> {
    requests: rtrb::Producer<Request<A>>,
    opened: rtrb::Consumer<Opened<A>>,
    lists: rtrb::Consumer<Box<DeviceList>>,
}

impl<A> OutputLink<A> {
    /// Queues `request`; gives it back when the ring is full.
    pub fn request(&mut self, request: Request<A>) -> Result<(), Request<A>> {
        self.requests.push(request).map_err(|e| match e {
            rtrb::PushError::Full(request) => request,
        })
    }

    /// The answer to an open, if it came. Does not allocate.
    pub fn opened(&mut self) -> Option<Opened<A>> {
        self.opened.pop().ok()
    }

    /// The newest device list, if one came; older ones are dropped. Does not allocate,
    /// frees only when lists overtook each other.
    pub fn list(&mut self) -> Option<Box<DeviceList>> {
        let mut latest = None;
        while let Ok(list) = self.lists.pop() {
            latest = Some(list);
        }
        latest
    }
}

/// The draw thread's end: new device dropdowns in, replaced ones back to be freed.
pub struct DeviceViews {
    views: rtrb::Consumer<DropdownView>,
    old: rtrb::Producer<DropdownView>,
}

impl DeviceViews {
    /// The newest device dropdown, if one arrived. Does not allocate.
    pub fn take(&mut self) -> Option<DropdownView> {
        let mut latest = None;
        while let Ok(view) = self.views.pop() {
            if let Some(older) = latest.replace(view) {
                self.give_back(older);
            }
        }
        latest
    }

    /// Hands a replaced dropdown to the output thread to free. With its ring full the view
    /// is freed here: more lists than the ring holds within 10 ms.
    pub fn give_back(&mut self, view: DropdownView) {
        let _ = self.old.push(view);
    }
}

/// The `slam-audio-output` thread; closes the output and joins on drop.
pub struct OutputThread {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for OutputThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The thread's state and its ends of the rings.
struct Worker<O: Opener> {
    opener: O,
    stream: Option<O::Stream>,
    /// The settings `stream` was opened with.
    current: AudioSettings,
    requests: rtrb::Consumer<Request<O::Audio>>,
    opened: rtrb::Producer<Opened<O::Audio>>,
    lists: rtrb::Producer<Box<DeviceList>>,
    views: rtrb::Producer<DropdownView>,
    old_views: rtrb::Consumer<DropdownView>,
}

impl<O: Opener> Worker<O> {
    /// Serves the requests waiting; frees the dropdowns that came back.
    fn serve(&mut self) {
        while self.old_views.pop().is_ok() {}
        while let Ok(request) = self.requests.pop() {
            match request {
                Request::Open(settings) => {
                    let opened = self.reopen(*settings);
                    // The update thread keeps one open in flight: there is room.
                    let _ = self.opened.push(opened);
                }
                Request::List(settings) => {
                    let devices = self.opener.devices(&settings);
                    let saved = settings.device;
                    if let Some(view) = self.opener.view(&devices, saved.as_deref()) {
                        let _ = self.views.push(view);
                    }
                    let _ = self.lists.push(Box::new(DeviceList { devices, saved }));
                }
                Request::Retire(audio) => drop(audio),
            }
        }
    }

    /// Closes the output, then opens one with `settings`, else the previous one again: an
    /// exclusive output holds its device until it closes.
    fn reopen(&mut self, settings: AudioSettings) -> Opened<O::Audio> {
        drop(self.stream.take());
        match self.opener.open(&settings) {
            Ok((audio, stream)) => {
                self.stream = Some(stream);
                self.current = settings;
                return Opened::Asked(Box::new(audio));
            }
            Err(e) => tracing::warn!("audio: cannot open the chosen output: {e}"),
        }
        match self.opener.open(&self.current) {
            Ok((audio, stream)) => {
                self.stream = Some(stream);
                Opened::Previous(Box::new(audio))
            }
            Err(e) => {
                tracing::warn!("audio: cannot open the previous output again: {e}");
                Opened::Lost
            }
        }
    }
}

/// Starts the `slam-audio-output` thread with `stream`, opened with `current`.
pub fn spawn<O: Opener>(
    opener: O,
    stream: O::Stream,
    current: AudioSettings,
) -> std::io::Result<(OutputLink<O::Audio>, DeviceViews, OutputThread)> {
    let (requests_tx, requests) = rtrb::RingBuffer::new(RING);
    let (opened, opened_rx) = rtrb::RingBuffer::new(RING);
    let (lists, lists_rx) = rtrb::RingBuffer::new(RING);
    let (views, views_rx) = rtrb::RingBuffer::new(RING);
    let (old_views_tx, old_views) = rtrb::RingBuffer::new(RING);
    let mut worker = Worker {
        opener,
        stream: Some(stream),
        current,
        requests,
        opened,
        lists,
        views,
        old_views,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("slam-audio-output".to_owned())
        .spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                worker.serve();
                std::thread::sleep(POLL);
            }
            // The output closes here, after the update thread stopped its sounds.
        })?;
    Ok((
        OutputLink {
            requests: requests_tx,
            opened: opened_rx,
            lists: lists_rx,
        },
        DeviceViews {
            views: views_rx,
            old: old_views_tx,
        },
        OutputThread {
            stop,
            thread: Some(thread),
        },
    ))
}

/// The thread's ends of an [`OutputLink`], for tests that play the thread.
#[cfg(test)]
pub struct TestEnds<A> {
    pub requests: rtrb::Consumer<Request<A>>,
    pub opened: rtrb::Producer<Opened<A>>,
    pub lists: rtrb::Producer<Box<DeviceList>>,
}

/// A link without a thread.
#[cfg(test)]
pub fn test_link<A>() -> (OutputLink<A>, TestEnds<A>) {
    let (requests_tx, requests) = rtrb::RingBuffer::new(RING);
    let (opened, opened_rx) = rtrb::RingBuffer::new(RING);
    let (lists, lists_rx) = rtrb::RingBuffer::new(RING);
    (
        OutputLink {
            requests: requests_tx,
            opened: opened_rx,
            lists: lists_rx,
        },
        TestEnds {
            requests,
            opened,
            lists,
        },
    )
}

/// Whether `settings` ask for another output than `requested`. Does not allocate.
pub fn output_changed(settings: &AudioSettings, requested: &AudioSettings) -> bool {
    settings.backend != requested.backend
        || settings.device != requested.device
        || settings.buffer_frames != requested.buffer_frames
}

/// Where a closed output left the track: the track frame last handed to the device and
/// whether it was moving. `None` without a track or once it ended.
pub fn resume_point(
    output: Option<PositionSnapshot>,
    music: Option<MusicSnapshot>,
) -> Option<(u64, bool)> {
    let (output, music) = (output?, music?);
    if music.ended {
        return None;
    }
    let frame = music.track_frame_at(output.frame as f64).max(0.0);
    Some((frame as u64, music.anchor.moves()))
}

#[cfg(test)]
mod tests;
