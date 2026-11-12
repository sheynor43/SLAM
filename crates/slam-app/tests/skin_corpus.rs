//! Screenshots of the local corpus of third-party stable skins (`docs/SKIN_CORPUS.md`,
//! ADR-0111): the scene beatmap drawn with each skin of `SLAM_SKIN_CORPUS` (default
//! `../slam-skins`) and compared with the references in its `reference` folder. Needs Mesa
//! with llvmpipe: `just skin-corpus [FILTER]`.
//!
//! `SLAM_SKIN_CORPUS_FILTER` keeps the skins whose name contains it. A mismatch writes the
//! frame and a map of the differing pixels to `target/tmp/skin-corpus`.
//! `SLAM_UPDATE_SCREENSHOTS=1` writes the references instead (`just skin-corpus-update`).

use std::path::{Path, PathBuf};

use slam_app::play::scene::PlaySettings;
use slam_app::screenshot::check::{self, Checker, Outcome};
use slam_app::screenshot::corpus::{self, Entry};
use slam_app::screenshot::{DEFAULT_SIZE, Shots, render};
use slam_skin::Skin;

/// Times in milliseconds of the scene beatmap, as in the default skin's screenshots.
const TIMES: &[f64] = &[900.0, 1700.0, 3900.0, 6450.0];

fn scene_map() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/screenshots/maps/scene");
    std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "osu"))
        .unwrap_or_else(|| panic!("no .osu in {}", dir.display()))
}

/// Draws the skin and checks it; the skin's failures are recorded, not raised. Returns the
/// summary of the skin.
fn shoot(entry: &Entry, root: &Path, map: &Path, checker: &mut Checker) -> &'static str {
    let dir = entry.dir(root);
    if !dir.is_dir() {
        checker.record(&entry.name, &format!("no folder {}", dir.display()));
        return "error";
    }
    match Skin::load(&dir) {
        Ok(skin) => {
            if let Err(e) = entry.check_version(skin.version(), skin.is_latest_version()) {
                checker.record(&entry.name, &e);
                return "error";
            }
        }
        Err(e) => {
            checker.record(&entry.name, &format!("loading the skin: {e}"));
            return "error";
        }
    }
    let shots = Shots {
        map: map.to_path_buf(),
        settings: PlaySettings {
            skin: Some(dir),
            // The skin under test is the one shown, not the scene beatmap's own.
            beatmap_skins: false,
            key_overlay: Some(slam_play::key_overlay::KeyAnimation::Stable),
            ..PlaySettings::default()
        },
        times: TIMES.to_vec(),
        size: DEFAULT_SIZE,
        any_renderer: false,
    };
    let frames = match render(&shots) {
        Ok(frames) => frames,
        Err(e) => {
            checker.record(&entry.name, &format!("rendering: {e}"));
            return "error";
        }
    };
    let mut summary = "ok";
    for (frame, time) in frames.iter().zip(TIMES) {
        match checker.check(&format!("{}-{time}", entry.name), frame) {
            Outcome::Failed(_) => summary = "mismatch",
            Outcome::Ok | Outcome::Updated => {}
        }
    }
    if check::update_requested() {
        "updated"
    } else {
        summary
    }
}

// One test per binary: SDL is process-global.
#[test]
#[ignore = "needs Mesa with llvmpipe and the local skin corpus"]
fn corpus_skins_draw_as_the_references() {
    let root = corpus::corpus_root().unwrap_or_else(|| {
        panic!(
            "skin corpus not found: set {} or create ../slam-skins (see docs/SKIN_CORPUS.md)",
            corpus::ROOT_VAR
        )
    });
    let manifest = root.join("manifest.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("{}: {e} (see docs/SKIN_CORPUS.md)", manifest.display()));
    let entries =
        corpus::parse_manifest(&text).unwrap_or_else(|e| panic!("{}: {e}", manifest.display()));
    let filter = std::env::var("SLAM_SKIN_CORPUS_FILTER").unwrap_or_default();
    let entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.name.contains(&filter))
        .collect();
    assert!(
        !entries.is_empty(),
        "no skins in the corpus match `{filter}`"
    );

    let mut checker = Checker::new(
        root.join("reference"),
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("skin-corpus"),
        check::update_requested(),
    );
    let map = scene_map();
    for entry in entries {
        let summary = shoot(entry, &root, &map, &mut checker);
        println!("{:<10} {}", summary, entry.name);
    }
    checker.assert_no_failures();
}
