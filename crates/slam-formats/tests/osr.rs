//! Integration tests for the `.osr` codec.

use lzma_rust2::{EncodeMode, LzmaOptions, LzmaWriter, MfType};
use slam_formats::osr::{
    ButtonState, FrameData, GeneratedFrame, MAX_DECOMPRESSED_SIZE, MAX_WARNINGS, OsrError, Replay,
    ReplayFrame, TimedFrame, WarningKind, decode, encode, frames_from_timeline, lazer_timeline,
    ticks_from_unix_millis, unix_millis_from_ticks,
};
use std::io::Write;

fn frame(delta: i32, x: f32, y: f32, buttons: i32) -> ReplayFrame {
    ReplayFrame {
        delta,
        x,
        y,
        buttons,
    }
}

fn sample(version: i32) -> Replay {
    Replay {
        mode: 0,
        version,
        beatmap_hash: Some("0123456789abcdef0123456789abcdef".into()),
        player_name: Some("Player Ünï".into()),
        replay_hash: Some("fedcba9876543210fedcba9876543210".into()),
        count_300: 300,
        count_100: 20,
        count_50: 3,
        count_geki: 40,
        count_katu: 5,
        count_miss: 1,
        total_score: 1_234_567,
        max_combo: 321,
        perfect: true,
        mods: 64 | 8,
        life_bar: Some("0|1,1000|0.95,2000|0.5,".into()),
        date_ticks: 638_000_000_000_000_000,
        frames: FrameData::Frames(vec![
            frame(0, 256.0, -500.0, 0),
            frame(16, 100.5, 200.25, 1),
            frame(-3, 0.00001, 131072.0, 5),
            frame(17, -12.5, 0.0, 0),
        ]),
        seed: Some(3_001),
        online_id: Some(1),
        lazer_block: None,
        trailing: Vec::new(),
    }
}

fn roundtrip(r: &Replay) {
    let bytes = encode(r);
    let d = decode(&bytes).unwrap();
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert_eq!(&d.replay, r);
}

/// Builds a file by hand: version-dependent online id handled by the caller via `tail`.
fn raw_file(version: i32, compressed: Option<&[u8]>, tail: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8];
    b.extend(version.to_le_bytes());
    for s in ["h", "p", "r"] {
        b.extend([0x0b, 1]);
        b.extend(s.as_bytes());
    }
    for c in [1u16, 2, 3, 4, 5, 6] {
        b.extend(c.to_le_bytes());
    }
    b.extend(1000i32.to_le_bytes());
    b.extend(7u16.to_le_bytes());
    b.push(1);
    b.extend(0i32.to_le_bytes());
    b.push(0); // null life bar
    b.extend(638_000_000_000_000_000i64.to_le_bytes());
    match compressed {
        None => b.extend((-1i32).to_le_bytes()),
        Some(c) => {
            b.extend((c.len() as i32).to_le_bytes());
            b.extend(c);
        }
    }
    b.extend(tail);
    b
}

fn lzma(text: &str, end_marker: bool) -> Vec<u8> {
    let o = LzmaOptions::new(1 << 21, 3, 0, 2, EncodeMode::Normal, 255, MfType::Bt4, 0);
    let size = (!end_marker).then_some(text.len() as u64);
    let mut w = LzmaWriter::new_use_header(Vec::new(), &o, size).unwrap();
    w.write_all(text.as_bytes()).unwrap();
    w.finish().unwrap()
}

fn frames_of(text: &str, mode: u8) -> (Replay, Vec<WarningKind>) {
    let mut bytes = raw_file(20151228, Some(&lzma(text, false)), &1i64.to_le_bytes());
    bytes[0] = mode;
    let d = decode(&bytes).unwrap();
    let kinds = d.warnings.iter().map(|w| w.kind).collect();
    (d.replay, kinds)
}

#[test]
fn roundtrip_stable() {
    roundtrip(&sample(20151228));
}

#[test]
fn roundtrip_old_versions() {
    let mut r = sample(20111111);
    r.online_id = None;
    roundtrip(&r);
    let mut r = sample(20130101);
    r.online_id = Some(-5);
    roundtrip(&r);
    let mut r = sample(20140721);
    r.online_id = Some(1 << 40);
    roundtrip(&r);
}

#[test]
fn roundtrip_lazer_block_and_trailing() {
    let mut r = sample(30_000_019);
    r.lazer_block = Some(vec![1, 2, 3, 0xff]);
    r.trailing = vec![9, 8, 7];
    roundtrip(&r);
    r.lazer_block = None;
    roundtrip(&r);
    r.lazer_block = Some(Vec::new());
    roundtrip(&r);
}

#[test]
fn roundtrip_nulls_and_empty() {
    let mut r = sample(20151228);
    r.beatmap_hash = None;
    r.player_name = Some(String::new());
    r.replay_hash = None;
    r.life_bar = None;
    r.seed = None;
    for f in [
        FrameData::Null,
        FrameData::Empty,
        FrameData::Frames(Vec::new()),
    ] {
        r.frames = f;
        roundtrip(&r);
    }
    r.seed = Some(-7);
    r.frames = FrameData::Frames(Vec::new());
    roundtrip(&r);
}

#[test]
fn roundtrip_mania_coordinate_limit() {
    let mut r = sample(20151228);
    r.mode = 3;
    r.frames = FrameData::Frames(vec![frame(10, 1_048_575.0, 0.0, 0)]);
    roundtrip(&r);
    // the same value is out of range for other modes and dropped with a warning
    let (rep, kinds) = frames_of("10|1048575|0|0,", 0);
    assert!(rep.frames().is_empty());
    assert_eq!(kinds, [WarningKind::MalformedFrame]);
}

#[test]
fn header_is_byte_exact_on_reencode() {
    let compressed = lzma("16|1|2|3,", false);
    let tail = 5i64.to_le_bytes();
    let original = raw_file(20151228, Some(&compressed), &tail);
    let reenc = encode(&decode(&original).unwrap().replay);
    // everything before the compressed array length and after it is identical
    let head = original.len() - tail.len() - compressed.len() - 4;
    assert_eq!(reenc[..head], original[..head]);
    assert_eq!(reenc[reenc.len() - tail.len()..], tail);
}

#[test]
fn lzma_layout_matches_lazer() {
    let bytes = encode(&sample(20151228));
    let start = bytes.len() - 8 - decode_len(&bytes);
    let c = &bytes[start..start + 13];
    assert_eq!(c[0], 0x5d);
    assert_eq!(u32::from_le_bytes(c[1..5].try_into().unwrap()), 1 << 21);
    let text_len = i64::from_le_bytes(c[5..13].try_into().unwrap());
    assert_eq!(
        text_len as usize,
        "0|256|-500|0,16|100.5|200.25|1,-3|0.00001|131072|5,17|-12.5|0|0,-12345|0|0|3001".len()
    );
}

/// Length of the compressed array of a v20151228 file produced by `encode`.
fn decode_len(bytes: &[u8]) -> usize {
    // walk back: i64 online id precedes nothing; find the length prefix by search
    for i in (0..bytes.len() - 12).rev() {
        let len = i32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        if len > 0 && i + 4 + len as usize + 8 == bytes.len() && bytes[i + 4] == 0x5d {
            return len as usize;
        }
    }
    panic!("not found");
}

#[test]
fn frame_text_rules() {
    let (r, w) = frames_of("16|1|2|3,-12345|0|0|42,,5|6|7|8,", 0);
    assert_eq!(r.frames(), [frame(16, 1.0, 2.0, 3), frame(5, 6.0, 7.0, 8)]);
    assert_eq!(r.seed, Some(42));
    assert!(w.is_empty());

    // short non-empty entry
    let (r, w) = frames_of("16|1|2,5|6|7|8", 0);
    assert_eq!(r.frames().len(), 1);
    assert_eq!(w, [WarningKind::ShortFrame]);

    // bad seed / several seeds
    let (r, w) = frames_of("-12345|0|0|x,-12345|0|0|1,-12345|0|0|2", 0);
    assert_eq!(r.seed, Some(2));
    assert_eq!(
        w,
        [
            WarningKind::InvalidSeed,
            WarningKind::MultipleSeeds,
            WarningKind::MultipleSeeds
        ]
    );
    let (r, w) = frames_of("-12345|0|0|x", 0);
    assert_eq!(r.seed, None);
    assert_eq!(w, [WarningKind::InvalidSeed]);

    // fractional deltas round half to even
    let (r, _) = frames_of("16.5|0|0|0,17.5|0|0|0,-0.5|0|0|0,1E1|0|0|0", 0);
    let d: Vec<i32> = r.frames().iter().map(|f| f.delta).collect();
    assert_eq!(d, [16, 18, 0, 10]);

    // malformed frames are dropped with a warning
    let (r, w) = frames_of("a|0|0|0,1|b|0|0,1|0|c|0,1|0|0|d,1|0|-131073|0,2|3|4|5", 0);
    assert_eq!(r.frames(), [frame(2, 3.0, 4.0, 5)]);
    assert_eq!(w, [WarningKind::MalformedFrame; 5]);
}

#[test]
fn empty_frame_text_is_present_but_empty() {
    let (r, w) = frames_of("", 0);
    assert_eq!(r.frames, FrameData::Frames(Vec::new()));
    assert!(w.is_empty());
}

#[test]
fn lzma_unknown_size_with_end_marker() {
    let text = "16|1|2|3,16|4|5|6,";
    let c = lzma(text, true);
    assert_eq!(i64::from_le_bytes(c[5..13].try_into().unwrap()), -1);
    let bytes = raw_file(20151228, Some(&c), &1i64.to_le_bytes());
    let d = decode(&bytes).unwrap();
    assert_eq!(d.replay.frames().len(), 2);
}

#[test]
fn decompression_bomb_is_rejected() {
    let mut c = lzma("1|1|1|1,", false);
    c[5..13].copy_from_slice(&((MAX_DECOMPRESSED_SIZE as i64) + 1).to_le_bytes());
    let bytes = raw_file(20151228, Some(&c), &1i64.to_le_bytes());
    assert!(matches!(
        decode(&bytes),
        Err(OsrError::DecompressedTooLarge { .. })
    ));
    // a huge dictionary with an unknown size is clamped to the output cap, not allocated as declared
    let mut c = lzma("1|1|1|1,", true);
    c[1..5].copy_from_slice(&u32::MAX.wrapping_sub(15).to_le_bytes());
    let bytes = raw_file(20151228, Some(&c), &1i64.to_le_bytes());
    let _ = decode(&bytes);
}

#[test]
fn invalid_structure_is_an_error() {
    // too short compressed block
    let bytes = raw_file(20151228, Some(&[1, 2, 3]), &1i64.to_le_bytes());
    assert_eq!(decode(&bytes), Err(OsrError::CompressedTooShort));
    // negative date
    let mut bytes = raw_file(20151228, None, &1i64.to_le_bytes());
    let pos = bytes.len() - 4 - 8 - 8;
    bytes[pos..pos + 8].copy_from_slice(&(-1i64).to_le_bytes());
    assert_eq!(decode(&bytes), Err(OsrError::InvalidDateTicks(-1)));
    bytes[pos..pos + 8].copy_from_slice(&3_155_378_976_000_000_000i64.to_le_bytes());
    assert!(matches!(decode(&bytes), Err(OsrError::InvalidDateTicks(_))));
    // string length larger than the input
    let mut b = vec![0u8, 0, 0, 0, 0, 0x0b, 0xff, 0xff, 0xff, 0xff, 0x0f];
    assert!(matches!(
        decode(&b),
        Err(OsrError::InvalidStringLength { .. })
    ));
    b[6..11].copy_from_slice(&[0xff, 0xff, 0xff, 0x7f, 0]);
    assert!(matches!(decode(&b), Err(OsrError::UnexpectedEof { .. })));
    // five continuation bytes
    b[6..11].copy_from_slice(&[0x80; 5]);
    assert!(matches!(
        decode(&b),
        Err(OsrError::InvalidStringLength { .. })
    ));
    // missing lazer block
    let bytes = raw_file(30_000_001, None, &1i64.to_le_bytes());
    assert!(matches!(
        decode(&bytes),
        Err(OsrError::UnexpectedEof { .. })
    ));
    // corrupt stream
    let mut c = lzma("1|1|1|1,2|2|2|2,", false);
    let n = c.len();
    c.truncate(n - 3);
    let bytes = raw_file(20151228, Some(&c), &1i64.to_le_bytes());
    assert!(matches!(decode(&bytes), Err(OsrError::Lzma(_))));
}

#[test]
fn invalid_utf8_string_warns() {
    let mut bytes = raw_file(20151228, None, &1i64.to_le_bytes());
    bytes[7] = 0xff; // first byte of the beatmap hash "h"
    let d = decode(&bytes).unwrap();
    assert_eq!(d.replay.beatmap_hash.as_deref(), Some("\u{fffd}"));
    assert_eq!(d.warnings[0].kind, WarningKind::InvalidUtf8);
}

#[test]
fn truncations_and_corruptions_never_panic() {
    let mut r = sample(30_000_019);
    r.lazer_block = Some(vec![1; 20]);
    r.trailing = vec![1, 2];
    let bytes = encode(&r);
    for n in 0..bytes.len() {
        let _ = decode(&bytes[..n]);
    }
    for i in 0..bytes.len() {
        for v in [0x00u8, 0xff, bytes[i] ^ 0x55] {
            let mut b = bytes.clone();
            b[i] = v;
            let _ = decode(&b);
        }
    }
}

#[test]
fn life_bar_and_dates() {
    let mut r = sample(20151228);
    r.life_bar = Some("0|1,1000|0.5,bad,|,7|x,2000.4|0.25,".into());
    let p = r.life_bar_points();
    assert_eq!(p.len(), 3);
    assert_eq!((p[1].time, p[1].hp), (1000, 0.5));
    assert_eq!(p[2].time, 2000);
    r.life_bar = None;
    assert!(r.life_bar_points().is_empty());

    assert_eq!(unix_millis_from_ticks(621_355_968_000_000_000), 0);
    assert_eq!(unix_millis_from_ticks(621_355_968_000_000_000 - 1), -1);
    assert_eq!(
        ticks_from_unix_millis(1000),
        621_355_968_000_000_000 + 10_000_000
    );
    let ms = 1_700_000_000_123;
    assert_eq!(unix_millis_from_ticks(ticks_from_unix_millis(ms)), ms);
}

#[test]
fn button_state_bits() {
    let b = ButtonState::LEFT1 | ButtonState::SMOKE;
    assert_eq!(b.bits(), 17);
    assert!(b.contains(ButtonState::LEFT1));
    assert!(!b.contains(ButtonState::RIGHT2));
    assert_eq!(
        frame(0, 0.0, 0.0, 6).button_state(),
        ButtonState::RIGHT1 | ButtonState::LEFT2
    );
}

// --- timeline, ported from LegacyScoreDecoderTest ---

fn gf(time: f64) -> GeneratedFrame {
    GeneratedFrame {
        time,
        x: 10.0,
        y: 20.0,
        buttons: 0,
    }
}

fn times(frames: &[TimedFrame]) -> Vec<i64> {
    frames.iter().map(|f| f.time).collect()
}

fn via_encoder(t: &[f64], v: i32) -> Vec<TimedFrame> {
    let g: Vec<_> = t.iter().map(|&x| gf(x)).collect();
    lazer_timeline(&frames_from_timeline(&g, v), v)
}

#[test]
fn negative_delta_frames_are_skipped() {
    assert_eq!(
        times(&via_encoder(&[0.0, 1000.0, 500.0, 2000.0], 14)),
        [0, 1000, 2000]
    );
}

#[test]
fn first_two_frames_swapped_if_wrong_order() {
    assert_eq!(
        times(&via_encoder(&[100.0, 50.0, 1000.0], 14)),
        [0, 100, 1000]
    );
}

#[test]
fn first_two_frames_pulled_toward_third() {
    assert_eq!(
        times(&via_encoder(&[0.0, 500.0, -1500.0], 14)),
        [-1500, -1500, -1500]
    );
}

#[test]
fn early_version_offset() {
    let frames = [frame(31, 1.0, 1.0, 0), frame(17, 1.0, 1.0, 0)];
    assert_eq!(times(&lazer_timeline(&frames, 3)), [55, 72]);
    assert_eq!(times(&lazer_timeline(&frames, 5)), [31, 48]);
    // the encoder shifts back, so the pair round-trips
    assert_eq!(times(&via_encoder(&[55.0, 72.0], 3)), [55, 72]);
}

#[test]
fn stable_dummy_frames_are_removed() {
    let frames = [
        frame(0, 256.0, -500.0, 0),
        frame(16, 256.0, -500.0, 0),
        frame(10, 1.0, 2.0, 1),
    ];
    let t = lazer_timeline(&frames, 14);
    assert_eq!(t.len(), 1);
    assert_eq!((t[0].time, t[0].x, t[0].buttons), (26, 1.0, 1));
    // only the first is a dummy
    let t = lazer_timeline(&[frame(0, 256.0, -500.0, 0), frame(5, 1.0, 2.0, 0)], 14);
    assert_eq!(times(&t), [5]);
}

#[test]
fn encoder_rounds_half_to_even() {
    let f = frames_from_timeline(&[gf(16.5), gf(33.5), gf(50.4)], 14);
    assert_eq!(f.iter().map(|f| f.delta).collect::<Vec<_>>(), [16, 18, 16]);
}

// --- optional real files ---

#[test]
fn real_lazer_test_replays_decode() {
    let dir = format!(
        "{}/../../../slam-refs/osu/osu.Game.Tests/Resources/Replays",
        env!("CARGO_MANIFEST_DIR")
    );
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: {dir} not found");
        return;
    };
    for e in entries.flatten() {
        let bytes = std::fs::read(e.path()).unwrap();
        let d = decode(&bytes).unwrap();
        assert!(!d.replay.frames().is_empty(), "{:?}", e.path());
        // value-exact re-encode
        let again = decode(&encode(&d.replay)).unwrap();
        assert_eq!(again.replay, d.replay);
    }
}

#[test]
fn bom_is_stripped_from_frame_text() {
    let (r, w) = frames_of("\u{feff}16|1|2|3,", 0);
    assert_eq!(r.frames(), [frame(16, 1.0, 2.0, 3)]);
    assert!(w.is_empty());
}

#[test]
fn integer_delta_forms() {
    let (r, w) = frames_of("2147483648|0|0|0,+16|0|0|0, 16 |0|0|0,-2147483649|0|0|0", 0);
    let d: Vec<i32> = r.frames().iter().map(|f| f.delta).collect();
    assert_eq!(d, [i32::MAX, 16, 16, i32::MIN]);
    assert!(w.is_empty());
}

#[test]
fn warnings_are_capped() {
    let text = "x|0|0|0,".repeat(MAX_WARNINGS + 50);
    let bytes = raw_file(20151228, Some(&lzma(&text, false)), &1i64.to_le_bytes());
    let d = decode(&bytes).unwrap();
    assert_eq!(d.warnings.len(), MAX_WARNINGS);
    assert_eq!(d.suppressed_warnings, 50);
}

#[test]
fn version_boundaries() {
    // (version, online id width in bytes, has lazer block)
    for (v, id, block) in [
        (20121007, 0, false),
        (20121008, 4, false),
        (20140720, 4, false),
        (20140721, 8, false),
        (30000000, 8, false),
        (30000001, 8, true),
    ] {
        let mut r = sample(v);
        r.online_id = (id > 0).then_some(7);
        r.lazer_block = block.then(|| vec![1, 2]);
        r.frames = FrameData::Null;
        r.seed = None;
        let bytes = encode(&r);
        let base = encode(&Replay {
            version: 20111111,
            online_id: None,
            lazer_block: None,
            ..r.clone()
        });
        assert_eq!(
            bytes.len() - base.len(),
            id + if block { 4 + 2 } else { 0 },
            "{v}"
        );
        let d = decode(&bytes).unwrap().replay;
        assert_eq!(d, r, "{v}");
    }
}

#[test]
fn dummy_frame_only_at_index_one_is_removed() {
    let f = [
        frame(0, 1.0, 1.0, 0),
        frame(5, 256.0, -500.0, 0),
        frame(5, 2.0, 2.0, 0),
    ];
    let t = lazer_timeline(&f, 14);
    assert_eq!(times(&t), [0, 10]);
    assert_eq!((t[1].x, t[1].y), (2.0, 2.0));
}

#[test]
fn dummy_frame_after_index_one_is_kept() {
    let f = [
        frame(0, 1.0, 1.0, 0),
        frame(5, 2.0, 2.0, 0),
        frame(5, 256.0, -500.0, 0),
    ];
    assert_eq!(times(&lazer_timeline(&f, 14)), [0, 5, 10]);
}

#[test]
fn mania_negative_x_limit() {
    let (r, w) = frames_of("1|-1048575|0|0,1|-1048576|0|0", 3);
    assert_eq!(r.frames().len(), 1);
    assert_eq!(w, [WarningKind::MalformedFrame]);
}

#[test]
fn timed_frame_converts_to_generated() {
    let t = TimedFrame {
        time: 42,
        x: 1.0,
        y: 2.0,
        buttons: 3,
    };
    assert_eq!(
        GeneratedFrame::from(t),
        GeneratedFrame {
            time: 42.0,
            x: 1.0,
            y: 2.0,
            buttons: 3
        }
    );
}
