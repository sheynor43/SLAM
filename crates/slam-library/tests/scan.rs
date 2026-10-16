//! Tests for the `Songs` folder scan.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use slam_library::scan::{BeatmapSetEntry, ScanEvent, ScanSummary, Scanner, scan_set, sort_sets};

struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!("slam-scan-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn osu(mode: i32, artist: &str, title: &str, version: &str) -> String {
    format!(
        "osu file format v14\n\n[General]\nMode: {mode}\n\n[Metadata]\nTitle:{title}\nArtist:{artist}\nCreator:Mapper\nVersion:{version}\n\n[Difficulty]\nHPDrainRate:4\nCircleSize:3\nOverallDifficulty:8\nApproachRate:9\n"
    )
}

fn write(dir: &Path, name: &str, content: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join(name), content).unwrap();
}

fn collect(root: PathBuf) -> (Vec<BeatmapSetEntry>, Result<ScanSummary, std::io::Error>) {
    let scanner = Scanner::spawn(root).unwrap();
    let start = Instant::now();
    let mut sets = Vec::new();
    loop {
        assert!(start.elapsed() < Duration::from_secs(30), "scan timed out");
        match scanner.try_recv() {
            Some(ScanEvent::Batch(b)) => sets.extend(b),
            Some(ScanEvent::Done(s)) => return (sets, Ok(s)),
            Some(ScanEvent::Failed(e)) => return (sets, Err(e)),
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

#[test]
fn keeps_only_standard() {
    let t = Tmp::new("std");
    let d = t.0.join("set");
    write(&d, "a.osu", &osu(0, "Art", "Song", "Hard"));
    write(&d, "b.osu", &osu(1, "Art", "Song", "Taiko"));
    write(&d, "c.osu", &osu(3, "Art", "Song", "Mania"));
    write(&t.0.join("maniaonly"), "m.osu", &osu(3, "X", "Y", "4K"));
    let e = scan_set(&d).unwrap().unwrap();
    assert_eq!(e.difficulties.len(), 1);
    assert_eq!(e.difficulties[0].version, "Hard");
    assert_eq!(e.difficulties[0].overall_difficulty, 8.0);
    assert_eq!(e.difficulties[0].approach_rate, 9.0);
    assert_eq!(e.difficulties[0].circle_size, 3.0);
    assert_eq!(e.difficulties[0].hp, 4.0);
    assert_eq!(
        (e.artist.as_str(), e.title.as_str(), e.creator.as_str()),
        ("Art", "Song", "Mapper")
    );
    assert!(scan_set(&t.0.join("maniaonly")).unwrap().is_none());
    let (sets, summary) = collect(t.0.clone());
    assert_eq!(sets.len(), 1);
    assert_eq!(summary.unwrap().sets, 1);
}

#[test]
fn extension_case_and_nesting() {
    let t = Tmp::new("ext");
    let d = t.0.join("set");
    write(&d, "a.OSU", &osu(0, "A", "T", "Zed"));
    write(&d, "b.osu", &osu(0, "A", "T", "alpha"));
    write(&d, "notes.txt", &osu(0, "A", "T", "Nope"));
    write(&d.join("nested"), "n.osu", &osu(0, "A", "T", "Nested"));
    let e = scan_set(&d).unwrap().unwrap();
    let versions: Vec<_> = e.difficulties.iter().map(|d| d.version.as_str()).collect();
    assert_eq!(versions, ["alpha", "Zed"]);
}

#[test]
fn broken_files_are_counted() {
    let t = Tmp::new("broken");
    let d = t.0.join("set");
    write(&d, "good.osu", &osu(0, "A", "T", "V"));
    write(&d, "empty.osu", "");
    let (sets, summary) = collect(t.0.clone());
    let s = summary.unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(s.skipped_files, 1);
    assert_eq!(s.difficulties, 1);
}

#[test]
fn scanner_batches_and_summary() {
    let t = Tmp::new("batches");
    for i in 0..150 {
        write(
            &t.0.join(format!("s{i:03}")),
            "a.osu",
            &osu(0, "A", &format!("T{i}"), "V"),
        );
    }
    let (sets, summary) = collect(t.0.clone());
    let s = summary.unwrap();
    assert_eq!(sets.len(), 150);
    assert_eq!((s.sets, s.difficulties, s.skipped_files), (150, 150, 0));
    assert!(sets[0].dir.ends_with("s000"));
}

#[test]
fn missing_root_fails() {
    let (_, r) = collect(std::env::temp_dir().join("slam-scan-definitely-missing-root"));
    assert!(r.is_err());
}

#[test]
fn drop_mid_scan_does_not_hang() {
    let t = Tmp::new("drop");
    for i in 0..300 {
        write(
            &t.0.join(format!("s{i:03}")),
            "a.osu",
            &osu(0, "A", "T", "V"),
        );
    }
    let start = Instant::now();
    drop(Scanner::spawn(t.0.clone()).unwrap());
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn sort_order() {
    let mk = |artist: &str, title: &str, dir: &str| BeatmapSetEntry {
        dir: dir.into(),
        artist: artist.into(),
        title: title.into(),
        creator: String::new(),
        difficulties: Vec::new(),
    };
    let mut v = vec![
        mk("b", "x", "4"),
        mk("A", "z", "3"),
        mk("a", "Y", "2"),
        mk("a", "y", "1"),
    ];
    sort_sets(&mut v);
    let dirs: Vec<_> = v.iter().map(|s| s.dir.to_str().unwrap()).collect();
    assert_eq!(dirs, ["1", "2", "3", "4"]);
}
