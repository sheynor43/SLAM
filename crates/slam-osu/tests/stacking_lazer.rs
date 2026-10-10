//! Bit-exact comparison of stack heights and stacked positions with values computed by lazer's
//! own `OsuBeatmapProcessor.ApplyStacking` (see the header of `data/lazer-stacking.txt`).
//!
//! Every case is a small `.osu` map, decoded and processed by [`Beatmap::from_file`].

use slam_formats::osu::{DecodeOptions, decode_with};
use slam_osu::Beatmap;
use slam_osu::objects::OsuHitObjectKind;

const FIXTURE: &str = include_str!("data/lazer-stacking.txt");

fn f(v: f32) -> String {
    format!("{:08x}", v.to_bits())
}

struct Expected<'a> {
    line: &'a str,
    stack_height: i32,
    stacked: [&'a str; 4],
}

fn check_case(name: &str, map: &str, timing: &[&str], objects: &[Expected]) {
    let [version, leniency, ar, cs, sm, beat]: [&str; 6] = map
        .split(' ')
        .collect::<Vec<_>>()
        .try_into()
        .expect("map line");

    let mut text = format!(
        "osu file format v{version}\n\n[General]\nStackLeniency: {leniency}\nMode: 0\n\n\
         [Difficulty]\nHPDrainRate:5\nCircleSize:{cs}\nOverallDifficulty:5\nApproachRate:{ar}\n\
         SliderMultiplier:{sm}\nSliderTickRate:1\n\n[TimingPoints]\n0,{beat},4,2,0,100,1,0\n"
    );
    for t in timing {
        text.push_str(t);
        text.push('\n');
    }
    text.push_str("\n[HitObjects]\n");
    for o in objects {
        text.push_str(o.line);
        text.push('\n');
    }

    // The generator does not apply the 24 ms offset of format versions before 5.
    let options = DecodeOptions {
        apply_offsets: false,
        ..DecodeOptions::default()
    };
    let decoded = decode_with(text.as_bytes(), &options).expect("decode");
    assert!(
        decoded.warnings.is_empty(),
        "{name}: {:?}",
        decoded.warnings
    );
    let beatmap = Beatmap::from_file(decoded.beatmap).expect("beatmap");

    assert_eq!(
        beatmap.hit_objects.len(),
        objects.len(),
        "{name}: object count"
    );
    for (i, (h, e)) in beatmap.hit_objects.iter().zip(objects).enumerate() {
        let stacked = h.stacked_position();
        let stacked_end = h.stacked_end_position();
        assert_eq!(
            (
                h.stack_height,
                [
                    f(stacked.x),
                    f(stacked.y),
                    f(stacked_end.x),
                    f(stacked_end.y)
                ]
            ),
            (e.stack_height, e.stacked.map(str::to_owned)),
            "{name}: object {i} `{}`",
            e.line
        );

        if let OsuHitObjectKind::Slider(slider) = &h.kind {
            for n in &slider.nested {
                assert_eq!(
                    n.stack_height, h.stack_height,
                    "{name}: nested of object {i}"
                );
            }
        }
    }
}

#[test]
#[cfg_attr(
    not(all(target_os = "linux", target_env = "gnu")),
    ignore = "fixture computed with glibc libm (issue #60)"
)]
fn stacking_matches_lazer() {
    let mut cases = 0;
    let mut nonzero = 0;
    let mut lines = FIXTURE.lines().filter(|l| !l.starts_with('#')).peekable();

    while let Some(header) = lines.next() {
        let name = header.strip_prefix("case ").expect("case line");
        let map = lines
            .next()
            .and_then(|l| l.strip_prefix("map "))
            .expect("map line");

        let mut timing = Vec::new();
        while let Some(line) = lines.next_if(|l| l.starts_with("timing ")) {
            timing.push(&line["timing ".len()..]);
        }

        let mut objects = Vec::new();
        while let Some(line) = lines.next_if(|l| l.starts_with("object ")) {
            let fields: Vec<&str> = line["object ".len()..].split(' ').collect();
            let [line, height, x, y, end_x, end_y] = fields[..] else {
                panic!("{name}: bad object line `{line}`");
            };
            let stack_height = height.parse().expect("stack height");
            if stack_height != 0 {
                nonzero += 1;
            }
            objects.push(Expected {
                line,
                stack_height,
                stacked: [x, y, end_x, end_y],
            });
        }

        check_case(name, map, &timing, &objects);
        cases += 1;
    }

    assert!(cases >= 400, "only {cases} cases");
    assert!(nonzero >= 1000, "only {nonzero} stacked objects");
}
