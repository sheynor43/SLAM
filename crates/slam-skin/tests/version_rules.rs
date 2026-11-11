//! The version rules table: one skin folder read as `Version: 1.0` and `Version: latest`,
//! the thresholds of each rule, and where the version comes from in a chain.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use slam_skin::{Skin, SkinChain, SkinVersion, VersionRules};

/// A temporary skin folder, removed on drop.
struct TempSkin(PathBuf);

impl TempSkin {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "slam-version-rules-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn ini(&self, text: &str) -> &Self {
        fs::write(self.0.join("skin.ini"), text).unwrap();
        self
    }

    fn load(&self) -> Skin {
        Skin::load(&self.0).unwrap()
    }
}

impl Drop for TempSkin {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn rules(version: &str) -> VersionRules {
    VersionRules::new(Some(SkinVersion::parse(version).unwrap()))
}

#[test]
fn the_same_folder_as_version_1_and_latest() {
    let dir = TempSkin::new();
    let v1 = VersionRules::of_skin(&dir.ini("[General]\nVersion: 1.0\n").load());
    assert_eq!(
        v1,
        VersionRules {
            number_short_fade: false,
            miss_falls: false,
            slider_points: true,
            reverse_arrow_rotates: true,
        }
    );
    let latest = VersionRules::of_skin(&dir.ini("[General]\nVersion: latest\n").load());
    assert_eq!(
        latest,
        VersionRules {
            number_short_fade: true,
            miss_falls: true,
            slider_points: false,
            reverse_arrow_rotates: false,
        }
    );
}

#[test]
fn a_skin_ini_without_version_is_version_1() {
    let dir = TempSkin::new();
    let rules = VersionRules::of_skin(&dir.ini("[General]\nName: x\n").load());
    assert_eq!(rules, VersionRules::new(Some(SkinVersion::V1)));
}

#[test]
fn a_skin_without_skin_ini_is_latest() {
    let dir = TempSkin::new();
    assert_eq!(
        VersionRules::of_skin(&dir.load()),
        VersionRules::new(Some(SkinVersion::LATEST))
    );
}

#[test]
fn the_number_fades_briefly_and_misses_fall_above_version_1() {
    for (version, above_1) in [("1", false), ("1.0", false), ("1.01", true), ("2", true)] {
        let r = rules(version);
        assert_eq!(r.number_short_fade, above_1, "{version}");
        assert_eq!(r.miss_falls, above_1, "{version}");
    }
}

#[test]
fn slider_points_are_used_below_version_2() {
    for (version, below_2) in [("1.0", true), ("1.99", true), ("2", false), ("2.0", false)] {
        assert_eq!(rules(version).slider_points, below_2, "{version}");
    }
}

#[test]
fn reverse_arrows_rotate_up_to_version_1() {
    for (version, up_to_1) in [("0.5", true), ("1.0", true), ("1.1", false), ("2.5", false)] {
        assert_eq!(rules(version).reverse_arrow_rotates, up_to_1, "{version}");
    }
}

#[test]
fn no_version_turns_every_rule_off() {
    assert_eq!(
        VersionRules::new(None),
        VersionRules {
            number_short_fade: false,
            miss_falls: false,
            slider_points: false,
            reverse_arrow_rotates: false,
        }
    );
}

#[test]
fn a_chain_reads_the_first_version_past_the_beatmap_skin() {
    let beatmap = TempSkin::new();
    let user = TempSkin::new();
    user.ini("[General]\nVersion: 1.0\n");
    let default = TempSkin::new();
    let osu = "osu file format v14\n\n[General]\nVersion: 2.5\n";
    let chain = SkinChain::new(Some(user.load()), default.load()).with_beatmap(Some(
        Skin::load_beatmap(&beatmap.0, osu.as_bytes()).unwrap(),
    ));
    assert_eq!(VersionRules::of_chain(&chain), rules("1.0"));
    // Without a user skin the default skin answers, which is the latest version.
    let chain = SkinChain::new(None, default.load());
    assert_eq!(VersionRules::of_chain(&chain), rules("2.7"));
}

#[test]
fn a_beatmap_skin_has_no_version_of_its_own() {
    let dir = TempSkin::new();
    let osu = "osu file format v14\n\n[General]\nVersion: 1.0\n";
    let skin = Skin::load_beatmap(&dir.0, osu.as_bytes()).unwrap();
    assert_eq!(VersionRules::of_skin(&skin), VersionRules::new(None));
}
