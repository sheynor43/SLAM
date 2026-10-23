use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use slam_audio::StreamConfig;
use slam_engine::EngineConfig;
use slam_engine::limiter::LimiterMode;
use slam_input::MouseButton;
use slam_osu::replay::Actions;
use slam_play::input::{Binding, Bindings, Input, MAX_BINDINGS};
use slam_play::{BackgroundSettings, FpsCorner};

use super::dirs::{Os, resolve};
use super::file::{self, LoadStatus};
use super::keys;
use super::model::{DrawRate, FpsScreens, Frame, InputSettings, KeySlot, Settings};

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
    assert_eq!(s.gameplay.hit_error_scale, 1.0);
    assert_eq!(s.skin.name, None);
    assert_eq!(s.audio.master_volume, 1.0);
    assert_eq!(s.audio.hitsound_volume, 1.0);
    assert_eq!(s.audio.effects_volume, 1.0);
}

#[test]
fn round_trip_non_default() {
    let dir = temp_dir("rt");
    let path = dir.join("sub").join("slam.toml");
    let mut s = Settings::default();
    s.paths.songs = Some(PathBuf::from("/games/osu/Songs"));
    s.skin.name = Some("My Skin".into());
    s.audio.backend = Some("pipewire".into());
    s.audio.device = Some("dev".into());
    s.audio.buffer_frames = 256;
    s.audio.master_volume = 0.5;
    s.audio.music_volume = 0.25;
    s.audio.hitsound_volume = 0.75;
    s.audio.effects_volume = 0.125;
    s.timing.global_offset_ms = -12.5;
    s.frame.update_hz = 2000.0;
    s.frame.draw = DrawRate::Hz(240.0);
    s.frame.menu_draw = DrawRate::Hz(60.0);
    s.frame.show_fps = true;
    s.frame.fps_corner = FpsCorner::TopLeft;
    s.frame.fps_screens = FpsScreens::Gameplay;
    s.hotkeys.toggle_fps = Some(keys::input_from_name("F11").unwrap());
    s.gameplay.dim = 0.3;
    s.gameplay.beatmap_skins = false;
    s.gameplay.beatmap_colours = false;
    s.gameplay.beatmap_hitsounds = false;
    s.gameplay.mouse_buttons = false;
    s.gameplay.hit_error_scale = 1.5;
    s.input.bindings = Bindings::new(&[
        Binding {
            input: keys::input_from_name("F").unwrap(),
            action: Actions::LEFT,
        },
        Binding {
            input: Input::Mouse(MouseButton::X1),
            action: Actions::SMOKE,
        },
    ]);
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
hitsound_volume = 0.6
effects_volume = 2.0
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
    assert_eq!(s.audio.hitsound_volume, 0.6);
    assert_eq!(s.audio.effects_volume, 1.0);
    assert_eq!(s.audio.buffer_frames, StreamConfig::default().buffer_frames);
    assert_eq!(s.timing.global_offset_ms, 12.0);
    assert_eq!(s.frame.draw, Settings::default().frame.draw);
    assert_eq!(s.frame.update_hz, 500.0);
    assert_eq!(s.gameplay.dim, 0.1);
    for field in [
        "audio.master_volume",
        "audio.effects_volume",
        "audio.buffer_frames",
        "frame.draw",
    ] {
        assert!(
            loaded.warnings.iter().any(|w| w.starts_with(field)),
            "{field}: {:?}",
            loaded.warnings
        );
    }
}

#[test]
fn beatmap_skins_defaults_to_true_and_reads_false() {
    let missing = file::from_toml_str("[gameplay]\ndim = 0.2\n");
    assert_eq!(missing.status, LoadStatus::Ok, "{:?}", missing.warnings);
    assert!(missing.settings.gameplay.beatmap_skins);
    let off = file::from_toml_str("[gameplay]\nbeatmap_skins = false\n");
    assert_eq!(off.status, LoadStatus::Ok, "{:?}", off.warnings);
    assert!(!off.settings.gameplay.beatmap_skins);
}

#[test]
fn hit_error_scale_missing_defaults_and_round_trips() {
    let missing = file::from_toml_str("[gameplay]\ndim = 0.2\n");
    assert_eq!(missing.status, LoadStatus::Ok, "{:?}", missing.warnings);
    assert_eq!(missing.settings.gameplay.hit_error_scale, 1.0);
    let int = file::from_toml_str("[gameplay]\nhit_error_scale = 2\n");
    assert_eq!(int.settings.gameplay.hit_error_scale, 2.0);
    let mut s = Settings::default();
    s.gameplay.hit_error_scale = 0.73;
    let text = file::to_toml_string(&s).unwrap();
    assert!(text.contains("hit_error_scale = 0.73"), "{text}");
    assert_eq!(file::from_toml_str(&text).settings, s);
}

#[test]
fn hit_error_scale_is_rounded_to_its_step_and_out_of_range_falls_back() {
    let rounded = file::from_toml_str("[gameplay]\nhit_error_scale = 1.044\n");
    assert_eq!(rounded.settings.gameplay.hit_error_scale, 1.04);
    for bad in ["0.1", "5.0", "\"big\""] {
        let loaded = file::from_toml_str(&format!("[gameplay]\nhit_error_scale = {bad}\n"));
        assert_eq!(loaded.status, LoadStatus::Recovered, "{bad}");
        assert_eq!(loaded.settings.gameplay.hit_error_scale, 1.0, "{bad}");
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.starts_with("gameplay.hit_error_scale")),
            "{bad}"
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
fn menu_draw_missing_defaults_and_round_trips() {
    let missing = file::from_toml_str("[frame]\ndraw = 240.0\n");
    assert_eq!(missing.status, LoadStatus::Ok, "{:?}", missing.warnings);
    assert_eq!(missing.settings.frame.menu_draw, Frame::default().draw);
    let set = file::from_toml_str("[frame]\nmenu_draw = \"unlimited\"\n");
    assert_eq!(set.settings.frame.menu_draw, DrawRate::Unlimited);
    let mut s = Settings::default();
    s.frame.menu_draw = DrawRate::Hz(144.0);
    let text = file::to_toml_string(&s).unwrap();
    assert!(text.contains("menu_draw = 144.0"), "{text}");
    assert_eq!(file::from_toml_str(&text).settings, s);
}

#[test]
fn menu_draw_invalid_falls_back_with_a_warning() {
    let bad = file::from_toml_str("[frame]\nmenu_draw = 1e9\n");
    assert_eq!(bad.status, LoadStatus::Recovered);
    assert_eq!(bad.settings.frame.menu_draw, Frame::default().menu_draw);
    assert!(
        bad.warnings
            .iter()
            .any(|w| w.starts_with("frame.menu_draw"))
    );
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
fn an_empty_skin_name_is_the_default_skin() {
    let loaded = file::from_toml_str("[skin]\nname = \"\"\n");
    assert_eq!(loaded.status, LoadStatus::Ok);
    assert_eq!(loaded.settings.skin.name, None);
    let loaded = file::from_toml_str("[skin]\nname = \"Cool\"\n");
    assert_eq!(loaded.settings.skin.name.as_deref(), Some("Cool"));
}

#[test]
fn bad_skin_values_warn_and_unknown_skin_keys_are_reported() {
    let loaded = file::from_toml_str("[skin]\nname = 5\nbogus = 1\n");
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.settings.skin.name, None);
    for needle in ["skin.name", "skin.bogus"] {
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
            action: Actions::LEFT,
        },
        Binding {
            input: Input::Mouse(MouseButton::Left),
            action: Actions::RIGHT,
        },
    ]);
    assert_eq!(loaded.settings.bindings(), expected);
    assert_eq!(loaded.warnings.len(), 2);

    let all_bad = file::from_toml_str("[[input.bindings]]\ninput = \"Nope\"\naction = \"left\"\n");
    assert_eq!(all_bad.settings.bindings(), Bindings::default());
}

#[test]
fn too_many_bindings_are_dropped() {
    let text: String = (0..MAX_BINDINGS + 3)
        .map(|i| {
            format!(
                "[[input.bindings]]\ninput = \"Scancode{}\"\naction = \"left\"\n",
                100 + i
            )
        })
        .collect();
    let loaded = file::from_toml_str(&text);
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.settings.bindings().as_slice().len(), MAX_BINDINGS);
    assert_eq!(
        loaded.settings.bindings().as_slice()[0].input,
        Input::Key(100)
    );
}

fn key(name: &str) -> Input {
    keys::input_from_name(name).unwrap()
}

#[test]
fn default_slots_follow_lazer() {
    let input = InputSettings::default();
    assert_eq!(input.slot(Actions::LEFT, 0), Some(key("Z")));
    assert_eq!(
        input.slot(Actions::LEFT, 1),
        Some(Input::Mouse(MouseButton::Left))
    );
    assert_eq!(input.slot(Actions::RIGHT, 0), Some(key("X")));
    assert_eq!(
        input.slot(Actions::RIGHT, 1),
        Some(Input::Mouse(MouseButton::Right))
    );
    assert_eq!(input.slot(Actions::SMOKE, 0), Some(key("C")));
    assert_eq!(input.slot(Actions::SMOKE, 1), None);
}

#[test]
fn binding_a_slot_replaces_its_input_or_appends() {
    let mut input = InputSettings::default();
    assert!(input.bind(Actions::LEFT, 0, key("A")));
    assert_eq!(input.slot(Actions::LEFT, 0), Some(key("A")));
    assert_eq!(
        input.slot(Actions::LEFT, 1),
        Some(Input::Mouse(MouseButton::Left))
    );

    assert!(input.bind(Actions::SMOKE, 1, key("V")));
    assert_eq!(input.slot(Actions::SMOKE, 1), Some(key("V")));
    assert_eq!(input.bindings.as_slice().len(), 6);
}

#[test]
fn conflicts_name_the_other_slot() {
    let input = InputSettings::default();
    assert_eq!(
        input.conflict(Actions::LEFT, 0, key("X")),
        Some((Actions::RIGHT, 0))
    );
    assert_eq!(
        input.conflict(Actions::SMOKE, 1, Input::Mouse(MouseButton::Left)),
        Some((Actions::LEFT, 1))
    );
    assert_eq!(input.conflict(Actions::LEFT, 0, key("Z")), None);
    assert_eq!(input.conflict(Actions::LEFT, 0, key("Q")), None);
}

#[test]
fn binding_a_taken_input_moves_it() {
    let mut input = InputSettings::default();
    assert!(input.bind(Actions::LEFT, 0, key("X")));
    assert_eq!(input.slot(Actions::LEFT, 0), Some(key("X")));
    assert_eq!(
        input.slot(Actions::RIGHT, 0),
        Some(Input::Mouse(MouseButton::Right))
    );
    assert_eq!(input.slot(Actions::RIGHT, 1), None);
    assert_eq!(input.bindings.actions_of(key("X")), Actions::LEFT);
}

#[test]
fn clearing_moves_later_bindings_up() {
    let mut input = InputSettings::default();
    input.clear(Actions::LEFT, 0);
    assert_eq!(
        input.slot(Actions::LEFT, 0),
        Some(Input::Mouse(MouseButton::Left))
    );
    assert_eq!(input.slot(Actions::LEFT, 1), None);
    input.clear(Actions::SMOKE, 1);
    assert_eq!(input.bindings.as_slice().len(), 4);

    input.reset();
    assert_eq!(input, InputSettings::default());
}

#[test]
fn a_full_list_takes_no_new_slot() {
    let mut input = InputSettings::default();
    for i in input.bindings.as_slice().len()..MAX_BINDINGS {
        assert!(input.bind(Actions::SMOKE, i, Input::Key(100 + i as u16)));
    }
    assert!(!input.bind(Actions::RIGHT, 2, key("Q")));
    assert!(input.bind(Actions::RIGHT, 0, key("Q")));
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

/// `path` made absolute on this platform: Windows needs a drive.
fn abs(path: &str) -> String {
    if cfg!(windows) {
        format!("C:{path}")
    } else {
        path.to_owned()
    }
}

#[test]
fn dirs_linux() {
    let none = |_: &Path| false;
    let d = resolve(&env_of(&[("HOME", &abs("/home/u"))]), None, &none, Os::Unix).unwrap();
    assert_eq!(d.config, Path::new(&abs("/home/u/.config/slam")));
    assert_eq!(d.data, Path::new(&abs("/home/u/.local/share/slam")));
    assert!(!d.portable);

    let d = resolve(
        &env_of(&[
            ("HOME", &abs("/home/u")),
            ("XDG_CONFIG_HOME", &abs("/c")),
            ("XDG_DATA_HOME", &abs("/d")),
        ]),
        None,
        &none,
        Os::Unix,
    )
    .unwrap();
    assert_eq!(d.config, Path::new(&abs("/c/slam")));
    assert_eq!(d.data, Path::new(&abs("/d/slam")));
    assert_eq!(d.settings_file(), Path::new(&abs("/c/slam/slam.toml")));

    let d = resolve(
        &env_of(&[
            ("HOME", &abs("/home/u")),
            ("XDG_CONFIG_HOME", ""),
            ("XDG_DATA_HOME", "rel/path"),
        ]),
        None,
        &none,
        Os::Unix,
    )
    .unwrap();
    assert_eq!(d.config, Path::new(&abs("/home/u/.config/slam")));
    assert_eq!(d.data, Path::new(&abs("/home/u/.local/share/slam")));

    let d = resolve(
        &env_of(&[
            ("XDG_CONFIG_HOME", &abs("/c")),
            ("XDG_DATA_HOME", &abs("/d")),
        ]),
        None,
        &none,
        Os::Unix,
    )
    .unwrap();
    assert_eq!(d.config, Path::new(&abs("/c/slam")));
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
    let d = resolve(&env_of(&[("HOME", &abs("/h"))]), Some(exe), &none, Os::Unix).unwrap();
    assert!(!d.portable);
}

mod saver_tests {
    use super::*;
    use crate::settings::saver;

    #[test]
    fn write_keeps_the_files_songs_folder() {
        let dir = temp_dir("saver-paths");
        let path = dir.join("slam.toml");
        let mut on_disk = Settings::default();
        on_disk.paths.songs = Some(PathBuf::from("/from/file"));
        file::save(&path, &on_disk).unwrap();

        let mut edited = Settings::default();
        edited.paths.songs = Some(PathBuf::from("/from/flag"));
        edited.audio.master_volume = 0.3;
        saver::write(&path, edited).unwrap();

        let loaded = file::load(&path).settings;
        assert_eq!(loaded.paths.songs, Some(PathBuf::from("/from/file")));
        assert_eq!(loaded.audio.master_volume, 0.3);
    }

    #[test]
    fn write_creates_a_missing_file_and_spares_a_broken_one() {
        let dir = temp_dir("saver-broken");
        let path = dir.join("slam.toml");
        saver::write(&path, Settings::default()).unwrap();
        assert_eq!(file::load(&path).status, LoadStatus::Ok);

        std::fs::write(&path, "= = broken [[[").unwrap();
        assert!(saver::write(&path, Settings::default()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "= = broken [[[");
    }

    #[test]
    fn the_thread_writes_what_was_queued_before_it_finished() {
        let dir = temp_dir("saver-thread");
        let path = dir.join("slam.toml");
        let (mut link, thread) = saver::spawn(path.clone()).unwrap();
        for volume in [0.1, 0.2, 0.7] {
            let mut s = Settings::default();
            s.audio.master_volume = volume;
            assert!(link.request(Box::new(s)).is_ok());
        }
        thread.finish();
        assert_eq!(file::load(&path).settings.audio.master_volume, 0.7);
    }
}

#[test]
fn the_fps_counter_defaults_follow_lazer() {
    let s = Settings::default();
    assert!(!s.frame.show_fps);
    assert_eq!(s.frame.fps_corner, FpsCorner::BottomRight);
    assert_eq!(s.frame.fps_screens, FpsScreens::Everywhere);
    assert_eq!(s.hotkeys.toggle_fps, None);
}

#[test]
fn fps_counter_values_are_read_by_name() {
    let loaded = file::from_toml_str(
        "[frame]\nfps_corner = \"top_right\"\nfps_screens = \"menus\"\n\
         [hotkeys]\ntoggle_fps = \"f11\"\n",
    );
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings.frame.fps_corner, FpsCorner::TopRight);
    assert_eq!(loaded.settings.frame.fps_screens, FpsScreens::Menus);
    assert_eq!(loaded.settings.hotkeys.toggle_fps, Some(key("F11")));

    let loaded = file::from_toml_str(
        "[frame]\nfps_corner = \"middle\"\nfps_screens = 3\n[hotkeys]\ntoggle_fps = \"Nope\"\n",
    );
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.warnings.len(), 3, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings.frame, Frame::default());
    assert_eq!(loaded.settings.hotkeys.toggle_fps, None);
}

#[test]
fn a_hotkey_on_a_gameplay_key_is_dropped() {
    let loaded = file::from_toml_str("[hotkeys]\ntoggle_fps = \"Z\"\n");
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.settings.hotkeys.toggle_fps, None);
}

#[test]
fn hotkeys_and_gameplay_keys_conflict_both_ways() {
    let mut s = Settings::default();
    let fps = KeySlot::ToggleFps;
    assert_eq!(
        s.key_conflict(fps, Input::Mouse(MouseButton::Right)),
        Some(KeySlot::Play(Actions::RIGHT, 1))
    );
    assert_eq!(s.key_conflict(fps, key("F11")), None);
    assert!(s.bind_key(fps, key("F11")));
    assert_eq!(s.key(fps), Some(key("F11")));
    assert_eq!(
        s.key_conflict(KeySlot::Play(Actions::SMOKE, 1), key("F11")),
        Some(fps)
    );
    // Taking the hotkey's input for gameplay unbinds the hotkey, and the other way round.
    assert!(s.bind_key(KeySlot::Play(Actions::SMOKE, 1), key("F11")));
    assert_eq!(s.key(fps), None);
    assert!(s.bind_key(fps, key("Z")));
    assert_eq!(s.key(fps), Some(key("Z")));
    assert_eq!(
        s.input.slot(Actions::LEFT, 0),
        Some(Input::Mouse(MouseButton::Left))
    );
    assert!(
        !s.input
            .bindings
            .as_slice()
            .iter()
            .any(|b| b.input == key("Z"))
    );

    s.clear_key(fps);
    assert_eq!(s.key(fps), None);
    s.bind_key(fps, key("F11"));
    s.reset_keys();
    assert_eq!(s.key(fps), None);
    assert_eq!(s.bindings(), Bindings::default());
}

#[test]
fn a_hotkey_cannot_take_menu_or_gameplay_screen_keys() {
    let fps = KeySlot::ToggleFps;
    for name in [
        "Space", "Escape", "Enter", "Up", "Left", "Tab", "=", "-", "`", "F10",
    ] {
        assert!(!Settings::key_allowed(fps, key(name)), "{name}");
    }
    for button in [
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::X1,
        MouseButton::X2,
    ] {
        assert!(
            !Settings::key_allowed(fps, Input::Mouse(button)),
            "{button:?}"
        );
    }
    assert!(Settings::key_allowed(fps, key("F11")));
    assert!(Settings::key_allowed(fps, key("Q")));
    // Gameplay slots take anything, as before.
    assert!(Settings::key_allowed(
        KeySlot::Play(Actions::LEFT, 0),
        Input::Mouse(MouseButton::Left)
    ));

    let loaded = file::from_toml_str("[hotkeys]\ntoggle_fps = \"Space\"\n");
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.settings.hotkeys.toggle_fps, None);
}
