//! Draw loop: paces frames with the limiter and draws the latest snapshot.

use crate::limiter::{FrameLimiter, FrameWait, InvalidRate, LimiterMode, Timer};
use crate::snapshot::SnapshotReader;

/// Rendering run by the draw thread.
pub trait Draw<S>: Send + 'static {
    /// Draws one frame. Must not allocate, lock or do I/O.
    fn frame(&mut self, info: &FrameInfo, snapshot: &S);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    /// Limiter result for this frame.
    pub wait: FrameWait,
    /// The snapshot was published after the one drawn in the previous frame.
    pub fresh: bool,
}

/// The draw thread's state. [`DrawLoop::step`] is one frame.
pub struct DrawLoop<S, D, T: Timer> {
    draw: D,
    reader: SnapshotReader<S>,
    limiter: FrameLimiter<T>,
    frames: u64,
}

impl<S, D: Draw<S>, T: Timer> DrawLoop<S, D, T> {
    pub fn new(draw: D, reader: SnapshotReader<S>, limiter: FrameLimiter<T>) -> Self {
        Self {
            draw,
            reader,
            limiter,
            frames: 0,
        }
    }

    /// Waits for the frame deadline, takes the latest snapshot and draws it. Never
    /// waits for the update thread. Allocation-free.
    pub fn step(&mut self) {
        let wait = self.limiter.wait();
        let (snapshot, fresh) = self.reader.read();
        self.draw.frame(&FrameInfo { wait, fresh }, snapshot);
        self.frames += 1;
    }

    /// Changes the frame rate. The limiter schedule restarts on the next frame.
    pub fn set_mode(&mut self, mode: LimiterMode) -> Result<(), InvalidRate> {
        self.limiter.set_mode(mode)
    }

    pub fn mode(&self) -> LimiterMode {
        self.limiter.mode()
    }

    pub fn state(&self) -> &D {
        &self.draw
    }

    /// Frames drawn since start.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn into_state(self) -> D {
        self.draw
    }
}
