//! Triple buffer for update → draw snapshots.
//!
//! Three slots: the writer owns one, the reader owns one, the third is the "back"
//! slot holding the latest published value. Publishing swaps the writer's slot with
//! the back slot; reading swaps the back slot with the reader's slot when it holds
//! something newer. Both sides are wait-free (one atomic swap) and never block each
//! other; the reader always gets the most recent complete snapshot.

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Set in `back` when the back slot holds a value the reader has not taken yet.
const FRESH: u8 = 0b100;
const INDEX: u8 = 0b011;

struct Shared<T> {
    slots: [UnsafeCell<T>; 3],
    /// Index of the back slot, plus [`FRESH`].
    back: AtomicU8,
}

// SAFETY: every slot is accessed by at most one side at a time: the writer only
// touches its own index, the reader only its own, and the back slot is never accessed
// directly. Ownership of a slot moves between threads through the AcqRel swap on
// `back`, which orders all writes to a slot before the other side reads it. Values
// move between threads, so `T: Send` is required, but are never shared, so `T: Sync`
// is not.
unsafe impl<T: Send> Sync for Shared<T> {}

/// Creates a triple buffer with every slot set to `initial`.
pub fn triple_buffer<T: Clone>(initial: T) -> (SnapshotWriter<T>, SnapshotReader<T>) {
    let shared = Arc::new(Shared {
        slots: [
            UnsafeCell::new(initial.clone()),
            UnsafeCell::new(initial.clone()),
            UnsafeCell::new(initial),
        ],
        back: AtomicU8::new(1),
    });
    (
        SnapshotWriter {
            shared: Arc::clone(&shared),
            index: 0,
        },
        SnapshotReader { shared, index: 2 },
    )
}

/// Writer end, owned by the update thread.
pub struct SnapshotWriter<T> {
    shared: Arc<Shared<T>>,
    index: u8,
}

// SAFETY: the writer only accesses its own slot; see `Shared`.
unsafe impl<T: Send> Send for SnapshotWriter<T> {}

impl<T> SnapshotWriter<T> {
    /// The slot being written. It holds an older snapshot (not necessarily the last
    /// published one), so the writer must overwrite every field it relies on.
    pub fn slot(&mut self) -> &mut T {
        // SAFETY: the writer has exclusive access to its slot until `publish`.
        unsafe { &mut *self.shared.slots[self.index as usize].get() }
    }

    /// Makes the written slot the latest snapshot and takes another slot to write.
    /// Wait-free, allocation-free.
    pub fn publish(&mut self) {
        let old_back = self.shared.back.swap(self.index | FRESH, Ordering::AcqRel);
        self.index = old_back & INDEX;
    }
}

/// Reader end, owned by the draw thread.
pub struct SnapshotReader<T> {
    shared: Arc<Shared<T>>,
    index: u8,
}

// SAFETY: the reader only accesses its own slot; see `Shared`.
unsafe impl<T: Send> Send for SnapshotReader<T> {}

impl<T> SnapshotReader<T> {
    /// The latest published snapshot, and whether it is newer than the one returned by
    /// the previous call. Never waits for the writer; wait-free, allocation-free.
    pub fn read(&mut self) -> (&T, bool) {
        let fresh = self.shared.back.load(Ordering::Relaxed) & FRESH != 0;
        if fresh {
            // Only the reader clears FRESH, so it is still set here; the swap may
            // pick up an even newer slot published since the load.
            let old_back = self.shared.back.swap(self.index, Ordering::AcqRel);
            self.index = old_back & INDEX;
        }
        // SAFETY: the reader has exclusive access to its slot until the next swap.
        (
            unsafe { &*self.shared.slots[self.index as usize].get() },
            fresh,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_sees_initial_then_each_publish() {
        let (mut w, mut r) = triple_buffer(0u32);
        assert_eq!(r.read(), (&0, false));
        *w.slot() = 1;
        w.publish();
        assert_eq!(r.read(), (&1, true));
        assert_eq!(r.read(), (&1, false));
    }

    #[test]
    fn reader_skips_to_latest() {
        let (mut w, mut r) = triple_buffer(0u32);
        for v in 1..=5 {
            *w.slot() = v;
            w.publish();
        }
        assert_eq!(r.read(), (&5, true));
    }

    #[test]
    fn writer_never_gets_the_reader_slot() {
        let (mut w, mut r) = triple_buffer(0u32);
        for v in 1..=10 {
            *w.slot() = v;
            w.publish();
            let (got, _) = r.read();
            assert_eq!(*got, v);
            // Writing the next value must not change what the reader holds.
            *w.slot() = 1000;
            assert_eq!(r.read(), (&v, false));
        }
    }
}
