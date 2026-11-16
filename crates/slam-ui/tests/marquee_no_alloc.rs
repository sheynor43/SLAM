//! A marquee's ride and the tracker that applies wheel and drags do not touch the
//! allocator (ADR-0148).

use slam_ui::marquee::{Marquee, MarqueeParams, ScrollTracker, TextScroll};
use slam_ui::menu::Rect;
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

const MS: u64 = 1_000_000;

#[test]
fn the_ride_and_the_tracker_do_not_allocate() {
    let params = MarqueeParams::new(&Theme::default(), 1.0);
    let mut ms = [Marquee::default(), Marquee::default()];
    for (i, m) in ms.iter_mut().enumerate() {
        m.set_overflow(100.0, 0);
        m.place(Rect {
            x: 0.0,
            y: i as f32 * 30.0,
            width: 100.0,
            height: 20.0,
        });
    }
    let mut tracker = ScrollTracker::default();
    let mut input = TextScroll::default();
    input.pointer([10.0, 10.0]);
    input.start_drag([10.0, 10.0]);
    let ((), allocs) = slam_testkit::count_allocs(|| {
        for frame in 0..1_000u64 {
            input.wheel(0.5);
            input.pointer([10.0 - frame as f32 * 0.1, 10.0 + (frame % 2) as f32 * 30.0]);
            tracker.apply(&input, frame * 8 * MS, &params, ms.iter_mut());
            for m in &mut ms {
                m.offset(frame * 8 * MS, &params);
            }
        }
    });
    assert_eq!(allocs.total(), 0, "{allocs}");
}
