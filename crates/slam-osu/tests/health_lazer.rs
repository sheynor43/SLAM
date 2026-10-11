//! Bit-exact comparison of drain rates with values computed by lazer's own health processors
//! (see the header of `data/lazer-health.txt`).
//!
//! Every case is a small `.osu` map, decoded and processed by [`Beatmap::from_file`], with
//! classic slider behaviour off and on.

use slam_formats::osu::decode_str;
use slam_osu::Beatmap;
use slam_osu::health::{HealthKind, HealthOptions, HealthProcessor};

const FIXTURE: &str = include_str!("data/lazer-health.txt");

fn bits(hex: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(hex, 16).expect("f64 bits"))
}

fn beatmap(name: &str, map: &str, timing: &[&str], events: &[&str], objects: &[&str]) -> Beatmap {
    let [version, leniency, hp, cs, od, ar, sm, tr, beat]: [&str; 9] = map
        .split(' ')
        .collect::<Vec<_>>()
        .try_into()
        .expect("map line");

    let mut text = format!(
        "osu file format v{version}\n\n[General]\nStackLeniency: {leniency}\nMode: 0\n\n\
         [Difficulty]\nHPDrainRate:{hp}\nCircleSize:{cs}\nOverallDifficulty:{od}\n\
         ApproachRate:{ar}\nSliderMultiplier:{sm}\nSliderTickRate:{tr}\n\n[Events]\n"
    );
    for e in events {
        text.push_str(e);
        text.push('\n');
    }
    text.push_str(&format!("\n[TimingPoints]\n0,{beat},4,2,0,100,1,0\n"));
    for t in timing {
        text.push_str(t);
        text.push('\n');
    }
    text.push_str("\n[HitObjects]\n");
    for o in objects {
        text.push_str(o);
        text.push('\n');
    }

    let decoded = decode_str(&text).expect("decode");
    assert!(
        decoded.warnings.is_empty(),
        "{name}: {:?}",
        decoded.warnings
    );
    let beatmap = Beatmap::from_file(decoded.beatmap).expect("beatmap");
    assert_eq!(beatmap.hit_objects.len(), objects.len(), "{name}: objects");
    beatmap
}

#[test]
fn drain_rates_match_lazer() {
    let mut lines = FIXTURE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .peekable();
    let mut cases = 0;

    while let Some(header) = lines.next() {
        let [name, drain_start, classic]: [&str; 3] = header
            .strip_prefix("case ")
            .expect("case line")
            .split(' ')
            .collect::<Vec<_>>()
            .try_into()
            .expect("case header");
        let map = lines
            .next()
            .and_then(|l| l.strip_prefix("map "))
            .expect("map line");
        let mut collect = |prefix: &str| {
            let mut out = Vec::new();
            while let Some(line) = lines.next_if(|l| l.starts_with(prefix)) {
                out.push(&line[prefix.len()..]);
            }
            out
        };
        let timing = collect("timing ");
        let events = collect("event ");
        let objects = collect("object ");
        let mut value = |key: &str| {
            bits(
                lines
                    .next()
                    .and_then(|l| l.strip_prefix(key))
                    .unwrap_or_else(|| panic!("{name}: {key}line")),
            )
        };
        let osu_drain_rate = value("osu_drain_rate ");
        let legacy_drain_rate = value("legacy_drain_rate ");
        let legacy_hp_multiplier = value("legacy_hp_multiplier ");

        let beatmap = beatmap(name, map, &timing, &events, &objects);
        let options = |kind| HealthOptions {
            kind,
            classic_slider_behaviour: classic == "1",
            drain_start_time: bits(drain_start),
        };

        let osu = HealthProcessor::new(&beatmap, options(HealthKind::Standard));
        assert_eq!(
            osu.drain_rate().to_bits(),
            osu_drain_rate.to_bits(),
            "{name} classic={classic}: osu drain rate {} != {osu_drain_rate}",
            osu.drain_rate()
        );

        let legacy = HealthProcessor::new(&beatmap, options(HealthKind::Legacy));
        assert_eq!(
            legacy.drain_rate().to_bits(),
            legacy_drain_rate.to_bits(),
            "{name} classic={classic}: legacy drain rate {} != {legacy_drain_rate}",
            legacy.drain_rate()
        );
        assert_eq!(
            legacy.hp_multiplier_normal().to_bits(),
            legacy_hp_multiplier.to_bits(),
            "{name} classic={classic}: legacy multiplier {} != {legacy_hp_multiplier}",
            legacy.hp_multiplier_normal()
        );
        cases += 1;
    }

    assert_eq!(cases, 92);
}
