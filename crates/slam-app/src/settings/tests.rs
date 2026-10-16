use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use slam_audio::StreamConfig;
use slam_engine::EngineConfig;
use slam_engine::limiter::LimiterMode;
use slam_input::MouseButton;
use slam_play::BackgroundSettings;
use slam_play::input::{Binding, Bindings, Input, MAX_BINDINGS};

use super::dirs::{Os, resolve};
use super::file::{self, LoadStatus};
use super::keys;
use super::model::{BindingEntry, DrawRate, Settings};

fn temp_dir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "slam-settings-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn defaults_match_hardcoded_values() {
    let s = Settings::default();
    assert_eq!(s.bindings(), Bindings::default());
    assert_eq!(s.frame.draw.to_limiter(), EngineConfig::default().draw);
    assert_eq!(s.frame.update_hz, 1000.0);
    assert_eq!(s.audio.buffer_frames, StreamConfig::default().buffer_frames);
    assert_eq!(s.gameplay.dim, BackgroundSettings::default().dim_level);
    assert_eq!(s.audio.master_volume, 1.0);
}

#[test]
fn round_trip_non_default() {
    let dir = temp_dir("rt");
    let path = dir.join("sub").join("slam.toml");
    let mut s = Settings::default();
    s.paths.songs = Some(PathBuf::from("/games/osu/Songs"));
    s.audio.backend = Some("pipewire".into());
    s.audio.device = Some("dev".into());
    s.audio.buffer_frames = 256;
    s.audio.master_volume = 0.5;
    s.audio.music_volume = 0.25;
    s.timing.global_offset_ms = -12.5;
    s.frame.update_hz = 2000.0;
    s.frame.draw = DrawRate::Hz(240.0);
    s.gameplay.dim = 0.3;
    s.gameplay.beatmap_colours = false;
    s.gameplay.mouse_buttons = false;
    s.input.bindings = vec![
        BindingEntry {
            input: "F".into(),
            action: "left".into(),
        },
        BindingEntry {
            input: "MouseX1".into(),
            action: "smoke".into(),
        },
    ];
    file::save(&path, &s).unwrap();
    assert!(!dir.join("sub").join("slam.toml.tmp").exists());
    let loaded = file::load(&path);
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings, s);

    s.frame.draw = DrawRate::Unlimited;
    file::save(&path, &s).unwrap();
    assert_eq!(file::load(&path).settings.frame.draw, DrawRate::Unlimited);
}

#[test]
fn defaults_round_trip() {
    let dir = temp_dir("defaults");
    let path = dir.join("slam.toml");
    file::save(&path, &Settings::default()).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("version = 1"), "{text}");
    let loaded = file::load(&path);
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings, Settings::default());
}

#[test]
fn missing_and_empty() {
    let dir = temp_dir("empty");
    let path = dir.join("slam.toml");
    let loaded = file::load(&path);
    assert_eq!(loaded.status, LoadStatus::Missing);
    assert_eq!(loaded.settings, Settings::default());
    std::fs::write(&path, "").unwrap();
    let loaded = file::load(&path);
    assert_eq!(loaded.status, LoadStatus::Ok);
    assert_eq!(loaded.settings, Settings::default());
}

#[test]
fn load_or_create_writes_defaults_only_when_missing() {
    let dir = temp_dir("create");
    let path = dir.join("cfg").join("slam.toml");
    let loaded = file::load_or_create(&path);
    assert_eq!(loaded.status, LoadStatus::Missing);
    assert!(path.exists());
    assert_eq!(file::load(&path).status, LoadStatus::Ok);
}

#[test]
fn partially_broken_keeps_good_fields() {
    let loaded = file::from_toml_str(
        r#"
version = 1
[audio]
master_volume = 3.0
music_volume = 0.4
buffer_frames = "big"
[timing]
global_offset_ms = 12
[frame]
draw = "fast"
update_hz = 500
[gameplay]
dim = 0.1
"#,
    );
    assert_eq!(loaded.status, LoadStatus::Recovered);
    let s = loaded.settings;
    assert_eq!(s.audio.master_volume, 1.0);
    assert_eq!(s.audio.music_volume, 0.4);
    assert_eq!(s.audio.buffer_frames, StreamConfig::default().buffer_frames);
    assert_eq!(s.timing.global_offset_ms, 12.0);
    assert_eq!(s.frame.draw, Settings::default().frame.draw);
    assert_eq!(s.frame.update_hz, 500.0);
    assert_eq!(s.gameplay.dim, 0.1);
    for field in ["audio.master_volume", "audio.buffer_frames", "frame.draw"] {
        assert!(
            loaded.warnings.iter().any(|w| w.starts_with(field)),
            "{field}: {:?}",
            loaded.warnings
        );
    }
}

#[test]
fn draw_rate_forms() {
    let hz = file::from_toml_str("[frame]\ndraw = 240.0\n");
    assert_eq!(hz.settings.frame.draw, DrawRate::Hz(240.0));
    assert_eq!(hz.settings.frame.draw.to_limiter(), LimiterMode::Hz(240.0));
    let int = file::from_toml_str("[frame]\ndraw = 144\n");
    assert_eq!(int.settings.frame.draw, DrawRate::Hz(144.0));
    let un = file::from_toml_str("[frame]\ndraw = \"Unlimited\"\n");
    assert_eq!(un.settings.frame.draw, DrawRate::Unlimited);
    assert_eq!(un.settings.frame.draw.to_limiter(), LimiterMode::Unlimited);
    let bad = file::from_toml_str("[frame]\ndraw = 1e9\n");
    assert_eq!(bad.status, LoadStatus::Recovered);
}

#[test]
fn garbage_is_broken_and_untouched() {
    let dir = temp_dir("garbage");
    let path = dir.join("slam.toml");
    std::fs::write(&path, "this is = = not toml [[[").unwrap();
    let loaded = file::load_or_create(&path);
    assert_eq!(loaded.status, LoadStatus::Broken);
    assert_eq!(loaded.settings, Settings::default());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "this is = = not toml [[["
    );
}

#[test]
fn recovered_file_is_not_overwritten() {
    let dir = temp_dir("recovered");
    let path = dir.join("slam.toml");
    let text = "[audio]\nmaster_volume = 9\n";
    std::fs::write(&path, text).unwrap();
    assert_eq!(file::load_or_create(&path).status, LoadStatus::Recovered);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn future_version_and_unknown_keys() {
    let loaded = file::from_toml_str(
        "version = 99\n[gameplay]\ndim = 0.2\nbogus = 1\n[extra]\nx = 1\ntop = 2\n",
    );
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.settings.gameplay.dim, 0.2);
    for needle in ["version", "gameplay.bogus", "extra"] {
        assert!(
            loaded.warnings.iter().any(|w| w.starts_with(needle)),
            "{needle}: {:?}",
            loaded.warnings
        );
    }
}

#[test]
fn bad_bindings_are_skipped() {
    let loaded = file::from_toml_str(
        r#"
[[input.bindings]]
input = "Q"
action = "left"
[[input.bindings]]
input = "Nope"
action = "left"
[[input.bindings]]
input = "W"
action = "jump"
[[input.bindings]]
input = "mouseleft"
action = "RIGHT"
"#,
    );
    assert_eq!(loaded.status, LoadStatus::Recovered);
    let expected = Bindings::new(&[
        Binding {
            input: Input::Key(20),
            action: slam_osu::replay::Actions::LEFT,
        },
        Binding {
            input: Input::Mouse(MouseButton::Left),
            action: slam_osu::replay::Actions::RIGHT,
        },
    ]);
    assert_eq!(loaded.settings.bindings(), expected);
    assert_eq!(loaded.warnings.len(), 2);

    let all_bad = file::from_toml_str("[[input.bindings]]\ninput = \"Nope\"\naction = \"left\"\n");
    assert_eq!(all_bad.settings.bindings(), Bindings::default());
}

#[test]
fn too_many_bindings_are_dropped() {
    let mut s = Settings::default();
    s.input.bindings = (0..MAX_BINDINGS + 3)
        .map(|i| BindingEntry {
            input: format!("Scancode{}", 100 + i),
            action: "left".into(),
        })
        .collect();
    assert_eq!(s.bindings().as_slice().len(), MAX_BINDINGS);
}

#[test]
fn key_names_round_trip() {
    let mut codes: Vec<u16> = (4..=69).collect();
    codes.extend([79, 80, 81, 82, 224, 225, 226, 228, 229, 230, 100, 500]);
    for code in codes {
        let name = keys::key_name(code);
        assert_eq!(keys::key_from_name(&name), Some(code), "{name}");
        assert_eq!(
            keys::key_from_name(&name.to_lowercase()),
            Some(code),
            "{name}"
        );
        assert_eq!(
            keys::key_from_name(&name.to_uppercase()),
            Some(code),
            "{name}"
        );
    }
    for button in [
        MouseButton::Left,
        MouseButton::Middle,
        MouseButton::Right,
        MouseButton::X1,
        MouseButton::X2,
    ] {
        let name = keys::input_name(Input::Mouse(button));
        assert_eq!(
            keys::input_from_name(&name.to_lowercase()),
            Some(Input::Mouse(button))
        );
    }
    assert_eq!(keys::key_name(29), "Z");
    assert_eq!(keys::key_name(39), "0");
    assert_eq!(keys::key_name(58), "F1");
    assert_eq!(keys::key_name(69), "F12");
    assert_eq!(keys::key_name(500), "Scancode500");
    assert_eq!(keys::key_from_name("F13"), None);
    assert_eq!(keys::key_from_name("F+1"), None);
    assert_eq!(keys::key_from_name(""), None);
}

fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |name| {
        pairs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| OsString::from(*v))
    }
}

#[test]
fn dirs_linux() {
    let none = |_: &Path| false;
    let d = resolve(&env_of(&[("HOME", "/home/u")]), None, &none, Os::Unix).unwrap();
    assert_eq!(d.config, Path::new("/home/u/.config/slam"));
    assert_eq!(d.data, Path::new("/home/u/.local/share/slam"));
    assert!(!d.portable);

    let d = resolve(
        &env_of(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/c"),
            ("XDG_DATA_HOME", "/d"),
        ]),
        None,
        &none,
        Os::Unix,
    )
    .unwrap();
    assert_eq!(d.config, Path::new("/c/slam"));
    assert_eq!(d.data, Path::new("/d/slam"));
    assert_eq!(d.settings_file(), Path::new("/c/slam/slam.toml"));

    let d = resolve(
        &env_of(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", ""),
            ("XDG_DATA_HOME", "rel/path"),
        ]),
        None,
        &none,
        Os::Unix,
    )
    .unwrap();
    assert_eq!(d.config, Path::new("/home/u/.config/slam"));
    assert_eq!(d.data, Path::new("/home/u/.local/share/slam"));

    let d = resolve(
        &env_of(&[("XDG_CONFIG_HOME", "/c"), ("XDG_DATA_HOME", "/d")]),
        None,
        &none,
        Os::Unix,
    )
    .unwrap();
    assert_eq!(d.config, Path::new("/c/slam"));
    assert!(resolve(&env_of(&[]), None, &none, Os::Unix).is_none());
    assert!(resolve(&env_of(&[("HOME", "relative")]), None, &none, Os::Unix).is_none());
}

#[test]
fn dirs_windows_and_portable() {
    let none = |_: &Path| false;
    let d = resolve(
        &env_of(&[("APPDATA", "C:\\Users\\u\\AppData\\Roaming")]),
        None,
        &none,
        Os::Windows,
    )
    .unwrap();
    assert_eq!(
        d.config,
        Path::new("C:\\Users\\u\\AppData\\Roaming").join("SLAM")
    );
    assert_eq!(d.config, d.data);
    assert!(resolve(&env_of(&[]), None, &none, Os::Windows).is_none());

    let exe = Path::new("/opt/slam");
    let marker = |p: &Path| p == Path::new("/opt/slam/portable");
    let d = resolve(
        &env_of(&[("HOME", "/home/u")]),
        Some(exe),
        &marker,
        Os::Unix,
    )
    .unwrap();
    assert!(d.portable);
    assert_eq!(d.config, exe);
    assert_eq!(d.data, exe);
    let d = resolve(&env_of(&[]), Some(exe), &marker, Os::Windows).unwrap();
    assert!(d.portable);
    let d = resolve(&env_of(&[("HOME", "/h")]), Some(exe), &none, Os::Unix).unwrap();
    assert!(!d.portable);
}
