//! `FrameLimiter::wait` must not allocate.

use slam_engine::limiter::{FrameLimiter, LimiterMode};
use slam_testkit::count_allocs;

slam_testkit::install_counting_allocator!();

#[test]
fn wait_does_not_allocate() {
    for mode in [
        LimiterMode::Hz(8000.0),
        LimiterMode::Hz(1000.0),
        LimiterMode::Unlimited,
    ] {
        let mut limiter = FrameLimiter::new(mode).unwrap();
        // Includes the first wait (schedule start) and resyncs after a forced stall.
        let ((), stats) = count_allocs(|| {
            for i in 0..200 {
                if i == 100 {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                std::hint::black_box(limiter.wait());
            }
        });
        assert_eq!(stats.total(), 0, "{mode:?}: {stats}");
    }
}
