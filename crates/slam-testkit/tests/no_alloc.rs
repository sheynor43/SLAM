//! Self-tests of the allocation counter.

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slam_testkit::{AllocStats, assert_no_alloc, count_allocs};

slam_testkit::install_counting_allocator!();

#[test]
fn non_allocating_code_passes() {
    let mut buf = [0u64; 64];
    let sum = assert_no_alloc(|| {
        for (i, x) in buf.iter_mut().enumerate() {
            *x = black_box(i as u64);
        }
        buf.iter().sum::<u64>()
    });
    assert_eq!(sum, 63 * 64 / 2);
}

#[test]
fn reusing_preallocated_vec_passes() {
    let mut v: Vec<u32> = Vec::with_capacity(16);
    assert_no_alloc(|| {
        for i in 0..16 {
            v.push(black_box(i));
        }
        v.clear();
    });
}

#[test]
#[should_panic(expected = "hot path allocated: 1 alloc(s), 0 realloc(s), 0 dealloc(s)")]
fn box_fails() {
    let b = assert_no_alloc(|| Box::new(black_box(1u64)));
    drop(b);
}

#[test]
#[should_panic(expected = "hot path allocated: 0 alloc(s), 0 realloc(s), 1 dealloc(s)")]
fn drop_fails() {
    let s = black_box(String::from("freed in the hot path"));
    assert_no_alloc(move || drop(s));
}

#[test]
#[should_panic(expected = "1 realloc(s)")]
fn growth_fails() {
    let mut v: Vec<u8> = Vec::with_capacity(1);
    v.push(0);
    assert_no_alloc(|| v.push(black_box(1)));
}

#[test]
fn counts_each_kind() {
    let ((), stats) = count_allocs(|| {
        let mut v: Vec<u8> = black_box(Vec::with_capacity(1));
        v.extend_from_slice(black_box(&[1, 2, 3, 4]));
        drop(black_box(v));
    });
    assert_eq!(
        stats,
        AllocStats {
            allocs: 1,
            reallocs: 1,
            deallocs: 1
        }
    );
}

#[test]
fn other_threads_are_not_counted() {
    let stop = Arc::new(AtomicBool::new(false));
    let foreign = Arc::new(AtomicU64::new(0));
    let worker = {
        let (stop, foreign) = (Arc::clone(&stop), Arc::clone(&foreign));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                drop(black_box(vec![0u8; 64]));
                foreign.fetch_add(1, Ordering::Relaxed);
            }
        })
    };

    let start = foreign.load(Ordering::Relaxed);
    assert_no_alloc(|| {
        // Stay in the checked region until the worker has allocated a lot.
        while foreign.load(Ordering::Relaxed) < start + 1000 && !worker.is_finished() {
            std::hint::spin_loop();
        }
    });

    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    assert!(foreign.load(Ordering::Relaxed) >= start + 1000);
}
