//! The bundled default skin contains everything osu!standard gameplay needs.

use std::path::{Path, PathBuf};

fn default_skin_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/default-skin")
}

/// Images and samples used by gameplay, besides the hitsounds of each sample set.
const REQUIRED: &[&str] = &[
    // Hit circles and combo numbers.
    "hitcircle@2x.png",
    "hitcircleoverlay@2x.png",
    "approachcircle@2x.png",
    "default-0@2x.png",
    "default-1@2x.png",
    "default-2@2x.png",
    "default-3@2x.png",
    "default-4@2x.png",
    "default-5@2x.png",
    "default-6@2x.png",
    "default-7@2x.png",
    "default-8@2x.png",
    "default-9@2x.png",
    "followpoint@2x.png",
    "lighting@2x.png",
    // Sliders.
    "sliderb0@2x.png",
    "sliderfollowcircle@2x.png",
    "sliderscorepoint@2x.png",
    "reversearrow@2x.png",
    // Spinner.
    "spinner-approachcircle@2x.png",
    "spinner-bottom@2x.png",
    "spinner-top@2x.png",
    "spinner-middle@2x.png",
    "spinner-middle2@2x.png",
    "spinner-glow@2x.png",
    "spinner-clear@2x.png",
    "spinner-spin@2x.png",
    "spinnerspin.wav",
    "spinnerbonus.wav",
    // Cursor.
    "cursor@2x.png",
    "cursormiddle@2x.png",
    "cursortrail@2x.png",
    // Judgements.
    "hit0@2x.png",
    "hit50@2x.png",
    "hit100@2x.png",
    "hit100k@2x.png",
    "hit300@2x.png",
    "hit300g@2x.png",
    "hit300k@2x.png",
    "sliderendmiss@2x.png",
    "slidertickmiss@2x.png",
    "combobreak.mp3",
    // Licence and attribution.
    "LICENCE.md",
    "README.md",
];

const SAMPLE_SETS: &[&str] = &["normal", "soft", "drum"];
const SAMPLES: &[&str] = &[
    "hitnormal",
    "hitwhistle",
    "hitfinish",
    "hitclap",
    "slidertick",
    "sliderslide",
    "sliderwhistle",
];

#[test]
fn default_skin_has_gameplay_elements() {
    let dir = default_skin_dir();
    let hitsounds = SAMPLE_SETS
        .iter()
        .flat_map(|set| SAMPLES.iter().map(move |s| format!("{set}-{s}.wav")));
    let missing: Vec<String> = REQUIRED
        .iter()
        .map(|f| (*f).to_owned())
        .chain(hitsounds)
        .filter(|f| !dir.join(f).is_file())
        .collect();
    assert!(
        missing.is_empty(),
        "missing from {}: {missing:?}",
        dir.display()
    );
}
