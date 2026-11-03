use std::sync::Mutex;

use slam_audio::MusicAnchor;

use super::*;
use crate::settings::Settings;

type Log = Arc<Mutex<Vec<String>>>;

/// Opens what `fails` does not name; records every open and close.
struct Fake {
    fails: Vec<Option<String>>,
    log: Arc<Mutex<Vec<String>>>,
}

/// Logs its close.
struct FakeStream {
    device: Option<String>,
    log: Arc<Mutex<Vec<String>>>,
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        self.log
            .lock()
            .unwrap()
            .push(format!("close {:?}", self.device));
    }
}

impl Opener for Fake {
    type Audio = Option<String>;
    type Stream = FakeStream;

    fn open(&mut self, settings: &AudioSettings) -> Result<(Self::Audio, FakeStream), String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("open {:?}", settings.device));
        if self.fails.contains(&settings.device) {
            return Err("busy".to_owned());
        }
        Ok((
            settings.device.clone(),
            FakeStream {
                device: settings.device.clone(),
                log: Arc::clone(&self.log),
            },
        ))
    }

    fn devices(&mut self, _: &AudioSettings) -> Vec<DeviceInfo> {
        vec![DeviceInfo {
            id: "hdmi".to_owned(),
            name: "HDMI".to_owned(),
        }]
    }

    fn view(&mut self, _: &[DeviceInfo], _: Option<&str>) -> Option<DropdownView> {
        None
    }
}

fn audio(device: Option<&str>) -> AudioSettings {
    AudioSettings {
        device: device.map(str::to_owned),
        ..Settings::default().audio
    }
}

/// A worker with the output of `device` open, and the update thread's end.
fn worker(
    device: Option<&str>,
    fails: &[Option<&str>],
) -> (Worker<Fake>, OutputLink<Option<String>>, Log) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (requests_tx, requests) = rtrb::RingBuffer::new(RING);
    let (opened, opened_rx) = rtrb::RingBuffer::new(RING);
    let (lists, lists_rx) = rtrb::RingBuffer::new(RING);
    let (views, _views_rx) = rtrb::RingBuffer::new(RING);
    let (_old_views_tx, old_views) = rtrb::RingBuffer::new(RING);
    let worker = Worker {
        opener: Fake {
            fails: fails.iter().map(|d| d.map(str::to_owned)).collect(),
            log: Arc::clone(&log),
        },
        stream: Some(FakeStream {
            device: device.map(str::to_owned),
            log: Arc::clone(&log),
        }),
        current: audio(device),
        requests,
        opened,
        lists,
        views,
        old_views,
    };
    let link = OutputLink {
        requests: requests_tx,
        opened: opened_rx,
        lists: lists_rx,
    };
    (worker, link, log)
}

fn open(link: &mut OutputLink<Option<String>>, device: Option<&str>) {
    assert!(link.request(Request::Open(Box::new(audio(device)))).is_ok());
}

#[test]
fn the_old_output_closes_before_the_new_one_opens() {
    let (mut worker, mut link, log) = worker(Some("speakers"), &[]);
    open(&mut link, Some("hdmi"));
    worker.serve();
    assert!(matches!(link.opened(), Some(Opened::Asked(a)) if a.as_deref() == Some("hdmi")));
    assert_eq!(
        *log.lock().unwrap(),
        ["close Some(\"speakers\")", "open Some(\"hdmi\")"]
    );
    assert_eq!(worker.current.device.as_deref(), Some("hdmi"));
}

#[test]
fn an_output_that_does_not_open_gives_the_previous_one_again() {
    let (mut worker, mut link, log) = worker(Some("speakers"), &[Some("hdmi")]);
    open(&mut link, Some("hdmi"));
    worker.serve();
    assert!(matches!(link.opened(), Some(Opened::Previous(a)) if a.as_deref() == Some("speakers")));
    assert_eq!(
        log.lock().unwrap().last().unwrap(),
        "open Some(\"speakers\")"
    );
    assert_eq!(worker.current.device.as_deref(), Some("speakers"));
    assert!(worker.stream.is_some());
}

#[test]
fn without_either_output_there_is_none() {
    let (mut worker, mut link, _) = worker(Some("speakers"), &[Some("hdmi"), Some("speakers")]);
    open(&mut link, Some("hdmi"));
    worker.serve();
    assert!(matches!(link.opened(), Some(Opened::Lost)));
    assert!(worker.stream.is_none());
    // The next change opens from nothing.
    open(&mut link, None);
    worker.serve();
    assert!(matches!(link.opened(), Some(Opened::Asked(a)) if a.is_none()));
}

#[test]
fn a_list_carries_the_device_it_was_made_for() {
    let (mut worker, mut link, _) = worker(None, &[]);
    assert!(
        link.request(Request::List(Box::new(audio(Some("gone")))))
            .is_ok()
    );
    worker.serve();
    let list = link.list().expect("a list");
    assert_eq!(list.devices[0].id, "hdmi");
    assert_eq!(list.saved.as_deref(), Some("gone"));
    assert!(link.list().is_none());
}

#[test]
fn retired_handles_are_freed_on_the_thread() {
    let (mut worker, mut link, _) = worker(None, &[]);
    assert!(
        link.request(Request::Retire(Box::new(Some("old".to_owned()))))
            .is_ok()
    );
    worker.serve();
    assert!(link.opened().is_none());
}

#[test]
fn the_thread_closes_the_output_when_it_stops() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let stream = FakeStream {
        device: None,
        log: Arc::clone(&log),
    };
    let opener = Fake {
        fails: Vec::new(),
        log: Arc::clone(&log),
    };
    let (_link, _views, thread) = spawn(opener, stream, audio(None)).expect("thread");
    drop(thread);
    assert_eq!(*log.lock().unwrap(), ["close None"]);
}

#[test]
fn only_the_backend_device_and_buffer_change_the_output() {
    let base = audio(None);
    let louder = AudioSettings {
        master_volume: 0.1,
        ..base.clone()
    };
    assert!(!output_changed(&louder, &base));
    assert!(output_changed(&audio(Some("hdmi")), &base));
    let buffer = AudioSettings {
        buffer_frames: base.buffer_frames * 2,
        ..base.clone()
    };
    assert!(output_changed(&buffer, &base));
    let backend = AudioSettings {
        backend: Some("pipewire".to_owned()),
        ..base.clone()
    };
    assert!(output_changed(&backend, &base));
}

fn snapshots(anchor: MusicAnchor, ended: bool) -> (PositionSnapshot, MusicSnapshot) {
    (
        PositionSnapshot {
            frame: 48_000,
            timestamp_ns: 1,
            latency_ns: 0,
            sample_rate: 48_000,
        },
        MusicSnapshot {
            anchor,
            previous: anchor,
            generation: 1,
            ended,
        },
    )
}

#[test]
fn a_moving_track_resumes_where_the_old_output_left_it() {
    let (output, music) = snapshots(
        MusicAnchor {
            device_frame: 24_000,
            track_frame: 1_000.0,
            speed: 1.5,
            accel: 0.0,
        },
        false,
    );
    assert_eq!(
        resume_point(Some(output), Some(music)),
        Some((1_000 + 36_000, true))
    );
}

#[test]
fn a_paused_track_stays_paused_at_its_frame() {
    let (output, music) = snapshots(
        MusicAnchor {
            device_frame: 24_000,
            track_frame: 5_000.0,
            speed: 0.0,
            accel: 0.0,
        },
        false,
    );
    assert_eq!(
        resume_point(Some(output), Some(music)),
        Some((5_000, false))
    );
}

#[test]
fn an_ended_or_missing_track_does_not_resume() {
    let (output, music) = snapshots(MusicAnchor::default(), true);
    assert_eq!(resume_point(Some(output), Some(music)), None);
    assert_eq!(resume_point(None, Some(music)), None);
    assert_eq!(resume_point(Some(output), None), None);
}
