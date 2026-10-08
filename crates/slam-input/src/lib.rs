//! Window and input handling, input event timestamps, input thread.
//!
//! The OS main thread owns the SDL window and blocks in [`InputWindow::run`] until the
//! OS delivers an event. Each keyboard/mouse event is converted to an [`InputEvent`]
//! stamped with the time SDL attached to it, translated into the engine clock by
//! [`TickMapper`], and pushed into a lock-free SPSC ring ([`EventSink`]) that the
//! update thread drains through [`EventSource`]. A [`Notify`] set on the sink wakes the
//! update thread after each queued event. A window made with
//! [`InputWindow::new_opengl`] hands its [`GlContext`] to the draw thread.

mod clock;
mod convert;
mod event;
mod gl;
mod ring;
mod window;

pub use clock::{
    MAX_BRACKET_NS, MAX_REJECTIONS, RECALIBRATE_INTERVAL_NS, RETRY_SPACING_NS, TickMapper,
    sdl_ticks_ns,
};
pub use convert::convert;
pub use event::{InputEvent, InputKind, MouseButton};
pub use gl::GlContext;
pub use ring::{BUTTON_RESERVE, EventSink, EventSource, Notify, event_ring};
pub use window::{InputWindow, WindowError, dispatch};
