//! The `slam-thumbnails` thread (ADR-0127): takes the update thread's requests, newest
//! first, makes the thumbnails (reads files, decodes, writes the cache) and hands the
//! pixels to the library thread, which owns the menu's texture requests.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use slam_render::image::Image;

use super::make::Maker;
use crate::songs::SongFiles;

/// How long the thread sleeps when it has nothing to do.
const POLL: Duration = Duration::from_millis(5);
/// Requests the thread keeps; past this the oldest is dropped.
pub const QUEUE: usize = 32;

/// What the update thread asks for: the thumbnail of `files[set][difficulty]`'s
/// background, `height` pixels tall.
pub struct ThumbRequest {
    pub id: u32,
    pub height: u32,
    pub files: Arc<SongFiles>,
    pub set: usize,
    pub difficulty: usize,
}

/// What became of a request.
pub enum Made {
    Image(Image),
    /// No background, or it cannot be read.
    Missing,
    /// Never looked at: the queue overflowed. The update thread may ask again.
    Dropped,
}

pub struct WorkerOut {
    pub id: u32,
    pub made: Made,
}

/// The requests waiting for the thread: newest taken first, the oldest dropped on
/// overflow.
#[derive(Default)]
pub struct Queue {
    waiting: Vec<ThumbRequest>,
}

impl Queue {
    pub fn new() -> Queue {
        Queue {
            waiting: Vec::with_capacity(QUEUE),
        }
    }

    /// Adds `request`; returns the id of the oldest one dropped to make room.
    pub fn push(&mut self, request: ThumbRequest) -> Option<u32> {
        let dropped = (self.waiting.len() == QUEUE).then(|| self.waiting.remove(0).id);
        self.waiting.push(request);
        dropped
    }

    /// The newest request.
    pub fn pop(&mut self) -> Option<ThumbRequest> {
        self.waiting.pop()
    }

    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }
}

/// The thread; ends and is joined on drop.
pub struct ThumbWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ThumbWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts the thread.
pub fn spawn(
    maker: Maker,
    requests: rtrb::Consumer<ThumbRequest>,
    out: rtrb::Producer<WorkerOut>,
) -> std::io::Result<ThumbWorker> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("slam-thumbnails".into())
        .spawn(move || run(&maker, requests, out, &flag))?;
    Ok(ThumbWorker {
        stop,
        thread: Some(thread),
    })
}

fn run(
    maker: &Maker,
    mut requests: rtrb::Consumer<ThumbRequest>,
    mut out: rtrb::Producer<WorkerOut>,
    stop: &AtomicBool,
) {
    let mut queue = Queue::new();
    let mut outbox: VecDeque<WorkerOut> = VecDeque::new();
    while !stop.load(Ordering::Relaxed) {
        while let Some(front) = outbox.pop_front() {
            if let Err(rtrb::PushError::Full(front)) = out.push(front) {
                outbox.push_front(front);
                break;
            }
        }
        // A full outbox waits: the library thread is busy, and more images would only
        // pile up here.
        if outbox.len() >= 8 {
            std::thread::sleep(POLL);
            continue;
        }
        while let Ok(request) = requests.pop() {
            if let Some(id) = queue.push(request) {
                outbox.push_back(WorkerOut {
                    id,
                    made: Made::Dropped,
                });
            }
        }
        let Some(request) = queue.pop() else {
            std::thread::sleep(POLL);
            continue;
        };
        let made = request
            .files
            .get(request.set)
            .and_then(|set| set.get(request.difficulty))
            .and_then(|file| {
                let name = file.background.as_deref()?;
                maker.make(file.path.parent()?, name, request.height)
            })
            .map_or(Made::Missing, Made::Image);
        outbox.push_back(WorkerOut {
            id: request.id,
            made,
        });
    }
}
