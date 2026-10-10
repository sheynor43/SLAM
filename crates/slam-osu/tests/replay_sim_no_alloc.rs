//! A simulation step must not allocate: everything is allocated when the simulator is built.

mod common;

use common::{PressRules, beatmap, noisy_frames, replay};
use slam_osu::replay::{NoRules, ReplayInput, Simulator};
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

#[test]
fn simulation_steps_do_not_allocate() {
    let map = beatmap();
    let osr = replay(&noisy_frames(7), map.format_version);

    let input = ReplayInput::from_replay(&osr, map.format_version);
    let mut sim = Simulator::new(&map, input.clone(), PressRules::new(&map));
    assert_no_alloc(|| sim.run());
    assert!(sim.is_finished());
    assert_eq!(sim.log().events().len(), 3);

    let mut sim = Simulator::new(&map, input, NoRules::new(&map));
    assert_no_alloc(|| sim.run());
    assert!(sim.is_finished());
}
