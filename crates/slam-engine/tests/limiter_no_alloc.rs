//! `FrameLimiter::wait` must not allocate.

use slam_engine::limiter::{FrameLimiter, LimiterMode};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAlloc;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: delegates to the system allocator and only bumps a thread-local counter.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        // SAFETY: forwarded with the caller's layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by `System` with this layout.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        // SAFETY: forwarded with the caller's arguments.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

#[test]
fn wait_does_not_allocate() {
    for mode in [
        LimiterMode::Hz(8000.0),
        LimiterMode::Hz(1000.0),
        LimiterMode::Unlimited,
    ] {
        let mut limiter = FrameLimiter::new(mode).unwrap();
        // Includes the first wait (schedule start) and resyncs after a forced stall.
        let before = allocs();
        for i in 0..200 {
            if i == 100 {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            std::hint::black_box(limiter.wait());
        }
        assert_eq!(allocs() - before, 0, "{mode:?}");
    }
}
