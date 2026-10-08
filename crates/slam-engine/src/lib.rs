//! Threads, clocks, frame pipeline, frame limiter, snapshot exchange.
//!
//! [`Engine`] runs two threads besides the input (OS main) thread: update ticks at a
//! fixed rate and whenever input arrives, writing a snapshot after each tick into a
//! triple buffer; draw paces frames with the [`limiter`] and draws the newest
//! snapshot, never waiting for update.

pub mod clock;
mod draw;
mod engine;
pub mod limiter;
mod snapshot;
mod update;
mod wake;

pub use draw::{Draw, DrawLoop, FrameInfo};
pub use engine::{Engine, EngineConfig, EngineError, RATE_PRESETS, Stopped};
pub use snapshot::{SnapshotReader, SnapshotWriter, triple_buffer};
pub use update::{InvalidUpdateConfig, MAX_UPDATE_HZ, TickInfo, Update, UpdateLoop, UpdateStats};
pub use wake::{Parker, Waker, wake_pair};
