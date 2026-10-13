//! A live input step, and the rules' update on it, do not allocate.

mod common;

use common::{Linear, beatmap, layout, ns, play};
use slam_osu::ModSet;
use slam_osu::judgement::JudgementLog;
use slam_osu::replay::Rules;
use slam_osu::rules::OsuRules;
use slam_play::input::{MAX_LIVE_STEP_EVENTS, MAX_PENDING_EVENTS};
use slam_play::{Bindings, LiveInput};
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

#[test]
fn live_steps_do_not_allocate() {
    let map = beatmap();
    let play = play(&map, 3);
    let mut live = LiveInput::new(Bindings::default(), layout());
    let mut rules = OsuRules::new(&map, &ModSet::default());
    let mut log = JudgementLog::with_capacity(rules.max_judgements());

    // Ticks every 4 ms, so a tick takes several events.
    let end = play.events.last().unwrap().time_ns;
    let mut next = 0;
    assert_no_alloc(|| {
        let mut now = 0;
        while now <= end + ns(4.0) {
            let start = next;
            while next < play.events.len() && play.events[next].time_ns <= now {
                next += 1;
            }
            let step = live.step(&play.events[start..next], now, &Linear);
            rules.update(&step, &mut log);
            now += ns(4.0);
        }
    });
    assert!(rules.is_complete());
    assert!(!log.events().is_empty());
}

#[test]
fn deferring_and_dropping_do_not_allocate() {
    let map = beatmap();
    let play = play(&map, 5);
    let mut live = LiveInput::new(Bindings::default(), layout());
    let buttons: Vec<_> = play
        .events
        .iter()
        .filter(|e| !matches!(e.kind, slam_input::InputKind::MouseMove { .. }))
        .copied()
        .collect();
    assert!(buttons.len() > MAX_LIVE_STEP_EVENTS);

    // Everything in one tick: some events wait, then a second huge tick overflows the queue.
    assert_no_alloc(|| {
        live.step(&buttons, ns(1e6), &Linear);
        for _ in 0..(MAX_PENDING_EVENTS / buttons.len() + 2) {
            live.step(&buttons, ns(1e6), &Linear);
        }
    });
    assert!(live.dropped() > 0);
}
