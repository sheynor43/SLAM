//! Integration tests for `.osu` header and timing point decoding.

use slam_formats::osu::{
    Break, DecodeError, DecodeOptions, Rgb, WarningKind, decode, decode_str, decode_str_with,
    decode_with,
};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/data/osu/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

#[test]
fn v14_all_sections() {
    let d = decode(&data("v14_full.osu")).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    let b = d.beatmap;
    assert_eq!(b.format_version, 14);
    assert_eq!(b.timing_offset, 0.0);

    let g = &b.general;
    assert_eq!(g.audio_filename, "audio.mp3");
    assert_eq!(g.audio_lead_in, 1000.0);
    assert_eq!(g.preview_time, 12345);
    assert_eq!((g.sample_set, g.sample_volume), (3, 80));
    assert_eq!(g.stack_leniency, 0.7f32);
    assert_eq!(g.mode, 0);
    assert!(g.letterbox_in_breaks && g.special_style && g.widescreen_storyboard);
    assert!(g.epilepsy_warning && g.samples_match_playback_rate);
    assert_eq!((g.countdown, g.countdown_offset), (2, 2));

    assert_eq!(b.editor.len(), 5);
    assert_eq!(b.editor[0], ("Bookmarks".into(), "1000,2000".into()));

    let m = &b.metadata;
    assert_eq!(m.title, "Full // Song"); // comments are not stripped in [Metadata]
    assert_eq!(m.title_unicode, "Full Song");
    assert_eq!(m.artist, "The Artist");
    assert_eq!(m.creator, "Mapper");
    assert_eq!((m.version.as_str(), m.source.as_str()), ("Insane", "Game"));
    assert_eq!(m.tags, "tag1 tag2");
    assert_eq!((m.beatmap_id, m.beatmap_set_id), (123, Some(456)));

    let diff = &b.difficulty;
    assert_eq!(diff.drain_rate, 6.0);
    assert_eq!(diff.circle_size, 4.2f32);
    assert_eq!(diff.overall_difficulty, 8.5);
    assert_eq!(diff.approach_rate, 9.3f32);
    assert_eq!((diff.slider_multiplier, diff.slider_tick_rate), (1.8, 1.0));

    assert_eq!(b.events.background.as_deref(), Some("sub/bg.PNG"));
    assert_eq!(
        b.events.breaks,
        [
            Break {
                start: 5000.0,
                end: 6000.0
            },
            Break {
                start: 9000.0,
                end: 9000.0
            }, // end clamped to start
        ]
    );

    let tp = &b.timing_points;
    assert_eq!(tp.len(), 3);
    assert_eq!((tp[0].time, tp[0].beat_length), (500.0, 400.0));
    assert!(tp[0].uninherited && !tp[0].kiai && !tp[0].omit_first_bar_line);
    assert_eq!(
        (tp[0].time_signature, tp[0].sample_set, tp[0].volume),
        (4, 1, 60)
    );
    assert_eq!(tp[1].beat_length, -80.0);
    assert!(!tp[1].uninherited && tp[1].kiai && !tp[1].omit_first_bar_line);
    assert_eq!(tp[1].custom_sample_index, 2);
    assert_eq!(tp[2].beat_length, 300.5);
    assert_eq!(
        (
            tp[2].time_signature,
            tp[2].sample_set,
            tp[2].custom_sample_index
        ),
        (3, 2, 1)
    );
    assert!(tp[2].kiai && tp[2].omit_first_bar_line);

    let c = &b.colours;
    assert_eq!(c.combo, [rgb(255, 0, 0), rgb(0, 255, 0), rgb(0, 0, 255)]);
    assert_eq!(c.custom, [("SliderBorder".to_string(), rgb(40, 50, 60))]);
}

#[test]
fn v4_offsets_applied() {
    let d = decode(&data("v4_old.osu")).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    let b = d.beatmap;
    assert_eq!(b.format_version, 4);
    assert_eq!(b.timing_offset, 24.0);
    assert_eq!(b.general.preview_time, 30024);
    assert_eq!(b.general.audio_filename, "old/song.mp3");
    assert_eq!(
        b.events.breaks,
        [Break {
            start: 1024.0,
            end: 2524.0
        }]
    );
    assert_eq!(b.events.background.as_deref(), Some("bg.jpg"));
    let tp = &b.timing_points;
    assert_eq!((tp[0].time, tp[1].time), (1024.0, 2024.0));
    // Missing fields fall back to defaults from [General] (SampleSet: Soft, volume 100).
    assert_eq!(
        (tp[0].time_signature, tp[0].sample_set, tp[0].volume),
        (4, 2, 100)
    );
    assert!(tp[0].uninherited);
    assert_eq!(
        (tp[1].time_signature, tp[1].sample_set, tp[1].volume),
        (3, 0, 30)
    );
    assert!(!tp[1].uninherited && tp[1].kiai);
    // OD present, AR absent: AR follows OD.
    assert_eq!(b.difficulty.approach_rate, 6.0);
    assert_eq!(b.difficulty.overall_difficulty, 6.0);
}

#[test]
fn v4_offsets_disabled() {
    let opts = DecodeOptions {
        apply_offsets: false,
        ..Default::default()
    };
    let b = decode_with(&data("v4_old.osu"), &opts).unwrap().beatmap;
    assert_eq!(b.timing_offset, 0.0);
    assert_eq!(b.general.preview_time, 30000);
    assert_eq!(
        b.events.breaks,
        [Break {
            start: 1000.0,
            end: 2500.0
        }]
    );
    assert_eq!(b.timing_points[0].time, 1000.0);
}

#[test]
fn bom_and_crlf() {
    let bytes = data("bom_crlf.osu");
    assert_eq!(&bytes[..3], [0xef, 0xbb, 0xbf]);
    let d = decode(&bytes).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert_eq!(d.beatmap.general.audio_filename, "a.mp3");
    assert_eq!(d.beatmap.metadata.title, "Windows");
    assert_eq!(d.beatmap.metadata.tags, "a//b");
    assert_eq!(d.beatmap.timing_points.len(), 1);

    // The BOM character in a &str is ignored too.
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(decode_str(&text).unwrap(), d);
}

#[test]
fn broken_lines_warn_and_good_lines_survive() {
    use WarningKind::*;
    let d = decode(&data("broken.osu")).unwrap();
    let b = &d.beatmap;
    let got: Vec<(usize, WarningKind)> = d.warnings.iter().map(|w| (w.line, w.kind)).collect();
    assert_eq!(
        got,
        [
            (7, InvalidNumber),       // AudioLeadIn: abc
            (8, NumberOutOfRange),    // PreviewTime: 99999999999
            (9, InvalidNumber),       // SampleVolume: 1.5
            (10, InvalidEnumValue),   // Countdown: Fast
            (11, UnsupportedRuleset), // Mode: 7
            (12, NotANumber),         // StackLeniency: NaN
            (14, UnknownSection),     // [Bogus]
            (18, InvalidNumber),      // HPDrainRate:oops
            (19, NumberOutOfRange),   // SliderMultiplier: 1e10
            (21, MissingField),       // "2,100": break without end
            (23, MissingField),       // "Video,0"
            (26, NanBeatLength),      // uninherited NaN
            (28, InvalidNumber),      // 4000,abc
            (29, MissingField),       // 5000
            (30, MissingField),       // empty time signature
            (31, MissingField),       // empty uninherited field
            (33, InvalidTimeSignature),
            (34, InvalidNumber), // effects "x"
            (37, InvalidColour),
            (38, InvalidColour),
        ]
    );
    assert_eq!(d.warnings[0].excerpt, "AudioLeadIn: abc");

    // Lines after the unknown section header are read as [General].
    assert_eq!(b.general.audio_filename, "from-unknown-section.mp3");
    assert_eq!(b.general.audio_lead_in, 0.0);
    assert_eq!(b.general.preview_time, -1);
    assert_eq!(b.general.sample_volume, 100);
    assert_eq!(b.general.mode, 0);
    assert_eq!(b.general.stack_leniency, 0.7f32);
    assert_eq!(b.difficulty.overall_difficulty, 7.0);
    assert_eq!(b.difficulty.approach_rate, 7.0);
    assert_eq!(b.difficulty.drain_rate, 5.0);
    assert_eq!(b.difficulty.slider_multiplier, 1.4);
    assert_eq!(
        b.events.breaks,
        [Break {
            start: 1000.0,
            end: 2000.0
        }]
    );

    // 1000 ok; 3000 is an inherited point with NaN beat length (allowed); 8000 has
    // time signature "0" (means 4); the padded 11000 line is fine.
    let times: Vec<f64> = b.timing_points.iter().map(|t| t.time).collect();
    assert_eq!(times, [1000.0, 3000.0, 8000.0, 11000.0]);
    assert!(b.timing_points[1].beat_length.is_nan());
    assert!(!b.timing_points[1].uninherited);
    assert_eq!(b.timing_points[2].time_signature, 4);
    assert_eq!(b.timing_points[3].beat_length, 250.5);

    assert_eq!(b.colours.combo, [rgb(9, 8, 7)]);
    // Combo9 is out of the combo range, so it is a custom colour.
    assert_eq!(b.colours.custom, [("Combo9".to_string(), rgb(1, 2, 3))]);
}

#[test]
fn header_errors() {
    assert_eq!(decode(b""), Err(DecodeError::NoContent));
    assert_eq!(decode(b" \r\n\n  \n"), Err(DecodeError::NoContent));
    let strict = DecodeOptions {
        require_header: true,
        ..Default::default()
    };
    assert_eq!(
        decode_with(b"[General]\n", &strict),
        Err(DecodeError::MissingHeader)
    );
    assert!(matches!(
        decode(b"osu file format vX\n"),
        Err(DecodeError::InvalidVersion(_))
    ));
    assert!(matches!(
        decode(b"osu file format v14 // c\n"),
        Err(DecodeError::InvalidVersion(_))
    ));
}

#[test]
fn header_fallback_like_lazer() {
    let d = decode_str("\n[Metadata]\nTitle:X\n").unwrap();
    assert_eq!(d.beatmap.format_version, 14);
    assert_eq!(d.beatmap.metadata.title, "X");
    assert_eq!(d.warnings[0].kind, WarningKind::MissingHeader);
}

#[test]
fn versions_and_leading_blank_lines() {
    let d = decode_str("\n  \n  osu file format v128  \n[Metadata]\nTitle:A\n").unwrap();
    assert_eq!(d.beatmap.format_version, 128);
    assert_eq!(d.beatmap.timing_offset, 0.0);
    assert_eq!(d.beatmap.metadata.title, "A");
    let old = decode_str("osu file format v0\n").unwrap().beatmap;
    assert_eq!(old.timing_offset, 24.0);
}

#[test]
fn invalid_utf8_is_lossy_with_warning() {
    let mut bytes = b"osu file format v14\n[Metadata]\nTitle:A".to_vec();
    bytes.push(0xff);
    bytes.extend_from_slice(b"B\n");
    let d = decode(&bytes).unwrap();
    assert_eq!(d.beatmap.metadata.title, "A\u{fffd}B");
    assert_eq!(d.warnings.len(), 1);
    assert_eq!(
        (d.warnings[0].line, d.warnings[0].kind),
        (0, WarningKind::InvalidEncoding)
    );
}

#[test]
fn section_and_comment_quirks() {
    let text = "osu file format v14\n\
        [Difficulty]\n  // indented comment\nHPDrainRate:3 // trailing\n//ApproachRate:1\n\
        [3]\nCircleSize:2\n[ General ]\nAudioFilename:x\n [Difficulty]\n\
        [Difficulty]\nApproachRate:9\nOverallDifficulty:2\n";
    let d = decode_str(text).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    let b = d.beatmap;
    assert_eq!(b.difficulty.drain_rate, 3.0);
    // Numeric section name 3 is [Difficulty].
    assert_eq!(b.difficulty.circle_size, 2.0);
    // "[ General ]" is trimmed by Enum.TryParse.
    assert_eq!(b.general.audio_filename, "x");
    // ApproachRate came first, so OverallDifficulty does not overwrite it.
    assert_eq!(
        (b.difficulty.approach_rate, b.difficulty.overall_difficulty),
        (9.0, 2.0)
    );
}

#[test]
fn unknown_section_resets_to_general() {
    let d = decode_str("osu file format v14\n[Foo]\nAudioFilename:g.mp3\n").unwrap();
    assert_eq!(d.beatmap.general.audio_filename, "g.mp3");
    assert_eq!(d.warnings.len(), 1);
    assert_eq!(d.warnings[0].kind, WarningKind::UnknownSection);
    assert_eq!(d.warnings[0].line, 2);
}

#[test]
fn background_rules() {
    let text = "osu file format v14\n[Events]\n4,0,Foreground,\"spr.png\",0,0\n\
        Video,0,\"old.JPG\"\n4,0,Foreground,\"other.png\",0,0\n";
    // The sprite sets the background first, a later non-video "video" overrides it,
    // and a later sprite does not.
    assert_eq!(
        decode_str(text)
            .unwrap()
            .beatmap
            .events
            .background
            .as_deref(),
        Some("old.JPG")
    );
    let text = "osu file format v14\n[Events]\nVideo,0,\"clip.avi\"\n";
    assert_eq!(decode_str(text).unwrap().beatmap.events.background, None);
}

// ---- fixtures imitating lazer's edge-case resources ----

fn title_creator(name: &str) -> (String, String) {
    let d = decode(&data(name)).unwrap();
    (d.beatmap.metadata.title, d.beatmap.metadata.creator)
}

#[test]
fn corrupted_header_falls_back_to_latest() {
    let d = decode(&data("corrupted-header.osu")).unwrap();
    assert_eq!(d.beatmap.format_version, 14);
    assert_eq!(d.beatmap.metadata.title, "Beatmap with corrupted header");
    assert_eq!(d.beatmap.metadata.creator, "Corrupt Author");
    assert_eq!(d.warnings[0].kind, WarningKind::MissingHeader);
}

#[test]
fn missing_header_first_line_is_section_switch() {
    let d = decode(&data("missing-header.osu")).unwrap();
    assert_eq!(d.beatmap.metadata.title, "Beatmap with no header");
    assert_eq!(d.beatmap.metadata.creator, "Headless Author");
    // Strict mode rejects it.
    let strict = DecodeOptions {
        require_header: true,
        ..Default::default()
    };
    assert_eq!(
        decode_with(&data("missing-header.osu"), &strict),
        Err(DecodeError::MissingHeader)
    );
}

#[test]
fn header_layout_variants() {
    assert_eq!(
        title_creator("no-empty-line-after-header.osu"),
        ("No empty line after header".into(), "Edge Author".into())
    );
    assert_eq!(
        title_creator("empty-line-instead-of-header.osu"),
        (
            "Header replaced by empty lines".into(),
            "Edge Author".into()
        )
    );
    assert_eq!(
        title_creator("empty-lines-at-start.osu"),
        ("Empty lines at start".into(), "Edge Author".into())
    );
    // Only the file that actually has a header keeps a warning-free decode.
    assert!(
        decode(&data("empty-lines-at-start.osu"))
            .unwrap()
            .warnings
            .is_empty()
    );
    assert!(
        decode(&data("no-empty-line-after-header.osu"))
            .unwrap()
            .warnings
            .is_empty()
    );
}

#[test]
fn invalid_events_still_decode() {
    let d = decode(&data("invalid-events.osu")).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings); // unknown event types are ignored
    assert_eq!(d.beatmap.events.background.as_deref(), Some("bg.jpg"));
    assert_eq!(
        d.beatmap.events.breaks,
        [Break {
            start: 12000.0,
            end: 14000.0
        }]
    );
}

#[test]
fn combo_colours_beyond_eight_become_custom() {
    let b = decode(&data("too-many-combo-colours.osu")).unwrap().beatmap;
    assert_eq!(b.colours.combo.len(), 8);
    assert_eq!(b.colours.combo[7], rgb(80, 40, 247));
    let names: Vec<&str> = b.colours.custom.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        names,
        [
            "Combo9", "Combo10", "Combo11", "Combo12", "Combo13", "Combo14"
        ]
    );
}

#[test]
fn approach_rate_resolution() {
    // Ported from osu!lazer TestUndefinedApproachRateInheritsOverallDifficulty and the
    // before/after variants in LegacyBeatmapDecoderTest.
    let d = decode(&data("undefined-approach-rate.osu"))
        .unwrap()
        .beatmap
        .difficulty;
    assert_eq!((d.approach_rate, d.overall_difficulty), (1.0, 1.0));
    for f in [
        "approach-rate-before-overall-difficulty.osu",
        "approach-rate-after-overall-difficulty.osu",
    ] {
        let d = decode(&data(f)).unwrap().beatmap.difficulty;
        assert_eq!((d.approach_rate, d.overall_difficulty), (9.0, 1.0), "{f}");
    }
}

#[test]
fn image_specified_as_video() {
    let d = decode_str("osu file format v14\n\n[Events]\nVideo,0,\"BG.jpg\",0,0\n").unwrap();
    assert_eq!(d.beatmap.events.background.as_deref(), Some("BG.jpg"));
}

#[test]
fn preview_point_with_offsets() {
    // Ported from osu!lazer TestPreviewPointWithOffsets: absent PreviewTime stays -1
    // whether or not offsets are applied.
    for apply_offsets in [false, true] {
        let opts = DecodeOptions {
            apply_offsets,
            ..Default::default()
        };
        let b = decode_with(&data("v4_old.osu"), &opts).unwrap().beatmap;
        assert_eq!(b.format_version, 4);
        let none = decode_str_with("osu file format v4\n[General]\nAudioFilename:a\n", &opts);
        assert_eq!(none.unwrap().beatmap.general.preview_time, -1);
    }
}

// ---- edge cases ----

fn preview(text: &str) -> i32 {
    decode_str(text).unwrap().beatmap.general.preview_time
}

#[test]
fn v4_preview_time_edges() {
    assert_eq!(
        preview("osu file format v4\n[General]\nPreviewTime: -1\n"),
        -1
    );
    assert_eq!(
        preview("osu file format v4\n[General]\nPreviewTime: 2147483647\n"),
        i32::MIN + 23 // unchecked int overflow in C#
    );
    assert_eq!(
        preview("osu file format v14\n[General]\nPreviewTime: 2147483647\n"),
        i32::MAX
    );
}

#[test]
fn sample_defaults_apply_only_to_later_points() {
    let text = "osu file format v14\n[General]\nSampleSet: Soft\nSampleVolume: 40\n\
        [TimingPoints]\n0,500\n[General]\nSampleSet: Drum\nSampleVolume: 70\n\
        [TimingPoints]\n1000,500\n2000,500,4,1,0,10\n";
    let tp = decode_str(text).unwrap().beatmap.timing_points;
    assert_eq!((tp[0].sample_set, tp[0].volume), (2, 40));
    assert_eq!((tp[1].sample_set, tp[1].volume), (3, 70));
    assert_eq!((tp[2].sample_set, tp[2].volume), (1, 10));
}

#[test]
fn undefined_sample_set_number_propagates() {
    let text = "osu file format v14\n[General]\nSampleSet: 7\n[TimingPoints]\n0,500\n";
    let d = decode_str(text).unwrap();
    assert!(d.warnings.is_empty());
    assert_eq!(d.beatmap.general.sample_set, 7);
    assert_eq!(d.beatmap.timing_points[0].sample_set, 7);
    // "1,2" is not a valid enum value (no numbers inside lists).
    let d = decode_str("osu file format v14\n[General]\nSampleSet: 1,2\n").unwrap();
    assert_eq!(d.warnings[0].kind, WarningKind::InvalidEnumValue);
    assert_eq!(d.beatmap.general.sample_set, 0);
}

#[test]
fn failing_lines_leave_state_untouched() {
    let text = "osu file format v14\n[Difficulty]\nOverallDifficulty:abc\n\
        [General]\nMode:abc\n";
    let d = decode_str(text).unwrap();
    assert_eq!(d.warnings.len(), 2);
    assert_eq!(d.beatmap.difficulty.overall_difficulty, 5.0);
    assert_eq!(d.beatmap.difficulty.approach_rate, 5.0);
    assert_eq!(d.beatmap.general.mode, 0);
    // A valid mode followed by an invalid one keeps the valid one.
    let d = decode_str("osu file format v14\n[General]\nMode:3\nMode:9\n").unwrap();
    assert_eq!(d.beatmap.general.mode, 3);
    assert_eq!(d.warnings[0].kind, WarningKind::UnsupportedRuleset);
}

#[test]
fn numeric_section_names() {
    let text = "osu file format v14\n[5]\n100,500\n[99]\nAudioFilename:ignored\nTitle:ignored\n\
        [0]\nAudioFilename:kept\n";
    let d = decode_str(text).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert_eq!(d.beatmap.timing_points.len(), 1);
    assert_eq!(d.beatmap.general.audio_filename, "kept");
    assert_eq!(d.beatmap.metadata.title, "");
}

#[test]
fn colour_with_unparsed_fourth_component() {
    let text = "osu file format v14\n[Colours]\nCombo1:1,2,3,x\n";
    let d = decode_str(text).unwrap();
    assert!(d.warnings.is_empty());
    assert_eq!(d.beatmap.colours.combo, [rgb(1, 2, 3)]);
}

#[test]
fn timing_point_field_errors() {
    use WarningKind::*;
    let text = "osu file format v14\n[TimingPoints]\n1000,,4\n2000,500, 0\n3000,500,4\n";
    let d = decode_str(text).unwrap();
    let got: Vec<_> = d.warnings.iter().map(|w| (w.line, w.kind)).collect();
    // Empty beat length is a format error; " 0" is not the '0' shortcut, parses to 0 and
    // the time signature constructor rejects it.
    assert_eq!(got, [(3, InvalidNumber), (4, InvalidTimeSignature)]);
    assert_eq!(d.beatmap.timing_points.len(), 1);
    assert_eq!(d.beatmap.timing_points[0].time, 3000.0);
}

// ---- encodings ----

const SAMPLE: &str = "osu file format v14\r\n\r\n[Metadata]\r\nTitle:Ünï 曲\r\n";

fn utf16(le: bool, text: &str) -> Vec<u8> {
    let mut out = if le {
        vec![0xFF, 0xFE]
    } else {
        vec![0xFE, 0xFF]
    };
    for u in text.encode_utf16() {
        out.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() });
    }
    out
}

#[test]
fn utf16_boms() {
    for le in [true, false] {
        let d = decode(&utf16(le, SAMPLE)).unwrap();
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        assert_eq!(d.beatmap.metadata.title, "Ünï 曲");
        assert_eq!(d.beatmap.format_version, 14);
    }
}

#[test]
fn utf32_boms() {
    let mut le = vec![0xFF, 0xFE, 0, 0];
    let mut be = vec![0, 0, 0xFE, 0xFF];
    for c in SAMPLE.chars() {
        le.extend_from_slice(&(c as u32).to_le_bytes());
        be.extend_from_slice(&(c as u32).to_be_bytes());
    }
    for bytes in [le, be] {
        let d = decode(&bytes).unwrap();
        assert!(d.warnings.is_empty());
        assert_eq!(d.beatmap.metadata.title, "Ünï 曲");
    }
}

#[test]
fn broken_wide_encodings_warn_without_panicking() {
    // Odd trailing byte.
    let mut bytes = utf16(true, SAMPLE);
    bytes.push(0x41);
    let d = decode(&bytes).unwrap();
    assert_eq!(d.warnings[0].kind, WarningKind::InvalidEncoding);
    assert_eq!(d.beatmap.metadata.title, "Ünï 曲");
    // Lone surrogate.
    let mut bytes = vec![0xFE, 0xFF];
    for u in "osu file format v14\n[Metadata]\nTitle:"
        .encode_utf16()
        .chain([0xD800])
    {
        bytes.extend_from_slice(&u.to_be_bytes());
    }
    let d = decode(&bytes).unwrap();
    assert_eq!(d.warnings[0].kind, WarningKind::InvalidEncoding);
    assert_eq!(d.beatmap.metadata.title, "\u{fffd}");
    // BOM only / invalid UTF-32 scalar / truncated UTF-32.
    assert_eq!(decode(&[0xFF, 0xFE]), Err(DecodeError::NoContent));
    let d = decode(&[0xFF, 0xFE, 0, 0, 0x00, 0x00, 0x11, 0x00, 1, 2]).unwrap();
    assert_eq!(d.warnings[0].kind, WarningKind::InvalidEncoding);
}
