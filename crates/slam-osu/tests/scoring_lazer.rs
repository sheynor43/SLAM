//! Bit-exact comparison of standardised scoring with values computed by lazer's own score
//! processor (see the header of `data/lazer-scoring.txt`).
//!
//! Every case is a small `.osu` map, decoded and processed by [`Beatmap::from_file`], with
//! classic slider behaviour off and on; the processor takes the fixture's results one by one.

use slam_formats::osu::decode_str;
use slam_osu::judgement::{HitResult, Judgement, ObjectRef};
use slam_osu::scoring::{ScoreProcessor, ScoringOptions};
use slam_osu::{Beatmap, Mod, ModSet};

const FIXTURE: &str = include_str!("data/lazer-scoring.txt");

fn bits(hex: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(hex, 16).expect("f64 bits"))
}

fn result(code: &str) -> HitResult {
    let code: usize = code.parse().expect("result code");
    HitResult::ALL[code]
}

fn counts(text: &str) -> Vec<(usize, u32)> {
    if text == "-" {
        return Vec::new();
    }
    text.split(',')
        .map(|pair| {
            let (code, count) = pair.split_once(':').expect("code:count");
            (code.parse().unwrap(), count.parse().unwrap())
        })
        .collect()
}

fn nonzero_counts(counts: &[u32; HitResult::COUNT]) -> Vec<(usize, u32)> {
    counts
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(i, n)| (i, *n))
        .collect()
}

fn fnv(text: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn beatmap(name: &str, map: &str, timing: &[&str], objects: &[&str]) -> Beatmap {
    let [version, leniency, hp, cs, od, ar, sm, tr, beat]: [&str; 9] = map
        .split(' ')
        .collect::<Vec<_>>()
        .try_into()
        .expect("map line");

    let mut text = format!(
        "osu file format v{version}\n\n[General]\nStackLeniency: {leniency}\nMode: 0\n\n\
         [Difficulty]\nHPDrainRate:{hp}\nCircleSize:{cs}\nOverallDifficulty:{od}\n\
         ApproachRate:{ar}\nSliderMultiplier:{sm}\nSliderTickRate:{tr}\n\n\
         [TimingPoints]\n0,{beat},4,2,0,100,1,0\n"
    );
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

/// A `state` line of the fixture for the processor's current values.
fn state(i: usize, p: &ScoreProcessor, mods: &ModSet) -> String {
    format!(
        "{} {} {:016x} {:016x} {:016x} {} {} {:?}",
        p.total_score(),
        p.total_score_without_mods(),
        p.accuracy().to_bits(),
        p.minimum_accuracy().to_bits(),
        p.maximum_accuracy().to_bits(),
        p.statistics().combo,
        p.statistics().max_combo,
        p.rank(mods),
    )
    .split_whitespace()
    .fold(format!("state {i}"), |line, field| line + " " + field)
}

#[test]
#[cfg_attr(
    not(all(target_os = "linux", target_env = "gnu")),
    ignore = "slider arcs of the fixture computed with glibc libm (issue #60)"
)]
fn scores_match_lazer() {
    let mut lines = FIXTURE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .peekable();
    let (mut cases, mut patterns) = (0, 0);

    while let Some(header) = lines.next() {
        let [name, classic, multiplier, hidden]: [&str; 4] = header
            .strip_prefix("case ")
            .expect("case line")
            .split(' ')
            .collect::<Vec<_>>()
            .try_into()
            .expect("case header");
        let name = format!("{name} classic={classic}");
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
        let objects = collect("object ");
        let mut field = |key: &str| {
            lines
                .next()
                .and_then(|l| l.strip_prefix(key))
                .unwrap_or_else(|| panic!("{name}: {key}line"))
        };
        let max_results: Vec<HitResult> = field("max_results ")
            .split(' ')
            .filter(|s| !s.is_empty())
            .map(result)
            .collect();
        let maxima: Vec<&str> = field("maxima ").split(' ').collect();

        let beatmap = beatmap(&name, map, &timing, &objects);
        let options = ScoringOptions {
            classic_slider_behaviour: classic == "1",
            score_multiplier: bits(multiplier),
        };
        let mods = if hidden == "1" {
            ModSet::new(vec![Mod::from_acronym("HD")]).unwrap()
        } else {
            ModSet::default()
        };
        let fresh = ScoreProcessor::new(&beatmap, options);

        // The combo portion also pins the order of the simulated perfect play.
        let (base, combo_portion, accuracy_judgements) = fresh.internal_maxima();
        assert_eq!(
            [
                format!("{:016x}", base.to_bits()),
                format!("{:016x}", combo_portion.to_bits()),
                accuracy_judgements.to_string(),
            ],
            [maxima[2], maxima[3], maxima[4]],
            "{name}: internal maxima"
        );
        let max_statistics = fresh.maximum_statistics();
        assert_eq!(
            fresh.maximum_total_score().to_string(),
            maxima[0],
            "{name}: maximum total score"
        );
        assert_eq!(
            max_statistics.max_combo.to_string(),
            maxima[1],
            "{name}: maximum combo"
        );
        assert_eq!(
            nonzero_counts(&max_statistics.counts),
            counts(maxima[5]),
            "{name}: maximum counts"
        );
        let judgements: u32 = max_statistics.counts.iter().sum();
        assert_eq!(judgements as usize, max_results.len(), "{name}: judgements");

        while lines.peek().is_some_and(|l| l.starts_with("pattern ")) {
            let (pattern, failed_from) = lines.next().unwrap()["pattern ".len()..]
                .split_once(' ')
                .expect("pattern line");
            let failed_from: i64 = failed_from.parse().unwrap();
            let results: Vec<HitResult> = lines
                .next()
                .and_then(|l| l.strip_prefix("results "))
                .expect("results line")
                .split(' ')
                .filter(|s| !s.is_empty())
                .map(result)
                .collect();
            assert_eq!(results.len(), max_results.len(), "{name} {pattern}");
            let mut expected_states = Vec::new();
            while let Some(line) = lines.next_if(|l| l.starts_with("state ")) {
                expected_states.push(line);
            }
            let hash = lines
                .next_if(|l| l.starts_with("hash "))
                .map(|l| &l["hash ".len()..]);
            let last = lines
                .next()
                .and_then(|l| l.strip_prefix("final "))
                .expect("final line");

            let mut p = fresh.clone();
            let mut states = Vec::with_capacity(results.len());
            for (i, (&r, &max)) in results.iter().zip(&max_results).enumerate() {
                let judgement = Judgement {
                    object: ObjectRef {
                        index: 0,
                        nested: None,
                    },
                    time: 0.0,
                    result: r,
                    max_result: max,
                };
                p.apply(&judgement, failed_from >= 0 && i as i64 >= failed_from);
                states.push(state(i, &p, &mods));
            }

            let context = format!("{name} {pattern}");
            match hash {
                None => assert_eq!(states, expected_states, "{context}"),
                Some(hash) => {
                    let n = states.len();
                    assert_eq!(&states[..16], &expected_states[..16], "{context}: first");
                    assert_eq!(&states[n - 16..], &expected_states[16..], "{context}: last");
                    assert_eq!(
                        format!("{:016x}", fnv(&states.join("\n"))),
                        hash,
                        "{context}: hash"
                    );
                }
            }
            let (final_state, final_counts) = last.rsplit_once(' ').expect("final counts");
            let got = states.last().map_or_else(String::new, |s| {
                s.splitn(3, ' ').nth(2).unwrap_or_default().to_owned()
            });
            assert_eq!(got, final_state, "{context}: final");
            assert_eq!(
                nonzero_counts(&p.statistics().counts),
                counts(final_counts),
                "{context}: final counts"
            );
            patterns += 1;
        }
        cases += 1;
    }

    assert_eq!((cases, patterns), (92, 514));
}
