//! Bit-exact comparison of slider defaults, nested objects and slider events with values
//! computed by lazer's own `Slider`, `SliderTick`, `SliderEndCircle`, `OsuHitObject` and
//! `SliderEventGenerator` (see the header of `data/lazer-slider-nested.txt`).
//!
//! Long lists are compared through an FNV-1a hash of their text lines; the fixture lists only
//! their first and last lines, which are compared one by one for readable failures.

use slam_formats::osu::{PathControlPoint, PathType, Vec2};
use slam_osu::control_points::{ControlPoints, TimingPoint};
use slam_osu::events::{self, SliderEvent, SliderEventKind};
use slam_osu::objects::{
    ComboInfo, NestedKind, NestedObject, ObjectDefaults, OsuHitObject, OsuHitObjectKind, Slider,
};
use slam_osu::{Difficulty, SliderPath};

const FIXTURE: &str = include_str!("data/lazer-slider-nested.txt");

fn f32_bits(hex: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(hex, 16).expect("f32 bits"))
}

fn f64_bits(hex: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(hex, 16).expect("f64 bits"))
}

fn f(v: f32) -> String {
    format!("{:08x}", v.to_bits())
}

fn d(v: f64) -> String {
    format!("{:016x}", v.to_bits())
}

fn fnv(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn path_type(code: &str) -> Option<PathType> {
    match code {
        "-" => None,
        "L" => Some(PathType::Linear),
        "C" => Some(PathType::Catmull),
        "P" => Some(PathType::PerfectCurve),
        "B" => Some(PathType::Bezier),
        _ => Some(PathType::BSpline {
            degree: code
                .strip_prefix('B')
                .expect("path type")
                .parse()
                .expect("degree"),
        }),
    }
}

/// Compares a list of lines with the fixture's `count` line and the listed lines that follow
/// it (`<tag> <index> <line>`), consuming them.
fn check_list<'a>(
    name: &str,
    tag: &str,
    actual: &[String],
    lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
) {
    let count_line = lines.next().expect("count line");
    let fields: Vec<&str> = count_line.split(' ').collect();
    assert_eq!(fields[0], "count", "{name}: count line");

    for line in std::iter::from_fn(|| lines.next_if(|l| l.starts_with(tag))) {
        let (index, expected) = line[tag.len() + 1..].split_once(' ').expect("listed line");
        let index: usize = index.parse().expect("index");
        assert_eq!(
            actual.get(index).map(String::as_str),
            Some(expected),
            "{name}: {tag} {index}"
        );
    }

    assert_eq!(
        actual.len(),
        fields[1].parse::<usize>().expect("count"),
        "{name}: {tag} count"
    );
    let all: String = actual.iter().map(|l| format!("{l}\n")).collect();
    assert_eq!(
        format!("{:016x}", fnv(&all)),
        fields[2],
        "{name}: {tag} hash"
    );
}

fn nested_line(n: &NestedObject) -> String {
    let (kind, extra) = match n.kind {
        NestedKind::Head => ("head", String::new()),
        NestedKind::Tick {
            span_index,
            span_start_time,
            path_progress,
        } => (
            "tick",
            format!(" {span_index} {} {}", d(span_start_time), d(path_progress)),
        ),
        NestedKind::Repeat {
            repeat_index,
            path_progress,
        } => ("repeat", format!(" {repeat_index} {}", d(path_progress))),
        NestedKind::Tail { repeat_index } => ("tail", format!(" {repeat_index}")),
    };
    format!(
        "{kind} {} {} {} {} {} {}{extra}",
        d(n.start_time),
        f(n.position.x),
        f(n.position.y),
        d(n.defaults.time_preempt),
        d(n.defaults.time_fade_in),
        f(n.defaults.scale),
    )
}

fn event_line(e: &SliderEvent) -> String {
    let kind = match e.kind {
        SliderEventKind::Tick => "Tick",
        SliderEventKind::LegacyLastTick => "LegacyLastTick",
        SliderEventKind::Head => "Head",
        SliderEventKind::Tail => "Tail",
        SliderEventKind::Repeat => "Repeat",
    };
    format!(
        "{kind} {} {} {} {}",
        d(e.time),
        e.span_index,
        d(e.span_start_time),
        d(e.path_progress)
    )
}

/// Builds the slider of an `input` line and its `point` lines and applies its defaults.
fn apply(input: &[&str], points: Vec<PathControlPoint>) -> OsuHitObject {
    let start_time = f64_bits(input[0]);
    let beat_length = f64_bits(input[1]);
    let slider_velocity = f64_bits(input[2]);
    let format_version: i32 = input[3].parse().expect("format version");
    let expected = (input[12] != "-").then(|| f64_bits(input[12]));

    let mut control_points = ControlPoints::new();
    control_points.add_timing(TimingPoint::new(0.0, beat_length, 4, false));

    let difficulty = Difficulty {
        drain_rate: 5.0,
        circle_size: f32_bits(input[8]),
        overall_difficulty: 5.0,
        approach_rate: f32_bits(input[7]),
        slider_multiplier: f64_bits(input[5]),
        slider_tick_rate: f64_bits(input[6]),
    };

    let path = SliderPath::new(points, expected, input[13] == "1");
    let slider = Slider {
        legacy_distance: path.distance(),
        path,
        repeat_count: input[11].parse().expect("repeat count"),
        node_samples: Vec::new(),
        generate_ticks: input[4] == "1",
        slider_velocity_multiplier: slider_velocity,
        // As `Slider::new`.
        tick_distance_multiplier: if format_version < 8 {
            1.0 / slider_velocity
        } else {
            1.0
        },
        velocity: 0.0,
        tick_distance: 0.0,
        nested: Vec::new(),
        tail_samples: Vec::new(),
    };

    let mut obj = OsuHitObject {
        start_time,
        position: Vec2::new(f32_bits(input[9]), f32_bits(input[10])),
        new_combo: true,
        combo_offset: 0,
        combo: ComboInfo::default(),
        stack_height: 0,
        defaults: ObjectDefaults::default(),
        samples: Vec::new(),
        kind: OsuHitObjectKind::Slider(Box::new(slider)),
    };
    obj.apply_defaults(&control_points, &difficulty);
    obj
}

#[test]
#[cfg_attr(
    not(all(target_os = "linux", target_env = "gnu")),
    ignore = "fixture computed with glibc libm (issue #60)"
)]
fn slider_defaults_and_nested_objects_match_lazer() {
    let mut cases = 0;
    let mut lines = FIXTURE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("sort") && !l.starts_with("compares "))
        .peekable();

    while let Some(header) = lines.next() {
        if header.starts_with("events ") {
            // Generator cases are checked by `slider_events_match_lazer`.
            for line in lines.by_ref() {
                if line == "end" {
                    break;
                }
            }
            continue;
        }

        let name = header.strip_prefix("slider ").expect("slider header");
        let input: Vec<&str> = lines
            .next()
            .and_then(|l| l.strip_prefix("input "))
            .expect("input line")
            .split(' ')
            .collect();

        let mut points = Vec::new();
        while let Some(line) = lines.next_if(|l| l.starts_with("point ")) {
            let p: Vec<&str> = line.split(' ').skip(1).collect();
            points.push(PathControlPoint {
                position: Vec2::new(f32_bits(p[0]), f32_bits(p[1])),
                path_type: path_type(p[2]),
            });
        }

        let obj = apply(&input, points);
        let OsuHitObjectKind::Slider(s) = &obj.kind else {
            unreachable!()
        };

        let result: Vec<&str> = lines
            .next()
            .and_then(|l| l.strip_prefix("result "))
            .expect("result line")
            .split(' ')
            .collect();
        let end_position = obj.position + s.curve_position_at(1.0);
        let actual = [
            d(s.velocity),
            d(s.tick_distance),
            d(s.tick_distance_multiplier),
            d(s.end_time(obj.start_time)),
            d(s.span_duration(obj.start_time)),
            d(obj.defaults.time_preempt),
            d(obj.defaults.time_fade_in),
            f(obj.defaults.scale),
            f(end_position.x),
            f(end_position.y),
        ];
        let labels = [
            "velocity",
            "tick distance",
            "tick distance multiplier",
            "end time",
            "span duration",
            "preempt",
            "fade-in",
            "scale",
            "end x",
            "end y",
        ];
        for ((label, a), e) in labels.iter().zip(&actual).zip(&result) {
            assert_eq!(a, e, "{name}: {label}");
        }

        let nested: Vec<String> = s.nested.iter().map(nested_line).collect();
        check_list(name, "nested", &nested, &mut lines);
        assert_eq!(lines.next(), Some("end"), "{name}: end");
        cases += 1;
    }
    assert!(cases > 300, "{cases} slider cases");
}

#[test]
fn slider_events_match_lazer() {
    let mut cases = 0;
    let mut lines = FIXTURE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("sort") && !l.starts_with("compares "))
        .peekable();

    while let Some(header) = lines.next() {
        let Some(rest) = header.strip_prefix("events ") else {
            continue;
        };
        let fields: Vec<&str> = rest.split(' ').collect();
        let name = fields[0];

        let mut events = Vec::new();
        events::generate(
            &mut events,
            f64_bits(fields[1]),
            f64_bits(fields[2]),
            f64_bits(fields[3]),
            f64_bits(fields[4]),
            f64_bits(fields[5]),
            fields[6].parse().expect("span count"),
        );
        let actual: Vec<String> = events.iter().map(event_line).collect();
        check_list(name, "event", &actual, &mut lines);
        assert_eq!(lines.next(), Some("end"), "{name}: end");
        cases += 1;
    }
    assert_eq!(cases, 8);
}
