//! Bit-exact comparison of mod settings and score multipliers with values computed by
//! osu-framework's bindables and lazer's multiplier functions (see the header of
//! `data/lazer-mods.txt`).
//!
//! Every setting case is one JSON value; it is wrapped into a lazer score block, read by
//! `slam-formats` and turned into a mod by [`Mod::from_api`], as lazer's `APIMod.ToMod` does.

use slam_formats::osr::{ScoreInfo, SettingValue};
use slam_osu::mods::{MultiplierContext, MultiplierVersion, RateAdjust};
use slam_osu::{Difficulty, GameplayMod, Mod};

const FIXTURE: &str = include_str!("data/lazer-mods.txt");

fn d(v: f64) -> String {
    format!("{:016x}", v.to_bits())
}

fn bits(hex: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(hex, 16).expect("hex bits"))
}

fn f(v: Option<f32>) -> String {
    match v {
        Some(v) => format!("{:08x}", v.to_bits()),
        None => "null".into(),
    }
}

fn float_bits(hex: &str) -> Option<f32> {
    match hex {
        "null" => None,
        _ => Some(f32::from_bits(
            u32::from_str_radix(hex, 16).expect("hex bits"),
        )),
    }
}

fn flag(v: bool) -> &'static str {
    if v { "1" } else { "0" }
}

fn read_mod(acronym: &str, settings: &[(&str, &str)]) -> Mod {
    let settings: Vec<String> = settings
        .iter()
        .map(|(key, json)| format!(r#""{key}":{json}"#))
        .collect();
    let block = format!(
        r#"{{"mods":[{{"acronym":"{acronym}","settings":{{{}}}}}]}}"#,
        settings.join(",")
    );
    let info = ScoreInfo::from_json(&block).expect("score block");
    Mod::from_api(&info.mods[0])
}

fn read_setting(acronym: &str, key: &str, json: &str) -> String {
    setting_value(&read_mod(acronym, &[(key, json)]), key)
}

fn setting_value(m: &Mod, key: &str) -> String {
    match (m, key) {
        (Mod::DifficultyAdjust(d), _) => match key {
            "circle_size" => f(d.circle_size),
            "approach_rate" => f(d.approach_rate),
            "drain_rate" => f(d.drain_rate),
            "overall_difficulty" => f(d.overall_difficulty),
            "extended_limits" => flag(d.extended_limits).into(),
            _ => panic!("unknown DA setting {key}"),
        },
        (Mod::Mirror { reflection }, _) => reflection.raw().to_string(),
        (Mod::DoubleTime(r) | Mod::HalfTime(r), "speed_change") => d(r.speed_change),
        (Mod::Nightcore { speed_change } | Mod::Daycore { speed_change }, _) => d(*speed_change),
        (Mod::DoubleTime(r) | Mod::HalfTime(r), "adjust_pitch") => flag(r.adjust_pitch).into(),
        (
            Mod::Hidden {
                only_fade_approach_circles,
            },
            _,
        ) => flag(*only_fade_approach_circles).into(),
        (Mod::Classic(c), _) => flag(match key {
            "no_slider_head_accuracy" => c.no_slider_head_accuracy,
            "classic_note_lock" => c.classic_note_lock,
            "always_play_tail_sample" => c.always_play_tail_sample,
            "fade_hit_circle_early" => c.fade_hit_circle_early,
            "classic_health" => c.classic_health,
            _ => panic!("unknown Classic setting {key}"),
        })
        .into(),
        (Mod::Easy { retries }, _) => retries.to_string(),
        _ => panic!("unexpected setting case {m:?} {key}"),
    }
}

fn multiplier(m: Mod, version: MultiplierVersion) -> String {
    multiplier_on(
        m,
        version,
        Difficulty {
            drain_rate: 5.0,
            circle_size: 5.0,
            overall_difficulty: 5.0,
            approach_rate: 5.0,
            slider_multiplier: 1.4,
            slider_tick_rate: 1.0,
        },
    )
}

fn multiplier_on(m: Mod, version: MultiplierVersion, difficulty: Difficulty) -> String {
    let context = MultiplierContext {
        version,
        difficulty_without_mods: &difficulty,
    };
    d(m.score_multiplier(&context))
}

/// A `da2` input: `<cs>,<ar>,<hp>,<od>/<base cs>,<base ar>,<base hp>,<base od>`. The settings
/// are values before the bindable's clamping; they are set like values of a score block.
fn difficulty_adjust_v2(input: &str) -> String {
    let (settings, base) = input.split_once('/').expect("da2 input");
    let mut m = Mod::from_acronym("DA");
    let keys = [
        "circle_size",
        "approach_rate",
        "drain_rate",
        "overall_difficulty",
    ];
    for (key, hex) in keys.iter().zip(settings.split(',')) {
        if let Some(v) = float_bits(hex) {
            assert!(m.set_setting(key, &SettingValue::Float(f64::from(v))));
        }
    }
    let b: Vec<f32> = base
        .split(',')
        .map(|h| float_bits(h).expect("base value"))
        .collect();
    let difficulty = Difficulty {
        circle_size: b[0],
        approach_rate: b[1],
        drain_rate: b[2],
        overall_difficulty: b[3],
        slider_multiplier: 1.4,
        slider_tick_rate: 1.0,
    };
    multiplier_on(m, MultiplierVersion::V2, difficulty)
}

fn rate(speed_change: f64) -> RateAdjust {
    RateAdjust {
        speed_change,
        adjust_pitch: false,
    }
}

#[test]
fn mods_match_lazer() {
    let mut settings = 0;
    let mut multipliers = 0;
    let mut failures = Vec::new();

    for line in FIXTURE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let fields: Vec<&str> = line.split('\t').collect();
        let (expected, actual) = match fields[..] {
            // Newtonsoft reads an integer beyond `u64` as a `BigInteger`, which lazer cannot
            // convert; `slam-formats` keeps no arbitrary precision and reads it as a double.
            ["setting", _, _, "100000000000000000000", _] => continue,
            ["setting", acronym, key, json, expected] => {
                settings += 1;
                (expected.to_owned(), read_setting(acronym, key, json))
            }
            // Two settings in one block, in order; the value of the float setting.
            ["setting2", acronym, key1, json1, key2, json2, expected] => {
                settings += 1;
                let m = read_mod(acronym, &[(key1, json1), (key2, json2)]);
                let key = if key1 == "extended_limits" {
                    key2
                } else {
                    key1
                };
                (expected.to_owned(), setting_value(&m, key))
            }
            ["mul", kind, input, expected] => {
                multipliers += 1;
                let actual = match kind {
                    "dt2" => multiplier(Mod::DoubleTime(rate(bits(input))), MultiplierVersion::V2),
                    "ht2" => multiplier(Mod::HalfTime(rate(bits(input))), MultiplierVersion::V2),
                    "v1" => multiplier(Mod::DoubleTime(rate(bits(input))), MultiplierVersion::V1),
                    "ez2" => multiplier(
                        Mod::Easy {
                            retries: input.parse().expect("retries"),
                        },
                        MultiplierVersion::V2,
                    ),
                    "hd2" => multiplier(
                        Mod::Hidden {
                            only_fade_approach_circles: input == "1",
                        },
                        MultiplierVersion::V2,
                    ),
                    "da1" => multiplier(Mod::from_acronym("DA"), MultiplierVersion::V1),
                    "da2" => difficulty_adjust_v2(input),
                    _ => panic!("unknown multiplier kind {kind}"),
                };
                (expected.to_owned(), actual)
            }
            _ => panic!("malformed fixture line: {line}"),
        };
        if expected != actual {
            failures.push(format!("{line} -> got {actual}"));
        }
    }

    assert!(
        settings > 10_000 && multipliers > 1_900,
        "fixture too small"
    );
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
