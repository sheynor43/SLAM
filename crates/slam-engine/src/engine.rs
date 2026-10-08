//! Starts and stops the update and draw threads.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use slam_input::EventSource;

use crate::draw::{Draw, DrawLoop};
use crate::limiter::{
    DEFAULT_SPIN_THRESHOLD_NS, FrameLimiter, InvalidRate, LimiterMode, SystemTimer, is_valid_hz,
};
use crate::snapshot::triple_buffer;
use crate::update::{InvalidUpdateConfig, Update, UpdateLoop, UpdateStats, validate_hz};
use crate::wake::{Waker, wake_pair};

/// Update and draw rates offered to the player, in Hz. Any rate between
/// [`MIN_HZ`](crate::limiter::MIN_HZ) and [`MAX_HZ`](crate::limiter::MAX_HZ) is accepted as well.
pub const RATE_PRESETS: [f64; 4] = [1000.0, 2000.0, 4000.0, 8000.0];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineConfig {
    /// Update rate in Hz, `MIN_HZ <= hz <= MAX_UPDATE_HZ`. Input wakes update in between.
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
            draw: LimiterMode::Hz(1000.0),
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
    rates: Arc<Rates>,
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
        // The loops start with these rates; each thread compares the shared ones
        // against them, so a change made before the thread first runs is not lost.
        let (initial_update, initial_draw) = (config.update_hz.to_bits(), encode_mode(config.draw));
        let rates = Arc::new(Rates {
            update: AtomicU64::new(initial_update),
            draw: AtomicU64::new(initial_draw),
        });
        let mut engine = Self {
            stop: Arc::clone(&stop),
            rates: Arc::clone(&rates),
            waker,
            update: None,
            draw: None,
        };
        engine.update = Some({
            let (stop, rates) = (Arc::clone(&stop), Arc::clone(&rates));
            std::thread::Builder::new()
                .name("slam-update".into())
                .spawn(move || {
                    let mut applied = initial_update;
                    while !stop.load(Ordering::Acquire) {
                        let wanted = rates.update.load(Ordering::Relaxed);
                        if wanted != applied {
                            // Validated by `Engine::set_update_hz`.
                            let _ = update_loop.set_hz(f64::from_bits(wanted));
                            applied = wanted;
                        }
                        update_loop.step();
                    }
                    update_loop
                })?
        });
        // On failure `engine` is dropped, which stops the update thread.
        engine.draw = Some(std::thread::Builder::new().name("slam-draw".into()).spawn(
            move || {
                let mut applied = initial_draw;
                while !stop.load(Ordering::Acquire) {
                    let wanted = rates.draw.load(Ordering::Relaxed);
                    if wanted != applied {
                        // Validated by `Engine::set_draw_mode`.
                        let _ = draw_loop.set_mode(decode_mode(wanted));
                        applied = wanted;
                    }
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

    /// Changes the update rate while running. Takes effect at once: a sleeping update
    /// thread is woken and restarts its schedule.
    pub fn set_update_hz(&self, hz: f64) -> Result<(), InvalidUpdateConfig> {
        validate_hz(hz)?;
        self.rates.update.store(hz.to_bits(), Ordering::Relaxed);
        self.waker.notify();
        Ok(())
    }

    pub fn update_hz(&self) -> f64 {
        f64::from_bits(self.rates.update.load(Ordering::Relaxed))
    }

    /// Changes the draw frame rate while running. Takes effect after the frame being
    /// waited for (at most `1 / MIN_HZ`).
    pub fn set_draw_mode(&self, mode: LimiterMode) -> Result<(), InvalidRate> {
        if let LimiterMode::Hz(hz) = mode
            && !is_valid_hz(hz)
        {
            return Err(InvalidRate);
        }
        self.rates.draw.store(encode_mode(mode), Ordering::Relaxed);
        Ok(())
    }

    pub fn draw_mode(&self) -> LimiterMode {
        decode_mode(self.rates.draw.load(Ordering::Relaxed))
    }

    /// Stops both threads and returns their state. A panic in either thread is
    /// resumed here. Draw stops after the frame it is waiting for (at most
    /// `1 / MIN_HZ`).
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

/// Rates requested through the engine handle, polled by each thread once per step.
struct Rates {
    /// `f64` bits of the update rate.
    update: AtomicU64,
    /// See [`encode_mode`].
    draw: AtomicU64,
}

/// `Unlimited` is 0; a rate is its `f64` bits, never 0 since the rate is positive.
fn encode_mode(mode: LimiterMode) -> u64 {
    match mode {
        LimiterMode::Unlimited => 0,
        LimiterMode::Hz(hz) => hz.to_bits(),
    }
}

fn decode_mode(bits: u64) -> LimiterMode {
    if bits == 0 {
        LimiterMode::Unlimited
    } else {
        LimiterMode::Hz(f64::from_bits(bits))
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
