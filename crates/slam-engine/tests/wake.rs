//! Update wake-up: deadline, early notification, pending notification.

use std::time::Duration;

use slam_engine::clock::now_ns;
use slam_engine::wake_pair;

const MS: u64 = 1_000_000;
const SPIN: u64 = 200_000;

#[test]
fn times_out_at_the_deadline() {
    let (_waker, mut parker) = wake_pair();
    for wait in [MS / 2, 5 * MS, 20 * MS] {
        let deadline = now_ns() + wait;
        assert!(!parker.wait_until(deadline, SPIN));
        let late = now_ns() - deadline;
        // The busy-wait returns right after the deadline; allow for preemption.
        assert!(late < 2 * MS, "{late} ns late for a {wait} ns wait");
    }
}

#[test]
fn pending_notification_returns_immediately_once() {
    let (waker, mut parker) = wake_pair();
    waker.notify();
    waker.notify();
    let far = now_ns() + 10_000 * MS;
    assert!(parker.wait_until(far, SPIN));
    // Notifications coalesce: the second wait times out.
    assert!(!parker.wait_until(now_ns() + MS, SPIN));
}

#[test]
fn notification_from_another_thread_ends_a_sleep() {
    let (waker, mut parker) = wake_pair();
    for _ in 0..50 {
        let notifier = {
            let waker = waker.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(2));
                waker.notify();
            })
        };
        let start = now_ns();
        assert!(parker.wait_until(start + 10_000 * MS, SPIN));
        assert!(now_ns() - start < 1_000 * MS);
        notifier.join().unwrap();
    }
}

#[test]
fn notification_during_the_spin_is_seen() {
    let (waker, mut parker) = wake_pair();
    // A spin threshold longer than the wait: the parker never sleeps.
    let notifier = {
        let waker = waker.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2));
            waker.notify();
        })
    };
    let start = now_ns();
    assert!(parker.wait_until(start + 1_000 * MS, 2_000 * MS));
    assert!(now_ns() - start < 500 * MS);
    notifier.join().unwrap();
}

/// Small deterministic generator for notification offsets.
fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 33
}

#[test]
fn racing_notifications_are_never_lost() {
    let (waker, mut parker) = wake_pair();
    let (tx, rx) = std::sync::mpsc::channel::<u64>();
    // The notifier fires at random offsets after each wait starts, landing in the
    // CAS → futex window, in the futex, in the spin, and at the timeout.
    let notifier = std::thread::spawn(move || {
        while let Ok(at) = rx.recv() {
            while now_ns() < at {
                std::hint::spin_loop();
            }
            waker.notify();
        }
    });
    let mut seed = 1;
    let mut late = 0;
    for i in 0..3000 {
        let start = now_ns();
        let offset = next(&mut seed) % 400_000;
        // Alternate a far deadline (only the notification can end the wait) and one
        // near the notification (it races the timeout).
        let deadline = if i % 2 == 0 {
            start + 10_000 * MS
        } else {
            start + offset
        };
        let spin = [0, 50_000, SPIN][i % 3];
        tx.send(start + offset).unwrap();
        let notified = parker.wait_until(deadline, spin);
        if i % 2 == 0 {
            assert!(notified, "iteration {i}: far wait timed out");
            late += u64::from(now_ns() - start > 500 * MS);
        } else if !notified {
            // Timed out first: the notification is still pending or arrives soon.
            assert!(parker.wait_until(now_ns() + 10_000 * MS, spin));
        }
        // Absorb a notification that came after a timeout-first wait was answered.
        while parker.wait_until(now_ns(), 0) {}
    }
    drop(tx);
    notifier.join().unwrap();
    assert_eq!(late, 0, "{late} waits ended long after their notification");
}
