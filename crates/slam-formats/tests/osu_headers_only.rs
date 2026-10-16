//! Tests for `DecodeOptions::headers_only`.

use slam_formats::osu::{DecodeOptions, Decoded, WarningKind, decode_str, decode_str_with};

const MAP: &str = "osu file format v14\n\n[General]\nMode: 1\n\n[Metadata]\nTitle:T\nArtist:A\nVersion:V\n\n[Difficulty]\nOverallDifficulty:7\nApproachRate:9\n\n[Events]\n0,0,\"bg.jpg\",0,0\n\n[TimingPoints]\n0,500,4,2,0,70,1,0\n\n[Colours]\nCombo1 : 1,2,3\n\n[HitObjects]\n256,192,1000,1,0,0:0:0:0:\n";

fn headers_only() -> DecodeOptions {
    DecodeOptions {
        headers_only: true,
        ..DecodeOptions::default()
    }
}

#[test]
fn headers_only_matches_full_header_fields() {
    let full = decode_str(MAP).unwrap().beatmap;
    let opts = DecodeOptions {
        headers_only: true,
        ..DecodeOptions::default()
    };
    let head = decode_str_with(MAP, &opts).unwrap().beatmap;
    assert_eq!(head.general, full.general);
    assert_eq!(head.metadata, full.metadata);
    assert_eq!(head.difficulty, full.difficulty);
    assert_eq!(head.general.mode, 1);
    assert!(head.hit_objects.is_empty());
    assert!(head.timing_points.is_empty());
    assert!(head.events.background.is_none());
    assert!(!full.hit_objects.is_empty());
    assert!(!full.timing_points.is_empty());
}

#[test]
fn default_is_full_decode() {
    assert!(!DecodeOptions::default().headers_only);
}

struct HeaderCase {
    name: &'static str,
    text: &'static str,
    check: fn(&Decoded),
}

const HEADER_CASES: &[HeaderCase] = &[
    HeaderCase {
        name: "sections in unusual order",
        text: "osu file format v14\n[HitObjects]\n256,192,1000,1,0,0:0:0:0:\n[Events]\n0,0,\"bg.jpg\",0,0\n[Metadata]\nTitle:T\nArtist:A\n[Difficulty]\nOverallDifficulty:7\nApproachRate:9\n[General]\nMode:1\n",
        check: |d| {
            assert_eq!(d.beatmap.general.mode, 1);
            assert_eq!(d.beatmap.metadata.title, "T");
            assert_eq!(d.beatmap.difficulty.approach_rate, 9.0);
        },
    },
    HeaderCase {
        name: "v4 preview time offset",
        text: "osu file format v4\n[General]\nPreviewTime: 1000\nMode: 0\n[Metadata]\nTitle:T\n[Difficulty]\nOverallDifficulty:7\n",
        check: |d| assert_eq!(d.beatmap.general.preview_time, 1024),
    },
    HeaderCase {
        name: "missing ApproachRate falls back to OverallDifficulty",
        text: "osu file format v14\n[Metadata]\nTitle:T\n[Difficulty]\nOverallDifficulty:7\n",
        check: |d| assert_eq!(d.beatmap.difficulty.approach_rate, 7.0),
    },
];

#[test]
fn header_fields_match_full_decode() {
    for case in HEADER_CASES {
        let full = decode_str(case.text).unwrap();
        let head = decode_str_with(case.text, &headers_only()).unwrap();
        assert_eq!(head.beatmap.general, full.beatmap.general, "{}", case.name);
        assert_eq!(
            head.beatmap.metadata, full.beatmap.metadata,
            "{}",
            case.name
        );
        assert_eq!(
            head.beatmap.difficulty, full.beatmap.difficulty,
            "{}",
            case.name
        );
        (case.check)(&full);
        (case.check)(&head);
    }
}

#[test]
fn malformed_metadata_line_warns_identically() {
    let text = "osu file format v14\n[Metadata]\nTitle:T\nBeatmapID:abc\n[Difficulty]\nOverallDifficulty:7\n";
    let full = decode_str(text).unwrap();
    let head = decode_str_with(text, &headers_only()).unwrap();
    assert_eq!(full.warnings.len(), 1);
    assert_eq!(full.warnings[0].kind, WarningKind::InvalidNumber);
    assert_eq!(full.warnings[0].line, 4);
    assert_eq!(head.warnings, full.warnings);
}

const SKIPPED_SECTION_CASES: &[(&str, &str)] = &[
    ("timing point", "[TimingPoints]\nabc,500,4,2,0,70,1,0\n"),
    ("hit object", "[HitObjects]\nx,192,1000,1,0,0:0:0:0:\n"),
    ("event", "[Events]\n2,abc,def\n"),
    ("colour", "[Colours]\nCombo1:1,2\n"),
];

#[test]
fn malformed_skipped_section_line_warns_only_in_full_decode() {
    for (name, section) in SKIPPED_SECTION_CASES {
        let text = format!(
            "osu file format v14\n[General]\nMode:0\n[Metadata]\nTitle:T\n[Difficulty]\nOverallDifficulty:7\n{section}"
        );
        let full = decode_str(&text).unwrap();
        let head = decode_str_with(&text, &headers_only()).unwrap();
        assert!(!full.warnings.is_empty(), "{name}: full decode should warn");
        assert!(
            head.warnings.is_empty(),
            "{name}: headers_only should not warn"
        );
    }
}
