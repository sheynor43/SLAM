//! Measures frame limiter jitter: how late `wait` returns relative to the deadline.
//!
//! Usage: `cargo run -p slam-engine --release --example limiter_jitter -- [seconds] [spin_us]`
//! (defaults: 5 s per rate, 200 us spin threshold).

use slam_engine::limiter::{FrameLimiter, LimiterMode};

const RATES: [f64; 3] = [1000.0, 4000.0, 8000.0];

fn percentile(sorted: &[u64], p: f64) -> u64 {
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let seconds: f64 = args.next().map_or(5.0, |s| s.parse().expect("seconds"));
    let spin_us: u64 = args.next().map_or(200, |s| s.parse().expect("spin_us"));

    println!("spin threshold: {spin_us} us, {seconds} s per rate");
    println!("| Hz | frames | resyncs | p50, us | p99, us | p99.9, us | max, us |");
    println!("|---:|---:|---:|---:|---:|---:|---:|");

    for hz in RATES {
        let frames = (hz * seconds) as usize;
        let mut lateness = Vec::with_capacity(frames);
        let mut resyncs = 0;
        let mut limiter = FrameLimiter::new(LimiterMode::Hz(hz)).unwrap();
        limiter.set_spin_threshold_ns(spin_us * 1000);
        for _ in 0..frames {
            let w = limiter.wait();
            if w.resynced {
                resyncs += 1;
            }
            lateness.push(w.lateness_ns());
        }
        lateness.sort_unstable();
        let us = |ns: u64| ns as f64 / 1000.0;
        println!(
            "| {hz} | {frames} | {resyncs} | {:.1} | {:.1} | {:.1} | {:.1} |",
            us(percentile(&lateness, 0.5)),
            us(percentile(&lateness, 0.99)),
            us(percentile(&lateness, 0.999)),
            us(*lateness.last().unwrap()),
        );
    }
}
