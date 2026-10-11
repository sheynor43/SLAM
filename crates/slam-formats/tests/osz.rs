//! Integration tests for `.osz` archives and the generic archive layer.

use std::io::Cursor;

use slam_formats::archive::{
    ArchiveError, ArchiveReader, ArchiveWriter, MAX_WARNINGS, WarningKind,
};
use slam_formats::osz::{self, OszReader};

const SMALL: &[u8] = include_bytes!("data/osz/small.osz");

/// One entry of a hand-built zip (stored method).
struct Raw {
    name: Vec<u8>,
    utf8_flag: bool,
    data: Vec<u8>,
    crc: Option<u32>,
    /// Extra general purpose flag bits (bit 0 is encryption).
    flags: u16,
    method: u16,
}

fn raw(name: &[u8], data: &[u8]) -> Raw {
    Raw {
        name: name.to_vec(),
        utf8_flag: false,
        data: data.to_vec(),
        crc: None,
        flags: 0,
        method: 0,
    }
}

fn build_zip(entries: &[Raw]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let crc = e.crc.unwrap_or_else(|| crc32fast::hash(&e.data));
        let flags: u16 = e.flags | if e.utf8_flag { 0x0800 } else { 0 };
        let offset = out.len() as u32;
        let size = e.data.len() as u32;
        let mut common = Vec::new();
        common.extend(flags.to_le_bytes());
        common.extend(e.method.to_le_bytes());
        common.extend(0u16.to_le_bytes()); // time
        common.extend(0x21u16.to_le_bytes()); // date 1980-01-01
        common.extend(crc.to_le_bytes());
        common.extend(size.to_le_bytes());
        common.extend(size.to_le_bytes());
        common.extend((e.name.len() as u16).to_le_bytes());
        common.extend(0u16.to_le_bytes()); // extra
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(&common);
        out.extend(&e.name);
        out.extend(&e.data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(&common);
        central.extend(0u16.to_le_bytes()); // comment
        central.extend(0u16.to_le_bytes()); // disk
        central.extend(0u16.to_le_bytes()); // internal attrs
        central.extend(0u32.to_le_bytes()); // external attrs
        central.extend(offset.to_le_bytes());
        central.extend(&e.name);
    }
    let cd_offset = out.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

fn open(bytes: Vec<u8>) -> ArchiveReader<Cursor<Vec<u8>>> {
    ArchiveReader::new(Cursor::new(bytes)).unwrap()
}

fn names(r: &ArchiveReader<Cursor<Vec<u8>>>) -> Vec<&str> {
    r.entries().iter().map(|e| e.name.as_str()).collect()
}

fn kinds(r: &ArchiveReader<Cursor<Vec<u8>>>) -> Vec<WarningKind> {
    r.warnings().iter().map(|w| w.kind).collect()
}

#[test]
fn lists_and_finds_files_of_a_real_archive() {
    let mut osz = OszReader::new(Cursor::new(SMALL)).unwrap();
    let a = osz.archive();
    let listed: Vec<_> = a.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        listed,
        ["Easy.osu", "Hard.OSU", "audio.mp3", "bg.jpg", "sb/x.png"]
    );
    assert!(a.warnings().is_empty());
    assert_eq!(a.find("SB/X.PNG").unwrap().name, "sb/x.png");
    assert_eq!(a.find("sb\\x.png").unwrap().name, "sb/x.png");
    assert_eq!(a.find("./sb//x.png").unwrap().name, "sb/x.png");
    assert!(a.find("missing.png").is_none());
    let beatmaps: Vec<_> = osz.beatmap_files().map(|e| e.name.clone()).collect();
    assert_eq!(beatmaps, ["Easy.osu", "Hard.OSU"]);
    assert!(osz.read_beatmap("easy.osu").unwrap().is_some());
    assert!(osz.read_beatmap("hard.osu").unwrap().is_some());
    assert!(osz.read_beatmap("nope.osu").unwrap().is_none());
    let mp3 = osz.archive_mut().read("audio.mp3").unwrap().unwrap();
    assert_eq!(mp3.len(), 1024);
    assert_eq!(mp3[255], 255);
}

#[test]
fn invalid_beatmap_is_an_osz_error() {
    let zip = build_zip(&[raw(b"a.osu", b"")]);
    let mut osz = OszReader::new(Cursor::new(zip)).unwrap();
    assert!(matches!(
        osz.read_beatmap("a.osu"),
        Err(osz::OszError::Beatmap { .. })
    ));
}

#[test]
fn cp932_name_without_flag() {
    let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("曲.osu");
    let mut r = open(build_zip(&[raw(&sjis, b"x")]));
    assert_eq!(names(&r), ["曲.osu"]);
    assert!(r.warnings().is_empty());
    assert_eq!(r.read("曲.OSU").unwrap().unwrap(), b"x");
}

#[test]
fn utf8_name_with_and_without_flag() {
    let mut flagged = raw("Ёжик 曲.osu".as_bytes(), b"1");
    flagged.utf8_flag = true;
    let unflagged = raw("🎵 b.osu".as_bytes(), b"2");
    let r = open(build_zip(&[flagged, unflagged]));
    assert_eq!(names(&r), ["Ёжик 曲.osu", "🎵 b.osu"]);
    assert!(r.warnings().is_empty());
}

#[test]
fn undecodable_name_warns_without_panic() {
    let r = open(build_zip(&[raw(&[0x81, b'.', b'o', b's', b'u'], b"x")]));
    assert_eq!(kinds(&r), [WarningKind::UndecodableName]);
    assert_eq!(r.entries().len(), 1);
}

#[test]
fn unsafe_paths_are_rejected() {
    let r = open(build_zip(&[
        raw(b"../evil.osu", b"x"),
        raw(b"/abs.osu", b"x"),
        raw(b"C:/x.osu", b"x"),
        raw(b"a\\..\\b.osu", b"x"),
        raw(b"\\abs2.osu", b"x"),
        raw(b"ok.osu", b"x"),
        raw(b"./sub//ok2.osu", b"x"),
    ]));
    assert_eq!(names(&r), ["ok.osu", "sub/ok2.osu"]);
    assert_eq!(kinds(&r), [WarningKind::UnsafePath; 5]);
}

#[test]
fn dot_names_are_skipped_and_empty_names_warn() {
    let r = open(build_zip(&[
        raw(b"./", b"x"),
        raw(b".", b"x"),
        raw(b"a", b"x"),
    ]));
    assert_eq!(names(&r), ["a"]);
    assert!(r.warnings().is_empty());
    let r = open(build_zip(&[raw(b"", b"x"), raw(b"a", b"x")]));
    assert_eq!(kinds(&r), [WarningKind::EmptyName]);
    assert_eq!(names(&r), ["a"]);
}

#[test]
fn windows_hazards_are_rejected() {
    let r = open(build_zip(&[
        raw(b"./C:/evil.osu", b"x"),
        raw(b"a/b:stream", b"x"),
        raw(b"a.osu.", b"x"),
        raw(b".. /x", b"x"),
        raw(b"nul.osu", b"x"),
        raw(b"sb/COM1", b"x"),
        raw(b"a\x01b", b"x"),
        raw(b"fine.osu", b"x"),
    ]));
    assert_eq!(names(&r), ["fine.osu"]);
    assert_eq!(kinds(&r), [WarningKind::UnsafePath; 7]);
}

#[test]
fn sjis_name_with_backslash_tail_byte_is_not_a_directory() {
    let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("bg/表");
    assert_eq!(*sjis.last().unwrap(), 0x5c);
    let r = open(build_zip(&[raw(&sjis, b"x")]));
    assert_eq!(names(&r), ["bg/表"]);
}

#[test]
fn utf8_flag_with_invalid_utf8_falls_back_to_cp932() {
    let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("曲.osu");
    let mut e = raw(&sjis, b"x");
    e.utf8_flag = true;
    let r = open(build_zip(&[e]));
    assert_eq!(names(&r), ["曲.osu"]);
    assert!(r.warnings().is_empty());
}

#[test]
fn identical_raw_names_are_reported() {
    let mut r = open(build_zip(&[
        raw(b"a.osu", b"first"),
        raw(b"a.osu", b"second"),
        raw(b"b", b"b"),
    ]));
    assert_eq!(names(&r), ["a.osu", "b"]);
    assert_eq!(kinds(&r), [WarningKind::CollapsedDuplicates { count: 1 }]);
    // The zip crate keeps the last data for identical names.
    assert!(r.read("a.osu").unwrap().is_some());
}

#[test]
fn encrypted_entry_is_unsupported() {
    let mut e = raw(b"a.bin", b"secret");
    e.flags = 1;
    let mut r = open(build_zip(&[e]));
    assert_eq!(names(&r), ["a.bin"]);
    assert!(matches!(
        r.read("a.bin"),
        Err(ArchiveError::Unsupported { .. })
    ));
}

#[test]
fn unsupported_method_is_unsupported() {
    let mut e = raw(b"a.bin", b"data");
    e.method = 12;
    let mut r = open(build_zip(&[e]));
    assert!(matches!(
        r.read("a.bin"),
        Err(ArchiveError::Unsupported { .. })
    ));
}

#[test]
fn warnings_are_capped() {
    let entries: Vec<Raw> = (0..150)
        .map(|i| raw(format!("../{i}").as_bytes(), b"x"))
        .collect();
    let r = open(build_zip(&entries));
    assert_eq!(r.warnings().len(), MAX_WARNINGS);
    assert_eq!(r.suppressed_warnings(), 50);
}

#[test]
fn forged_zip64_entry_count_is_rejected_without_allocating() {
    let mut z = vec![0u8; 100];
    let record_pos = z.len() as u64;
    z.extend(0x0606_4b50u32.to_le_bytes());
    z.extend(44u64.to_le_bytes());
    z.extend(45u16.to_le_bytes());
    z.extend(45u16.to_le_bytes());
    z.extend(0u32.to_le_bytes());
    z.extend(0u32.to_le_bytes());
    z.extend((1u64 << 40).to_le_bytes());
    z.extend((1u64 << 40).to_le_bytes());
    z.extend(0u64.to_le_bytes());
    z.extend(0u64.to_le_bytes());
    z.extend(0x0706_4b50u32.to_le_bytes());
    z.extend(0u32.to_le_bytes());
    z.extend(record_pos.to_le_bytes());
    z.extend(1u32.to_le_bytes());
    z.extend(0x0605_4b50u32.to_le_bytes());
    z.extend(0u16.to_le_bytes());
    z.extend(0u16.to_le_bytes());
    z.extend(0xffffu16.to_le_bytes());
    z.extend(0xffffu16.to_le_bytes());
    z.extend(0xffff_ffffu32.to_le_bytes());
    z.extend(0xffff_ffffu32.to_le_bytes());
    z.extend(0u16.to_le_bytes());
    assert!(matches!(
        ArchiveReader::new(Cursor::new(z)),
        Err(ArchiveError::TooManyEntries { .. })
    ));
}

#[test]
fn writer_output_is_stable_across_hosts() {
    let written = osz::write(
        Cursor::new(Vec::new()),
        [
            ("a.osu", &b"osu file format v14\n"[..]),
            ("sb/\u{66f2}.txt", &[7u8; 300][..]),
        ],
    )
    .unwrap()
    .into_inner();
    assert_eq!(
        (written.len(), crc32fast::hash(&written)),
        (234, 1_831_808_101)
    );
}

#[test]
fn system_files_and_directories_are_skipped_silently() {
    let r = open(build_zip(&[
        raw(b"__MACOSX/._a.osu", b"x"),
        raw(b".DS_Store", b"x"),
        raw(b"sb/Thumbs.db", b"x"),
        raw(b"desktop.ini", b"x"),
        raw(b"folder/", b""),
        raw(b"win\\", b""),
        raw(b"a.osu", b"x"),
    ]));
    assert_eq!(names(&r), ["a.osu"]);
    assert!(r.warnings().is_empty());
}

#[test]
fn case_insensitive_duplicate_is_dropped() {
    let mut r = open(build_zip(&[
        raw(b"A.osu", b"first"),
        raw(b"a.OSU", b"second"),
        raw(b"b", b"b"),
    ]));
    assert_eq!(names(&r), ["A.osu", "b"]);
    assert_eq!(kinds(&r), [WarningKind::DuplicateName]);
    assert_eq!(r.read("a.osu").unwrap().unwrap(), b"first");
}

#[test]
fn crc_mismatch_is_an_error_on_read() {
    let mut e = raw(b"a.bin", b"hello");
    e.crc = Some(0xdead_beef);
    let mut r = open(build_zip(&[e, raw(b"b.bin", b"fine")]));
    assert!(matches!(
        r.read("a.bin"),
        Err(ArchiveError::CorruptEntry { .. })
    ));
    assert_eq!(r.read("b.bin").unwrap().unwrap(), b"fine");
}

#[test]
fn broken_archives_are_errors() {
    assert!(ArchiveReader::new(Cursor::new(Vec::new())).is_err());
    let noise: Vec<u8> = (0..4096u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    assert!(ArchiveReader::new(Cursor::new(noise)).is_err());
    assert!(ArchiveReader::new(Cursor::new(b"PK\x05\x06".to_vec())).is_err());
}

#[test]
fn truncated_archive_never_panics() {
    for len in 0..SMALL.len() {
        let cut = SMALL[..len].to_vec();
        // Cutting off the end record always makes the archive unreadable.
        assert!(ArchiveReader::new(Cursor::new(cut)).is_err(), "len {len}");
    }
}

#[test]
fn corrupted_archive_never_panics() {
    for i in 0..SMALL.len() {
        let mut bytes = SMALL.to_vec();
        bytes[i] ^= 0xa5;
        if let Ok(mut r) = ArchiveReader::new(Cursor::new(bytes)) {
            for e in r.entries().to_vec() {
                let _ = r.read_entry(&e);
            }
        }
    }
}

fn read_all(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut r = ArchiveReader::new(Cursor::new(bytes)).unwrap();
    r.entries()
        .to_vec()
        .iter()
        .map(|e| (e.name.clone(), r.read_entry(e).unwrap()))
        .collect()
}

#[test]
fn export_round_trip_is_byte_identical() {
    let files = read_all(SMALL);
    let written = osz::write(
        Cursor::new(Vec::new()),
        files.iter().map(|(n, d)| (n.as_str(), d.as_slice())),
    )
    .unwrap()
    .into_inner();
    assert_eq!(read_all(&written), files);
    let mut r = OszReader::new(Cursor::new(&written)).unwrap();
    assert_eq!(r.beatmap_files().count(), 2);
    assert!(r.read_beatmap("Easy.osu").unwrap().is_some());
}

#[test]
fn non_ascii_names_round_trip() {
    let name = "Ёжик 曲 🎵.osu";
    let data = b"osu file format v14\n";
    let written = osz::write(Cursor::new(Vec::new()), [(name, &data[..])])
        .unwrap()
        .into_inner();
    assert_eq!(read_all(&written), [(name.to_owned(), data.to_vec())]);
    // The raw name is UTF-8.
    assert!(written.windows(name.len()).any(|w| w == name.as_bytes()));
}

#[test]
fn writer_rejects_bad_names() {
    let mut w = ArchiveWriter::new(Cursor::new(Vec::new()));
    for bad in [
        "../x",
        "/x",
        "C:/x",
        "a\\..\\b",
        "",
        ".",
        "dir/",
        ".DS_Store",
    ] {
        assert!(
            matches!(w.add_file(bad, b""), Err(ArchiveError::InvalidName { .. })),
            "{bad:?}"
        );
    }
    w.add_file("a.osu", b"1").unwrap();
    assert!(matches!(
        w.add_file("A.OSU", b"2"),
        Err(ArchiveError::DuplicateName(_))
    ));
    assert!(matches!(
        w.add_file("./a.osu", b"2"),
        Err(ArchiveError::DuplicateName(_))
    ));
    let bytes = w.finish().unwrap().into_inner();
    assert_eq!(read_all(&bytes), [("a.osu".to_owned(), b"1".to_vec())]);
}

#[test]
fn writer_normalizes_names() {
    let written = osz::write(Cursor::new(Vec::new()), [("sb\\x.png", &b"x"[..])])
        .unwrap()
        .into_inner();
    assert_eq!(read_all(&written)[0].0, "sb/x.png");
}

#[test]
fn writing_is_deterministic() {
    let files = read_all(SMALL);
    let write = || {
        osz::write(
            Cursor::new(Vec::new()),
            files.iter().map(|(n, d)| (n.as_str(), d.as_slice())),
        )
        .unwrap()
        .into_inner()
    };
    assert_eq!(write(), write());
}

#[test]
fn trailing_garbage_after_the_end_record() {
    let zip = build_zip(&[raw(b"a.osu", b"x")]);
    let mut huge = zip.clone();
    huge.extend(vec![0u8; 70_000]);
    assert!(matches!(
        ArchiveReader::new(Cursor::new(huge)),
        Err(ArchiveError::InvalidArchive(_))
    ));
    // Within the comment window the end record is still found.
    let mut small = zip;
    small.extend(vec![0u8; 100]);
    match ArchiveReader::new(Cursor::new(small)) {
        Ok(r) => assert_eq!(names(&r), ["a.osu"]),
        Err(e) => assert!(matches!(e, ArchiveError::InvalidArchive(_)), "{e}"),
    }
}

#[test]
fn zip64_record_must_be_at_the_locator_offset() {
    let mut z = vec![0u8; 100];
    z.extend(0x0706_4b50u32.to_le_bytes());
    z.extend(0u32.to_le_bytes());
    z.extend(50u64.to_le_bytes()); // points at zeros, not at an end record
    z.extend(1u32.to_le_bytes());
    z.extend(0x0605_4b50u32.to_le_bytes());
    z.extend([0u8; 4]);
    z.extend(0xffffu16.to_le_bytes());
    z.extend(0xffffu16.to_le_bytes());
    z.extend(0xffff_ffffu32.to_le_bytes());
    z.extend(0xffff_ffffu32.to_le_bytes());
    z.extend(0u16.to_le_bytes());
    assert!(matches!(
        ArchiveReader::new(Cursor::new(z)),
        Err(ArchiveError::InvalidArchive(_))
    ));
}

#[test]
fn foreign_entry_is_rejected() {
    let mut a = open(build_zip(&[raw(b"a", b"1")]));
    let mut b = open(build_zip(&[raw(b"b", b"2")]));
    let entry = a.entries()[0].clone();
    assert!(matches!(
        b.read_entry(&entry),
        Err(ArchiveError::ForeignEntry(_))
    ));
    assert_eq!(a.read_entry(&entry).unwrap(), b"1");
}
