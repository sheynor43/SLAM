//! A simulation step must not allocate: everything is allocated when the simulator is built.

mod common;

use common::{PressRules, beatmap, noisy_frames, replay};
use slam_formats::osu::decode_str;
use slam_osu::replay::{NoRules, ReplayInput, Simulator};
use slam_osu::rules::{HitPolicy, OsuRules};
use slam_osu::{Beatmap, HealthProcessor, Mod, ModSet};
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

    for rules in [
        OsuRules::new(&map, &ModSet::default()),
        OsuRules::with_policy(
            &map,
            HitPolicy::Legacy {
                hittable_range: 400.0,
            },
        ),
    ] {
        let mut sim = Simulator::new(&map, input.clone(), rules);
        assert_no_alloc(|| sim.run());
        assert!(sim.is_finished());
        assert_eq!(sim.log().events().len(), 3);
    }

    let mut sim = Simulator::new(&map, input, NoRules::new(&map));
    assert_no_alloc(|| sim.run());
    assert!(sim.is_finished());
}

#[test]
fn slider_steps_do_not_allocate() {
    // Sliders with ticks and repeats between circles, mashed through.
    let text = "osu file format v14\n\n[Difficulty]\nCircleSize:4\nOverallDifficulty:8\n\
                ApproachRate:9\nSliderMultiplier:1.4\nSliderTickRate:2\n\n[TimingPoints]\n\
                0,300,4,2,0,100,1,0\n\n[HitObjects]\n100,100,500,1,0\n\
                120,120,800,2,0,P|200:60|300:120,2,250\n300,200,1900,1,0\n\
                300,200,2100,2,0,L|100:200,1,150\n256,192,3000,2,0,B|300:100|400:300,3,120\n";
    let map = Beatmap::from_file(decode_str(text).unwrap().beatmap).unwrap();
    let classic = ModSet::new(vec![Mod::from_acronym("CL")]).unwrap();
    for seed in [3, 7, 11] {
        let osr = replay(&noisy_frames(seed), map.format_version);
        let input = ReplayInput::from_replay(&osr, map.format_version);
        for rules in [
            OsuRules::new(&map, &ModSet::default()),
            OsuRules::new(&map, &classic),
        ] {
            let mut sim = Simulator::new(&map, input.clone(), rules);
            assert_no_alloc(|| sim.run());
            assert!(sim.is_finished());
            assert!(!sim.log().events().is_empty());
        }
    }
}

#[test]
fn spinner_steps_do_not_allocate() {
    // Spinners, one of them with a circle inside, between circles, spun through by the cursor.
    let text = "osu file format v14\n\n[Difficulty]\nCircleSize:4\nOverallDifficulty:8\n\
                ApproachRate:9\nSliderMultiplier:1.4\nSliderTickRate:1\n\n[TimingPoints]\n\
                0,300,4,2,0,100,1,0\n\n[HitObjects]\n100,100,300,1,0\n256,192,500,12,0,2000\n\
                300,200,2200,1,0\n256,192,2400,12,0,3900\n120,80,3000,1,0\n";
    let map = Beatmap::from_file(decode_str(text).unwrap().beatmap).unwrap();
    let classic = ModSet::new(vec![Mod::from_acronym("CL")]).unwrap();
    let double_time = ModSet::new(vec![Mod::from_acronym("DT")]).unwrap();
    for seed in [3, 7, 11] {
        let osr = replay(&noisy_frames(seed), map.format_version);
        let input = ReplayInput::from_replay(&osr, map.format_version);
        for rules in [
            OsuRules::new(&map, &ModSet::default()),
            OsuRules::new(&map, &classic),
            OsuRules::new(&map, &double_time),
        ] {
            let mut sim = Simulator::new(&map, input.clone(), rules);
            assert_no_alloc(|| sim.run());
            assert!(sim.is_finished());
            assert!(!sim.log().events().is_empty());
        }
    }
}

#[test]
fn health_steps_do_not_allocate() {
    // Circles, sliders and spinners, mashed through with health tracked: No Fail keeps the play
    // going past every fail, Easy refills the health, Classic uses the legacy processor.
    let text = "osu file format v14\n\n[Difficulty]\nHPDrainRate:9\nCircleSize:4\n\
                OverallDifficulty:8\nApproachRate:9\nSliderMultiplier:1.4\nSliderTickRate:2\n\n\
                [TimingPoints]\n0,300,4,2,0,100,1,0\n\n[HitObjects]\n100,100,500,1,0\n\
                120,120,800,2,0,P|200:60|300:120,2,250\n256,192,1900,12,0,3000\n\
                300,200,3200,5,0\n300,200,3400,2,0,L|100:200,1,150\n";
    let map = Beatmap::from_file(decode_str(text).unwrap().beatmap).unwrap();
    for acronyms in [&["NF"][..], &["NF", "CL"], &["EZ"], &[]] {
        let mods = ModSet::new(acronyms.iter().map(|a| Mod::from_acronym(a)).collect()).unwrap();
        for seed in [3, 7, 11] {
            let osr = replay(&noisy_frames(seed), map.format_version);
            let input = ReplayInput::from_replay(&osr, map.format_version);
            let mut sim = Simulator::new(&map, input, OsuRules::new(&map, &mods))
                .with_health(HealthProcessor::for_play(&map, &mods), mods.clone());
            assert_no_alloc(|| sim.run());
            assert!(sim.is_finished());
            assert!(!sim.log().events().is_empty());
        }
    }
}
