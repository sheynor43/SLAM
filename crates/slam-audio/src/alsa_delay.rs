//! ALSA driver delay that PipeWire does not report (ADR-0026).
//!
//! PipeWire's stream delay covers only the ALSA ring buffer. Some drivers hold more audio
//! past the ring: snd-usb-audio keeps 10–15 ms in its URB queue. The kernel reports the
//! full figure as `delay` in `/proc/asound/cardC/pcmDp/subS/status`; the excess over the
//! ring fill is the driver delay. The PipeWire main-loop thread polls it, averages it over
//! a window and publishes it in an atomic that the audio callback reads.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// How often the status file is read. Not a whole number of milliseconds, so successive
/// reads do not keep hitting the same phase of the driver's queue sawtooth (URB periods
/// of 1 ms, graph quanta of 2.5 or 2.67 ms).
pub(crate) const POLL_INTERVAL: Duration = Duration::from_micros(50_321);

/// Samples averaged into the published value: about one second at [`POLL_INTERVAL`].
const WINDOW: usize = 20;

/// Samples needed before anything is published for a sink, so that one raw sample does
/// not stand for the mean.
const MIN_SAMPLES: usize = 5;

/// ALSA playback PCM behind a PipeWire sink node (its `alsa.card`, `alsa.device` and
/// `alsa.subdevice` properties).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AlsaPcm {
    pub card: u32,
    pub device: u32,
    pub subdevice: u32,
}

impl AlsaPcm {
    /// From node properties; `None` if the node is not backed by an ALSA PCM.
    pub(crate) fn from_props<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Option<Self> {
        let num = |key| get(key).and_then(|v: &str| v.trim().parse().ok());
        Some(Self {
            card: num("alsa.card")?,
            device: num("alsa.device").unwrap_or(0),
            subdevice: num("alsa.subdevice").unwrap_or(0),
        })
    }

    fn dir(self) -> PathBuf {
        PathBuf::from(format!(
            "/proc/asound/card{}/pcm{}p/sub{}",
            self.card, self.device, self.subdevice
        ))
    }
}

/// Fields of a PCM `status` file that matter here, in device frames. All come from one
/// kernel snapshot; `hw_ptr` and `appl_ptr` in the same file are read live after it and
/// may already have moved, so they are not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PcmStatus {
    pub running: bool,
    pub delay: i64,
    pub avail: u64,
    /// When the PCM was last started (`trigger_time`, as written). A new value means
    /// it was reopened, possibly with another rate or buffer size.
    pub trigger: Option<(u64, u32)>,
}

impl PcmStatus {
    /// Delay held by the driver past the ring buffer, in device frames:
    /// `delay − (buffer_size − avail)`, which is the kernel's `runtime->delay`. `None`
    /// unless the PCM is running and consistent with `buffer_size`: `avail` above it
    /// means an underrun, and a delay below the ring fill cannot come from one snapshot,
    /// so `buffer_size` is stale.
    pub(crate) fn driver_frames(self, buffer_size: u64) -> Option<u64> {
        if !self.running || self.avail > buffer_size {
            return None;
        }
        let ring = buffer_size - self.avail;
        u64::try_from(self.delay).ok()?.checked_sub(ring)
    }
}

/// Parses `/proc/asound/.../status` (`key : value` lines). `None` for a closed PCM or an
/// unexpected format.
pub(crate) fn parse_status(text: &str) -> Option<PcmStatus> {
    let mut state = None;
    let mut delay = None;
    let mut avail = None;
    let mut trigger = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "state" => state = Some(value),
            "delay" => delay = value.parse().ok(),
            "avail" => avail = value.parse().ok(),
            "trigger_time" => {
                trigger = value
                    .split_once('.')
                    .and_then(|(s, ns)| Some((s.parse().ok()?, ns.parse().ok()?)));
            }
            _ => {}
        }
    }
    Some(PcmStatus {
        running: state? == "RUNNING",
        delay: delay?,
        avail: avail?,
        trigger,
    })
}

/// Device rate and ring buffer size from `/proc/asound/.../hw_params`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HwParams {
    pub rate: u32,
    pub buffer_size: u64,
}

/// Parses `/proc/asound/.../hw_params` (`rate: 48000 (48000/1)`, `buffer_size: 32768`).
/// `None` for a closed PCM.
pub(crate) fn parse_hw_params(text: &str) -> Option<HwParams> {
    let mut rate = None;
    let mut buffer_size = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let first = value.split_whitespace().next();
        match key.trim() {
            "rate" => rate = first.and_then(|v| v.parse().ok()),
            "buffer_size" => buffer_size = first.and_then(|v| v.parse().ok()),
            _ => {}
        }
    }
    Some(HwParams {
        rate: rate.filter(|&r| r > 0)?,
        buffer_size: buffer_size?,
    })
}

/// Sink our node feeds: the input node of a link whose output node is `own`, if that
/// sink is an ALSA PCM. Links are `(output node, input node)`. With several such sinks,
/// the lowest node id wins, so the choice does not depend on map order.
pub(crate) fn resolve_sink(
    own: Option<u32>,
    links: &HashMap<u32, (u32, u32)>,
    sinks: &HashMap<u32, AlsaPcm>,
) -> Option<(u32, AlsaPcm)> {
    let own = own?;
    links
        .values()
        .filter(|(out, _)| *out == own)
        .filter_map(|(_, input)| sinks.get(input).map(|pcm| (*input, *pcm)))
        .min_by_key(|(id, _)| *id)
}

/// Mean of the last [`WINDOW`] samples.
#[derive(Debug, Default)]
pub(crate) struct DelayWindow {
    samples: [u64; WINDOW],
    len: usize,
    next: usize,
}

impl DelayWindow {
    pub(crate) fn push(&mut self, value: u64) {
        self.samples[self.next] = value;
        self.next = (self.next + 1) % WINDOW;
        self.len = (self.len + 1).min(WINDOW);
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn mean(&self) -> Option<u64> {
        if self.len == 0 {
            return None;
        }
        let sum: u128 = self.samples[..self.len]
            .iter()
            .map(|&v| u128::from(v))
            .sum();
        u64::try_from(sum / self.len as u128).ok()
    }

    pub(crate) fn clear(&mut self) {
        self.len = 0;
        self.next = 0;
    }
}

/// Frames at `rate` in nanoseconds.
fn frames_to_ns(frames: u64, rate: u32) -> u64 {
    u64::try_from(u128::from(frames) * 1_000_000_000 / u128::from(rate.max(1))).unwrap_or(u64::MAX)
}

/// Graph state seen through the registry and the poll that turns it into a published
/// driver delay. Lives on the PipeWire main-loop thread.
#[derive(Debug)]
pub(crate) struct DriverDelayTracker {
    /// Our stream's node id, once connected.
    pub own_node: Option<u32>,
    /// Link id → (output node, input node).
    pub links: HashMap<u32, (u32, u32)>,
    /// Sink node id → its ALSA PCM.
    pub sinks: HashMap<u32, AlsaPcm>,
    current: Option<(u32, AlsaPcm)>,
    /// Read when the PCM starts; forgotten (with the window) whenever it is not running
    /// or its trigger time changes, since PipeWire may reopen it with another rate or
    /// buffer size under the same node.
    hw_params: Option<HwParams>,
    /// Trigger time of the run `hw_params` belong to.
    trigger: Option<(u64, u32)>,
    window: DelayWindow,
    text: String,
    published: Arc<AtomicU64>,
}

impl DriverDelayTracker {
    pub(crate) fn new(published: Arc<AtomicU64>) -> Self {
        Self {
            own_node: None,
            links: HashMap::new(),
            sinks: HashMap::new(),
            current: None,
            hw_params: None,
            trigger: None,
            window: DelayWindow::default(),
            text: String::new(),
            published,
        }
    }

    /// Forgets a removed registry object.
    pub(crate) fn remove(&mut self, id: u32) {
        self.links.remove(&id);
        self.sinks.remove(&id);
    }

    /// Starts the window over and rereads `hw_params` on the next poll. The published
    /// value stays until the new window fills.
    fn restart(&mut self) {
        self.hw_params = None;
        self.trigger = None;
        self.window.clear();
    }

    /// One poll from the real `/proc/asound` files.
    pub(crate) fn poll(&mut self) {
        self.tick(|path, out| {
            out.clear();
            File::open(path)
                .and_then(|mut f| f.read_to_string(out))
                .is_ok()
        });
    }

    /// One poll: follows the sink, reads its files through `read` (fills the string,
    /// returns `false` on failure) and publishes the window mean in nanoseconds once the
    /// window holds [`MIN_SAMPLES`]. Until then (after start or a sink change) the
    /// published delay is 0; a value from another device would be no better.
    pub(crate) fn tick(&mut self, mut read: impl FnMut(&Path, &mut String) -> bool) {
        let sink = resolve_sink(self.own_node, &self.links, &self.sinks);
        if sink != self.current {
            self.current = sink;
            self.window.clear();
            self.hw_params = None;
            self.published.store(0, Ordering::Relaxed);
            if let Some((id, pcm)) = sink {
                tracing::debug!(node = id, ?pcm, "audio sink is an ALSA PCM");
            }
        }
        let Some((_, pcm)) = self.current else {
            return;
        };
        let dir = pcm.dir();
        let status = if read(&dir.join("status"), &mut self.text) {
            parse_status(&self.text).filter(|s| s.running)
        } else {
            None
        };
        let Some(status) = status else {
            // Closed, stopped or unreadable: the next run may use other parameters.
            self.restart();
            return;
        };
        if self.hw_params.is_none() || status.trigger != self.trigger {
            self.restart();
            self.trigger = status.trigger;
            if read(&dir.join("hw_params"), &mut self.text) {
                self.hw_params = parse_hw_params(&self.text);
            }
        }
        let Some(hw) = self.hw_params else {
            return;
        };
        let Some(frames) = status.driver_frames(hw.buffer_size) else {
            self.restart();
            return;
        };
        self.window.push(frames);
        if self.window.len() >= MIN_SAMPLES
            && let Some(mean) = self.window.mean()
        {
            self.published
                .store(frames_to_ns(mean, hw.rate), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live UR12 reading: ring fill 32768 − 32451 = 317, so 971 − 317 = 654 frames past it.
    const RUNNING: &str = "state: RUNNING
owner_pid   : 1234
trigger_time: 100.123456789
tstamp      : 101.000000000
delay       : 971
avail       : 32451
avail_max   : 32500
-----
hw_ptr      : 99971
appl_ptr    : 100288
";

    const HW_PARAMS: &str = "access: MMAP_INTERLEAVED
format: S32_LE
subformat: STD
channels: 2
rate: 48000 (48000/1)
period_size: 512
buffer_size: 32768
";

    /// 654 frames at 48 kHz.
    const RUNNING_NS: u64 = 13_625_000;

    fn status(delay: i64, avail: u64) -> String {
        RUNNING
            .replace("delay       : 971", &format!("delay       : {delay}"))
            .replace("avail       : 32451", &format!("avail       : {avail}"))
    }

    #[test]
    fn parses_running_status() {
        let status = parse_status(RUNNING).unwrap();
        assert_eq!(
            status,
            PcmStatus {
                running: true,
                delay: 971,
                avail: 32_451,
                trigger: Some((100, 123_456_789)),
            }
        );
        assert_eq!(status.driver_frames(32_768), Some(654));
    }

    #[test]
    fn closed_or_stopped_pcm_gives_nothing() {
        assert_eq!(parse_status("closed\n"), None);
        let prepared = RUNNING.replace("RUNNING", "PREPARED");
        let status = parse_status(&prepared).unwrap();
        assert!(!status.running);
        assert_eq!(status.driver_frames(32_768), None);
    }

    #[test]
    fn inconsistent_status_is_skipped() {
        let base = PcmStatus {
            running: true,
            delay: 900,
            avail: 32_000,
            trigger: None,
        };
        // Underrun: more space than the buffer.
        let underrun = PcmStatus {
            avail: 40_000,
            ..base
        };
        assert_eq!(underrun.driver_frames(32_768), None);
        let negative = PcmStatus { delay: -5, ..base };
        assert_eq!(negative.driver_frames(32_768), None);
        // A delay below the ring fill is impossible in one snapshot: stale buffer size.
        let small = PcmStatus { delay: 80, ..base };
        assert_eq!(small.driver_frames(32_768), None);
        assert_eq!(small.driver_frames(32_000), Some(80));
        // No driver queue: the delay is exactly the ring fill.
        let exact = PcmStatus { delay: 768, ..base };
        assert_eq!(exact.driver_frames(32_768), Some(0));
    }

    #[test]
    fn parses_hw_params() {
        assert_eq!(
            parse_hw_params(HW_PARAMS),
            Some(HwParams {
                rate: 48_000,
                buffer_size: 32_768
            })
        );
        assert_eq!(parse_hw_params("closed\n"), None);
        assert_eq!(parse_hw_params("rate: 48000 (48000/1)\n"), None);
    }

    #[test]
    fn pcm_from_props() {
        let props: HashMap<&str, &str> = [("alsa.card", "1"), ("alsa.device", "3")]
            .into_iter()
            .collect();
        assert_eq!(
            AlsaPcm::from_props(|k| props.get(k).copied()),
            Some(AlsaPcm {
                card: 1,
                device: 3,
                subdevice: 0
            })
        );
        assert_eq!(AlsaPcm::from_props(|_| None), None);
        assert_eq!(
            AlsaPcm {
                card: 1,
                device: 0,
                subdevice: 0
            }
            .dir(),
            PathBuf::from("/proc/asound/card1/pcm0p/sub0")
        );
    }

    const PCM1: AlsaPcm = AlsaPcm {
        card: 1,
        device: 0,
        subdevice: 0,
    };
    const PCM2: AlsaPcm = AlsaPcm {
        card: 2,
        device: 0,
        subdevice: 0,
    };

    #[test]
    fn sink_follows_links_from_own_node() {
        let sinks: HashMap<u32, AlsaPcm> = [(43, PCM1)].into_iter().collect();
        let mut links = HashMap::new();
        assert_eq!(resolve_sink(Some(90), &links, &sinks), None);
        // Another client's link to the same sink does not count.
        links.insert(1, (77, 43));
        assert_eq!(resolve_sink(Some(90), &links, &sinks), None);
        links.insert(2, (90, 43));
        assert_eq!(resolve_sink(Some(90), &links, &sinks), Some((43, PCM1)));
        assert_eq!(resolve_sink(None, &links, &sinks), None);
        // A non-ALSA sink (not in `sinks`) gives nothing.
        links.insert(2, (90, 50));
        assert_eq!(resolve_sink(Some(90), &links, &sinks), None);
    }

    #[test]
    fn several_sinks_pick_lowest_id() {
        let sinks: HashMap<u32, AlsaPcm> = [(43, PCM1), (30, PCM2)].into_iter().collect();
        let links: HashMap<u32, (u32, u32)> = [(1, (90, 43)), (2, (90, 30))].into_iter().collect();
        assert_eq!(resolve_sink(Some(90), &links, &sinks), Some((30, PCM2)));
    }

    #[test]
    fn window_averages_last_samples() {
        let mut window = DelayWindow::default();
        assert_eq!(window.mean(), None);
        window.push(10);
        window.push(20);
        assert_eq!(window.mean(), Some(15));
        for _ in 0..WINDOW {
            window.push(100);
        }
        assert_eq!(window.mean(), Some(100));
        assert_eq!(window.len(), WINDOW);
        window.clear();
        assert_eq!(window.mean(), None);
    }

    fn tracker_on_sink() -> (DriverDelayTracker, Arc<AtomicU64>) {
        let published = Arc::new(AtomicU64::new(0));
        let mut tracker = DriverDelayTracker::new(Arc::clone(&published));
        tracker.own_node = Some(90);
        tracker.sinks.insert(43, PCM1);
        tracker.links.insert(5, (90, 43));
        (tracker, published)
    }

    /// Fake `/proc`: `hw_params` and `status` texts; `None` makes the read fail. Counts
    /// `hw_params` reads.
    struct FakeProc {
        hw_params: Option<String>,
        status: Option<String>,
        hw_reads: usize,
    }

    impl FakeProc {
        fn new(status: &str) -> Self {
            Self {
                hw_params: Some(HW_PARAMS.to_owned()),
                status: Some(status.to_owned()),
                hw_reads: 0,
            }
        }

        fn read(&mut self) -> impl FnMut(&Path, &mut String) -> bool + '_ {
            move |path, out| {
                out.clear();
                let text = match path.file_name().and_then(|n| n.to_str()) {
                    Some("hw_params") => {
                        self.hw_reads += 1;
                        &self.hw_params
                    }
                    Some("status") => &self.status,
                    _ => return false,
                };
                match text {
                    Some(text) => {
                        out.push_str(text);
                        true
                    }
                    None => false,
                }
            }
        }
    }

    fn ticks(tracker: &mut DriverDelayTracker, proc_: &mut FakeProc, n: usize) {
        for _ in 0..n {
            tracker.tick(proc_.read());
        }
    }

    #[test]
    fn tracker_publishes_after_min_samples() {
        let (mut tracker, published) = tracker_on_sink();
        let mut proc_ = FakeProc::new(RUNNING);
        ticks(&mut tracker, &mut proc_, MIN_SAMPLES - 1);
        assert_eq!(published.load(Ordering::Relaxed), 0);
        ticks(&mut tracker, &mut proc_, 1);
        assert_eq!(published.load(Ordering::Relaxed), RUNNING_NS);
        // hw_params is read once while the PCM keeps running.
        assert_eq!(proc_.hw_reads, 1);
    }

    #[test]
    fn tracker_averages_samples() {
        let (mut tracker, published) = tracker_on_sink();
        // Ring fill 317: driver delays 600 and 700 frames alternate, mean 650.
        let mut low = FakeProc::new(&status(917, 32_451));
        let mut high = FakeProc::new(&status(1017, 32_451));
        for _ in 0..MIN_SAMPLES * 2 {
            tracker.tick(low.read());
            tracker.tick(high.read());
        }
        // 650 frames at 48 kHz.
        assert_eq!(published.load(Ordering::Relaxed), 13_541_666);
    }

    #[test]
    fn tracker_resets_when_sink_goes_away() {
        let (mut tracker, published) = tracker_on_sink();
        let mut proc_ = FakeProc::new(RUNNING);
        ticks(&mut tracker, &mut proc_, MIN_SAMPLES);
        assert_ne!(published.load(Ordering::Relaxed), 0);
        tracker.remove(5);
        ticks(&mut tracker, &mut proc_, 1);
        assert_eq!(published.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn tracker_switching_sinks_drops_old_samples() {
        let (mut tracker, published) = tracker_on_sink();
        let mut old = FakeProc::new(RUNNING);
        ticks(&mut tracker, &mut old, WINDOW);
        tracker.sinks.insert(44, PCM2);
        tracker.links.insert(5, (90, 44));
        // New device: 100 frames past the ring.
        let mut new = FakeProc::new(&status(417, 32_451));
        ticks(&mut tracker, &mut new, 1);
        assert_eq!(published.load(Ordering::Relaxed), 0);
        ticks(&mut tracker, &mut new, MIN_SAMPLES - 1);
        // 100 frames at 48 kHz, no trace of the old 654.
        assert_eq!(published.load(Ordering::Relaxed), 2_083_333);
    }

    #[test]
    fn tracker_retries_unreadable_hw_params() {
        let (mut tracker, published) = tracker_on_sink();
        let mut proc_ = FakeProc::new(RUNNING);
        proc_.hw_params = None;
        ticks(&mut tracker, &mut proc_, MIN_SAMPLES);
        assert_eq!(published.load(Ordering::Relaxed), 0);
        proc_.hw_params = Some(HW_PARAMS.to_owned());
        ticks(&mut tracker, &mut proc_, MIN_SAMPLES);
        assert_eq!(published.load(Ordering::Relaxed), RUNNING_NS);
    }

    #[test]
    fn tracker_rereads_hw_params_after_a_stop() {
        let (mut tracker, published) = tracker_on_sink();
        let mut proc_ = FakeProc::new(RUNNING);
        ticks(&mut tracker, &mut proc_, MIN_SAMPLES);
        // The PCM closes and PipeWire reopens it at 44.1 kHz under the same node.
        proc_.status = Some("closed\n".to_owned());
        ticks(&mut tracker, &mut proc_, 1);
        // The last value is kept while it is closed.
        assert_eq!(published.load(Ordering::Relaxed), RUNNING_NS);
        proc_.status = Some(RUNNING.to_owned());
        proc_.hw_params = Some(HW_PARAMS.replace("48000 (48000/1)", "44100 (44100/1)"));
        ticks(&mut tracker, &mut proc_, MIN_SAMPLES - 1);
        assert_eq!(proc_.hw_reads, 2);
        assert_eq!(published.load(Ordering::Relaxed), RUNNING_NS);
        ticks(&mut tracker, &mut proc_, 1);
        // 654 frames at 44.1 kHz.
        assert_eq!(published.load(Ordering::Relaxed), 14_829_931);
    }

    #[test]
    fn tracker_rereads_hw_params_on_a_new_trigger() {
        let (mut tracker, published) = tracker_on_sink();
        let mut proc_ = FakeProc::new(RUNNING);
        ticks(&mut tracker, &mut proc_, MIN_SAMPLES);
        // Reopened at 44.1 kHz between two polls: never seen stopped, new trigger time.
        proc_.status = Some(RUNNING.replace("100.123456789", "102.000000001"));
        proc_.hw_params = Some(HW_PARAMS.replace("48000 (48000/1)", "44100 (44100/1)"));
        ticks(&mut tracker, &mut proc_, MIN_SAMPLES - 1);
        assert_eq!(proc_.hw_reads, 2);
        assert_eq!(published.load(Ordering::Relaxed), RUNNING_NS);
        ticks(&mut tracker, &mut proc_, 1);
        assert_eq!(published.load(Ordering::Relaxed), 14_829_931);
    }

    #[test]
    fn tracker_restarts_on_underrun() {
        let (mut tracker, published) = tracker_on_sink();
        let mut proc_ = FakeProc::new(RUNNING);
        ticks(&mut tracker, &mut proc_, MIN_SAMPLES);
        proc_.status = Some(status(971, 40_000));
        ticks(&mut tracker, &mut proc_, 1);
        // Value held, parameters reread on the next poll.
        assert_eq!(published.load(Ordering::Relaxed), RUNNING_NS);
        proc_.status = Some(status(417, 32_451));
        ticks(&mut tracker, &mut proc_, MIN_SAMPLES);
        assert_eq!(proc_.hw_reads, 2);
        // 100 frames at 48 kHz from a fresh window.
        assert_eq!(published.load(Ordering::Relaxed), 2_083_333);
    }
}
