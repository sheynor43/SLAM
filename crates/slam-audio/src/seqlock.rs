//! Single-writer seqlock over `N` atomic words, shared by the reports the audio thread
//! publishes for other threads (ADR-0022, ADR-0033).
//!
//! Neither side blocks: the reader retries while a write is in progress. All words are
//! atomics, so a torn read is detected, never UB.
//!
//! The reader is lock-free, not wait-free: if the writer is preempted between its two
//! sequence stores (no real-time priority), a reader spins until it resumes. The
//! stress tests run on x86 (TSO) only and cannot catch an ordering regression on
//! weaker memory models; there is no loom model yet.

use std::sync::atomic::{AtomicU64, Ordering, fence};

#[derive(Debug)]
pub(crate) struct SeqLock<const N: usize> {
    /// Even: stable; odd: write in progress; zero: nothing published yet.
    seq: AtomicU64,
    words: [AtomicU64; N],
}

impl<const N: usize> Default for SeqLock<N> {
    fn default() -> Self {
        Self {
            seq: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl<const N: usize> SeqLock<N> {
    /// Publishes `words`. Allocation-free, lock-free, wait-free.
    ///
    /// Must only be called from one thread at a time; concurrent writers corrupt the
    /// sequence.
    pub(crate) fn write(&self, words: [u64; N]) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        // Orders the odd sequence before the word stores below.
        fence(Ordering::Release);
        for (slot, word) in self.words.iter().zip(words) {
            slot.store(word, Ordering::Relaxed);
        }
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// Latest consistent words, or `None` before the first write. Allocation-free and
    /// lock-free; spins only while a write is in progress (a few stores).
    pub(crate) fn read(&self) -> Option<[u64; N]> {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            if before == 0 {
                return None;
            }
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let words = std::array::from_fn(|i| self.words[i].load(Ordering::Relaxed));
            // Orders the word loads before the sequence re-check.
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                return Some(words);
            }
        }
    }
}
