//! Starts and stops the update and draw threads.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use slam_input::EventSource;

use crate::draw::{Draw, DrawLoop};
use crate::limiter::{
    DEFAULT_SPIN_THRESHOLD_NS, FrameLimiter, InvalidRate, LimiterMode, SystemTimer,
};
use crate::snapshot::triple_buffer;
use crate::update::{InvalidUpdateConfig, Update, UpdateLoop, UpdateStats};
use crate::wake::{Waker, wake_pair};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineConfig {
    /// Update rate in Hz, `0 < hz <= MAX_UPDATE_HZ`. Input wakes update in between.
    pub update_hz: f64,
    pub draw: LimiterMode,
    /// Most input events passed to one tick.
    pub event_batch: usize,
    /// Busy-wait before update and draw deadlines.
    pub spin_threshold_ns: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            update_hz: 1000.0,
            draw: LimiterMode::Unlimited,
            event_batch: 1024,
            spin_threshold_ns: DEFAULT_SPIN_THRESHOLD_NS,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Update(#[from] InvalidUpdateConfig),
    #[error(transparent)]
    DrawRate(#[from] InvalidRate),
    #[error("failed to spawn a thread: {0}")]
    Spawn(#[from] std::io::Error),
}

type UpdateHandle<U> = JoinHandle<UpdateLoop<U>>;
type DrawHandle<U, D> = JoinHandle<DrawLoop<<U as Update>::Snapshot, D, SystemTimer>>;

/// Running update and draw threads. Dropping it stops and joins them.
pub struct Engine<U: Update, D: Draw<U::Snapshot>> {
    stop: Arc<AtomicBool>,
    waker: Waker,
    update: Option<UpdateHandle<U>>,
    draw: Option<DrawHandle<U, D>>,
}

/// State of the threads after [`Engine::shutdown`].
pub struct Stopped<U, D> {
    pub update: U,
    pub draw: D,
    pub update_stats: UpdateStats,
    pub frames: u64,
}

impl<U: Update, D: Draw<U::Snapshot>> Engine<U, D> {
    /// Spawns the update thread (drains `source`, ticks `update`) and the draw thread
    /// (draws snapshots with `draw`). Every snapshot slot starts as `initial`.
    pub fn spawn(
        config: EngineConfig,
        update: U,
        draw: D,
        initial: U::Snapshot,
        source: EventSource,
    ) -> Result<Self, EngineError>
    where
        U::Snapshot: Clone,
    {
        let (writer, reader) = triple_buffer(initial);
        let (waker, parker) = wake_pair();
        let mut update_loop = UpdateLoop::new(
            update,
            source,
            parker,
            writer,
            config.update_hz,
            config.event_batch,
            config.spin_threshold_ns,
        )?;
        let mut limiter = FrameLimiter::new(config.draw)?;
        limiter.set_spin_threshold_ns(config.spin_threshold_ns);
        let mut draw_loop = DrawLoop::new(draw, reader, limiter);

        let stop = Arc::new(AtomicBool::new(false));
        let mut engine = Self {
            stop: Arc::clone(&stop),
            waker,
            update: None,
            draw: None,
        };
        engine.update = Some({
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("slam-update".into())
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        update_loop.step();
                    }
                    update_loop
                })?
        });
        // On failure `engine` is dropped, which stops the update thread.
        engine.draw = Some(std::thread::Builder::new().name("slam-draw".into()).spawn(
            move || {
                while !stop.load(Ordering::Acquire) {
                    draw_loop.step();
                }
                draw_loop
            },
        )?);
        Ok(engine)
    }

    /// Wakes update when the input thread queues an event; pass to
    /// [`slam_input::EventSink::set_notify`].
    pub fn waker(&self) -> &Waker {
        &self.waker
    }

    /// Stops both threads and returns their state. A panic in either thread is
    /// resumed here.
    pub fn shutdown(mut self) -> Stopped<U, D> {
        self.request_stop();
        let update = join(self.update.take());
        let draw = join(self.draw.take());
        let update_stats = update.stats();
        let frames = draw.frames();
        Stopped {
            update: update.into_state(),
            draw: draw.into_state(),
            update_stats,
            frames,
        }
    }

    fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        // Update may be asleep until a distant deadline.
        self.waker.notify();
    }
}

fn join<T>(handle: Option<JoinHandle<T>>) -> T {
    // Both handles are set by `spawn` and taken only once, by `shutdown`.
    let handle = handle.expect("thread already joined");
    handle
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

impl<U: Update, D: Draw<U::Snapshot>> Drop for Engine<U, D> {
    fn drop(&mut self) {
        self.request_stop();
        // Panics are dropped here: resuming them in drop could abort.
        if let Some(handle) = self.update.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.draw.take() {
            let _ = handle.join();
        }
    }
}
