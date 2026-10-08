use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{InputEvent, InputKind};

/// Wakes the consumer of the ring. Called by [`EventSink::push`] on the input thread
/// after every queued event, so it must be cheap, lock-free and allocation-free.
pub trait Notify: Send + Sync {
    fn notify(&self);
}

/// Free slots kept for key and button events: once no more slots than this remain,
/// mouse moves are dropped so a press or release is never the one lost first.
pub const BUTTON_RESERVE: usize = 16;

/// Creates the input → update ring with room for `capacity` events.
///
/// # Panics
/// If `capacity <= BUTTON_RESERVE`.
pub fn event_ring(capacity: usize) -> (EventSink, EventSource) {
    assert!(
        capacity > BUTTON_RESERVE,
        "ring capacity must exceed BUTTON_RESERVE"
    );
    let (producer, consumer) = rtrb::RingBuffer::new(capacity);
    let dropped = Arc::new(Dropped::default());
    (
        EventSink {
            producer,
            dropped: Arc::clone(&dropped),
            notify: None,
        },
        EventSource { consumer, dropped },
    )
}

#[derive(Default)]
struct Dropped {
    /// All dropped events.
    total: AtomicU64,
    /// Dropped key and button events only.
    buttons: AtomicU64,
}

/// Producer end, owned by the input thread.
pub struct EventSink {
    producer: rtrb::Producer<InputEvent>,
    dropped: Arc<Dropped>,
    notify: Option<Arc<dyn Notify>>,
}

impl EventSink {
    /// Sets the consumer wake-up called after every queued event.
    pub fn set_notify(&mut self, notify: Arc<dyn Notify>) {
        self.notify = Some(notify);
    }

    /// Pushes an event without blocking; returns whether it was queued. Mouse moves
    /// are dropped once at most [`BUTTON_RESERVE`] slots are free, keys and buttons
    /// only when the ring is full. Every drop is counted. A queued event calls the
    /// [`Notify`] set by [`EventSink::set_notify`].
    pub fn push(&mut self, event: InputEvent) -> bool {
        if matches!(event.kind, InputKind::MouseMove { .. })
            && self.producer.slots() <= BUTTON_RESERVE
        {
            self.dropped.total.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        match self.producer.push(event) {
            Ok(()) => {
                if let Some(notify) = &self.notify {
                    notify.notify();
                }
                true
            }
            Err(_) => {
                self.dropped.total.fetch_add(1, Ordering::Relaxed);
                if !matches!(event.kind, InputKind::MouseMove { .. }) {
                    self.dropped.buttons.fetch_add(1, Ordering::Relaxed);
                }
                false
            }
        }
    }
}

/// Consumer end, owned by the update thread.
pub struct EventSource {
    consumer: rtrb::Consumer<InputEvent>,
    dropped: Arc<Dropped>,
}

impl EventSource {
    /// Takes the oldest queued event, if any.
    pub fn pop(&mut self) -> Option<InputEvent> {
        self.consumer.pop().ok()
    }

    /// Number of events queued right now.
    pub fn len(&self) -> usize {
        self.consumer.slots()
    }

    pub fn is_empty(&self) -> bool {
        self.consumer.is_empty()
    }

    /// Total number of events lost to a full ring since creation.
    pub fn dropped(&self) -> u64 {
        self.dropped.total.load(Ordering::Relaxed)
    }

    /// Number of key and button events lost since creation. When it grows, a press or
    /// release was lost and held-button state should be re-synced.
    pub fn dropped_buttons(&self) -> u64 {
        self.dropped.buttons.load(Ordering::Relaxed)
    }
}
