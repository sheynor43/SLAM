//! Integration tests for `.osu` `[HitObjects]` decoding.

use slam_formats::osu::{
    DecodeOptions, HitObject, HitObjectKind, HitSample, HitSampleName, PathControlPoint, PathType,
    SampleBank, Slider, Vec2, WarningKind, decode, decode_str, decode_str_with,
};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/data/osu/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// Decodes `lines` as the `[HitObjects]` section of a file with the given format version and
/// asserts that there are no warnings.
fn objects_v(version: i32, lines: &[&str]) -> Vec<HitObject> {
    let text = format!(
        "osu file format v{version}\n\n[HitObjects]\n{}\n",
        lines.join("\n")
    );
    let d = decode_str(&text).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    d.beatmap.hit_objects
}

/// Objects of a lazer-format file (version 128 and above), which keeps fractional coordinates
/// and the lazer perfect-curve rules.
fn objects(lines: &[&str]) -> Vec<HitObject> {
    objects_v(128, lines)
}

fn one(line: &str) -> HitObject {
    let mut v = objects(&[line]);
    assert_eq!(v.len(), 1);
    v.remove(0)
}

fn slider(o: &HitObject) -> &Slider {
    match &o.kind {
        HitObjectKind::Slider(s) => s,
        k => panic!("not a slider: {k:?}"),
    }
}

fn cp(x: f32, y: f32, t: Option<PathType>) -> PathControlPoint {
    PathControlPoint {
        position: Vec2::new(x, y),
        path_type: t,
    }
}

fn sample(
    name: HitSampleName,
    bank: Option<SampleBank>,
    volume: i32,
    csb: i32,
    layered: bool,
    auto: bool,
) -> HitSample {
    HitSample {
        name,
        bank: bank.unwrap_or(SampleBank::Normal),
        bank_specified: bank.is_some(),
        suffix: (csb >= 2).then_some(csb),
        volume,
        editor_auto_bank: auto,
        use_beatmap_samples: csb >= 1,
        is_layered: layered,
        filename: None,
    }
}

fn names(s: &[HitSample]) -> Vec<HitSampleName> {
    s.iter().map(|s| s.name).collect()
}

#[test]
fn circle_basic() {
    let o = one("64,80.5,1000,1,0");
    assert_eq!(o.start_time, 1000.0);
    assert_eq!(o.position, Vec2::new(64.0, 80.5));
    assert_eq!(o.kind, HitObjectKind::Circle);
    assert_eq!(o.legacy_type, 1);
    assert!(o.new_combo, "first object always starts a combo");
    assert_eq!(o.combo_offset, 0);
    assert_eq!(
        o.samples,
        vec![sample(HitSampleName::Normal, None, 0, 0, false, true)]
    );
    assert_eq!(o.end_time(), None);
}

#[test]
fn combo_flags_and_offset() {
    let v = objects(&[
        "0,0,0,1,0",  // first: forced new combo, no offset
        "0,0,1,1,0",  // plain
        "0,0,2,5,0",  // new combo, offset 0
        "0,0,3,53,0", // new combo + offset 3
        "0,0,4,49,0", // offset bits without new combo: ignored
        "0,0,5,2,0,L|10:0,1,10",
        "0,0,6,22,0,L|10:0,1,10", // slider new combo + offset 1
    ]);
    let flags: Vec<_> = v.iter().map(|o| (o.new_combo, o.combo_offset)).collect();
    assert_eq!(
        flags,
        vec![
            (true, 0),
            (false, 0),
            (true, 0),
            (true, 3),
            (false, 0),
            (false, 0),
            (true, 1)
        ]
    );
    // legacy type has the combo bits removed
    assert_eq!(v[3].legacy_type, 1);
    assert_eq!(v[6].legacy_type, 2);
}

#[test]
fn combo_after_spinner_and_spinner_rules() {
    let v = objects(&[
        "0,0,0,1,0",
        "0,0,100,12,0,200",        // spinner with new combo
        "0,0,300,17,0", // circle after spinner: forced new combo, offset 1 ignored (no NC flag)
        "0,0,400,8,0,500", // spinner without new combo flag
        "0,0,600,2,0,L|10:0,1,10", // slider after spinner
        "0,0,700,117,0", // new combo with offset 7 ... bits 4-6 = 7
    ]);
    let flags: Vec<_> = v.iter().map(|o| (o.new_combo, o.combo_offset)).collect();
    assert_eq!(
        flags,
        vec![
            (true, 0),
            (true, 0),
            (true, 0),
            (false, 0),
            (true, 0),
            (true, 7)
        ]
    );
}

#[test]
fn spinner_end_time_and_position() {
    let o = one("12,34,1000,8,0,3500.5");
    assert_eq!(o.position, Vec2::new(256.0, 192.0));
    assert_eq!(o.kind, HitObjectKind::Spinner { duration: 2500.5 });
    assert_eq!(o.end_time(), Some(3500.5));
    assert!(!o.new_combo || o.start_time == 1000.0);

    // end before start gives a zero duration
    let o = one("0,0,1000,8,0,900");
    assert_eq!(o.kind, HitObjectKind::Spinner { duration: 0.0 });
}

#[test]
fn spinner_with_hit_sample() {
    let o = one("0,0,1000,8,2,2000,3:2:5:40:");
    assert_eq!(
        o.samples,
        vec![
            sample(
                HitSampleName::Normal,
                Some(SampleBank::Drum),
                40,
                5,
                true,
                true
            ),
            sample(
                HitSampleName::Whistle,
                Some(SampleBank::Soft),
                40,
                5,
                false,
                false
            ),
        ]
    );
    assert_eq!(o.samples[0].suffix, Some(5));
    assert_eq!(o.samples[0].custom_sample_bank(), 5);
}

#[test]
fn mania_hold() {
    let o = one("64,192,1000,128,0,1500:1:2:3:4:hold.wav");
    assert_eq!(o.kind, HitObjectKind::Hold { duration: 500.0 });
    assert_eq!(o.end_time(), Some(1500.0));
    assert!(!o.new_combo);
    assert_eq!(o.legacy_type, 128);
    assert_eq!(o.samples.len(), 1);
    assert_eq!(o.samples[0].filename.as_deref(), Some("hold.wav"));
    assert_eq!(o.samples[0].volume, 4);

    // without extras, end time equals start time
    let o = one("64,192,1000,128,0");
    assert_eq!(o.kind, HitObjectKind::Hold { duration: 0.0 });
    // end time earlier than the start is clamped
    let o = one("64,192,1000,128,0,500:0:0:0:0:");
    assert_eq!(o.kind, HitObjectKind::Hold { duration: 0.0 });
    // empty field
    let o = one("64,192,1000,128,0,");
    assert_eq!(o.kind, HitObjectKind::Hold { duration: 0.0 });
}

#[test]
fn hit_sample_forms() {
    // full
    let o = one("0,0,0,1,0,2:3:4:50:");
    assert_eq!(
        o.samples,
        vec![sample(
            HitSampleName::Normal,
            Some(SampleBank::Soft),
            50,
            4,
            false,
            true
        )]
    );
    // addition bank 0 means editor auto bank and the normal bank is used for additions
    let o = one("0,0,0,1,4,2:0:0:0:");
    assert_eq!(
        names(&o.samples),
        vec![HitSampleName::Normal, HitSampleName::Finish]
    );
    assert_eq!(o.samples[1].bank, SampleBank::Soft);
    assert!(o.samples[1].editor_auto_bank);
    assert!(o.samples[1].bank_specified);
    // partial: banks only
    let o = one("0,0,0,1,0,3:1");
    assert_eq!(o.samples[0].bank, SampleBank::Drum);
    assert_eq!(o.samples[0].volume, 0);
    assert_eq!(o.samples[0].custom_sample_bank(), 0);
    // partial: banks and index
    let o = one("0,0,0,1,0,1:1:1");
    assert_eq!(o.samples[0].custom_sample_bank(), 1);
    assert_eq!(o.samples[0].suffix, None);
    assert!(o.samples[0].use_beatmap_samples);
    // undefined bank numbers become normal; none is unspecified
    let o = one("0,0,0,1,0,9:0");
    assert_eq!(o.samples[0].bank, SampleBank::Normal);
    assert!(o.samples[0].bank_specified);
    let o = one("0,0,0,1,0,0:0");
    assert!(!o.samples[0].bank_specified);
    // negative volume clamps to zero
    let o = one("0,0,0,1,0,1:1:0:-5:");
    assert_eq!(o.samples[0].volume, 0);
    // missing hit sample
    let o = one("0,0,0,1,0");
    assert!(!o.samples[0].bank_specified);
}

#[test]
fn custom_filename_sample() {
    let o = one("0,0,0,1,4,1:1:0:70:custom.wav");
    assert_eq!(o.samples.len(), 2);
    let f = &o.samples[0];
    assert_eq!(f.filename.as_deref(), Some("custom.wav"));
    assert_eq!(f.name, HitSampleName::Normal);
    assert_eq!(f.bank, SampleBank::Normal);
    assert_eq!(f.volume, 70);
    assert_eq!(f.custom_sample_bank(), 1);
    assert!(!f.editor_auto_bank && !f.is_layered && f.bank_specified);
    assert_eq!(o.samples[1].name, HitSampleName::Finish);
    assert_eq!(o.samples[1].filename, None);

    // an empty filename is no filename
    let o = one("0,0,0,1,0,1:1:0:70:");
    assert_eq!(o.samples[0].filename, None);
}

#[test]
fn sound_type_conversion() {
    let n = |t: i32| names(&one(&format!("0,0,0,1,{t}")).samples);
    use HitSampleName::*;
    assert_eq!(n(0), vec![Normal]);
    assert_eq!(n(1), vec![Normal]);
    assert_eq!(n(2), vec![Normal, Whistle]);
    assert_eq!(n(4), vec![Normal, Finish]);
    assert_eq!(n(8), vec![Normal, Clap]);
    assert_eq!(n(15), vec![Normal, Finish, Whistle, Clap]);
    // layered only when a sound type exists without the Normal flag
    let layered = |t: i32| one(&format!("0,0,0,1,{t}")).samples[0].is_layered;
    assert!(!layered(0) && !layered(1) && !layered(3));
    assert!(layered(2) && layered(8));
}

#[test]
fn slider_basic() {
    let o = one("100,100,2000,2,0,L|200:100,2,150.5");
    let s = slider(&o);
    assert_eq!(
        s.control_points,
        vec![cp(0.0, 0.0, Some(PathType::Linear)), cp(100.0, 0.0, None)]
    );
    assert_eq!(s.expected_distance, Some(150.5));
    assert_eq!(s.repeat_count, 1);
    assert_eq!(s.span_count(), 2);
    assert_eq!(s.node_samples.len(), 3);
    assert_eq!(o.samples, s.node_samples[0]);
    assert_eq!(o.end_time(), None);
}

#[test]
fn slider_repeats_and_length() {
    let rc = |l: &str| {
        let o = one(l);
        let s = slider(&o).clone();
        (s.repeat_count, s.node_samples.len(), s.expected_distance)
    };
    assert_eq!(rc("0,0,0,2,0,L|10:0,1,10"), (0, 2, Some(10.0)));
    assert_eq!(rc("0,0,0,2,0,L|10:0,0,10"), (0, 2, Some(10.0)));
    assert_eq!(rc("0,0,0,2,0,L|10:0,-3,10"), (0, 2, Some(10.0)));
    assert_eq!(rc("0,0,0,2,0,L|10:0,9000,10"), (8999, 9001, Some(10.0)));
    // zero, negative or absent length is None
    assert_eq!(rc("0,0,0,2,0,L|10:0,1,0").2, None);
    assert_eq!(rc("0,0,0,2,0,L|10:0,1,-5").2, None);
    assert_eq!(rc("0,0,0,2,0,L|10:0,1").2, None);
}

#[test]
fn slider_edge_sounds_and_sets() {
    use HitSampleName::*;
    // full: sounds and sets per node
    let o = one("0,0,0,2,0,L|10:0,2,10,0|2|4,1:2|3:0|2:3,1:2:0:0:");
    let s = slider(&o);
    assert_eq!(s.node_samples.len(), 3);
    assert_eq!(names(&s.node_samples[0]), vec![Normal]);
    assert_eq!(names(&s.node_samples[1]), vec![Normal, Whistle]);
    assert_eq!(names(&s.node_samples[2]), vec![Normal, Finish]);
    // node 0: normal:soft
    assert_eq!(s.node_samples[0][0].bank, SampleBank::Normal);
    // node 1: drum with additions from drum (add bank none)
    assert_eq!(s.node_samples[1][0].bank, SampleBank::Drum);
    assert_eq!(s.node_samples[1][1].bank, SampleBank::Drum);
    assert!(s.node_samples[1][1].editor_auto_bank);
    // node 2: soft/drum
    assert_eq!(s.node_samples[2][0].bank, SampleBank::Soft);
    assert_eq!(s.node_samples[2][1].bank, SampleBank::Drum);
    // body samples use the hit sample column (banks only for sliders)
    assert_eq!(o.samples[0].bank, SampleBank::Normal);
    assert_eq!(o.samples[0].volume, 0);
    assert_eq!(o.samples[0].custom_sample_bank(), 0);

    // partial: fewer sounds than nodes, rest default to the object's sound type
    let o = one("0,0,0,2,2,L|10:0,3,10,4");
    let s = slider(&o);
    assert_eq!(s.node_samples.len(), 4);
    assert_eq!(names(&s.node_samples[0]), vec![Normal, Finish]);
    for i in 1..4 {
        assert_eq!(names(&s.node_samples[i]), vec![Normal, Whistle]);
    }
    assert!(s.node_samples[1][0].is_layered);

    // unparsable edge sound becomes 0, extra entries are ignored
    let o = one("0,0,0,2,2,L|10:0,1,10,x|8|8|8");
    let s = slider(&o);
    assert_eq!(names(&s.node_samples[0]), vec![Normal]);
    assert_eq!(names(&s.node_samples[1]), vec![Normal, Clap]);

    // empty set entries keep the default; a set keeps the volume and index but drops the filename
    let o = one("0,0,0,2,0,L|10:0,1,10,,|3:3|,1:1:2:30:f.wav");
    let s = slider(&o);
    assert_eq!(s.node_samples[0][0].bank, SampleBank::Normal);
    let o = one("0,0,0,2,0,L|10:0,1,10,0,|3:3,1:1:2:30:f.wav");
    let s = slider(&o);
    assert_eq!(s.node_samples[1][0].bank, SampleBank::Drum);
    // slider-level banks-only column does not set volume/filename
    assert_eq!(o.samples[0].volume, 0);
    assert_eq!(o.samples[0].filename, None);
}

#[test]
fn slider_node_set_keeps_custom_values() {
    // The per-node set only has two fields: volume/custom index come from the object (0 here),
    // and nothing is inherited from a filename.
    let o = one("0,0,0,2,0,L|10:0,1,10,0|0,2:2|3:3,0:0:0:0:");
    let s = slider(&o);
    assert_eq!(s.node_samples[0][0].bank, SampleBank::Soft);
    assert_eq!(s.node_samples[1][0].bank, SampleBank::Drum);
}

#[test]
fn curve_types() {
    let t = |p: &str| {
        let o = one(&format!("0,0,0,2,0,{p},1,100"));
        slider(&o).control_points.clone()
    };
    let ty = |p: &str| t(p)[0].path_type;
    assert_eq!(ty("L|1:1"), Some(PathType::Linear));
    assert_eq!(ty("C|1:1|2:2"), Some(PathType::Catmull));
    assert_eq!(ty("B|1:1|2:2"), Some(PathType::Bezier));
    assert_eq!(ty("B3|1:1|2:2"), Some(PathType::BSpline { degree: 3 }));
    assert_eq!(ty("B0|1:1"), Some(PathType::Bezier));
    assert_eq!(ty("B-2|1:1"), Some(PathType::Bezier));
    assert_eq!(ty("Bx|1:1"), Some(PathType::Bezier));
    assert_eq!(ty("P|1:1|2:0"), Some(PathType::PerfectCurve));
    // any other letter is catmull
    assert_eq!(ty("Z|1:1"), Some(PathType::Catmull));
    assert_eq!(ty("l|1:1"), Some(PathType::Catmull));
}

#[test]
fn control_points_relative_to_position() {
    let o = one("100,50,0,2,0,B|110:60|130:70,1,100");
    assert_eq!(
        slider(&o).control_points,
        vec![
            cp(0.0, 0.0, Some(PathType::Bezier)),
            cp(10.0, 10.0, None),
            cp(30.0, 20.0, None)
        ]
    );
}

#[test]
fn multi_segment_duplicate_points() {
    // X|1:1|2:2|2:2|3:3|Y|1:1|2:2 -> implicit split at the duplicate (2,2)
    let o = one("0,0,0,2,0,B|1:1|2:2|2:2|3:3|L|1:1|2:2,1,100");
    assert_eq!(
        slider(&o).control_points,
        vec![
            cp(0.0, 0.0, Some(PathType::Bezier)),
            cp(1.0, 1.0, None),
            cp(2.0, 2.0, Some(PathType::Bezier)),
            // the duplicate (2,2) is omitted; (3,3) is the end of the second segment
            cp(3.0, 3.0, None),
            // explicit segment: its first point is the (3,3) endpoint repeated, typed L
            cp(1.0, 1.0, Some(PathType::Linear)),
            cp(2.0, 2.0, None),
        ]
    );
}

#[test]
fn duplicate_point_at_end_and_start_is_not_split() {
    // duplicate as the last pair never starts a segment
    let o = one("0,0,0,2,0,B|5:5|5:5,1,100");
    assert_eq!(
        slider(&o).control_points,
        vec![
            cp(0.0, 0.0, Some(PathType::Bezier)),
            cp(5.0, 5.0, None),
            cp(5.0, 5.0, None)
        ]
    );
    // duplicate of the slider position at the start (0,0),(0,0) is split like any other
    let o = one("0,0,0,2,0,B|0:0|5:5|6:6,1,100");
    assert_eq!(
        slider(&o).control_points,
        vec![
            cp(0.0, 0.0, Some(PathType::Bezier)),
            cp(5.0, 5.0, None),
            cp(6.0, 6.0, None)
        ]
    );
}

#[test]
fn catmull_duplicates_old_vs_new() {
    let line = "0,0,0,2,0,C|1:1|1:1|2:2|2:2|3:3,1,100";
    // old format: catmull duplicates are not split (apart from the first control point)
    let o = &objects_v(5, &[line])[0];
    let s = slider(o);
    assert_eq!(s.control_points.len(), 6);
    assert_eq!(s.control_points[2].path_type, None);
    // lazer-format versions (128+) split them
    let o = &objects_v(128, &[line])[0];
    let s = slider(o);
    assert!(
        s.control_points
            .iter()
            .filter(|c| c.path_type.is_some())
            .count()
            > 1
    );
}

#[test]
fn perfect_curve_point_counts() {
    let ty = |v: i32, p: &str| {
        let o = &objects_v(v, &[&format!("0,0,0,2,0,{p},1,100")])[0];
        slider(o).control_points[0].path_type
    };
    // exactly three points (including the implicit zero point): stays perfect
    assert_eq!(ty(128, "P|10:10|20:0"), Some(PathType::PerfectCurve));
    assert_eq!(ty(14, "P|10:10|20:0"), Some(PathType::PerfectCurve));
    // four points: bezier in every version
    assert_eq!(ty(128, "P|10:10|20:0|30:10"), Some(PathType::Bezier));
    assert_eq!(ty(14, "P|10:10|20:0|30:10"), Some(PathType::Bezier));
    // two points: bezier in old formats (below 128), kept in lazer-era format
    assert_eq!(ty(14, "P|10:10"), Some(PathType::Bezier));
    assert_eq!(ty(128, "P|10:10"), Some(PathType::PerfectCurve));
    // collinear: linear in old format only
    assert_eq!(ty(14, "P|10:0|20:0"), Some(PathType::Linear));
    assert_eq!(ty(128, "P|10:0|20:0"), Some(PathType::PerfectCurve));
    // a perfect segment followed by another segment counts the shared end point
    let o = &objects_v(128, &["0,0,0,2,0,P|10:10|L|20:0|30:0,1,100"])[0];
    let cps = &slider(o).control_points;
    assert_eq!(cps[0].path_type, Some(PathType::PerfectCurve));
    assert_eq!(cps[1].position, Vec2::new(10.0, 10.0));
    assert_eq!(cps[2].path_type, Some(PathType::Linear));
    let o = &objects_v(128, &["0,0,0,2,0,P|10:10|20:20|L|30:30,1,100"])[0];
    assert_eq!(
        slider(o).control_points[0].path_type,
        Some(PathType::Bezier)
    );
}

#[test]
fn points_before_first_type_and_no_type() {
    // no segment at all: empty path
    let o = one("0,0,0,2,0,5:5|6:6,1,100");
    assert!(slider(&o).control_points.is_empty());
}

#[test]
fn old_version_truncates_coordinates_and_applies_offset() {
    let v = objects_v(
        4,
        &[
            "100.9,200.7,1000,1,0",
            "10.5,10.5,2000,2,0,L|20.9:10.5,1,100",
            "0,0,3000,8,0,3500",
            "0,192,4000,128,0,4400",
        ],
    );
    assert_eq!(v[0].start_time, 1024.0);
    assert_eq!(v[0].position, Vec2::new(100.0, 200.0));
    assert_eq!(v[1].start_time, 2024.0);
    assert_eq!(
        slider(&v[1]).control_points[1].position,
        Vec2::new(10.0, 0.0)
    );
    assert_eq!(v[2].start_time, 3024.0);
    assert_eq!(v[2].end_time(), Some(3524.0));
    assert_eq!(v[3].start_time, 4024.0);
    // hold: end time is max(start, raw end time) + offset
    assert_eq!(v[3].kind, HitObjectKind::Hold { duration: 400.0 });
    assert_eq!(v[3].end_time(), Some(4424.0));

    // version 5: no offset, coordinates still truncated; 128 and above keep fractions
    let v = objects_v(5, &["100.9,200.7,1000,1,0"]);
    assert_eq!(v[0].start_time, 1000.0);
    assert_eq!(v[0].position, Vec2::new(100.0, 200.0));
    let v = objects_v(128, &["100.9,200.7,1000,1,0"]);
    assert_eq!(v[0].position, Vec2::new(100.9, 200.7));

    // offsets can be disabled
    let d = decode_str_with(
        "osu file format v4\n[HitObjects]\n0,0,1000,1,0\n",
        &DecodeOptions {
            apply_offsets: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(d.beatmap.hit_objects[0].start_time, 1000.0);
}

#[test]
fn hold_end_time_with_offset_quirk() {
    // lazer clamps against the offset start time but adds the offset to the raw end time
    let v = objects_v(4, &["0,0,1000,128,0,1100:0:0:0:0:"]);
    assert_eq!(v[0].kind, HitObjectKind::Hold { duration: 100.0 });
    assert_eq!(v[0].end_time(), Some(1124.0));
}

#[test]
fn objects_keep_file_order() {
    let v = objects(&["0,0,3000,1,0", "0,0,1000,1,0", "0,0,2000,1,0"]);
    let t: Vec<_> = v.iter().map(|o| o.start_time).collect();
    assert_eq!(t, vec![3000.0, 1000.0, 2000.0]);
}

#[test]
fn data_file_v14() {
    let d = decode(&data("hit_objects_v14.osu")).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    let v = d.beatmap.hit_objects;
    assert_eq!(v.len(), 6);
    assert!(v[0].new_combo);
    assert_eq!(v[0].samples[0].bank, SampleBank::Normal);
    // versions below 128 truncate coordinates
    assert_eq!(v[1].position, Vec2::new(128.0, 96.0));
    assert_eq!((v[1].new_combo, v[1].combo_offset), (true, 3));
    assert_eq!(v[1].samples[0].bank, SampleBank::Soft);
    assert_eq!(v[1].samples[0].volume, 60);
    assert_eq!(v[1].samples[0].suffix, Some(2));
    let s = slider(&v[2]);
    assert_eq!(s.repeat_count, 1);
    assert_eq!(s.expected_distance, Some(300.5));
    // the duplicate (100,0) point is the segment boundary and is not returned
    assert_eq!(s.control_points.len(), 3);
    assert_eq!(s.control_points[1].path_type, Some(PathType::Bezier));
    assert_eq!(
        names(&s.node_samples[1]),
        vec![HitSampleName::Normal, HitSampleName::Whistle]
    );
    assert_eq!(v[3].end_time(), Some(6000.0));
    assert!(v[3].new_combo);
    assert_eq!((v[4].new_combo, v[4].combo_offset), (true, 0));
    assert_eq!(v[5].kind, HitObjectKind::Hold { duration: 500.0 });
    assert_eq!(v[5].samples[0].filename.as_deref(), Some("hold.wav"));
}

#[test]
fn data_file_v4() {
    let d = decode(&data("hit_objects_v4.osu")).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    let v = d.beatmap.hit_objects;
    assert_eq!(v.len(), 4);
    assert_eq!(v[0].start_time, 1024.0);
    assert_eq!(v[0].position, Vec2::new(100.0, 200.0));
    // old format: a collinear three-point perfect curve becomes linear
    assert_eq!(
        slider(&v[1]).control_points[0].path_type,
        Some(PathType::Linear)
    );
    assert_eq!(v[2].end_time(), Some(3524.0));
    assert_eq!(v[3].start_time, 4024.0);
}

#[test]
fn broken_lines_warn_and_are_skipped() {
    use WarningKind::*;
    let d = decode(&data("hit_objects_broken.osu")).unwrap();
    let w: Vec<_> = d.warnings.iter().map(|w| (w.line, w.kind)).collect();
    assert_eq!(
        w,
        vec![
            (4, MissingField),
            (5, InvalidNumber),
            (7, NotANumber),
            (8, MissingField),
            (9, MissingField),
            (10, NumberOutOfRange),
            (11, MissingField),
            (12, MissingField),
            (13, MissingField),
            (14, MissingField),
            (15, UnknownHitObjectType),
            (16, InvalidNumber),
        ]
    );
    let t: Vec<_> = d.beatmap.hit_objects.iter().map(|o| o.start_time).collect();
    assert_eq!(t, vec![1000.0, 2100.0]);
    // failed lines do not consume the "first object" state
    assert!(d.beatmap.hit_objects[0].new_combo);
    assert!(!d.beatmap.hit_objects[1].new_combo);
}

#[test]
fn failed_object_does_not_clear_first_object_flag() {
    // a circle with broken banks fails after lazer's `lastObject` assignment but before
    // `firstObject = false`, so the next object is still "first"
    let d = decode_str("osu file format v14\n[HitObjects]\n0,0,0,1,0,x\n0,0,1,1,0\n").unwrap();
    assert_eq!(d.warnings.len(), 1);
    assert!(d.beatmap.hit_objects[0].new_combo);
}

#[test]
fn failed_spinner_still_counts_as_last_object() {
    // spinner line fails while reading its banks, but lazer already assigned `lastObject`
    let d = decode_str("osu file format v14\n[HitObjects]\n0,0,0,1,0\n0,0,1,8,0,5,x\n0,0,10,1,0\n")
        .unwrap();
    assert_eq!(d.warnings.len(), 1);
    assert_eq!(d.beatmap.hit_objects.len(), 2);
    assert!(d.beatmap.hit_objects[1].new_combo);
}

#[test]
fn garbage_never_panics() {
    let lines = [
        "",
        ",",
        ",,,,,,,,,,",
        "a,b,c,d,e",
        "1e999,0,0,1,0",
        "0,0,1e999,1,0",
        "0,0,0,99999999999,0",
        "0,0,0,1,99999999999",
        "0,0,0,-1,0",
        "0,0,0,2,0,|,1,1",
        "0,0,0,2,0,L|,1,1",
        "0,0,0,2,0,L|1:,1,1",
        "0,0,0,2,0,L|:1,1,1",
        "0,0,0,2,0,L|1:1:1,1,1",
        "0,0,0,2,0,L|1:1,2147483647,1",
        "0,0,0,2,0,L|1:1,-2147483648,1",
        "0,0,0,2,0,L|1:1,1,NaN",
        "0,0,0,2,0,L|1:1,1,1e400",
        "0,0,0,2,0,L|1:1,1,1,|||,|||,",
        "0,0,0,2,0,L|1:1,1,1,x|y,a:b",
        "0,0,0,8,0,NaN",
        "0,0,0,8,0",
        "0,0,0,128,0,:::",
        "0,0,0,128,0,NaN:1",
        "0,0,0,128,0,1:1",
        "0,0,0,1,0,:",
        "0,0,0,1,0,1:1:99999999999",
        "0,0,0,1,0,é:é",
        "0,0,0,2,0,é|1:1,1,1",
        "0,0,0,2,0,𝒜|1:1,1,1",
        "0,0,0,2,0,B2147483648|1:1,1,1",
        "0,0,0,2,0,P|1:1|P|P|P|P|P|P,1,1",
    ];
    for l in lines {
        for v in [4, 14, 128] {
            let text = format!("osu file format v{v}\n[HitObjects]\n{l}\n0,0,0,1,0\n");
            let d = decode_str(&text).unwrap();
            assert!(!d.beatmap.hit_objects.is_empty(), "{l}");
        }
    }
}

#[test]
fn old_format_catmull_first_point_duplicating_start() {
    // `C|0:0|1:1` relative to the position is (0,0),(0,0),(1,1): the first-point exemption of
    // the catmull rule lets the duplicate split the path, and the duplicate is dropped.
    let o = &objects_v(14, &["5,5,0,2,0,C|5:5|6:6,1,100"])[0];
    assert_eq!(
        slider(o).control_points,
        vec![cp(0.0, 0.0, Some(PathType::Catmull)), cp(1.0, 1.0, None)]
    );
}

#[test]
fn old_format_hold_without_end_field() {
    // end = max(start, raw time) with start = 1024: duration = 1024 + 24 - 1024
    let v = objects_v(4, &["0,0,1000,128,0"]);
    assert_eq!(v[0].kind, HitObjectKind::Hold { duration: 24.0 });
}

#[test]
fn old_format_negative_fraction_truncates_toward_zero() {
    let v = objects_v(14, &["-0.7,-5.9,0,1,0"]);
    assert_eq!(v[0].position, Vec2::new(0.0, -5.0));
}

#[test]
fn slider_edge_sets_count_mismatch() {
    // more sets than nodes: extras ignored
    let o = one("0,0,0,2,0,L|10:0,1,10,0|0,2:2|3:3|1:1|1:1");
    let s = slider(&o);
    assert_eq!(s.node_samples.len(), 2);
    assert_eq!(s.node_samples[0][0].bank, SampleBank::Soft);
    assert_eq!(s.node_samples[1][0].bank, SampleBank::Drum);
    // fewer sets than nodes: the rest keep the slider-level banks (soft)
    let o = one("0,0,0,2,0,L|10:0,3,10,0|0|0|0,3:3,2:2:0:0:");
    let s = slider(&o);
    assert_eq!(s.node_samples.len(), 4);
    assert_eq!(s.node_samples[0][0].bank, SampleBank::Drum);
    assert_eq!(s.node_samples[1][0].bank, SampleBank::Soft);
    assert_eq!(s.node_samples[3][0].bank, SampleBank::Soft);
}
