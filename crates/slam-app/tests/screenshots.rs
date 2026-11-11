//! Skin screenshot tests (ADR-0111): plays of the test beatmaps drawn at fixed times on Mesa's
//! software renderer, compared with the references in `tests/data/screenshots/reference`.
//! Needs Mesa with llvmpipe:
//! `cargo test -p slam-app --test screenshots -- --ignored` (`just screenshots`).
//!
//! A mismatch writes the frame and a map of the differing pixels to
//! `target/tmp/screenshots`. `SLAM_UPDATE_SCREENSHOTS=1` writes the references instead
//! (`just screenshots-update`); look at every changed one before committing it.

use std::path::{Path, PathBuf};

use slam_app::play::scene::PlaySettings;
use slam_app::screenshot::compare::{Mismatch, Tolerance, compare};
use slam_app::screenshot::{DEFAULT_SIZE, Frame, Shots, render};

/// One beatmap and skin, shot at several times.
struct Case {
    /// Prefix of the references' names: `<name>-<time>.png`.
    name: &'static str,
    /// Folder under `maps/` with one `.osu`.
    map: &'static str,
    /// Times in milliseconds of the beatmap.
    times: &'static [f64],
}

/// The scene beatmap on the default skin: approach circles and follow points (900), a
/// slider with its ball, follow circle and repeat arrow (1700), a spinner with the SPM
/// counter (3900), and judgements and the HUD after the last circle (6450).
const CASES: &[Case] = &[Case {
    name: "default",
    map: "scene",
    times: &[900.0, 1700.0, 3900.0, 6450.0],
}];

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/screenshots")
}

fn beatmap(folder: &str) -> PathBuf {
    let dir = data().join("maps").join(folder);
    std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "osu"))
        .unwrap_or_else(|| panic!("no .osu in {}", dir.display()))
}

// One test per binary: SDL is process-global.
#[test]
#[ignore = "needs Mesa with llvmpipe"]
fn skins_draw_as_the_references() {
    let update = std::env::var_os("SLAM_UPDATE_SCREENSHOTS").is_some_and(|v| v == "1");
    let references = data().join("reference");
    let failures_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("screenshots");
    let _ = std::fs::remove_dir_all(&failures_dir);
    let mut failures = Vec::new();
    for case in CASES {
        let shots = Shots {
            map: beatmap(case.map),
            // The defaults of the settings, with the background as the play shows it.
            settings: PlaySettings::default(),
            times: case.times.to_vec(),
            size: DEFAULT_SIZE,
            any_renderer: false,
        };
        let frames = render(&shots).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        for (frame, time) in frames.iter().zip(case.times) {
            let name = format!("{}-{time}", case.name);
            let reference = references.join(format!("{name}.png"));
            if update {
                frame.write_png(&reference).unwrap();
                continue;
            }
            let expected = match Frame::read_png(&reference) {
                Ok(expected) => expected,
                Err(e) => {
                    failures.push(format!("{name}: {e}"));
                    save(&failures_dir, &name, frame, None);
                    continue;
                }
            };
            match compare(&expected, frame, Tolerance::default()) {
                Ok(_) => {}
                Err(mismatch) => {
                    failures.push(format!("{name}: {mismatch}"));
                    let diff = match &mismatch {
                        Mismatch::Pixels(d) => Some(&d.image),
                        Mismatch::Size { .. } => None,
                    };
                    save(&failures_dir, &name, frame, diff);
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "screenshots differ from the references (frames and differences in {}):\n{}",
        failures_dir.display(),
        failures.join("\n")
    );
}

fn save(dir: &Path, name: &str, actual: &Frame, diff: Option<&Frame>) {
    std::fs::create_dir_all(dir).unwrap();
    actual
        .write_png(&dir.join(format!("{name}.actual.png")))
        .unwrap();
    if let Some(diff) = diff {
        diff.write_png(&dir.join(format!("{name}.diff.png")))
            .unwrap();
    }
}
