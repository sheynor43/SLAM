use super::*;
use std::cell::Cell;

/// Fake time: each `now_ns` read advances the clock by `read_step_ns` (models the
/// cost of spinning), each sleep lands `oversleep_ns` after its target.
struct FakeTimer {
    now: Cell<u64>,
    read_step_ns: u64,
    oversleep_ns: u64,
    sleeps: Vec<u64>,
}

impl FakeTimer {
    fn new(start_ns: u64) -> Self {
        Self {
            now: Cell::new(start_ns),
            read_step_ns: 1,
            oversleep_ns: 0,
            sleeps: Vec::new(),
        }
    }

    fn advance(&self, ns: u64) {
        self.now.set(self.now.get() + ns);
    }
}

impl Timer for FakeTimer {
    fn now_ns(&self) -> u64 {
        let t = self.now.get();
        self.now.set(t + self.read_step_ns);
        t
    }

    fn sleep_until(&mut self, deadline_ns: u64) {
        self.sleeps.push(deadline_ns);
        let woke = deadline_ns + self.oversleep_ns;
        if woke > self.now.get() {
            self.now.set(woke);
        }
    }
}

fn limiter(hz: f64) -> FrameLimiter<FakeTimer> {
    FrameLimiter::with_timer(FakeTimer::new(1_000), LimiterMode::Hz(hz)).unwrap()
}

#[test]
fn rejects_invalid_rates() {
    for hz in [0.0, -1.0, MAX_HZ + 1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            FrameLimiter::with_timer(FakeTimer::new(0), LimiterMode::Hz(hz)).err(),
            Some(InvalidRate),
            "{hz}"
        );
    }
    assert!(FrameLimiter::with_timer(FakeTimer::new(0), LimiterMode::Hz(MAX_HZ)).is_ok());
}

#[test]
fn mode_returns_exact_rate() {
    let mut l = limiter(1000.0);
    // Rates for which 1e9 / (1e9 / hz) != hz.
    for hz in [143.856, 59.94, 7777.7] {
        l.set_mode(LimiterMode::Hz(hz)).unwrap();
        assert_eq!(l.mode(), LimiterMode::Hz(hz));
    }
    l.set_mode(LimiterMode::Unlimited).unwrap();
    assert_eq!(l.mode(), LimiterMode::Unlimited);
}

#[test]
fn invalid_rate_keeps_previous_mode() {
    let mut l = limiter(1000.0);
    assert_eq!(l.set_mode(LimiterMode::Hz(0.0)), Err(InvalidRate));
    assert_eq!(l.mode(), LimiterMode::Hz(1000.0));
}

#[test]
fn unlimited_returns_immediately() {
    let mut l = FrameLimiter::with_timer(FakeTimer::new(500), LimiterMode::Unlimited).unwrap();
    let w = l.wait();
    assert_eq!(w.deadline_ns, w.woke_ns);
    assert!(!w.resynced);
    assert!(l.timer().sleeps.is_empty());
}

#[test]
fn deadlines_are_absolute_and_do_not_drift() {
    // 7919 Hz: the period (126_278.570... ns) is not an integer number of nanoseconds,
    // so adding a rounded period each frame would drift.
    const HZ: u128 = 7919;
    let mut l = limiter(HZ as f64);
    // Keep the fake spin short.
    l.set_spin_threshold_ns(100);
    let epoch = 1_000;
    let check = |n: u64, w: FrameWait| {
        let exact = epoch + (n as u128 * 1_000_000_000 / HZ) as u64;
        assert!(w.deadline_ns.abs_diff(exact) <= 1, "frame {n}");
        assert!(w.woke_ns >= w.deadline_ns);
        assert!(!w.resynced);
    };
    for n in 1..=10_000 {
        check(n, l.wait());
    }
    // Jump to about 30 days of frames; time catches up through the fake sleep.
    let far = 30 * 24 * 3600 * HZ as u64;
    l.frame = far;
    l.timer()
        .now
        .set(epoch + (far as u128 * 1_000_000_000 / HZ) as u64);
    for n in far + 1..=far + 10_000 {
        check(n, l.wait());
    }
}

#[test]
fn sleeps_until_spin_threshold_then_spins() {
    let mut l = limiter(1000.0);
    l.set_spin_threshold_ns(200_000);
    let w = l.wait();
    assert_eq!(w.deadline_ns, 1_000 + 1_000_000);
    assert_eq!(l.timer().sleeps, [w.deadline_ns - 200_000]);
    // Spinning stops at the first read at or after the deadline.
    assert_eq!(w.lateness_ns(), 0);
}

#[test]
fn no_sleep_when_inside_spin_threshold() {
    let mut l = limiter(1000.0);
    l.set_spin_threshold_ns(2_000_000);
    l.wait();
    assert!(l.timer().sleeps.is_empty());
}

#[test]
fn oversleep_past_deadline_skips_spin() {
    let mut l = limiter(1000.0);
    l.timer.oversleep_ns = 250_000;
    let w = l.wait();
    assert_eq!(w.woke_ns, w.deadline_ns + 50_000);
    assert!(!w.resynced);
}

#[test]
fn small_lateness_keeps_schedule() {
    let mut l = limiter(1000.0);
    let first = l.wait();
    // Work overruns the next deadline by less than a period.
    l.timer().advance(1_400_000);
    let late = l.wait();
    assert_eq!(late.deadline_ns, first.deadline_ns + 1_000_000);
    assert!(!late.resynced);
    assert!(late.lateness_ns() > 0);
    let next = l.wait();
    assert_eq!(next.deadline_ns, first.deadline_ns + 2_000_000);
    assert_eq!(next.lateness_ns(), 0);
}

#[test]
fn missed_deadline_resyncs_without_debt() {
    let mut l = limiter(1000.0);
    l.wait();
    // Stall for 50 periods.
    l.timer().advance(50_000_000);
    let missed = l.wait();
    assert!(missed.resynced);

    // The next frame is one full period after the stall, not a burst of catch-up frames.
    let next = l.wait();
    assert!(!next.resynced);
    assert_eq!(next.deadline_ns, missed.woke_ns + 1_000_000);
    assert_eq!(next.lateness_ns(), 0);
}

#[test]
fn mode_change_restarts_schedule() {
    let mut l = limiter(1000.0);
    l.wait();
    l.timer().advance(10_000_000);
    l.set_mode(LimiterMode::Hz(500.0)).unwrap();
    let before = l.timer().now.get();
    let w = l.wait();
    assert!(!w.resynced);
    assert_eq!(w.deadline_ns, before + 2_000_000);
}

/// Limiter on a clock that only moves on sleeps, so lateness can be set exactly.
fn frozen_limiter(hz: f64) -> FrameLimiter<FakeTimer> {
    let mut timer = FakeTimer::new(1_000);
    timer.read_step_ns = 0;
    let mut l = FrameLimiter::with_timer(timer, LimiterMode::Hz(hz)).unwrap();
    // Sleep exactly to the deadline: with a frozen clock a spin would never end.
    l.set_spin_threshold_ns(0);
    l
}

#[test]
fn resync_boundary_is_one_full_period() {
    const P: u64 = 1_000_000;

    let mut l = frozen_limiter(1000.0);
    let first = l.wait();
    assert_eq!(first.woke_ns, first.deadline_ns);
    l.timer().advance(P + P - 1);
    let w = l.wait();
    assert_eq!(w.lateness_ns(), P - 1);
    assert!(!w.resynced);

    let mut l = frozen_limiter(1000.0);
    l.wait();
    l.timer().advance(P + P);
    let w = l.wait();
    assert_eq!(w.lateness_ns(), P);
    assert!(w.resynced);
}

#[test]
fn zero_spin_threshold_sleeps_to_deadline() {
    let mut l = frozen_limiter(1000.0);
    let w = l.wait();
    assert_eq!(l.timer().sleeps, [w.deadline_ns]);
    assert_eq!(w.lateness_ns(), 0);
}

#[test]
fn unlimited_and_back_restarts_schedule() {
    let mut l = limiter(1000.0);
    l.wait();
    l.set_mode(LimiterMode::Unlimited).unwrap();
    l.timer().advance(7_777_777);
    l.wait();
    l.set_mode(LimiterMode::Hz(1000.0)).unwrap();
    let before = l.timer().now.get();
    let w = l.wait();
    assert!(!w.resynced);
    assert_eq!(w.deadline_ns, before + 1_000_000);
}
