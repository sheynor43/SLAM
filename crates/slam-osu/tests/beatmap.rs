//! Beatmap processing: control points from `[TimingPoints]`, difficulty clamping, sorting,
//! combos and samples.
//!
//! Files under `tests/data` and the asserted values come from osu!lazer 2026.1005.0-lazer:
//! osu.Game.Tests/Resources and osu.Game.Tests/Beatmaps/Formats/LegacyBeatmapDecoderTest.cs.

use std::path::PathBuf;

use slam_formats::osu::{DecodeOptions, decode_str, decode_with};
use slam_osu::control_points::{ControlPoints, TimingPoint};
use slam_osu::objects::{OsuHitObject, OsuHitObjectKind, Slider};
use slam_osu::samples::{Bank, HitSample, SampleName};
use slam_osu::{Beatmap, BeatmapError};

fn load_bytes(bytes: &[u8]) -> Beatmap {
    let options = DecodeOptions {
        apply_offsets: false,
        ..DecodeOptions::default()
    };
    let decoded = decode_with(bytes, &options).unwrap();
    Beatmap::from_file(decoded.beatmap).unwrap()
}

fn load(name: &str) -> Beatmap {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    load_bytes(&std::fs::read(path).unwrap())
}

fn load_text(text: &str) -> Beatmap {
    let decoded = decode_str(text).unwrap();
    assert!(decoded.warnings.is_empty(), "{:?}", decoded.warnings);
    Beatmap::from_file(decoded.beatmap).unwrap()
}

/// The first name lazer's `HitSampleInfo.LookupNames` gives for a sample.
fn lookup_name(sample: &HitSample) -> String {
    if let Some(filename) = &sample.filename {
        return filename.clone();
    }
    let suffix = sample.suffix.map(|s| s.to_string()).unwrap_or_default();
    format!(
        "Gameplay/{}-{}{suffix}",
        bank_name(sample.bank),
        sample.name.as_str()
    )
}

fn bank_name(bank: Bank) -> String {
    match bank {
        Bank::Normal => "normal".to_owned(),
        Bank::Soft => "soft".to_owned(),
        Bank::Drum => "drum".to_owned(),
        Bank::Other(n) => n.to_string(),
    }
}

fn slider(obj: &OsuHitObject) -> &Slider {
    match &obj.kind {
        OsuHitObjectKind::Slider(s) => s,
        other => panic!("not a slider: {other:?}"),
    }
}

// --- Control points from files ---

#[test]
fn renatus_timing_points() {
    let path = PathBuf::from(format!(
        "{}/../../../slam-refs/osu/osu.Game.Tests/Resources/Soleily - Renatus (Gamu) [Insane].osu",
        env!("CARGO_MANIFEST_DIR")
    ));
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("skipped: {} not found", path.display());
        return;
    };
    let beatmap = load_bytes(&bytes);
    let cp = &beatmap.control_points;

    assert_eq!(cp.timing_points().len(), 4);
    assert_eq!(cp.difficulty_points().len(), 5);
    assert_eq!(cp.sample_points().len(), 34);
    assert_eq!(cp.effect_points().len(), 8);

    let t = cp.timing_point_at(0.0);
    assert_eq!(t.time, 956.0);
    assert_eq!(t.beat_length, 329.67032967033);
    assert_eq!(t.time_signature, 4);
    assert!(!t.omit_first_bar_line);

    let t = cp.timing_point_at(48428.0);
    assert_eq!(t.time, 956.0);
    assert_eq!(t.beat_length, 329.67032967033);

    let t = cp.timing_point_at(119637.0);
    assert_eq!(t.time, 119637.0);
    assert_eq!(t.beat_length, 659.340659340659);
    assert_eq!(t.time_signature, 4);
    assert!(!t.omit_first_bar_line);

    let d = cp.difficulty_point_at(0.0);
    assert_eq!(d.time, 0.0);
    assert_eq!(d.slider_velocity, 1.0);
    let d = cp.difficulty_point_at(48428.0);
    assert_eq!(d.time, 0.0);
    assert_eq!(d.slider_velocity, 1.0);
    let d = cp.difficulty_point_at(116999.0);
    assert_eq!(d.time, 116999.0);
    assert!((d.slider_velocity - 0.75).abs() <= 0.1);

    let s = cp.sample_point_at(0.0);
    assert_eq!((s.time, s.bank, s.volume), (956.0, Bank::Soft, 60));
    let s = cp.sample_point_at(53373.0);
    assert_eq!((s.time, s.bank, s.volume), (53373.0, Bank::Soft, 60));
    let s = cp.sample_point_at(119637.0);
    assert_eq!((s.time, s.bank, s.volume), (119637.0, Bank::Soft, 80));

    let e = cp.effect_point_at(0.0);
    assert_eq!((e.time, e.kiai), (0.0, false));
    let e = cp.effect_point_at(53703.0);
    assert_eq!((e.time, e.kiai), (53703.0, true));
    let e = cp.effect_point_at(116637.0);
    assert_eq!((e.time, e.kiai), (95901.0, false));

    // TestDecodeBeatmapDifficulty
    let diff = beatmap.difficulty;
    assert_eq!(diff.drain_rate, 6.5);
    assert_eq!(diff.circle_size, 4.0);
    assert_eq!(diff.overall_difficulty, 8.0);
    assert_eq!(diff.approach_rate, 9.0);
    assert_eq!(diff.slider_multiplier, 1.8);
    assert_eq!(diff.slider_tick_rate, 2.0);

    // TestDecodeBeatmapHitObjects
    let objects = &beatmap.hit_objects;
    assert!(matches!(objects[0].kind, OsuHitObjectKind::Slider(_)));
    assert_eq!(
        (objects[0].position.x, objects[0].position.y),
        (192.0, 168.0)
    );
    assert_eq!(objects[0].start_time, 956.0);
    assert!(
        objects[0]
            .samples
            .iter()
            .any(|s| s.name == SampleName::Normal)
    );
    assert_eq!(
        (objects[1].position.x, objects[1].position.y),
        (304.0, 56.0)
    );
    assert_eq!(objects[1].start_time, 1285.0);
    assert!(
        objects[1]
            .samples
            .iter()
            .any(|s| s.name == SampleName::Clap)
    );
}

#[test]
fn overlapping_timing_points() {
    let cp = load("overlapping-control-points.osu").control_points;

    assert_eq!(cp.timing_points().len(), 4);
    assert_eq!(cp.difficulty_points().len(), 3);
    assert_eq!(cp.effect_points().len(), 3);
    assert_eq!(cp.sample_points().len(), 3);

    let sv = |t| cp.difficulty_point_at(t).slider_velocity;
    assert!((sv(500.0) - 1.5).abs() <= 0.1);
    assert!((sv(1500.0) - 1.5).abs() <= 0.1);
    assert!((sv(2500.0) - 0.75).abs() <= 0.1);
    assert!((sv(3500.0) - 1.5).abs() <= 0.1);

    assert!(cp.effect_point_at(500.0).kiai);
    assert!(cp.effect_point_at(1500.0).kiai);
    assert!(!cp.effect_point_at(2500.0).kiai);
    assert!(cp.effect_point_at(3500.0).kiai);

    assert_eq!(cp.sample_point_at(500.0).bank, Bank::Drum);
    assert_eq!(cp.sample_point_at(1500.0).bank, Bank::Drum);
    assert_eq!(cp.sample_point_at(2500.0).bank, Bank::Normal);
    assert_eq!(cp.sample_point_at(3500.0).bank, Bank::Drum);

    let bl = |t| cp.timing_point_at(t).beat_length;
    assert!((bl(500.0) - 500.0).abs() <= 0.1);
    assert!((bl(1500.0) - 500.0).abs() <= 0.1);
    assert!((bl(2500.0) - 250.0).abs() <= 0.1);
    assert!((bl(3500.0) - 500.0).abs() <= 0.1);
}

#[test]
fn omit_bar_line_effect() {
    let cp = load("omit-barline-control-points.osu").control_points;

    assert_eq!(cp.timing_points().len(), 6);
    assert_eq!(cp.effect_points().len(), 0);

    let omit = |t| cp.timing_point_at(t).omit_first_bar_line;
    assert!(!omit(500.0));
    assert!(omit(1500.0));
    assert!(!omit(2500.0));
    assert!(!omit(3500.0));
    assert!(!omit(4500.0));
    assert!(omit(5500.0));
}

#[test]
fn timing_point_resets_speed_multiplier() {
    let cp = load("timingpoint-speedmultiplier-reset.osu").control_points;
    assert!((cp.difficulty_point_at(0.0).slider_velocity - 0.5).abs() <= 0.1);
    assert!((cp.difficulty_point_at(2000.0).slider_velocity - 1.0).abs() <= 0.1);
}

#[test]
fn control_point_difficulty_change() {
    let cp = load("controlpoint-difficulty-multiplier.osu").control_points;
    assert_eq!(cp.difficulty_point_at(5.0).slider_velocity, 1.0);
    assert_eq!(cp.difficulty_point_at(1000.0).slider_velocity, 10.0);
    assert_eq!(
        cp.difficulty_point_at(2000.0).slider_velocity,
        1.8518518518518519
    );
    assert_eq!(cp.difficulty_point_at(3000.0).slider_velocity, 0.5);
}

#[test]
fn nan_control_points() {
    let cp = load("nan-control-points.osu").control_points;

    assert_eq!(cp.timing_points().len(), 1);
    assert_eq!(cp.difficulty_points().len(), 2);

    assert_eq!(cp.timing_point_at(1000.0).beat_length, 500.0);

    assert_eq!(cp.difficulty_point_at(2000.0).slider_velocity, 1.0);
    assert_eq!(cp.difficulty_point_at(3000.0).slider_velocity, 1.0);

    assert!(!cp.difficulty_point_at(2000.0).generate_ticks);
    assert!(cp.difficulty_point_at(3000.0).generate_ticks);
}

// --- Merging of lines with equal times (LegacyBeatmapDecoder.flushPendingPoints) ---

fn points(lines: &str) -> ControlPoints {
    load_text(&format!("osu file format v14\n\n[TimingPoints]\n{lines}\n")).control_points
}

#[test]
fn first_uninherited_line_wins_at_equal_time() {
    let cp = points("1000,500,4,1,0,100,1,0\n1000,250,3,2,0,50,1,1");
    assert_eq!(
        cp.timing_points(),
        &[TimingPoint::new(1000.0, 500.0, 4, false)]
    );
    assert_eq!(cp.sample_point_at(1000.0).bank, Bank::Normal);
    assert!(!cp.effect_point_at(1000.0).kiai);
}

#[test]
fn inherited_line_overrides_uninherited_at_equal_time() {
    // The inherited line comes first in the file and still wins.
    let cp = points("1000,-50,4,2,0,40,0,1\n1000,500,4,1,0,100,1,0");
    assert_eq!(cp.timing_points().len(), 1);
    assert_eq!(cp.difficulty_point_at(1000.0).slider_velocity, 2.0);
    let s = cp.sample_point_at(1000.0);
    assert_eq!((s.bank, s.volume), (Bank::Soft, 40));
    assert!(cp.effect_point_at(1000.0).kiai);
}

#[test]
fn last_inherited_line_wins_at_equal_time() {
    let cp = points("0,500,4,1,0,100,1,0\n1000,-50,4,2,0,40,0,0\n1000,-200,4,3,0,30,0,0");
    assert_eq!(cp.difficulty_point_at(1000.0).slider_velocity, 0.5);
    assert_eq!(cp.sample_point_at(1000.0).bank, Bank::Drum);
}

#[test]
fn redundant_lines_are_dropped() {
    let cp = points("0,500,4,1,0,100,1,0\n1000,-100,4,1,0,100,0,0\n2000,500,4,1,0,100,1,0");
    // Timing points are never redundant.
    assert_eq!(cp.timing_points().len(), 2);
    // SV 1 equals the default.
    assert_eq!(cp.difficulty_points().len(), 0);
    assert_eq!(cp.effect_points().len(), 0);
    // Only the first sample point.
    assert_eq!(cp.sample_points().len(), 1);
}

#[test]
fn sample_bank_values() {
    let cp = points("0,500,4,0,0,100,1,0\n1000,500,4,3,0,100,1,0\n2000,500,4,4,0,100,1,0");
    // `None` is the normal bank; values outside the enum keep their number.
    assert_eq!(cp.sample_point_at(0.0).bank, Bank::Normal);
    assert_eq!(cp.sample_point_at(1000.0).bank, Bank::Drum);
    assert_eq!(cp.sample_point_at(2000.0).bank, Bank::Other(4));
}

#[test]
fn out_of_order_lines() {
    let cp = points("2000,250,4,1,0,100,1,0\n0,500,4,1,0,100,1,0\n2000,1000,4,1,0,100,1,0");
    // The second line at 2000 is flushed separately and replaces the first.
    let beat_lengths: Vec<(f64, f64)> = cp
        .timing_points()
        .iter()
        .map(|p| (p.time, p.beat_length))
        .collect();
    assert_eq!(beat_lengths, [(0.0, 500.0), (2000.0, 1000.0)]);
}

// --- Difficulty ---

#[test]
fn difficulty_is_clamped() {
    let d = load("out-of-range-difficulties.osu").difficulty;
    assert_eq!(d.drain_rate, 10.0);
    assert_eq!(d.circle_size, 10.0);
    assert_eq!(d.overall_difficulty, 10.0);
    assert_eq!(d.approach_rate, 10.0);
    assert_eq!(d.slider_multiplier, 3.6);
    assert_eq!(d.slider_tick_rate, 8.0);
}

#[test]
fn difficulty_lower_bounds() {
    let b = load_text(
        "osu file format v14\n\n[Difficulty]\nHPDrainRate:-1\nCircleSize:-1\nOverallDifficulty:-1\nApproachRate:-1\nSliderMultiplier:0.1\nSliderTickRate:0.1\n",
    );
    let d = b.difficulty;
    assert_eq!(
        (
            d.drain_rate,
            d.circle_size,
            d.overall_difficulty,
            d.approach_rate
        ),
        (0.0, 0.0, 0.0, 0.0)
    );
    assert_eq!((d.slider_multiplier, d.slider_tick_rate), (0.4, 0.5));
}

#[test]
fn other_modes_are_rejected() {
    let decoded = decode_str("osu file format v14\n\n[General]\nMode: 3\n").unwrap();
    assert_eq!(
        Beatmap::from_file(decoded.beatmap),
        Err(BeatmapError::UnsupportedMode(3))
    );
}

// --- Combos ---

#[test]
fn new_combo_after_break() {
    let objects = load("break-between-objects.osu").hit_objects;
    assert!(objects[0].new_combo);
    assert!(objects[1].new_combo);
    assert!(!objects[2].new_combo);
}

#[test]
fn combo_offsets() {
    let objects = load("hitobject-combo-offset.osu").hit_objects;
    let idx = |i: usize| objects[i].combo.combo_index_with_offsets;
    assert_eq!(idx(0), 1);
    assert_eq!(idx(2), 2);
    assert_eq!(idx(4), 3);
    assert_eq!(idx(6), 4);
    assert_eq!(idx(8), 8);
    assert_eq!(idx(11), 9);
}

#[test]
fn spinner_new_combo_between_objects() {
    let objects = load("spinner-between-objects.osu").hit_objects;
    let idx = |i: usize| objects[i].combo.combo_index_with_offsets;
    assert_eq!(idx(0), 1);
    assert_eq!(idx(2), 2);
    assert_eq!(idx(3), 2);
    assert_eq!(idx(5), 3);
    assert_eq!(idx(6), 3);
    assert_eq!(idx(8), 4);
    assert_eq!(idx(9), 4);
    assert_eq!(idx(11), 5);
    assert_eq!(idx(12), 6);
    assert_eq!(idx(14), 7);
    assert_eq!(idx(15), 8);
    assert_eq!(idx(17), 9);
}

#[test]
fn combo_info_fields() {
    // Circle, circle, spinner, circle (forced new combo after the spinner), circle with a new
    // combo and a colour skip of 2.
    let b = load_text(
        "osu file format v14\n\n[HitObjects]\n0,0,1000,1,0\n0,0,2000,1,0\n256,192,3000,12,0,3500\n0,0,4000,1,0\n0,0,5000,37,0\n",
    );
    let combos: Vec<(i32, i32, i32, bool)> = b
        .hit_objects
        .iter()
        .map(|o| {
            let c = o.combo;
            (
                c.combo_index,
                c.combo_index_with_offsets,
                c.index_in_current_combo,
                c.last_in_combo,
            )
        })
        .collect();
    assert_eq!(
        combos,
        [
            (1, 1, 0, false),
            (1, 1, 1, false),
            // The spinner's new combo flag is ignored for combo colours.
            (1, 1, 2, true),
            (2, 2, 0, true),
            // The last object of the map is never marked last in combo.
            (3, 5, 0, false),
        ]
    );
}

#[test]
fn spinner_does_not_get_forced_new_combo() {
    // A spinner as the first object keeps the parser's flag (none here); the object after it
    // gets a forced new combo.
    let b = load_text(
        "osu file format v14\n\n[HitObjects]\n256,192,1000,8,0,1500\n0,0,2000,1,0\n0,0,3000,1,0\n",
    );
    let flags: Vec<bool> = b.hit_objects.iter().map(|o| o.new_combo).collect();
    assert_eq!(flags, [false, true, false]);
    // A spinner never starts a combo, even as the first object.
    let indices: Vec<i32> = b.hit_objects.iter().map(|o| o.combo.combo_index).collect();
    assert_eq!(indices, [0, 1, 1]);
}

// --- Conversion ---

#[test]
fn objects_are_sorted_stably() {
    let b = load_text(
        "osu file format v14\n\n[HitObjects]\n1,1,2000,1,0\n2,2,1000,1,0\n3,3,2000,1,0\n4,4,1000,1,0\n",
    );
    let order: Vec<(f64, f32)> = b
        .hit_objects
        .iter()
        .map(|o| (o.start_time, o.position.x))
        .collect();
    assert_eq!(
        order,
        [(1000.0, 2.0), (1000.0, 4.0), (2000.0, 1.0), (2000.0, 3.0)]
    );
}

#[test]
fn breaks_are_sorted() {
    let b = load_text(
        "osu file format v14\n\n[Events]\n2,5000,6000\n2,1000,3000\n2,1000,2000\n\n[HitObjects]\n0,0,0,1,0\n0,0,2500,1,0\n0,0,7000,1,0\n",
    );
    let breaks: Vec<(f64, f64)> = b.breaks.iter().map(|b| (b.start, b.end)).collect();
    assert_eq!(
        breaks,
        [(1000.0, 2000.0), (1000.0, 3000.0), (5000.0, 6000.0)]
    );
    // Only the break ending at 2000 is over by 2500.
    let flags: Vec<bool> = b.hit_objects.iter().map(|o| o.new_combo).collect();
    assert_eq!(flags, [true, true, true]);
}

#[test]
fn slider_difficulty_fields() {
    let text = |version: i32| {
        format!(
            "osu file format v{version}\n\n[TimingPoints]\n0,500,4,1,0,100,1,0\n1000,-50,4,1,0,100,0,0\n3000,NaN,4,1,0,100,0,0\n\n[HitObjects]\n0,0,500,2,0,L|100:0,1,100\n0,0,1000,2,0,L|100:0,1,100\n0,0,3000,2,0,L|100:0,1,100\n"
        )
    };

    let v14 = load_text(&text(14));
    let fields: Vec<(bool, f64, f64)> = v14
        .hit_objects
        .iter()
        .map(|o| {
            let s = slider(o);
            (
                s.generate_ticks,
                s.slider_velocity_multiplier,
                s.tick_distance_multiplier,
            )
        })
        .collect();
    assert_eq!(
        fields,
        [(true, 1.0, 1.0), (true, 2.0, 1.0), (false, 1.0, 1.0)]
    );

    let tick_multipliers = |version: i32| -> Vec<f64> {
        load_text(&text(version))
            .hit_objects
            .iter()
            .map(|o| slider(o).tick_distance_multiplier)
            .collect()
    };
    assert_eq!(tick_multipliers(7), [1.0, 0.5, 1.0]);
    assert_eq!(tick_multipliers(8), [1.0, 1.0, 1.0]);
}

#[test]
fn spinner_end_time() {
    let b = load_text("osu file format v14\n\n[HitObjects]\n256,192,1000,12,0,3500\n");
    assert_eq!(
        b.hit_objects[0].kind,
        OsuHitObjectKind::Spinner { end_time: 3500.0 }
    );
}

#[test]
fn hold_becomes_spinner() {
    let b = load_text("osu file format v14\n\n[HitObjects]\n64,192,1000,128,0,1500:0:0:0:0:\n");
    let obj = &b.hit_objects[0];
    assert_eq!(obj.kind, OsuHitObjectKind::Spinner { end_time: 1500.0 });
    assert_eq!((obj.position.x, obj.position.y), (64.0, 192.0));
}

// --- Samples ---

#[test]
fn control_point_custom_sample_bank() {
    let b = load("controlpoint-custom-samplebank.osu");
    let objects = &b.hit_objects;
    let first = |i: usize| lookup_name(&objects[i].samples[0]);

    assert_eq!(first(0), "Gameplay/normal-hitnormal");
    assert_eq!(first(1), "Gameplay/normal-hitnormal");
    assert_eq!(first(2), "Gameplay/normal-hitnormal2");
    assert_eq!(first(3), "Gameplay/normal-hitnormal");

    // The body samples of a slider are looked up just after its start time.
    assert_eq!(first(4), "Gameplay/soft-hitnormal11");

    // Node samples are looked up at their own times: the tail uses bank 8 rather than 11.
    // The path is a straight line no shorter than its pixel length, so its distance is the
    // expected distance. Dividing by the velocity rounds the result in binary floating point,
    // as in lazer.
    let s = slider(&objects[4]);
    assert_eq!(Some(s.legacy_distance), s.path.expected_distance());
    let duration = s.legacy_duration(&b.control_points, &b.difficulty, objects[4].start_time);
    assert_eq!(duration, 117.18749999999999);
    let nodes = &s.node_samples;
    assert_eq!(lookup_name(&nodes[0][0]), "Gameplay/soft-hitnormal11");
    assert_eq!(lookup_name(&nodes[1][0]), "Gameplay/soft-hitnormal8");
}

#[test]
fn hit_object_custom_sample_bank() {
    let objects = load("hitobject-custom-samplebank.osu").hit_objects;
    let first = |i: usize| lookup_name(&objects[i].samples[0]);
    assert_eq!(first(0), "Gameplay/normal-hitnormal");
    assert_eq!(first(1), "Gameplay/normal-hitnormal2");
    assert_eq!(first(2), "Gameplay/normal-hitnormal3");
}

#[test]
fn hit_object_file_samples() {
    let objects = load("hitobject-file-samples.osu").hit_objects;
    let first = |i: usize| &objects[i].samples[0];
    assert_eq!(lookup_name(first(0)), "hit_1.wav");
    assert_eq!(lookup_name(first(1)), "hit_2.wav");
    assert_eq!(lookup_name(first(2)), "Gameplay/normal-hitnormal2");
    assert_eq!(lookup_name(first(3)), "hit_1.wav");
    assert_eq!(first(3).volume, 70);
}

#[test]
fn slider_node_sample_names() {
    let objects = load("slider-samples.osu").hit_objects;
    let names = |i: usize| -> Vec<Vec<SampleName>> {
        slider(&objects[i])
            .node_samples
            .iter()
            .map(|node| node.iter().map(|s| s.name).collect())
            .collect()
    };
    use SampleName::{Clap, Normal, Whistle};
    assert_eq!(names(0), [vec![Normal], vec![Normal], vec![Normal]]);
    assert_eq!(
        names(1),
        [vec![Normal, Clap], vec![Normal, Clap], vec![Normal, Clap]]
    );
    assert_eq!(
        names(2),
        [vec![Normal, Whistle], vec![Normal], vec![Normal, Clap]]
    );
}

#[test]
fn null_addition_bank() {
    let objects = load("hitobject-no-addition-bank.osu").hit_objects;
    assert_eq!(objects[0].samples[0].bank, objects[0].samples[1].bank);
}

#[test]
fn invalid_bank_defaults_to_normal() {
    let objects = load("invalid-bank.osu").hit_objects;
    let banks = |i: usize| -> Vec<Bank> { objects[i].samples.iter().map(|s| s.bank).collect() };

    assert_eq!(banks(0)[0], Bank::Drum);
    assert_eq!(banks(1)[0], Bank::Normal);
    assert_eq!(banks(2)[0], Bank::Soft);
    assert_eq!(banks(3)[0], Bank::Drum);
    assert_eq!(banks(4)[0], Bank::Normal);

    assert_eq!(banks(5)[..2], [Bank::Drum, Bank::Drum]);
    assert_eq!(banks(6)[..2], [Bank::Drum, Bank::Normal]);
    assert_eq!(banks(7)[..2], [Bank::Drum, Bank::Soft]);
    assert_eq!(banks(8)[..2], [Bank::Drum, Bank::Drum]);
    assert_eq!(banks(9)[..2], [Bank::Drum, Bank::Normal]);
}

#[test]
fn sample_point_leniency() {
    let objects = load("sample-point-leniency.osu").hit_objects;
    assert_eq!(objects.len(), 1);
    assert!(objects[0].samples.iter().all(|s| s.volume == 70));
}

#[test]
fn spinner_samples_use_end_time() {
    let b = load_text(
        "osu file format v14\n\n[TimingPoints]\n0,500,4,1,0,40,1,0\n2000,-100,4,2,0,90,0,0\n\n[HitObjects]\n256,192,1000,12,0,1996\n256,192,3000,12,0,3500\n",
    );
    // End 1996 + 5 reaches the point at 2000.
    assert_eq!(b.hit_objects[0].samples[0].volume, 90);
    assert_eq!(b.hit_objects[0].samples[0].bank, Bank::Soft);
}

#[test]
fn sample_volume_and_custom_bank_resolution() {
    // Object volume wins over the point; object custom bank wins when above 0.
    let b = load_text(
        "osu file format v14\n\n[TimingPoints]\n0,500,4,1,3,40,1,0\n\n[HitObjects]\n0,0,1000,1,0,0:0:0:0:\n0,0,2000,1,0,2:0:5:55:\n",
    );
    let s0 = &b.hit_objects[0].samples[0];
    assert_eq!(
        (s0.bank, s0.volume, s0.suffix, s0.use_beatmap_samples),
        (Bank::Normal, 40, Some(3), true)
    );
    let s1 = &b.hit_objects[1].samples[0];
    assert_eq!(
        (s1.bank, s1.volume, s1.suffix, s1.use_beatmap_samples),
        (Bank::Soft, 55, Some(5), true)
    );
    assert_eq!(s1.custom_sample_bank(), 5);
}

#[test]
fn sample_lookup_leniency_boundaries() {
    // A circle looks up its samples at start + 5, a slider body at start + 6 and a spinner at
    // end + 5.
    let volume_with_point_at = |offset: i32, object: &str| -> i32 {
        let text = format!(
            "osu file format v14\n\n[TimingPoints]\n0,500,4,1,0,40,1,0\n{},-100,4,1,0,90,0,0\n\n[HitObjects]\n{object}\n",
            1000 + offset
        );
        load_text(&text).hit_objects[0].samples[0].volume
    };
    let circle = "0,0,1000,1,0";
    let slider = "0,0,1000,2,0,L|100:0,1,100";
    let spinner = "256,192,500,8,0,1000";

    assert_eq!(volume_with_point_at(5, circle), 90);
    assert_eq!(volume_with_point_at(6, circle), 40);
    assert_eq!(volume_with_point_at(6, slider), 90);
    assert_eq!(volume_with_point_at(7, slider), 40);
    assert_eq!(volume_with_point_at(5, spinner), 90);
    assert_eq!(volume_with_point_at(6, spinner), 40);
}

#[test]
fn node_sample_lookup_times() {
    // SV 1, multiplier 1.4, beat length 500: 140 px take 500 ms per span. Two spans put the
    // nodes at 1000, 1500 and (just below) 2000, each looked up 5 ms later.
    let b = load_text(
        "osu file format v14\n\n[Difficulty]\nSliderMultiplier:1.4\n\n[TimingPoints]\n0,500,4,1,0,40,1,0\n1505,-100,4,1,0,60,0,0\n2006,-100,4,1,0,80,0,0\n\n[HitObjects]\n0,0,1000,2,0,L|140:0,2,140\n",
    );
    let obj = &b.hit_objects[0];
    let s = slider(obj);
    assert_eq!(
        s.legacy_duration(&b.control_points, &b.difficulty, obj.start_time),
        999.9999999999999
    );

    let nodes = &s.node_samples;
    let volumes: Vec<i32> = nodes.iter().map(|n| n[0].volume).collect();
    assert_eq!(volumes, [40, 60, 60]);
}

#[test]
fn beatmap_version() {
    assert_eq!(load("beatmap-version-6.osu").format_version, 6);
}

#[test]
fn zero_length_slider_loses_its_repeats() {
    // Three repeats on a path ending where it starts: five nodes, of which the head (whistle)
    // and tail (clap) samples remain.
    let b = load_text(
        "osu file format v14\n\n[Difficulty]\nSliderMultiplier:1.4\n\n[TimingPoints]\n0,500,4,1,0,40,1,0\n\n[HitObjects]\n0,0,1000,2,0,L|0:0,4,0,2|4|4|4|8\n",
    );
    let s = slider(&b.hit_objects[0]);
    assert_eq!(s.legacy_distance, 0.0);
    assert_eq!(s.repeat_count, 0);
    assert_eq!(s.node_samples.len(), 2);
    let names =
        |node: &[HitSample]| -> Vec<SampleName> { node.iter().map(|sample| sample.name).collect() };
    assert!(names(&s.node_samples[0]).contains(&SampleName::Whistle));
    assert!(names(&s.node_samples[1]).contains(&SampleName::Clap));
}

#[test]
fn short_slider_keeps_its_repeats() {
    let b = load_text(
        "osu file format v14\n\n[TimingPoints]\n0,500,4,1,0,40,1,0\n\n[HitObjects]\n0,0,1000,2,0,L|1:0,4,1\n",
    );
    let s = slider(&b.hit_objects[0]);
    assert_eq!(s.legacy_distance, 1.0);
    assert_eq!(s.repeat_count, 3);
    assert_eq!(s.node_samples.len(), 5);
}

#[test]
fn catmull_optimisation_applies_only_to_the_gameplay_path() {
    // Stacked knots form bulbs that the optimisation removes; the legacy path keeps them.
    let b = load_text(
        "osu file format v14\n\n[TimingPoints]\n0,500,4,1,0,40,1,0\n\n[HitObjects]\n0,0,1000,2,0,C|0:0|100:50|100:50|200:0,1\n",
    );
    let s = slider(&b.hit_objects[0]);
    let legacy = slam_osu::SliderPath::new(
        s.path.control_points().to_vec(),
        s.path.expected_distance(),
        false,
    );
    assert_eq!(s.legacy_distance, legacy.distance());
    assert!(s.path.calculated_path().len() < legacy.calculated_path().len());
}
