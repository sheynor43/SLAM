//! Bit-exact comparison of slider paths with values computed by lazer's own `SliderPath` and
//! osu-framework's `PathApproximator` (see the header of `data/lazer-slider-paths.txt`).
//!
//! Vertex lists are compared through an FNV-1a hash of their bits to keep the fixture small.

use slam_formats::osu::{PathControlPoint, PathType, Vec2};
use slam_osu::SliderPath;

const FIXTURE: &str = include_str!("data/lazer-slider-paths.txt");

fn f32_bits(hex: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(hex, 16).expect("f32 bits"))
}

fn f64_bits(hex: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(hex, 16).expect("f64 bits"))
}

fn fnv(words: impl Iterator<Item = u64>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for w in words {
        for byte in w.to_le_bytes() {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

fn hash_vertices(vertices: &[Vec2]) -> u64 {
    fnv(vertices
        .iter()
        .map(|p| (u64::from(p.x.to_bits()) << 32) | u64::from(p.y.to_bits())))
}

fn path_type(code: &str) -> Option<PathType> {
    match code {
        "-" => None,
        "L" => Some(PathType::Linear),
        "C" => Some(PathType::Catmull),
        "P" => Some(PathType::PerfectCurve),
        "B" => Some(PathType::Bezier),
        _ => {
            let degree = code
                .strip_prefix('B')
                .expect("path type")
                .parse()
                .expect("degree");
            Some(PathType::BSpline { degree })
        }
    }
}

/// Bits of a vector, so that mismatches print readably and NaNs compare equal.
fn bits(v: Vec2) -> (u32, u32) {
    (v.x.to_bits(), v.y.to_bits())
}

#[test]
fn osutk_vector_operators() {
    let mut count = 0;
    for line in FIXTURE.lines().filter(|l| l.starts_with("probe ")) {
        let f: Vec<f32> = line.split(' ').skip(1).map(f32_bits).collect();
        let v = Vec2::new(f[0], f[1]);
        let s = f[2];

        assert_eq!(bits(v / s), bits(Vec2::new(f[3], f[4])), "{v:?} / {s}");
        assert_eq!(
            bits(v.normalized()),
            bits(Vec2::new(f[5], f[6])),
            "{v:?} normalized"
        );
        assert_eq!(v.length().to_bits(), f[7].to_bits(), "{v:?} length");
        assert_eq!(
            v.distance(v.normalized()).to_bits(),
            f[8].to_bits(),
            "{v:?} distance"
        );
        count += 1;
    }
    assert_eq!(count, 64);
}

#[test]
#[cfg_attr(
    not(all(target_os = "linux", target_env = "gnu")),
    ignore = "fixture computed with glibc libm (issue #60)"
)]
fn slider_paths_match_lazer() {
    let mut cases = 0;
    let mut lines = FIXTURE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("probe "));

    while let Some(header) = lines.next() {
        let name = header.strip_prefix("case ").expect("case header");
        let mut optimise = false;
        let mut expected = None;
        let mut points = Vec::new();
        let mut path = None;

        for line in lines.by_ref() {
            let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
            let fields: Vec<&str> = rest.split(' ').filter(|f| !f.is_empty()).collect();

            // Every expectation follows the inputs, so the path is evaluated on first use.
            let mut evaluated = || -> SliderPath {
                path.get_or_insert_with(|| {
                    SliderPath::new(std::mem::take(&mut points), expected, optimise)
                })
                .clone()
            };

            match key {
                "optimise" => optimise = fields[0] == "1",
                "expected" => expected = (fields[0] != "-").then(|| f64_bits(fields[0])),
                "point" => points.push(PathControlPoint {
                    position: Vec2::new(f32_bits(fields[0]), f32_bits(fields[1])),
                    path_type: path_type(fields[2]),
                }),
                "vertices" => {
                    let p = evaluated();
                    let vertices = p.calculated_path();
                    assert_eq!(
                        vertices.len(),
                        fields[0].parse::<usize>().unwrap(),
                        "{name}: vertex count"
                    );
                    assert_eq!(
                        format!("{:016x}", hash_vertices(vertices)),
                        fields[1],
                        "{name}: vertices"
                    );
                }
                "lengths" => {
                    let p = evaluated();
                    let lengths = p.cumulative_length();
                    assert_eq!(
                        lengths.len(),
                        fields[0].parse::<usize>().unwrap(),
                        "{name}: length count"
                    );
                    let hash = fnv(lengths.iter().map(|l| l.to_bits()));
                    assert_eq!(
                        format!("{hash:016x}"),
                        fields[1],
                        "{name}: cumulative lengths"
                    );
                }
                "calculated" => {
                    let p = evaluated();
                    assert_eq!(
                        p.calculated_distance().to_bits(),
                        f64_bits(fields[0]).to_bits(),
                        "{name}: calculated distance"
                    );
                }
                "distance" => {
                    let p = evaluated();
                    assert_eq!(
                        p.distance().to_bits(),
                        f64_bits(fields[0]).to_bits(),
                        "{name}: distance"
                    );
                }
                "ends" => {
                    let p = evaluated();
                    let actual: Vec<u64> = p.segment_ends().map(f64::to_bits).collect();
                    let expected: Vec<u64> = fields.iter().map(|f| f64_bits(f).to_bits()).collect();
                    assert_eq!(actual, expected, "{name}: segment ends");
                }
                "positions" => {
                    let p = evaluated();
                    for (i, pair) in fields.chunks(2).enumerate() {
                        let expected = Vec2::new(f32_bits(pair[0]), f32_bits(pair[1]));
                        let progress = f64::from(i as u32) / 16.0;
                        assert_eq!(
                            bits(p.position_at(progress)),
                            bits(expected),
                            "{name}: position at {progress}"
                        );
                    }
                }
                "partial" => {
                    let p = evaluated();
                    let mut part = Vec::new();
                    p.path_to_progress(&mut part, 0.25, 0.75);
                    assert_eq!(
                        part.len(),
                        fields[0].parse::<usize>().unwrap(),
                        "{name}: partial count"
                    );
                    assert_eq!(
                        format!("{:016x}", hash_vertices(&part)),
                        fields[1],
                        "{name}: partial path"
                    );
                }
                "end" => break,
                other => panic!("{name}: unknown key {other}"),
            }
        }

        cases += 1;
    }

    assert!(cases > 400, "only {cases} cases");
}
