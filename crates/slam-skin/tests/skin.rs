//! Lookup behaviour of `Skin` and `SkinChain` on skins built in a temporary directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use slam_formats::skin_ini::{Rgba, SkinVersion};
use slam_skin::{AnimationOptions, Skin, SkinChain, SkinSource, registry};

/// A temporary skin folder, removed on drop.
struct TempSkin(PathBuf);

impl TempSkin {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "slam-skin-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn with(self, files: &[&str]) -> Self {
        for f in files {
            self.write(f, b"x");
        }
        self
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn ini(self, text: &str) -> Self {
        self.write("skin.ini", text.as_bytes());
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

fn file_name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

fn anim_opts(sep: &str) -> AnimationOptions<'_> {
    AnimationOptions {
        animatable: true,
        looping: true,
        apply_config_frame_rate: false,
        separator: sep,
    }
}

#[test]
fn not_a_directory() {
    let dir = TempSkin::new().with(&["a.png"]);
    assert!(Skin::load(dir.0.join("a.png")).is_err());
    assert!(Skin::load(dir.0.join("missing")).is_err());
}

#[test]
fn missing_ini_means_latest() {
    let dir = TempSkin::new().with(&["hitcircle.png"]);
    let skin = dir.load();
    assert!(!skin.has_ini());
    assert_eq!(skin.version(), SkinVersion::LATEST);
    assert!(skin.is_latest_version());
}

#[test]
fn ini_is_found_case_insensitively() {
    let dir = TempSkin::new().ini("[General]\nVersion: 1.5\nName: x\n");
    fs::rename(dir.0.join("skin.ini"), dir.0.join("Skin.INI")).unwrap();
    let skin = dir.load();
    assert!(skin.has_ini());
    assert_eq!(skin.version(), SkinVersion::new(15, 1));
    assert!(!skin.is_latest_version());
}

#[test]
fn ini_without_version_is_v1_and_warnings_are_kept() {
    let dir = TempSkin::new().ini("[Colours]\nCombo1: 1,2\n");
    let skin = dir.load();
    assert_eq!(skin.version(), SkinVersion::V1);
    assert_eq!(skin.warnings().len(), 1);
}

#[test]
fn prefers_2x_and_reports_scale() {
    let dir = TempSkin::new().with(&["hitcircle.png", "hitcircle@2x.png", "cursor.png"]);
    let skin = dir.load();
    let hc = skin.texture("hitcircle").unwrap();
    assert_eq!(
        (file_name(&hc.path).as_str(), hc.scale),
        ("hitcircle@2x.png", 2.0)
    );
    let cursor = skin.texture("cursor").unwrap();
    assert_eq!(
        (file_name(&cursor.path).as_str(), cursor.scale),
        ("cursor.png", 1.0)
    );
    assert!(skin.texture("nothing").is_none());
    assert!(skin.texture("").is_none());
}

#[test]
fn texture_2x_wins_over_1x_even_with_other_extension() {
    let dir = TempSkin::new().with(&["a.png", "a@2x.jpg"]);
    let t = dir.load().texture("a").unwrap();
    assert_eq!((file_name(&t.path).as_str(), t.scale), ("a@2x.jpg", 2.0));
}

#[test]
fn png_before_jpg_and_jpg_fallback() {
    let dir = TempSkin::new().with(&["both.png", "both.jpg", "only.jpg"]);
    let skin = dir.load();
    assert_eq!(file_name(&skin.texture("both").unwrap().path), "both.png");
    assert_eq!(file_name(&skin.texture("only").unwrap().path), "only.jpg");
}

#[test]
fn name_with_extension_and_2x_in_name() {
    let dir = TempSkin::new().with(&["a.png", "a@2x.png"]);
    let skin = dir.load();
    // An explicit extension is kept, and `@2x` inside the name is stripped like stable does.
    assert_eq!(skin.texture("a.png").unwrap().scale, 2.0);
    assert_eq!(skin.texture("a@2x").unwrap().scale, 2.0);
    assert_eq!(skin.texture("a@2x.png").unwrap().scale, 2.0);
}

#[test]
fn case_insensitive_names_and_subfolders() {
    let dir = TempSkin::new().with(&["HitCircle.PNG", "Fonts/Default-0@2x.png"]);
    let skin = dir.load();
    assert!(skin.texture("hitcircle").is_some());
    assert!(skin.texture("HITCIRCLE").is_some());
    // Backslash prefixes from skin.ini resolve to subfolders.
    let t = skin.texture("fonts\\default-0").unwrap();
    assert_eq!(t.scale, 2.0);
    assert!(skin.texture("fonts/default-0").is_some());
    assert!(skin.file("FONTS\\DEFAULT-0@2X.PNG").is_some());
}

#[test]
fn font_prefix_with_folder() {
    let dir = TempSkin::new()
        .ini("[Fonts]\nHitCirclePrefix: fonts\\default\n")
        .with(&["fonts/default-1.png"]);
    let skin = dir.load();
    assert_eq!(skin.hit_circle_prefix(), "fonts\\default");
    let name = format!("{}-1", skin.hit_circle_prefix());
    assert!(skin.texture(&name).is_some());
    // Pattern files of the prefix folder are not reported as unknown.
    assert!(
        skin.unknown_files().is_empty(),
        "{:?}",
        skin.unknown_files()
    );
}

#[test]
fn animation_collects_frames_until_first_gap() {
    let dir = TempSkin::new().with(&[
        "hit300-0.png",
        "hit300-1@2x.png",
        "hit300-2.png",
        "hit300-4.png",
        "hit300.png",
    ]);
    let anim = dir.load().animation("hit300", anim_opts("-")).unwrap();
    assert_eq!(anim.frames.len(), 3);
    assert_eq!(file_name(&anim.frames[1].path), "hit300-1@2x.png");
    assert!(anim.looping);
    assert!(!anim.is_static());
}

#[test]
fn animation_falls_back_to_static() {
    let dir = TempSkin::new().with(&["hit300.png", "other-1.png"]);
    let skin = dir.load();
    let a = skin.animation("hit300", anim_opts("-")).unwrap();
    assert!(a.is_static());
    // Frames must start at index 0.
    assert!(skin.animation("other", anim_opts("-")).is_none());
    // Not animatable: frames are ignored.
    let dir = TempSkin::new().with(&["x-0.png", "x-1.png", "x.png"]);
    let a = dir
        .load()
        .animation("x", AnimationOptions::default())
        .unwrap();
    assert_eq!(file_name(&a.frames[0].path), "x.png");
    assert!(a.is_static());
    let dir = TempSkin::new().with(&["x-0.png"]);
    assert!(
        dir.load()
            .animation("x", AnimationOptions::default())
            .is_none()
    );
}

#[test]
fn slider_ball_has_no_separator() {
    let dir = TempSkin::new().with(&[
        "sliderb0.png",
        "sliderb1.png",
        "sliderb2@2x.png",
        "sliderb-nd.png",
    ]);
    let skin = dir.load();
    assert_eq!(
        skin.animation("sliderb", anim_opts(""))
            .unwrap()
            .frames
            .len(),
        3
    );
    // With the default separator there are no frames and no static `sliderb`.
    assert!(skin.animation("sliderb", anim_opts("-")).is_none());
}

#[test]
fn frame_length_rules() {
    let dir = TempSkin::new()
        .with(&["a-0.png", "a-1.png", "a-2.png", "a-3.png"])
        .ini("[General]\nAnimationFramerate: 24\n");
    let skin = dir.load();
    let sixty = 1000.0 / 60.0;
    // Not applying the config rate: always 60 fps.
    assert_eq!(
        skin.animation("a", anim_opts("-")).unwrap().frame_length_ms,
        sixty
    );
    let apply = AnimationOptions {
        apply_config_frame_rate: true,
        ..anim_opts("-")
    };
    // `1000f / 24` in single precision.
    assert_eq!(
        skin.animation("a", apply).unwrap().frame_length_ms,
        f64::from(1000f32 / 24.0)
    );
    // No framerate, or one that is not positive: spread over the frame count.
    for ini in [
        "",
        "[General]\nAnimationFramerate: 0\n",
        "[General]\nAnimationFramerate: -5\n",
    ] {
        let dir = TempSkin::new()
            .with(&["a-0.png", "a-1.png", "a-2.png", "a-3.png"])
            .ini(ini);
        assert_eq!(
            dir.load().animation("a", apply).unwrap().frame_length_ms,
            f64::from(1000f32 / 4.0),
            "{ini:?}"
        );
    }
}

#[test]
fn sample_extension_order_and_fallback_names() {
    let dir = TempSkin::new().with(&[
        "a.ogg",
        "a.mp3",
        "b.mp3",
        "b.ogg",
        "c.ogg",
        "d.wav",
        "d.mp3",
        "exact.WAV",
        "Gameplay/e.wav",
        "f.wav",
    ]);
    let skin = dir.load();
    assert_eq!(file_name(&skin.sample("a").unwrap()), "a.mp3");
    assert_eq!(file_name(&skin.sample("b").unwrap()), "b.mp3");
    assert_eq!(file_name(&skin.sample("c").unwrap()), "c.ogg");
    assert_eq!(file_name(&skin.sample("d").unwrap()), "d.wav");
    assert_eq!(file_name(&skin.sample("EXACT").unwrap()), "exact.WAV");
    assert_eq!(file_name(&skin.sample("d.mp3").unwrap()), "d.mp3");
    assert!(skin.sample("zzz").is_none());
    // The full name wins, the last segment is the fallback.
    assert!(skin.sample("Gameplay/e").is_some());
    assert_eq!(file_name(&skin.sample("Gameplay/f").unwrap()), "f.wav");
    assert_eq!(file_name(&skin.sample("Gameplay/e").unwrap()), "e.wav");
}

#[test]
fn config_defaults_and_values() {
    let skin = TempSkin::new().load();
    assert_eq!(skin.hit_circle_prefix(), "default");
    assert_eq!(skin.hit_circle_overlap(), -2.0);
    assert_eq!(skin.score_prefix(), "score");
    assert_eq!(skin.combo_prefix(), "score");
    assert_eq!(skin.score_overlap(), 0.0);
    assert_eq!(skin.animation_framerate(), None);
    assert!(skin.cursor_rotate() && skin.cursor_centre() && skin.cursor_expand());
    assert!(skin.cursor_trail_rotate() && skin.hit_circle_overlay_above_number());
    assert!(skin.spinner_frequency_modulate() && skin.layered_hit_sounds());
    assert!(!skin.allow_slider_ball_tint() && !skin.spinner_no_blink());
    assert_eq!(skin.slider_ball_flip(), None);
    assert_eq!(skin.slider_border(), None);
    assert_eq!(skin.slider_track_override(), None);
    assert_eq!(skin.slider_ball_colour(), Rgba::rgb(255, 255, 255));
    assert_eq!(skin.combo_colours().len(), 4);

    let skin = TempSkin::new()
        .ini(
            "[General]\nCursorRotate: 0\nCursorExpand: false\nCursorCentre: 2\nLayeredHitSounds: 0\n\
             AllowSliderBallTint: TRUE\nSliderBallFlip: 1\nSpinnerNoBlink: 1\nCursorTrailRotate: abc\n\
             HitCircleOverlayAboveNumer: 0\nAnimationFramerate: 30\n\
             [Fonts]\nHitCircleOverlap: 4.5\nScorePrefix: sc\n\
             [Colours]\nSliderBorder: 1,2,3\nSliderTrackOverride: 4,5,6\nCombo1: 9,9,9\n",
        )
        .load();
    assert!(!skin.cursor_rotate());
    assert!(!skin.cursor_expand());
    assert!(skin.cursor_centre());
    assert!(!skin.layered_hit_sounds());
    assert!(skin.allow_slider_ball_tint());
    assert_eq!(skin.slider_ball_flip(), Some(true));
    assert!(skin.spinner_no_blink());
    // Unparsable booleans fall back to the default.
    assert!(skin.cursor_trail_rotate());
    // The typo'd key is honoured when the correct one is absent.
    assert!(!skin.hit_circle_overlay_above_number());
    assert_eq!(skin.animation_framerate(), Some(30));
    assert_eq!(skin.hit_circle_overlap(), 4.5);
    assert_eq!(skin.score_prefix(), "sc");
    assert_eq!(skin.slider_border(), Some(Rgba::rgb(1, 2, 3)));
    assert_eq!(skin.slider_track_override(), Some(Rgba::rgb(4, 5, 6)));
    assert_eq!(skin.combo_colours(), [Rgba::rgb(9, 9, 9)]);
}

#[test]
fn bool_conversion_matches_lazer() {
    let skin = TempSkin::new()
        .ini("[General]\nA: True\nB: false\nC: 2\nD: -1\nE: 0\nF: yes\nG: 1.0\nH:\n")
        .load();
    assert_eq!(skin.setting_bool("A"), Some(true));
    assert_eq!(skin.setting_bool("B"), Some(false));
    assert_eq!(skin.setting_bool("C"), Some(true));
    assert_eq!(skin.setting_bool("D"), Some(true));
    assert_eq!(skin.setting_bool("E"), Some(false));
    assert_eq!(skin.setting_bool("F"), None);
    assert_eq!(skin.setting_bool("G"), None);
    assert_eq!(skin.setting_bool("H"), None);
    assert_eq!(skin.setting_bool("missing"), None);
}

#[test]
fn unknown_files_listing() {
    let dir = TempSkin::new().ini("[General]\n").with(&[
        "hitcircle@2x.png",
        "default-3.png",
        "hit300-2.png",
        "sliderb4.jpg",
        "normal-hitnormal.wav",
        "normal-hitnormal2.wav",
        "spinnerspin.ogg",
        "menu-background.jpg",
        "ranking-S.png",
        "readme.txt",
        "sub/extra.png",
    ]);
    let skin = dir.load();
    assert_eq!(
        skin.unknown_files(),
        [
            "menu-background.jpg",
            "ranking-s.png",
            "readme.txt",
            "sub/extra.png"
        ]
    );
}

#[test]
fn chain_falls_back_per_texture_and_sample() {
    let user = TempSkin::new().with(&["hitcircle.png", "normal-hitnormal.wav"]);
    let default = TempSkin::new().with(&[
        "hitcircle@2x.png",
        "hitcircleoverlay@2x.png",
        "normal-hitnormal.wav",
        "normal-hitwhistle.wav",
    ]);
    let chain = SkinChain::new(Some(user.load()), default.load());

    let hc = chain.texture("hitcircle").unwrap();
    assert_eq!(hc.source, SkinSource::User);
    assert_eq!(hc.value.scale, 1.0);
    let overlay = chain.texture("hitcircleoverlay").unwrap();
    assert_eq!(overlay.source, SkinSource::Default);
    assert!(chain.texture("zzz").is_none());

    assert_eq!(
        chain.sample("normal-hitnormal").unwrap().source,
        SkinSource::User
    );
    assert_eq!(
        chain.sample("normal-hitwhistle").unwrap().source,
        SkinSource::Default
    );

    let no_user = SkinChain::new(None, default.load());
    assert_eq!(
        no_user.texture("hitcircle").unwrap().source,
        SkinSource::Default
    );
    assert_eq!(no_user.skin(SkinSource::User).root(), default.0);
}

#[test]
fn chain_animation_comes_from_one_provider() {
    // The user skin has frame 0 only; the default has three frames. All frames come from the user.
    let user = TempSkin::new().with(&["hit300-0.png"]);
    let default = TempSkin::new().with(&["hit300-0.png", "hit300-1.png", "hit300-2.png"]);
    let chain = SkinChain::new(Some(user.load()), default.load());
    let a = chain.animation("hit300", anim_opts("-")).unwrap();
    assert_eq!(a.source, SkinSource::User);
    assert_eq!(a.value.frames.len(), 1);

    // A user skin with only the static texture also wins over a default animation.
    let user = TempSkin::new().with(&["hit300.png"]);
    let chain = SkinChain::new(Some(user.load()), default.load());
    let a = chain.animation("hit300", anim_opts("-")).unwrap();
    assert_eq!(a.source, SkinSource::User);
    assert!(a.value.is_static());

    // Nothing in the user skin: the default is used.
    let user = TempSkin::new().with(&["unrelated.png"]);
    let chain = SkinChain::new(Some(user.load()), default.load());
    let a = chain.animation("hit300", anim_opts("-")).unwrap();
    assert_eq!(a.source, SkinSource::Default);
    assert_eq!(a.value.frames.len(), 3);
    assert!(chain.animation("nope", anim_opts("-")).is_none());
}

#[test]
fn chain_config_and_provider() {
    let user = TempSkin::new()
        .ini("[Fonts]\nHitCirclePrefix: u\n")
        .with(&["hitcircle.png"]);
    let default = TempSkin::new().ini("[Fonts]\nScorePrefix: d\nHitCirclePrefix: x\n");
    let chain = SkinChain::new(Some(user.load()), default.load());
    assert_eq!(
        chain.find_provider(|s| s.texture("hitcircle").is_some()),
        Some(SkinSource::User)
    );
    let f = chain
        .config(|s| s.setting("ScorePrefix").map(str::to_owned))
        .unwrap();
    assert_eq!((f.source, f.value.as_str()), (SkinSource::Default, "d"));
    let f = chain
        .config(|s| s.setting("HitCirclePrefix").map(str::to_owned))
        .unwrap();
    assert_eq!((f.source, f.value.as_str()), (SkinSource::User, "u"));
}

#[test]
fn registry_basics() {
    assert!(registry::element("hitcircle").is_some());
    assert!(registry::element("nope").is_none());
    let mut ids: Vec<_> = registry::ELEMENTS.iter().map(|e| e.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), registry::ELEMENTS.len(), "ids are unique");
    let numbers = registry::element("combo-numbers").unwrap().textures[0];
    assert_eq!(numbers.expand("fonts\\hc")[3], "fonts/hc-3");
}

#[test]
fn mixed_density_frames_keep_per_frame_scale() {
    let dir = TempSkin::new().with(&["a-0@2x.png", "a-1.png", "a-2@2x.png"]);
    let a = dir.load().animation("a", anim_opts("-")).unwrap();
    let scales: Vec<f32> = a.frames.iter().map(|f| f.scale).collect();
    assert_eq!(scales, [2.0, 1.0, 2.0]);
}

#[test]
fn zero_length_files_are_skipped() {
    let dir = TempSkin::new().with(&["hitcircle.png", "normal-hitnormal.mp3"]);
    dir.write("hitcircle@2x.png", b"");
    dir.write("normal-hitnormal.wav", b"");
    dir.write("only-empty.png", b"");
    let skin = dir.load();
    // The empty 2x file is ignored, so lookup continues with the 1x file.
    let t = skin.texture("hitcircle").unwrap();
    assert_eq!(
        (file_name(&t.path).as_str(), t.scale),
        ("hitcircle.png", 1.0)
    );
    assert_eq!(
        file_name(&skin.sample("normal-hitnormal").unwrap()),
        "normal-hitnormal.mp3"
    );
    assert!(skin.texture("only-empty").is_none());
    // Zero-length files fall through to the default skin in a chain.
    let default = TempSkin::new().with(&["only-empty.png"]);
    let chain = SkinChain::new(Some(skin), default.load());
    assert_eq!(
        chain.texture("only-empty").unwrap().source,
        SkinSource::Default
    );
}

#[cfg(unix)]
#[test]
fn unreadable_entries_are_treated_as_absent() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempSkin::new().with(&["a.png", "locked/b.png"]);
    dir.write("skin.ini", b"[General]\nVersion: 1.5\n");
    let set = |p: &str, mode| {
        fs::set_permissions(dir.0.join(p), fs::Permissions::from_mode(mode)).unwrap()
    };
    set("locked", 0o000);
    set("skin.ini", 0o000);
    let readable =
        fs::read_dir(dir.0.join("locked")).is_ok() || fs::read(dir.0.join("skin.ini")).is_ok();
    let loaded = Skin::load(&dir.0);
    set("locked", 0o755);
    set("skin.ini", 0o644);
    if readable {
        return; // running as a user that ignores permissions
    }
    let skin = loaded.unwrap();
    assert!(skin.texture("a").is_some());
    assert!(skin.texture("b").is_none());
    assert!(!skin.has_ini());
    assert_eq!(skin.version(), SkinVersion::LATEST);
}

#[test]
fn extensionless_exact_names_are_known() {
    let dir = TempSkin::new().with(&["hitcircle", "normal-hitnormal", "readme"]);
    assert_eq!(dir.load().unknown_files(), ["readme"]);
}

#[test]
fn texture_name_that_is_only_2x_is_empty() {
    let dir = TempSkin::new().with(&[".png"]);
    assert!(dir.load().texture("@2x").is_none());
}
