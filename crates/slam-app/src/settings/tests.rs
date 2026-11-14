use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use slam_audio::StreamConfig;
use slam_engine::EngineConfig;
use slam_engine::limiter::LimiterMode;
use slam_input::{MouseButton, VideoDriver, WindowMode};
use slam_osu::replay::Actions;
use slam_play::input::{Binding, Bindings, Input, MAX_BINDINGS};
use slam_play::{BackgroundSettings, FpsCorner};
use slam_ui::cursor::CursorStyle;
use slam_ui::screen::TransitionStyle;
use slam_ui::theme::ColourScheme;

use super::dirs::{Os, resolve};
use super::file::{self, LoadStatus};
use super::keys;
use super::model::{
    Combo, DrawRate, FpsScreens, Frame, HitsoundDelay, InputSettings, KeyOverlayAnimation, KeySlot,
    MenuBackground, MenuSounds, Mods, PauseMenuStyle, RendererChoice, Settings, Window,
};

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
    assert_eq!(s.hud.layout, "builtin:stable");
    assert_eq!(s.skin.name, None);
    assert_eq!(s.audio.master_volume, 1.0);
    assert_eq!(s.audio.hitsound_volume, 1.0);
    assert_eq!(s.audio.effects_volume, 1.0);
    assert!(s.audio.menu_music);
    assert_eq!(s.audio.menu_music_volume, 1.0);
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
    s.audio.menu_music = false;
    s.audio.menu_music_volume = 0.375;
    s.audio.hitsound_delay = HitsoundDelay::Stable;
    s.audio.positional_hitsounds_level = 0.65;
    s.timing.global_offset_ms = -12.5;
    s.frame.update_hz = 2000.0;
    s.frame.draw = DrawRate::Hz(240.0);
    s.frame.menu_draw = DrawRate::Hz(60.0);
    s.frame.show_fps = true;
    s.frame.fps_corner = FpsCorner::TopLeft;
    s.frame.fps_screens = FpsScreens::Gameplay;
    s.hotkeys.toggle_fps = Some(plain("F12"));
    s.gameplay.dim = 0.3;
    s.gameplay.beatmap_skins = false;
    s.gameplay.beatmap_colours = false;
    s.gameplay.beatmap_hitsounds = false;
    s.gameplay.always_play_first_combo_break = false;
    s.gameplay.disable_win_key = false;
    s.window.video_driver = VideoDriver::Wayland;
    s.window.renderer = RendererChoice::D3d11;
    s.gameplay.mouse_buttons = false;
    s.gameplay.snaking_in = true;
    s.gameplay.snaking_out = true;
    s.gameplay.hit_lighting = false;
    s.gameplay.key_overlay = true;
    s.gameplay.key_overlay_animation = KeyOverlayAnimation::Lazer;
    s.gameplay.hud_animations = false;
    s.gameplay.cursor_trail = false;
    s.gameplay.cursor_size = 0.65;
    s.gameplay.auto_cursor_size = true;
    s.general.colour_scheme = ColourScheme::Mono;
    s.general.menu_scale = 1.05;
    s.general.game_scale = 0.8;
    s.general.skew = 14.0;
    s.general.menu_background = MenuBackground::Skin;
    s.general.transitions = TransitionStyle::Fancy;
    s.gameplay.pause_menu_style = PauseMenuStyle::Stable;
    s.gameplay.pause_menu_scale = 1.25;
    s.general.menu_cursor = CursorStyle::System;
    s.gameplay.cursor_style = CursorStyle::Slam;
    s.audio.menu_sounds = MenuSounds::Off;
    s.hud.layout = "my-hud".into();
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
fn view_settings_default_to_no_snaking_with_lighting_and_trail() {
    let s = Settings::default();
    assert!(!s.gameplay.snaking_in);
    assert!(!s.gameplay.snaking_out);
    assert!(s.gameplay.hit_lighting);
    assert!(s.gameplay.cursor_trail);
    let missing = file::from_toml_str("[gameplay]\ndim = 0.2\n");
    assert_eq!(missing.status, LoadStatus::Ok, "{:?}", missing.warnings);
    assert!(!missing.settings.gameplay.snaking_in);
    assert!(!missing.settings.gameplay.snaking_out);
    assert!(missing.settings.gameplay.hit_lighting);
    assert!(missing.settings.gameplay.cursor_trail);
    assert!(!s.gameplay.key_overlay);
    assert!(s.gameplay.hud_animations);
    assert!(missing.settings.gameplay.hud_animations);
    assert_eq!(
        s.gameplay.key_overlay_animation,
        KeyOverlayAnimation::Stable
    );
    assert!(!missing.settings.gameplay.key_overlay);
    assert_eq!(
        missing.settings.gameplay.key_overlay_animation,
        KeyOverlayAnimation::Stable
    );
}

#[test]
fn the_key_overlay_animation_is_read_by_name() {
    let loaded = file::from_toml_str("[gameplay]\nkey_overlay_animation = \"lazer\"\n");
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(
        loaded.settings.gameplay.key_overlay_animation,
        KeyOverlayAnimation::Lazer
    );
    let loaded = file::from_toml_str("[gameplay]\nkey_overlay_animation = \"bass\"\n");
    assert_ne!(loaded.status, LoadStatus::Ok);
    assert_eq!(
        loaded.settings.gameplay.key_overlay_animation,
        KeyOverlayAnimation::Stable
    );
}

#[test]
fn hud_layout_defaults_to_stable_and_round_trips() {
    let missing = file::from_toml_str("[gameplay]\ndim = 0.2\n");
    assert_eq!(missing.status, LoadStatus::Ok, "{:?}", missing.warnings);
    assert_eq!(missing.settings.hud.layout, "builtin:stable");
    let mut s = Settings::default();
    s.hud.layout = "my-hud".into();
    let text = file::to_toml_string(&s).unwrap();
    assert!(text.contains("[hud]\nlayout = \"my-hud\""), "{text}");
    let loaded = file::from_toml_str(&text);
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings, s);
    let builtin = file::from_toml_str("[hud]\nlayout = \"builtin:lazer\"\n");
    assert_eq!(builtin.settings.hud.layout, "builtin:lazer");
}

#[test]
fn an_invalid_hud_layout_id_falls_back_with_a_warning() {
    for bad in ["\"../x\"", "\"Has Space\"", "\"\"", "\"builtin:nope\"", "5"] {
        let loaded = file::from_toml_str(&format!("[hud]\nlayout = {bad}\n"));
        assert_eq!(loaded.status, LoadStatus::Recovered, "{bad}");
        assert_eq!(loaded.settings.hud.layout, "builtin:stable", "{bad}");
        assert!(
            loaded.warnings.iter().any(|w| w.starts_with("hud.layout")),
            "{bad}"
        );
    }
}

#[test]
fn the_old_hit_error_scale_is_read_for_migration_and_never_written() {
    let none = file::from_toml_str("[gameplay]\ndim = 0.2\n");
    assert_eq!(none.legacy_hit_error_scale, None);
    let int = file::from_toml_str("[gameplay]\nhit_error_scale = 2\n");
    assert_eq!(int.status, LoadStatus::Ok, "{:?}", int.warnings);
    assert_eq!(int.legacy_hit_error_scale, Some(2.0));
    let rounded = file::from_toml_str("[gameplay]\nhit_error_scale = 1.044\n");
    assert_eq!(rounded.legacy_hit_error_scale, Some(1.04));
    for bad in ["0.1", "5.0", "\"big\""] {
        let loaded = file::from_toml_str(&format!("[gameplay]\nhit_error_scale = {bad}\n"));
        assert_eq!(loaded.status, LoadStatus::Recovered, "{bad}");
        assert_eq!(loaded.legacy_hit_error_scale, None, "{bad}");
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.starts_with("gameplay.hit_error_scale")),
            "{bad}"
        );
    }
    let text = file::to_toml_string(&int.settings).unwrap();
    assert!(!text.contains("hit_error_scale"), "{text}");
}

#[test]
fn cursor_size_defaults_to_lazers_rounds_and_falls_back_out_of_range() {
    let missing = file::from_toml_str("[gameplay]\ndim = 0.2\n");
    assert_eq!(missing.status, LoadStatus::Ok, "{:?}", missing.warnings);
    assert_eq!(missing.settings.gameplay.cursor_size, 1.0);
    assert!(!missing.settings.gameplay.auto_cursor_size);
    let read = file::from_toml_str("[gameplay]\ncursor_size = 0.104\nauto_cursor_size = true\n");
    assert_eq!(read.status, LoadStatus::Ok, "{:?}", read.warnings);
    assert_eq!(read.settings.gameplay.cursor_size, 0.1);
    assert!(read.settings.gameplay.auto_cursor_size);
    assert_eq!(
        file::from_toml_str("[gameplay]\ncursor_size = 2\n")
            .settings
            .gameplay
            .cursor_size,
        2.0
    );
    for bad in ["0.05", "2.5", "\"big\""] {
        let loaded = file::from_toml_str(&format!("[gameplay]\ncursor_size = {bad}\n"));
        assert_eq!(loaded.status, LoadStatus::Recovered, "{bad}");
        assert_eq!(loaded.settings.gameplay.cursor_size, 1.0, "{bad}");
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.starts_with("gameplay.cursor_size")),
            "{bad}"
        );
    }
}

#[test]
fn positional_hitsounds_level_defaults_to_lazers_rounds_and_falls_back_out_of_range() {
    let missing = file::from_toml_str("[audio]\nmaster_volume = 1.0\n");
    assert_eq!(missing.status, LoadStatus::Ok, "{:?}", missing.warnings);
    assert_eq!(missing.settings.audio.positional_hitsounds_level, 0.2);
    let read = file::from_toml_str("[audio]\npositional_hitsounds_level = 0.504\n");
    assert_eq!(read.status, LoadStatus::Ok, "{:?}", read.warnings);
    assert_eq!(read.settings.audio.positional_hitsounds_level, 0.5);
    for (ok, want) in [("0", 0.0), ("1", 1.0)] {
        let loaded = file::from_toml_str(&format!("[audio]\npositional_hitsounds_level = {ok}\n"));
        assert_eq!(loaded.settings.audio.positional_hitsounds_level, want);
    }
    for bad in ["-0.1", "1.5", "\"loud\""] {
        let loaded = file::from_toml_str(&format!("[audio]\npositional_hitsounds_level = {bad}\n"));
        assert_eq!(loaded.status, LoadStatus::Recovered, "{bad}");
        assert_eq!(
            loaded.settings.audio.positional_hitsounds_level, 0.2,
            "{bad}"
        );
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.starts_with("audio.positional_hitsounds_level")),
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

fn plain(name: &str) -> Combo {
    Combo::plain(key(name))
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
    fn write_songs_changes_only_the_songs_folder() {
        let dir = temp_dir("saver-songs");
        let path = dir.join("slam.toml");
        let mut on_disk = Settings::default();
        on_disk.audio.master_volume = 0.4;
        file::save(&path, &on_disk).unwrap();

        saver::write_songs(&path, Path::new("/picked/Songs")).unwrap();
        let loaded = file::load(&path).settings;
        assert_eq!(loaded.paths.songs, Some(PathBuf::from("/picked/Songs")));
        assert_eq!(loaded.audio.master_volume, 0.4);

        // The saver keeps it afterwards.
        saver::write(&path, Settings::default()).unwrap();
        let loaded = file::load(&path).settings;
        assert_eq!(loaded.paths.songs, Some(PathBuf::from("/picked/Songs")));

        std::fs::write(&path, "= = broken [[[").unwrap();
        assert!(saver::write_songs(&path, Path::new("/x")).is_err());
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
         [hotkeys]\ntoggle_fps = \"f12\"\n",
    );
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings.frame.fps_corner, FpsCorner::TopRight);
    assert_eq!(loaded.settings.frame.fps_screens, FpsScreens::Menus);
    assert_eq!(loaded.settings.hotkeys.toggle_fps, Some(plain("F12")));

    let loaded = file::from_toml_str(
        "[frame]\nfps_corner = \"middle\"\nfps_screens = 3\n[hotkeys]\ntoggle_fps = \"Nope\"\n",
    );
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.warnings.len(), 3, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings.frame, Frame::default());
    assert_eq!(loaded.settings.hotkeys.toggle_fps, None);
}

#[test]
fn the_hitsound_delay_is_read_by_name() {
    assert_eq!(
        Settings::default().audio.hitsound_delay,
        HitsoundDelay::None
    );
    let loaded = file::from_toml_str("[audio]\nhitsound_delay = \"stable\"\n");
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings.audio.hitsound_delay, HitsoundDelay::Stable);

    let loaded = file::from_toml_str("[audio]\nhitsound_delay = \"bass\"\n");
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings.audio.hitsound_delay, HitsoundDelay::None);
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
        s.key_conflict(fps, Combo::plain(Input::Mouse(MouseButton::Right))),
        Some(KeySlot::Play(Actions::RIGHT, 1))
    );
    assert_eq!(s.key_conflict(fps, plain("F12")), None);
    assert!(s.bind_key(fps, plain("F12")));
    assert_eq!(s.key(fps), Some(plain("F12")));
    assert_eq!(
        s.key_conflict(KeySlot::Play(Actions::SMOKE, 1), plain("F12")),
        Some(fps)
    );
    // Taking the hotkey's input for gameplay unbinds the hotkey, and the other way round.
    assert!(s.bind_key(KeySlot::Play(Actions::SMOKE, 1), plain("F12")));
    assert_eq!(s.key(fps), None);
    assert!(s.bind_key(fps, plain("Z")));
    assert_eq!(s.key(fps), Some(plain("Z")));
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
    s.bind_key(fps, plain("F12"));
    s.reset_keys();
    assert_eq!(s.key(fps), None);
    assert_eq!(s.bindings(), Bindings::default());
}

#[test]
fn a_hotkey_cannot_take_menu_or_gameplay_screen_keys() {
    let fps = KeySlot::ToggleFps;
    for name in [
        "Space", "Escape", "Enter", "Up", "Left", "Tab", "=", "-", "`", "F1", "F10",
    ] {
        assert!(!Settings::key_allowed(fps, plain(name)), "{name}");
    }
    for button in [
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::X1,
        MouseButton::X2,
    ] {
        assert!(
            !Settings::key_allowed(fps, Combo::plain(Input::Mouse(button))),
            "{button:?}"
        );
    }
    assert!(Settings::key_allowed(fps, plain("F12")));
    assert!(Settings::key_allowed(fps, plain("Q")));
    // F11 cycles the window mode (ADR-0070): no slot takes it.
    assert!(!Settings::key_allowed(fps, plain("F11")));
    assert!(!Settings::key_allowed(
        KeySlot::Play(Actions::LEFT, 0),
        plain("F11")
    ));
    // Gameplay slots take anything else, as before.
    assert!(Settings::key_allowed(
        KeySlot::Play(Actions::LEFT, 0),
        Combo::plain(Input::Mouse(MouseButton::Left))
    ));

    let loaded = file::from_toml_str("[hotkeys]\ntoggle_fps = \"Space\"\n");
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.settings.hotkeys.toggle_fps, None);
}

#[test]
fn mouse_defaults_follow_the_platform() {
    let input = InputSettings::default();
    assert_eq!(input.high_precision_mouse, cfg!(target_os = "windows"));
    assert_eq!(input.sensitivity, 1.0);
    let loaded = file::from_toml_str("[input]\n");
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings.input, input);
}

#[test]
fn mouse_settings_parse() {
    let loaded = file::from_toml_str("[input]\nhigh_precision_mouse = true\nsensitivity = 2.25\n");
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert!(loaded.settings.input.high_precision_mouse);
    assert_eq!(loaded.settings.input.sensitivity, 2.25);
    let int = file::from_toml_str("[input]\nsensitivity = 3\n");
    assert_eq!(int.settings.input.sensitivity, 3.0);
}

#[test]
fn sensitivity_is_clamped_and_rounded_on_load() {
    let rounded = file::from_toml_str("[input]\nsensitivity = 1.234\n");
    assert_eq!(rounded.status, LoadStatus::Ok);
    assert_eq!(rounded.settings.input.sensitivity, 1.23);
    for (text, want) in [("0.0", 0.1), ("-4", 0.1), ("50", 10.0), ("10.004", 10.0)] {
        let loaded = file::from_toml_str(&format!("[input]\nsensitivity = {text}\n"));
        assert_eq!(loaded.settings.input.sensitivity, want, "{text}");
    }
    let clamped = file::from_toml_str("[input]\nsensitivity = 50\n");
    assert!(
        clamped
            .warnings
            .iter()
            .any(|w| w.starts_with("input.sensitivity")),
    );
}

#[test]
fn invalid_mouse_values_fall_back_with_a_warning() {
    let loaded =
        file::from_toml_str("[input]\nhigh_precision_mouse = \"yes\"\nsensitivity = \"fast\"\n");
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.settings.input, InputSettings::default());
    assert_eq!(loaded.warnings.len(), 2, "{:?}", loaded.warnings);
}

#[test]
fn mouse_settings_round_trip_next_to_the_bindings() {
    let mut s = Settings::default();
    s.input.high_precision_mouse = !s.input.high_precision_mouse;
    s.input.sensitivity = 1.5;
    let text = file::to_toml_string(&s).unwrap();
    assert!(text.contains("sensitivity = 1.5"), "{text}");
    assert!(text.contains("high_precision_mouse"), "{text}");
    let loaded = file::from_toml_str(&text);
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings, s);
}

#[test]
fn window_defaults_are_lazer_first_start() {
    let window = Settings::default().window;
    let mode = if cfg!(target_os = "macos") {
        WindowMode::Borderless
    } else {
        WindowMode::Fullscreen
    };
    assert_eq!(window.mode, mode);
    assert_eq!(window.fullscreen_size, None);
    assert_eq!(window.windowed_size, [1366, 768]);
    assert_eq!(window.display, 0);
    assert_eq!(window.windowed_position, [0.5, 0.5]);
    assert!(window.minimise_on_focus_loss);
    assert_eq!(window.video_driver, VideoDriver::Auto);
    assert_eq!(window.renderer, RendererChoice::OpenGl);
}

#[test]
fn video_driver_is_read_and_an_invalid_one_falls_back_to_auto() {
    for (name, driver) in [
        ("auto", VideoDriver::Auto),
        ("wayland", VideoDriver::Wayland),
        ("x11", VideoDriver::X11),
    ] {
        let loaded = file::from_toml_str(&format!("[window]\nvideo_driver = \"{name}\"\n"));
        assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
        assert_eq!(loaded.settings.window.video_driver, driver);
        let text = file::to_toml_string(&loaded.settings).unwrap();
        assert!(
            text.contains(&format!("video_driver = \"{name}\"")),
            "{text}"
        );
    }
    let loaded = file::from_toml_str("[window]\nvideo_driver = \"mir\"\n");
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.settings.window.video_driver, VideoDriver::Auto);
    assert!(
        loaded
            .warnings
            .iter()
            .any(|w| w.contains("window.video_driver"))
    );
}

#[test]
fn renderer_is_read_and_an_invalid_one_falls_back_to_opengl() {
    for (name, renderer) in [
        ("opengl", RendererChoice::OpenGl),
        ("d3d11", RendererChoice::D3d11),
    ] {
        let loaded = file::from_toml_str(&format!("[window]\nrenderer = \"{name}\"\n"));
        assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
        assert_eq!(loaded.settings.window.renderer, renderer);
        let text = file::to_toml_string(&loaded.settings).unwrap();
        assert!(text.contains(&format!("renderer = \"{name}\"")), "{text}");
    }
    for name in ["vulkan", "auto"] {
        let loaded = file::from_toml_str(&format!("[window]\nrenderer = \"{name}\"\n"));
        assert_eq!(loaded.status, LoadStatus::Recovered);
        assert_eq!(loaded.settings.window.renderer, RendererChoice::OpenGl);
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.contains("window.renderer"))
        );
    }
}

#[test]
fn the_renderer_choice_maps_to_a_backend() {
    use crate::backend::Renderer;
    assert_eq!(RendererChoice::OpenGl.to_backend(), Renderer::Auto);
    assert_eq!(RendererChoice::D3d11.to_backend(), Renderer::PreferD3d11);
    assert_eq!(RendererChoice::default(), RendererChoice::OpenGl);
}

#[test]
fn window_section_is_read_and_written() {
    let loaded = file::from_toml_str(
        "[window]\nmode = \"borderless\"\nfullscreen_size = \"1920x1080\"\n\
         windowed_size = \"1280x720\"\ndisplay = 1\nwindowed_position = [0.3, -0.5]\n\
         minimise_on_focus_loss = false\n",
    );
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    let window = loaded.settings.window;
    assert_eq!(
        window,
        Window {
            mode: WindowMode::Borderless,
            fullscreen_size: Some([1920, 1080]),
            windowed_size: [1280, 720],
            display: 1,
            windowed_position: [0.3, -0.5],
            minimise_on_focus_loss: false,
            video_driver: VideoDriver::Auto,
            renderer: RendererChoice::OpenGl,
        }
    );
    let text = file::to_toml_string(&loaded.settings).unwrap();
    assert!(text.contains("[window]\nmode = \"borderless\"\nfullscreen_size = \"1920x1080\"\nwindowed_size = \"1280x720\"\ndisplay = 1\nwindowed_position = [0.3, -0.5]\nminimise_on_focus_loss = false\nvideo_driver = \"auto\"\n"), "{text}");
    assert_eq!(file::from_toml_str(&text).settings.window, window);

    let native = file::to_toml_string(&Settings::default()).unwrap();
    assert!(native.contains("fullscreen_size = \"native\""), "{native}");
    let back = file::from_toml_str(&native);
    assert_eq!(back.settings.window.fullscreen_size, None);
}

#[test]
fn invalid_window_values_fall_back_with_a_warning() {
    for (line, key) in [
        ("mode = \"maximised\"", "window.mode"),
        ("fullscreen_size = \"huge\"", "window.fullscreen_size"),
        ("fullscreen_size = \"300x200\"", "window.fullscreen_size"),
        ("fullscreen_size = \"20000x1080\"", "window.fullscreen_size"),
        ("windowed_size = \"639x480\"", "window.windowed_size"),
        ("windowed_size = \"1280\"", "window.windowed_size"),
        ("display = 64", "window.display"),
        ("display = -1", "window.display"),
        ("windowed_position = [0.5]", "window.windowed_position"),
        ("windowed_position = [0.5, 1.6]", "window.windowed_position"),
        ("windowed_position = \"centre\"", "window.windowed_position"),
        (
            "minimise_on_focus_loss = 1",
            "window.minimise_on_focus_loss",
        ),
        ("size = 3", "window.size"),
    ] {
        let loaded = file::from_toml_str(&format!("[window]\n{line}\n"));
        assert_eq!(loaded.status, LoadStatus::Recovered, "{line}");
        assert_eq!(loaded.settings.window, Window::default(), "{line}");
        assert!(
            loaded.warnings.iter().any(|w| w.starts_with(key)),
            "{line}: {:?}",
            loaded.warnings
        );
    }
    // The smallest sizes lazer allows are fine.
    let smallest = file::from_toml_str(
        "[window]\nfullscreen_size = \"320x240\"\nwindowed_size = \"640x480\"\n\
         windowed_position = [-0.5, 1]\n",
    );
    assert_eq!(smallest.status, LoadStatus::Ok, "{:?}", smallest.warnings);
}

#[test]
fn a_gameplay_binding_on_f11_is_skipped() {
    let loaded = file::from_toml_str(
        "[input]\nbindings = [{ input = \"F11\", action = \"left\" }, { input = \"X\", action = \"left\" }]\n",
    );
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert!(
        loaded
            .warnings
            .iter()
            .any(|w| w.starts_with("input.bindings[0]")),
        "{:?}",
        loaded.warnings
    );
    assert!(
        loaded
            .settings
            .input
            .bindings
            .as_slice()
            .iter()
            .all(|b| b.input != key("F11"))
    );
}

#[test]
fn language_reads_built_in_tags_and_falls_back_to_english() {
    let missing = file::from_toml_str("[audio]\nmaster_volume = 1.0\n");
    assert_eq!(missing.status, LoadStatus::Ok, "{:?}", missing.warnings);
    assert_eq!(missing.settings.general.language, None);
    for tag in ["en", "ru"] {
        let read = file::from_toml_str(&format!("[general]\nlanguage = \"{tag}\"\n"));
        assert_eq!(read.status, LoadStatus::Ok, "{:?}", read.warnings);
        assert_eq!(read.settings.general.language.as_deref(), Some(tag));
    }
    for bad in ["\"de\"", "\"\"", "1"] {
        let loaded = file::from_toml_str(&format!("[general]\nlanguage = {bad}\n"));
        assert_eq!(loaded.status, LoadStatus::Recovered, "{bad}");
        assert_eq!(loaded.settings.general.language, None, "{bad}");
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.starts_with("general.language")),
            "{bad}"
        );
    }
}

#[test]
fn intro_defaults_on_and_reads_a_bool() {
    assert!(Settings::default().general.intro);
    let missing = file::from_toml_str("[general]\nlanguage = \"en\"\n");
    assert_eq!(missing.status, LoadStatus::Ok, "{:?}", missing.warnings);
    assert!(missing.settings.general.intro);
    let off = file::from_toml_str("[general]\nintro = false\n");
    assert_eq!(off.status, LoadStatus::Ok, "{:?}", off.warnings);
    assert!(!off.settings.general.intro);
    let bad = file::from_toml_str("[general]\nintro = \"yes\"\n");
    assert_eq!(bad.status, LoadStatus::Recovered);
    assert!(bad.settings.general.intro);
    assert!(bad.warnings.iter().any(|w| w.starts_with("general.intro")));
    let mut settings = Settings::default();
    settings.general.intro = false;
    let text = file::to_toml_string(&settings).unwrap();
    assert_eq!(file::from_toml_str(&text).settings, settings);
}

#[test]
fn effects_defaults_on_and_reads_a_bool() {
    assert!(Settings::default().general.effects);
    let missing = file::from_toml_str("[general]\nlanguage = \"en\"\n");
    assert_eq!(missing.status, LoadStatus::Ok, "{:?}", missing.warnings);
    assert!(missing.settings.general.effects);
    let off = file::from_toml_str("[general]\neffects = false\n");
    assert_eq!(off.status, LoadStatus::Ok, "{:?}", off.warnings);
    assert!(!off.settings.general.effects);
    let bad = file::from_toml_str("[general]\neffects = \"yes\"\n");
    assert_eq!(bad.status, LoadStatus::Recovered);
    assert!(bad.settings.general.effects);
    assert!(
        bad.warnings
            .iter()
            .any(|w| w.starts_with("general.effects"))
    );
    let mut settings = Settings::default();
    settings.general.effects = false;
    let text = file::to_toml_string(&settings).unwrap();
    assert_eq!(file::from_toml_str(&text).settings, settings);
}

#[test]
fn language_survives_a_write_and_read() {
    let mut settings = Settings::default();
    settings.general.language = Some("ru".to_owned());
    let text = file::to_toml_string(&settings).unwrap();
    assert!(text.contains("[general]\nlanguage = \"ru\""), "{text}");
    assert_eq!(file::from_toml_str(&text).settings, settings);
}

fn combo(mods: Mods, name: &str) -> Combo {
    Combo {
        mods,
        input: key(name),
    }
}

#[test]
fn combo_names_round_trip() {
    let ctrl_o = combo(Mods::CTRL, "O");
    assert_eq!(keys::combo_name(ctrl_o), "Ctrl+O");
    assert_eq!(keys::combo_from_name("Ctrl+O"), Some(ctrl_o));

    let chord = combo(Mods::CTRL.with(Mods::SHIFT), "F5");
    assert_eq!(keys::combo_name(chord), "Ctrl+Shift+F5");
    assert_eq!(keys::combo_from_name("Ctrl+Shift+F5"), Some(chord));

    // Prefixes in any order and any case; written back in the canonical order.
    let parsed = keys::combo_from_name("alt+ctrl+x").unwrap();
    assert_eq!(parsed, combo(Mods::CTRL.with(Mods::ALT), "X"));
    assert_eq!(keys::combo_name(parsed), "Ctrl+Alt+X");

    // No modifiers is the plain input's name.
    assert_eq!(keys::combo_name(plain("F12")), "F12");
    assert_eq!(keys::combo_from_name("f12"), Some(plain("F12")));

    assert_eq!(keys::combo_from_name(""), None);
    assert_eq!(keys::combo_from_name("Ctrl+"), None);
    assert_eq!(keys::combo_from_name("Nope"), None);
    assert_eq!(keys::combo_from_name("Ctrl+Nope"), None);
}

#[test]
fn toggle_settings_defaults_to_ctrl_o_and_is_written() {
    let s = Settings::default();
    assert_eq!(s.hotkeys.toggle_settings, Some(combo(Mods::CTRL, "O")));
    assert_eq!(s.key(KeySlot::ToggleSettings), Some(combo(Mods::CTRL, "O")));
    let text = file::to_toml_string(&s).unwrap();
    assert!(text.contains("toggle_settings = \"Ctrl+O\""), "{text}");
    // A file without the entry keeps the default.
    let loaded = file::from_toml_str("[audio]\nmaster_volume = 0.5\n");
    assert_eq!(
        loaded.settings.hotkeys.toggle_settings,
        s.hotkeys.toggle_settings
    );

    let mut s = s;
    s.clear_key(KeySlot::ToggleSettings);
    s.reset_keys();
    assert_eq!(s.hotkeys.toggle_settings, Some(combo(Mods::CTRL, "O")));
}

#[test]
fn an_unbound_toggle_settings_survives_a_save_and_load() {
    let dir = temp_dir("unbound-toggle");
    let path = dir.join("slam.toml");
    let mut s = Settings::default();
    s.clear_key(KeySlot::ToggleSettings);
    let text = file::to_toml_string(&s).unwrap();
    assert!(text.contains("toggle_settings = \"\""), "{text}");
    file::save(&path, &s).unwrap();
    let loaded = file::load(&path);
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings.hotkeys.toggle_settings, None);

    let loaded = file::from_toml_str("[hotkeys]\ntoggle_settings = \"Ctrl+Shift+P\"\n");
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(
        loaded.settings.hotkeys.toggle_settings,
        Some(combo(Mods::CTRL.with(Mods::SHIFT), "P"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_hotkey_with_modifiers_does_not_conflict_with_the_gameplay_key() {
    let mut s = Settings::default();
    let fps = KeySlot::ToggleFps;
    // Z is the left key.
    let ctrl_z = combo(Mods::CTRL, "Z");
    assert_eq!(s.key_conflict(fps, ctrl_z), None);
    assert!(s.bind_key(fps, ctrl_z));
    assert_eq!(s.key(fps), Some(ctrl_z));
    assert_eq!(s.input.slot(Actions::LEFT, 0), Some(key("Z")));
    // A plain one does, and taking it unbinds the gameplay slot.
    assert_eq!(
        s.key_conflict(fps, plain("Z")),
        Some(KeySlot::Play(Actions::LEFT, 0))
    );
    // The gameplay key meets a plain hotkey only.
    assert!(s.bind_key(fps, plain("F12")));
    assert_eq!(
        s.key_conflict(KeySlot::Play(Actions::SMOKE, 1), plain("F12")),
        Some(fps)
    );
}

#[test]
fn binding_a_combo_clears_the_other_hotkey_with_it() {
    let mut s = Settings::default();
    let ctrl_o = combo(Mods::CTRL, "O");
    assert!(s.bind_key(KeySlot::ToggleFps, ctrl_o));
    assert_eq!(s.key(KeySlot::ToggleFps), Some(ctrl_o));
    assert_eq!(s.key(KeySlot::ToggleSettings), None);
    assert_eq!(s.hotkey(ctrl_o), Some(KeySlot::ToggleFps));
    // Different modifiers are a different combo.
    assert!(s.bind_key(
        KeySlot::ToggleSettings,
        combo(Mods::CTRL.with(Mods::SHIFT), "O")
    ));
    assert_eq!(s.key(KeySlot::ToggleFps), Some(ctrl_o));
}

#[test]
fn modifiers_let_a_hotkey_take_menu_keys_but_not_gameplay_slots() {
    let fps = KeySlot::ToggleFps;
    assert!(!Settings::key_allowed(fps, plain("Tab")));
    assert!(Settings::key_allowed(fps, combo(Mods::CTRL, "Tab")));
    let left = KeySlot::Play(Actions::LEFT, 0);
    assert!(!Settings::key_allowed(left, combo(Mods::CTRL, "Q")));
    assert!(Settings::key_allowed(left, plain("Q")));
    // F11 belongs to the window, with or without modifiers.
    assert!(!Settings::key_allowed(fps, plain("F11")));
    assert!(!Settings::key_allowed(fps, combo(Mods::ALT, "F11")));
    assert!(!Settings::key_allowed(left, plain("F11")));
}

#[test]
fn song_select_dim_defaults_and_round_trips() {
    assert_eq!(Settings::default().song_select.background_dim, 0.7);
    let loaded = file::from_toml_str("[song_select]\nbackground_dim = 0.25\n");
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings.song_select.background_dim, 0.25);
    let missing = file::from_toml_str("[gameplay]\ndim = 0.2\n");
    assert_eq!(missing.settings.song_select.background_dim, 0.7);
}

#[test]
fn song_select_dim_out_of_range_or_nan_falls_back() {
    for text in ["1.5", "-0.1", "nan"] {
        let loaded = file::from_toml_str(&format!("[song_select]\nbackground_dim = {text}\n"));
        assert_eq!(loaded.status, LoadStatus::Recovered, "{text}");
        assert_eq!(loaded.settings.song_select.background_dim, 0.7, "{text}");
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.starts_with("song_select.background_dim")),
            "{text}: {:?}",
            loaded.warnings
        );
    }
}

#[test]
fn the_interface_settings_default_and_read_by_name() {
    let d = Settings::default();
    assert_eq!(d.general.colour_scheme, ColourScheme::Pink);
    assert_eq!((d.general.menu_scale, d.general.game_scale), (1.0, 1.0));
    assert_eq!(d.general.menu_background, MenuBackground::Beatmap);
    assert_eq!(d.audio.menu_sounds, MenuSounds::Slam);
    let loaded = file::from_toml_str(
        "[general]\ncolour_scheme = \"blue\"\nmenu_scale = 1.25\ngame_scale = 0.8\n\
         menu_background = \"none\"\n[audio]\nmenu_sounds = \"skin\"\n",
    );
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    let g = &loaded.settings.general;
    assert_eq!(g.colour_scheme, ColourScheme::Blue);
    assert_eq!((g.menu_scale, g.game_scale), (1.25, 0.8));
    assert_eq!(g.menu_background, MenuBackground::None);
    assert_eq!(loaded.settings.audio.menu_sounds, MenuSounds::Skin);
}

#[test]
fn bad_interface_settings_fall_back_with_a_warning() {
    let loaded = file::from_toml_str(
        "[general]\ncolour_scheme = \"teal\"\nmenu_scale = 3.0\ngame_scale = \"big\"\n\
         menu_background = \"video\"\n[audio]\nmenu_sounds = \"loud\"\n",
    );
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.settings.general, Settings::default().general);
    assert_eq!(loaded.settings.audio.menu_sounds, MenuSounds::Slam);
    for key in [
        "general.colour_scheme",
        "general.menu_scale",
        "general.game_scale",
        "general.menu_background",
        "audio.menu_sounds",
    ] {
        assert!(
            loaded.warnings.iter().any(|w| w.starts_with(key)),
            "{key}: {:?}",
            loaded.warnings
        );
    }
}

#[test]
fn scales_snap_to_their_step_and_write_cleanly() {
    let loaded = file::from_toml_str("[general]\nmenu_scale = 1.07\n");
    assert_eq!(loaded.settings.general.menu_scale, 1.05);
    let mut settings = Settings::default();
    settings.general.colour_scheme = ColourScheme::Orange;
    settings.general.menu_scale = 1.05;
    settings.general.menu_background = MenuBackground::Skin;
    settings.audio.menu_sounds = MenuSounds::Off;
    let text = file::to_toml_string(&settings).unwrap();
    assert!(text.contains("colour_scheme = \"orange\""), "{text}");
    assert!(text.contains("menu_scale = 1.05\n"), "{text}");
    assert!(text.contains("menu_background = \"skin\""), "{text}");
    assert!(text.contains("menu_sounds = \"off\""), "{text}");
    assert_eq!(file::from_toml_str(&text).settings, settings);
}

#[test]
fn the_pause_menu_settings_default_and_read_by_name() {
    let d = Settings::default();
    assert_eq!(d.gameplay.pause_menu_style, PauseMenuStyle::Slam);
    assert_eq!(d.gameplay.pause_menu_scale, 1.0);
    let loaded =
        file::from_toml_str("[gameplay]\npause_menu_style = \"stable\"\npause_menu_scale = 1.07\n");
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(
        loaded.settings.gameplay.pause_menu_style,
        PauseMenuStyle::Stable
    );
    assert_eq!(loaded.settings.gameplay.pause_menu_scale, 1.05);
    for style in PauseMenuStyle::ALL {
        assert_eq!(PauseMenuStyle::from_name(style.name()), Some(style));
    }
    let mut settings = Settings::default();
    settings.gameplay.pause_menu_style = PauseMenuStyle::Lazer;
    settings.gameplay.pause_menu_scale = 0.75;
    let text = file::to_toml_string(&settings).unwrap();
    assert!(text.contains("pause_menu_style = \"lazer\""), "{text}");
    assert!(text.contains("pause_menu_scale = 0.75\n"), "{text}");
    assert_eq!(file::from_toml_str(&text).settings, settings);
}

#[test]
fn bad_pause_menu_settings_fall_back_with_a_warning() {
    let loaded =
        file::from_toml_str("[gameplay]\npause_menu_style = \"fancy\"\npause_menu_scale = 9.0\n");
    assert_eq!(loaded.status, LoadStatus::Recovered);
    assert_eq!(loaded.settings.gameplay, Settings::default().gameplay);
    for key in ["gameplay.pause_menu_style", "gameplay.pause_menu_scale"] {
        assert!(
            loaded.warnings.iter().any(|w| w.starts_with(key)),
            "{key}: {:?}",
            loaded.warnings
        );
    }
}

#[test]
fn transitions_default_to_simple_and_read_by_name() {
    assert_eq!(
        Settings::default().general.transitions,
        TransitionStyle::Simple
    );
    for style in TransitionStyle::ALL {
        let loaded =
            file::from_toml_str(&format!("[general]\ntransitions = \"{}\"\n", style.name()));
        assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
        assert_eq!(loaded.settings.general.transitions, style);
    }
    let bad = file::from_toml_str("[general]\ntransitions = \"wild\"\n");
    assert_eq!(bad.status, LoadStatus::Recovered);
    assert_eq!(bad.settings.general.transitions, TransitionStyle::Simple);
    assert!(
        bad.warnings
            .iter()
            .any(|w| w.starts_with("general.transitions"))
    );
}

#[test]
fn the_thumbnails_setting_defaults_to_128_and_round_trips() {
    use super::Thumbnails;
    assert_eq!(Settings::default().song_select.thumbnails, Thumbnails::H128);
    for (name, value, height) in [
        ("off", Thumbnails::Off, None),
        ("64", Thumbnails::H64, Some(64)),
        ("128", Thumbnails::H128, Some(128)),
        ("256", Thumbnails::H256, Some(256)),
    ] {
        let loaded = file::from_toml_str(&format!("[song_select]\nthumbnails = \"{name}\"\n"));
        assert_eq!(
            loaded.status,
            LoadStatus::Ok,
            "{name}: {:?}",
            loaded.warnings
        );
        assert_eq!(loaded.settings.song_select.thumbnails, value);
        assert_eq!(value.height(), height);
        let mut settings = Settings::default();
        settings.song_select.thumbnails = value;
        let text = file::to_toml_string(&settings).unwrap();
        assert!(text.contains(&format!("thumbnails = \"{name}\"")), "{text}");
        assert_eq!(file::from_toml_str(&text).settings, settings);
    }
}

#[test]
fn a_bad_thumbnails_value_falls_back_with_a_warning() {
    for text in ["\"huge\"", "128", "true"] {
        let loaded = file::from_toml_str(&format!("[song_select]\nthumbnails = {text}\n"));
        assert_eq!(loaded.status, LoadStatus::Recovered, "{text}");
        assert_eq!(
            loaded.settings.song_select.thumbnails,
            Settings::default().song_select.thumbnails
        );
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.starts_with("song_select.thumbnails")),
            "{text}: {:?}",
            loaded.warnings
        );
    }
}

#[test]
fn the_cursor_styles_default_read_by_name_and_round_trip() {
    let d = Settings::default();
    assert_eq!(d.general.menu_cursor, CursorStyle::Slam);
    assert_eq!(d.gameplay.cursor_style, CursorStyle::Skin);
    let loaded = file::from_toml_str(
        "[general]\nmenu_cursor = \"system\"\n[gameplay]\ncursor_style = \"slam\"\n",
    );
    assert_eq!(loaded.status, LoadStatus::Ok, "{:?}", loaded.warnings);
    assert_eq!(loaded.settings.general.menu_cursor, CursorStyle::System);
    assert_eq!(loaded.settings.gameplay.cursor_style, CursorStyle::Slam);
    let text = file::to_toml_string(&loaded.settings).unwrap();
    assert!(text.contains("menu_cursor = \"system\""), "{text}");
    assert!(text.contains("cursor_style = \"slam\""), "{text}");
    let bad =
        file::from_toml_str("[general]\nmenu_cursor = \"big\"\n[gameplay]\ncursor_style = 3\n");
    assert_eq!(bad.settings.general.menu_cursor, CursorStyle::Slam);
    assert_eq!(bad.settings.gameplay.cursor_style, CursorStyle::Skin);
    assert!(
        bad.warnings
            .iter()
            .any(|w| w.starts_with("general.menu_cursor"))
    );
}

#[test]
fn the_skew_defaults_snaps_to_degrees_and_falls_back() {
    assert_eq!(Settings::default().general.skew, 10.0);
    let loaded = file::from_toml_str("[general]\nskew = 7.4\n");
    assert_eq!(loaded.settings.general.skew, 7.0);
    let loaded = file::from_toml_str("[general]\nskew = 20.0\n");
    assert_eq!(loaded.settings.general.skew, 20.0);
    let loaded = file::from_toml_str("[general]\nskew = 45.0\n");
    assert_eq!(loaded.settings.general.skew, 10.0);
    assert!(
        loaded
            .warnings
            .iter()
            .any(|w| w.starts_with("general.skew"))
    );
    let mut settings = Settings::default();
    settings.general.skew = 0.0;
    let text = file::to_toml_string(&settings).unwrap();
    assert!(text.contains("skew = 0.0"), "{text}");
    assert_eq!(file::from_toml_str(&text).settings, settings);
}
