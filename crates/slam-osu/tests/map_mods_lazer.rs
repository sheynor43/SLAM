//! Bit-exact comparison of beatmaps built with map-changing mods (Difficulty Adjust, Hard
//! Rock, Easy, Mirror) with lazer's own `GetPlayableBeatmap` order of the mod hooks, defaults
//! and stacking (see the header of `data/lazer-map-mods.txt`).
//!
//! Every case is a small `.osu` map and a list of mods; the mods are read from a lazer score
//! block, as from a replay.

use slam_formats::osr::ScoreInfo;
use slam_formats::osu::{DecodeOptions, decode_with};
use slam_osu::objects::{NestedKind, OsuHitObjectKind};
use slam_osu::{Mod, ModSet};

const FIXTURE: &str = include_str!("data/lazer-map-mods.txt");

fn f(v: f32) -> String {
    format!("{:08x}", v.to_bits())
}

fn d(v: f64) -> String {
    format!("{:016x}", v.to_bits())
}

/// A mod spec of the fixture: `HR`, `EZ`, `MR:<int>` or `DA:<cs>,<ar>,<hp>,<od>,<ext 0/1>`
/// with JSON values.
fn read_mod(spec: &str) -> Mod {
    let (acronym, settings) = match spec.split_once(':') {
        None => (spec, String::new()),
        Some(("MR", v)) => ("MR", format!(r#""reflection":{v}"#)),
        Some(("DA", v)) => {
            let v: Vec<&str> = v.split(',').collect();
            let [cs, ar, hp, od, ext] = v[..] else {
                panic!("bad DA spec {spec}");
            };
            let ext = if ext == "1" { "true" } else { "false" };
            (
                "DA",
                format!(
                    r#""circle_size":{cs},"approach_rate":{ar},"drain_rate":{hp},"overall_difficulty":{od},"extended_limits":{ext}"#
                ),
            )
        }
        _ => panic!("unknown mod spec {spec}"),
    };
    let block = format!(r#"{{"mods":[{{"acronym":"{acronym}","settings":{{{settings}}}}}]}}"#);
    let info = ScoreInfo::from_json(&block).expect("score block");
    Mod::from_api(&info.mods[0])
}

struct Case<'a> {
    name: &'a str,
    map: &'a str,
    mods: &'a str,
    timing: Vec<&'a str>,
    difficulty: &'a str,
    /// Object lines (`.osu` line and expected values), each with its `nested` lines.
    objects: Vec<(&'a str, Vec<&'a str>)>,
}

fn kind_name(kind: NestedKind) -> &'static str {
    match kind {
        NestedKind::Head => "head",
        NestedKind::Tick { .. } => "tick",
        NestedKind::Repeat { .. } => "repeat",
        NestedKind::Tail { .. } => "tail",
    }
}

/// Builds the case and returns its mismatches.
fn check_case(c: &Case, failures: &mut Vec<String>) {
    let [version, leniency, hp, cs, od, ar, sm, tick_rate, beat]: [&str; 9] = c
        .map
        .split(' ')
        .collect::<Vec<_>>()
        .try_into()
        .expect("map line");

    let mut text = format!(
        "osu file format v{version}\n\n[General]\nStackLeniency: {leniency}\nMode: 0\n\n\
         [Difficulty]\nHPDrainRate:{hp}\nCircleSize:{cs}\nOverallDifficulty:{od}\n\
         ApproachRate:{ar}\nSliderMultiplier:{sm}\nSliderTickRate:{tick_rate}\n\n\
         [TimingPoints]\n0,{beat},4,2,0,100,1,0\n"
    );
    for t in &c.timing {
        text.push_str(t);
        text.push('\n');
    }
    text.push_str("\n[HitObjects]\n");
    let mut expected_objects = Vec::new();
    for (line, nested) in &c.objects {
        let (osu_line, values) = line.split_once(' ').expect("object line");
        text.push_str(osu_line);
        text.push('\n');
        expected_objects.push((osu_line, values, nested));
    }

    // The generator does not apply the 24 ms offset of format versions before 5.
    let options = DecodeOptions {
        apply_offsets: false,
        ..DecodeOptions::default()
    };
    let decoded = decode_with(text.as_bytes(), &options).expect("decode");
    assert!(
        decoded.warnings.is_empty(),
        "{}: {:?}",
        c.name,
        decoded.warnings
    );

    let mods = c
        .mods
        .split(' ')
        .filter(|s| !s.is_empty())
        .map(read_mod)
        .collect();
    let mut set = ModSet::new(mods).expect("mod set");
    let beatmap = set.playable_beatmap(decoded.beatmap).expect("beatmap");

    let mut fail = |what: String, expected: &str, actual: String| {
        if expected != actual {
            failures.push(format!(
                "{} [{}] {what}: expected `{expected}`, got `{actual}`",
                c.name, c.mods
            ));
        }
    };

    let diff = &beatmap.difficulty;
    fail(
        "difficulty".into(),
        c.difficulty,
        [
            diff.drain_rate,
            diff.circle_size,
            diff.overall_difficulty,
            diff.approach_rate,
        ]
        .map(f)
        .join(" "),
    );

    if beatmap.hit_objects.len() != expected_objects.len() {
        fail(
            "object count".into(),
            &expected_objects.len().to_string(),
            beatmap.hit_objects.len().to_string(),
        );
        return;
    }

    for (i, (h, (osu_line, values, nested))) in beatmap
        .hit_objects
        .iter()
        .zip(&expected_objects)
        .enumerate()
    {
        let stacked = h.stacked_position();
        let stacked_end = h.stacked_end_position();
        let distance = match &h.kind {
            OsuHitObjectKind::Slider(slider) => d(slider.path.distance()),
            _ => "-".to_string(),
        };
        let actual = format!(
            "{} {} {} {} {} {} {} {} {}",
            h.stack_height,
            f(h.position.x),
            f(h.position.y),
            f(stacked.x),
            f(stacked.y),
            f(stacked_end.x),
            f(stacked_end.y),
            d(h.end_time()),
            distance
        );
        fail(format!("object {i} `{osu_line}`"), values, actual);

        let actual_nested: Vec<String> = match &h.kind {
            OsuHitObjectKind::Slider(slider) => slider
                .nested
                .iter()
                .map(|n| {
                    let stacked = n.stacked_position();
                    format!(
                        "{} {} {} {} {} {}",
                        kind_name(n.kind),
                        d(n.start_time),
                        f(n.position.x),
                        f(n.position.y),
                        f(stacked.x),
                        f(stacked.y)
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        if actual_nested.len() != nested.len() {
            fail(
                format!("nested count of object {i}"),
                &nested.len().to_string(),
                actual_nested.len().to_string(),
            );
            continue;
        }
        for (j, (e, a)) in nested.iter().zip(actual_nested).enumerate() {
            fail(format!("object {i} nested {j}"), e, a);
        }
    }
}

#[test]
#[cfg_attr(
    not(all(target_os = "linux", target_env = "gnu")),
    ignore = "fixture computed with glibc libm (issue #60)"
)]
fn map_mods_match_lazer() {
    let mut cases = 0;
    let mut sliders = 0;
    let mut failures = Vec::new();
    let mut lines = FIXTURE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .peekable();

    while let Some(header) = lines.next() {
        let name = header.strip_prefix("case ").expect("case line");
        let mut field = |prefix: &str| {
            lines
                .next()
                .and_then(|l| l.strip_prefix(prefix))
                .unwrap_or_else(|| panic!("{name}: no `{prefix}` line"))
        };
        let map = field("map ");
        let mods = field("mods").trim_start();

        let mut timing = Vec::new();
        while let Some(line) = lines.next_if(|l| l.starts_with("timing ")) {
            timing.push(&line["timing ".len()..]);
        }
        let difficulty = lines
            .next()
            .and_then(|l| l.strip_prefix("difficulty "))
            .unwrap_or_else(|| panic!("{name}: no difficulty line"));

        let mut objects = Vec::new();
        while let Some(line) = lines.next_if(|l| l.starts_with("object ")) {
            let mut nested = Vec::new();
            while let Some(n) = lines.next_if(|l| l.starts_with("nested ")) {
                nested.push(&n["nested ".len()..]);
            }
            if !nested.is_empty() {
                sliders += 1;
            }
            objects.push((&line["object ".len()..], nested));
        }

        check_case(
            &Case {
                name,
                map,
                mods,
                timing,
                difficulty,
                objects,
            },
            &mut failures,
        );
        cases += 1;
    }

    assert!(cases >= 150, "only {cases} cases");
    assert!(sliders >= 200, "only {sliders} sliders");
    assert!(
        failures.is_empty(),
        "{} mismatches, first:\n{}",
        failures.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
