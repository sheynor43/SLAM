//! An update step (wait, drain, sort, tick, publish) and a draw step (limiter, read,
//! draw) must not allocate.

use slam_engine::limiter::{FrameLimiter, LimiterMode};
use slam_engine::{
    Draw, DrawLoop, FrameInfo, TickInfo, Update, UpdateLoop, triple_buffer, wake_pair,
};
use slam_input::{InputEvent, InputKind, event_ring};
use slam_testkit::count_allocs;

slam_testkit::install_counting_allocator!();

type Snap = [u64; 16];

struct Sum(u64);

impl Update for Sum {
    type Snapshot = Snap;

    fn tick(&mut self, info: &TickInfo, events: &[InputEvent], snapshot: &mut Snap) {
        for e in events {
            self.0 = self.0.wrapping_add(e.time_ns);
        }
        snapshot.fill(self.0 ^ info.now_ns);
    }
}

struct Read(u64);

impl Draw<Snap> for Read {
    fn frame(&mut self, _: &FrameInfo, snapshot: &Snap) {
        self.0 = self.0.wrapping_add(snapshot[0]);
    }
}

fn mv(time_ns: u64) -> InputEvent {
    InputEvent {
        time_ns,
        kind: InputKind::MouseMove { x: 1.0, y: 2.0 },
    }
}

#[test]
fn update_and_draw_steps_do_not_allocate() {
    let (mut sink, source) = event_ring(256);
    let (waker, parker) = wake_pair();
    sink.set_notify(waker.as_notify());
    let (writer, reader) = triple_buffer([0; 16]);
    let batch = 8;
    let mut update =
        UpdateLoop::new(Sum(0), source, parker, writer, 8000.0, batch, 50_000).unwrap();
    let limiter = FrameLimiter::new(LimiterMode::Hz(8000.0)).unwrap();
    let mut draw = DrawLoop::new(Read(0), reader, limiter);

    let ((), stats) = count_allocs(|| {
        for i in 0..400u64 {
            // Out of order, sometimes more than one batch, sometimes nothing.
            for j in 0..(i % 20) {
                sink.push(mv(i * 100 + (j * 7) % 13));
            }
            update.step();
            draw.step();
        }
    });
    assert_eq!(stats.total(), 0, "{stats}");
    assert!(update.stats().ticks >= 400);
    assert!(update.stats().reordered > 0);
    assert_eq!(draw.frames(), 400);
}
