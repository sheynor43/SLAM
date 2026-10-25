//! Window and input handling, input event timestamps, input thread.
//!
//! The OS main thread owns the SDL window and blocks in [`InputWindow::run`] until the
//! OS delivers an event. Each keyboard/mouse event is converted to an [`InputEvent`]
//! stamped with the time SDL attached to it, translated into the engine clock by
//! [`TickMapper`], and pushed into a lock-free SPSC ring ([`EventSink`]) that the
//! update thread drains through [`EventSource`]. A [`Notify`] set on the sink wakes the
//! update thread after each queued event. A window made with
//! [`InputWindow::new_opengl`] hands its [`GlContext`] to the draw thread.
//! [`InputCounters`] count the main thread's wake-ups and queued events. A
//! [`FolderDialog`] shows the OS folder picker from any thread. [`TextInput`] turns typed
//! text ([`InputKind::Text`]) on and off from any thread. [`MouseSettings`] turns relative
//! ("high precision") mouse mode on and sets its sensitivity (ADR-0069).

mod clock;
mod convert;
mod counters;
mod dialog;
mod event;
mod gl;
mod mode;
mod relative;
mod ring;
mod window;
mod windowing;

pub use clock::{
    MAX_BRACKET_NS, MAX_REJECTIONS, RECALIBRATE_INTERVAL_NS, RETRY_SPACING_NS, TickMapper,
    sdl_ticks_ns,
};
pub use convert::{convert, convert_text};
pub use counters::InputCounters;
pub use dialog::{FolderCallback, FolderDialog, FolderPick};
pub use event::{InputEvent, InputKind, MouseButton, TextChunk};
pub use gl::GlContext;
pub use mode::{
    DEFAULT_WINDOWED_SIZE, MAX_DISPLAY, MAX_SIDE, MIN_WINDOWED_SIZE, WindowMode, WindowScale,
    WindowSettings, WindowState,
};
pub use relative::{MAX_SENSITIVITY, MIN_SENSITIVITY, MouseSettings};
pub use ring::{BUTTON_RESERVE, EventSink, EventSource, Notify, event_ring};
pub use window::{CursorVisibility, InputWindow, QuitHandle, TextInput, WindowError, dispatch};
