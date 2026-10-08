//! Test-only infrastructure. Use only as a `[dev-dependencies]` entry (ADR-0016).
//!
//! Hot-path allocation check: install the counting allocator once per test
//! binary and wrap the hot path in [`assert_no_alloc`].
//!
//! ```ignore
//! slam_testkit::install_counting_allocator!();
//!
//! #[test]
//! fn tick_does_not_allocate() {
//!     let mut state = State::new();
//!     slam_testkit::assert_no_alloc(|| state.tick());
//! }
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fmt;

/// Global allocator that forwards to [`System`] and counts operations per thread.
///
/// Install it with [`install_counting_allocator!`].
///
/// Only Rust heap operations are seen: memory taken by C code behind FFI, direct
/// `System` calls and `mmap` are not counted. Requires a target with native TLS
/// (Linux, Windows MSVC, macOS); on OS-key TLS targets such as
/// `x86_64-pc-windows-gnu` the first counter access allocates and recurses.
pub struct CountingAlloc;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static REALLOCS: Cell<u64> = const { Cell::new(0) };
    static DEALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn bump(counter: &'static std::thread::LocalKey<Cell<u64>>) {
    // `try_with`: never panic inside the allocator, even during thread teardown.
    let _ = counter.try_with(|c| c.set(c.get() + 1));
}

// SAFETY: every method forwards to `System` with the caller's arguments and only
// bumps a const-initialised thread-local counter, which itself never allocates.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump(&ALLOCS);
        // SAFETY: forwarded with the caller's layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump(&ALLOCS);
        // SAFETY: forwarded with the caller's layout.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        bump(&DEALLOCS);
        // SAFETY: `ptr` was allocated by `System` with this layout.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump(&REALLOCS);
        // SAFETY: forwarded with the caller's arguments.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Declares [`CountingAlloc`] as the `#[global_allocator]` of the current binary.
///
/// Call once at the top level of a test file (each integration test is its own binary).
#[macro_export]
macro_rules! install_counting_allocator {
    () => {
        #[global_allocator]
        static SLAM_TESTKIT_COUNTING_ALLOC: $crate::CountingAlloc = $crate::CountingAlloc;
    };
}

/// Allocator operations performed by the current thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocStats {
    /// `alloc` and `alloc_zeroed` calls.
    pub allocs: u64,
    /// `realloc` calls.
    pub reallocs: u64,
    /// `dealloc` calls.
    pub deallocs: u64,
}

impl AllocStats {
    /// Counters of the current thread since it started.
    pub fn current() -> Self {
        let get = |k: &'static std::thread::LocalKey<Cell<u64>>| k.try_with(Cell::get).unwrap_or(0);
        Self {
            allocs: get(&ALLOCS),
            reallocs: get(&REALLOCS),
            deallocs: get(&DEALLOCS),
        }
    }

    /// Sum of all operations.
    pub fn total(self) -> u64 {
        self.allocs + self.reallocs + self.deallocs
    }

    fn since(self, earlier: Self) -> Self {
        Self {
            allocs: self.allocs - earlier.allocs,
            reallocs: self.reallocs - earlier.reallocs,
            deallocs: self.deallocs - earlier.deallocs,
        }
    }
}

impl fmt::Display for AllocStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} alloc(s), {} realloc(s), {} dealloc(s)",
            self.allocs, self.reallocs, self.deallocs
        )
    }
}

/// Runs `f` and returns its result together with the allocator operations it
/// performed on the current thread. Work done by other threads is not counted.
///
/// # Panics
/// If [`CountingAlloc`] is not the global allocator of this binary.
#[track_caller]
pub fn count_allocs<R>(f: impl FnOnce() -> R) -> (R, AllocStats) {
    ensure_installed();
    let before = AllocStats::current();
    let result = f();
    let after = AllocStats::current();
    (result, after.since(before))
}

/// Runs `f` and panics if it allocated, reallocated or freed memory on the
/// current thread. Freeing counts too: `free` may take a lock.
///
/// The returned value is dropped by the caller, outside the checked region.
///
/// # Panics
/// On any allocator operation inside `f`, or if [`CountingAlloc`] is not installed.
#[track_caller]
pub fn assert_no_alloc<R>(f: impl FnOnce() -> R) -> R {
    let (result, stats) = count_allocs(f);
    assert!(stats.total() == 0, "hot path allocated: {stats}");
    result
}

#[track_caller]
fn ensure_installed() {
    let before = AllocStats::current();
    drop(std::hint::black_box(Box::new(0u8)));
    let after = AllocStats::current();
    assert!(
        after.allocs > before.allocs,
        "slam_testkit::CountingAlloc is not the global allocator; \
         add `slam_testkit::install_counting_allocator!();` to this test binary"
    );
}
