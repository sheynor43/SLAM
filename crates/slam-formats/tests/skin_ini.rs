//! Tests for the `skin.ini` decoder.

use slam_formats::skin_ini::{
    DEFAULT_COMBO_COLOURS, Rgba, SkinVersion, WarningKind, decode, decode_str,
};

const REALISTIC: &str = include_str!("data/skin/realistic.ini");
const BOM_SPACE: &[u8] = include_bytes!("data/skin/bom-space.ini");
const COLOURS: &str = include_str!("data/skin/colours.ini");
const CATCH: &str = include_str!("data/skin/catch.ini");

fn kinds(d: &slam_formats::skin_ini::Decoded) -> Vec<WarningKind> {
    d.warnings.iter().map(|w| w.kind).collect()
}

#[test]
fn combo_colours_in_order_with_alpha_dropped() {
    let d = decode_str(COLOURS);
    assert!(d.warnings.is_empty());
    assert_eq!(
        d.skin.combo_colours,
        [
            Rgba::rgb(142, 199, 255),
            Rgba::rgb(255, 128, 128),
            Rgba::rgb(128, 255, 255),
            Rgba::rgb(100, 100, 100),
        ]
    );
}

#[test]
fn empty_file_is_defaults() {
    for d in [decode_str(""), decode(b"")] {
        assert!(d.warnings.is_empty());
        assert!(d.skin.combo_colours.is_empty());
        assert_eq!(d.skin.version, SkinVersion::V1);
        assert!(!d.skin.is_latest);
        assert!(d.skin.settings.is_empty());
    }
    assert_eq!(DEFAULT_COMBO_COLOURS.len(), 4);
}

#[test]
fn name_and_custom_lookup() {
    let d = decode_str("[General]\nName: test skin\nTestLookup: TestValue\n");
    assert_eq!(d.skin.name, "test skin");
    assert_eq!(d.skin.settings.get("TestLookup"), Some("TestValue"));
    assert_eq!(d.skin.settings.get("testlookup"), None);
    // Name is consumed, not stored.
    assert_eq!(d.skin.settings.get("Name"), None);
}

#[test]
fn versions() {
    let v = |s: &str| decode_str(&format!("[General]\nVersion: {s}\n")).skin;
    assert_eq!(v("2").version, SkinVersion::new(20, 1));
    assert_eq!(v("2.0").version, SkinVersion::new(2, 0));
    assert_eq!(v("1.9").version, SkinVersion::new(19, 1));
    assert!(v("1.9").version < SkinVersion::new(2, 0));
    let latest = v("latest");
    assert_eq!(latest.version, SkinVersion::LATEST);
    assert!(latest.is_latest);
    // Case-sensitive keyword.
    let bad = decode_str("[General]\nVersion: Latest\n");
    assert_eq!(bad.skin.version, SkinVersion::V1);
    assert_eq!(kinds(&bad), [WarningKind::InvalidVersion]);
    // A bad value keeps the previous one.
    let d = decode_str("[General]\nVersion: 2.2\nVersion: abc\n");
    assert_eq!(d.skin.version, SkinVersion::new(22, 1));
    // A numeric version after `latest` clears the flag.
    let d = decode_str("[General]\nVersion: latest\nVersion: 1.5\n");
    assert!(!d.skin.is_latest);
    // No version key.
    assert_eq!(
        decode_str("[General]\nName: x\n").skin.version,
        SkinVersion::V1
    );
    // Version is not a setting.
    assert_eq!(v("2").settings.get("Version"), None);
}

#[test]
fn version_with_spaces_and_bom() {
    let d = decode(BOM_SPACE);
    assert!(d.warnings.is_empty());
    assert_eq!(d.skin.name, "Spaced");
    assert_eq!(d.skin.version, SkinVersion::new(2, 0));
}

#[test]
fn crlf_and_lone_cr() {
    let d = decode_str("[General]\r\nName: a\r\nAuthor: b\rVersion: 2.1\r\n");
    assert_eq!(d.skin.name, "a");
    assert_eq!(d.skin.author, "b");
    assert_eq!(d.skin.version, SkinVersion::new(21, 1));
}

#[test]
fn utf16_bom() {
    let text = "[General]\nName: Zażółć\n";
    let mut le = vec![0xFF, 0xFE];
    le.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let mut be = vec![0xFE, 0xFF];
    be.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    for bytes in [le, be] {
        let d = decode(&bytes);
        assert!(d.warnings.is_empty());
        assert_eq!(d.skin.name, "Zażółć");
    }
}

#[test]
fn invalid_utf8_warns_once() {
    let d = decode(b"[General]\nName: a\xFFb\nAuthor: c\xFF\n");
    assert_eq!(kinds(&d), [WarningKind::InvalidEncoding]);
    assert_eq!(d.warnings[0].line, 0);
    assert_eq!(d.skin.name, "a\u{fffd}b");
    assert_eq!(d.skin.author, "c\u{fffd}");
}

#[test]
fn broken_colour_lines_are_dropped() {
    let d = decode_str(
        "[Colours]\nCombo1: 1,2\nCombo2: 1,2,3,4,5\nCombo3: 1,2,x\nCombo4: 256,0,0\nCombo5: -1,0,0\nCombo6: 9, 8 ,7\nCombo7\n",
    );
    assert_eq!(d.skin.combo_colours, [Rgba::rgb(9, 8, 7)]);
    assert_eq!(d.warnings.len(), 6);
    assert!(
        d.warnings
            .iter()
            .all(|w| w.kind == WarningKind::InvalidColour)
    );
    assert_eq!(d.warnings[0].line, 2);
    assert_eq!(d.warnings[0].excerpt, "Combo1: 1,2");
}

#[test]
fn unknown_section_falls_back_to_general() {
    let d = decode_str(
        "[Bogus]\nName: n\nFoo: bar\n[Colours]\nCombo1: 1,2,3\n[colours]\nCombo2: 4,5,6\n",
    );
    assert_eq!(
        kinds(&d),
        [WarningKind::UnknownSection, WarningKind::UnknownSection]
    );
    assert_eq!(d.warnings[0].line, 1);
    assert_eq!(d.skin.name, "n");
    assert_eq!(d.skin.settings.get("Foo"), Some("bar"));
    // After the second unknown section the colour line is read as a General key.
    assert_eq!(d.skin.combo_colours, [Rgba::rgb(1, 2, 3)]);
    assert_eq!(d.skin.settings.get("Combo2"), Some("4,5,6"));
}

#[test]
fn comments() {
    let d = decode_str(
        "// header\n[General]\n  // indented\nName: a // tail\nAuthor: //x\nKey: v//w\n//Key2: no\n",
    );
    assert!(d.warnings.is_empty());
    assert_eq!(d.skin.name, "a");
    // `//` at index > 0 starts a comment, even directly after the colon.
    assert_eq!(d.skin.author, "");
    assert_eq!(d.skin.settings.get("Key"), Some("v"));
    assert_eq!(d.skin.settings.get("Key2"), None);
}

#[test]
fn fonts_keys_are_settings() {
    let d = decode_str("[Fonts]\nHitCirclePrefix: fonts\\default\nHitCircleOverlap: 3\n");
    assert_eq!(
        d.skin.settings.get("HitCirclePrefix"),
        Some("fonts\\default")
    );
    assert_eq!(d.skin.settings.get("HitCircleOverlap"), Some("3"));
}

#[test]
fn more_than_eight_combo_keys() {
    let mut text = String::from("[Colours]\n");
    for i in 0..=10u8 {
        text.push_str(&format!("Combo{i}: {i},0,0\n"));
    }
    let d = decode_str(&text);
    // Combo0, Combo9 and Combo10 are custom colours; 1..=8 are combo colours in file order.
    let reds: Vec<u8> = d.skin.combo_colours.iter().map(|c| c.r).collect();
    assert_eq!(reds, [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(d.skin.custom_colour("Combo9"), Some(Rgba::rgb(9, 0, 0)));
    assert_eq!(d.skin.custom_colour("Combo10"), Some(Rgba::rgb(10, 0, 0)));
    assert_eq!(d.skin.custom_colour("Combo0"), Some(Rgba::rgb(0, 0, 0)));
}

#[test]
fn combo_index_does_not_choose_position() {
    let d = decode_str("[Colours]\nCombo3: 3,0,0\nCombo1: 1,0,0\nCombo3: 33,0,0\n");
    let reds: Vec<u8> = d.skin.combo_colours.iter().map(|c| c.r).collect();
    assert_eq!(reds, [3, 1, 33]);
}

#[test]
fn custom_colours_replace_in_place() {
    let d = decode_str("[Colours]\nA: 1,1,1\nB: 2,2,2\nA: 3,3,3\n");
    assert_eq!(
        d.skin.custom_colours,
        [
            ("A".to_owned(), Rgba::rgb(3, 3, 3)),
            ("B".to_owned(), Rgba::rgb(2, 2, 2))
        ]
    );
}

#[test]
fn catch_the_beat_keeps_alpha() {
    let d = decode_str(CATCH);
    assert!(d.warnings.is_empty());
    // [Colours] drops alpha; [CatchTheBeat] keeps it, and its Combo keys share the list.
    assert_eq!(
        d.skin.combo_colours,
        [
            Rgba::rgb(1, 2, 3),
            Rgba {
                r: 10,
                g: 20,
                b: 30,
                a: 40
            }
        ]
    );
    assert_eq!(d.skin.custom_colour("Custom"), Some(Rgba::rgb(5, 6, 7)));
    assert_eq!(
        d.skin.custom_colour("Fruit"),
        Some(Rgba {
            r: 50,
            g: 60,
            b: 70,
            a: 80
        })
    );
    // Colour sections never feed the settings dictionary.
    assert!(d.skin.settings.is_empty());
}

#[test]
fn last_write_wins_and_empty_key_ignored() {
    let d = decode_str("[General]\nK: 1\n: orphan\nK: 2\nNoColon\n");
    assert_eq!(d.skin.settings.get("K"), Some("2"));
    assert_eq!(d.skin.settings.get(""), None);
    assert_eq!(d.skin.settings.get("NoColon"), Some(""));
    assert_eq!(d.skin.settings.len(), 2);
    let keys: Vec<_> = d.skin.settings.iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["K", "NoColon"]);
}

#[test]
fn value_keeps_inner_colons() {
    let d = decode_str("[General]\nURL: host:80/a\n");
    assert_eq!(d.skin.settings.get("URL"), Some("host:80/a"));
}

#[test]
fn realistic_skin() {
    let d = decode_str(REALISTIC);
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    let s = &d.skin;
    assert_eq!(s.name, "Example Skin");
    assert_eq!(s.author, "Tester");
    assert_eq!(s.version, SkinVersion::new(25, 1));
    assert!(!s.is_latest);
    assert_eq!(s.settings.get("AnimationFramerate"), Some("24"));
    assert_eq!(s.settings.get("CursorRotate"), Some("1"));
    assert_eq!(s.settings.get("LayeredHitSounds"), Some("0"));
    assert_eq!(s.combo_colours.len(), 4);
    assert_eq!(s.combo_colours[3], Rgba::rgb(242, 24, 57));
    assert_eq!(s.custom_colour("SliderBall"), Some(Rgba::rgb(2, 170, 255)));
    assert_eq!(
        s.custom_colour("SliderTrackOverride"),
        Some(Rgba::rgb(10, 10, 10))
    );
    assert_eq!(s.settings.get("HitCirclePrefix"), Some("fonts\\default"));
    assert_eq!(s.settings.get("Keys"), Some("4"));
    assert_eq!(s.settings.get("ColumnWidth"), Some("30,30,30,30"));
}

#[test]
fn metadata_section_keeps_double_slash() {
    let d = decode_str("[Metadata]\nTitle: a // b\n[General]\nName: a // b\n");
    assert_eq!(d.skin.settings.get("Title"), Some("a // b"));
    assert_eq!(d.skin.name, "a");
}

#[test]
fn numeric_section_header() {
    // `Enum.TryParse("6")` selects Colours.
    let d = decode_str("[6]\nCombo1: 1,2,3\n");
    assert!(d.warnings.is_empty());
    assert_eq!(d.skin.combo_colours, [Rgba::rgb(1, 2, 3)]);
}

#[test]
fn utf32_bom() {
    let text = "[General]\nName: Zażółć\n";
    let mut le = vec![0xFF, 0xFE, 0x00, 0x00];
    le.extend(text.chars().flat_map(|c| (c as u32).to_le_bytes()));
    let mut be = vec![0x00, 0x00, 0xFE, 0xFF];
    be.extend(text.chars().flat_map(|c| (c as u32).to_be_bytes()));
    for bytes in [le, be] {
        let d = decode(&bytes);
        assert!(d.warnings.is_empty());
        assert_eq!(d.skin.name, "Zażółć");
    }
}

#[test]
fn version_with_trailing_comment() {
    let d = decode_str("[General]\nVersion: 2.5 // c\n");
    assert_eq!(d.skin.version, SkinVersion::new(25, 1));
    assert!(d.warnings.is_empty());
}

#[test]
fn combo_key_edge_cases() {
    // `Combo` without an index is a custom colour; `Combo 1` parses as index 1 (.NET allows
    // surrounding white space) and keys are trimmed only at the ends.
    let d = decode_str("[Colours]\nCombo: 1,1,1\nCombo 1: 2,2,2\n");
    assert_eq!(d.skin.custom_colour("Combo"), Some(Rgba::rgb(1, 1, 1)));
    assert_eq!(d.skin.combo_colours, [Rgba::rgb(2, 2, 2)]);
    assert_eq!(d.skin.custom_colours.len(), 1);
}
