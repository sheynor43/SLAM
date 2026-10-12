//! The bundled classic skin provides what the registry says it does, through the loader.

use std::path::{Path, PathBuf};

use slam_formats::skin_ini::{DEFAULT_COMBO_COLOURS, Rgba, SkinVersion};
use slam_skin::{AnimationOptions, Skin, registry};

fn default_skin_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/default-skin")
}

#[test]
fn registry_elements_exist_in_default_skin() {
    let skin = Skin::load_default(default_skin_dir()).unwrap();
    let mut missing = Vec::new();
    for element in registry::ELEMENTS {
        for tex in element.textures.iter().filter(|t| t.in_default_skin) {
            for name in tex.expand(skin.hit_circle_prefix()) {
                let options = AnimationOptions {
                    animatable: tex.animation.is_some(),
                    separator: tex.animation.unwrap_or("-"),
                    ..AnimationOptions::default()
                };
                if skin.animation(&name, options).is_none() {
                    missing.push(name);
                }
            }
        }
        for sample in element.samples {
            if skin.sample(sample).is_none() {
                missing.push((*sample).to_owned());
            }
        }
    }
    assert!(
        missing.is_empty(),
        "missing from the default skin: {missing:?}"
    );
}

#[test]
fn registry_marks_absent_textures() {
    let skin = Skin::load_default(default_skin_dir()).unwrap();
    for element in registry::ELEMENTS {
        for tex in element.textures.iter().filter(|t| !t.in_default_skin) {
            assert!(skin.texture(tex.name).is_none(), "{} exists", tex.name);
        }
    }
}

#[test]
fn default_skin_configuration() {
    let skin = Skin::load_default(default_skin_dir()).unwrap();
    assert!(!skin.has_ini());
    assert_eq!(skin.version(), SkinVersion::LATEST);
    assert!(skin.is_latest_version());
    assert_eq!(skin.combo_colours(), DEFAULT_COMBO_COLOURS);
    assert_eq!(skin.slider_ball_colour(), Rgba::rgb(2, 170, 255));
    assert!(skin.allow_slider_ball_tint());
    assert!(skin.texture("hitcircle").is_some_and(|t| t.scale == 2.0));
    // The slider ball is `sliderb0..9` without a separator.
    let ball = skin
        .animation(
            "sliderb",
            AnimationOptions {
                animatable: true,
                separator: "",
                ..AnimationOptions::default()
            },
        )
        .unwrap();
    assert_eq!(ball.frames.len(), 10);
}

#[test]
fn default_skin_unknown_files_are_menu_assets() {
    let skin = Skin::load_default(default_skin_dir()).unwrap();
    let unknown = skin.unknown_files();
    assert!(unknown.contains(&"ranking-s@2x.png"));
    assert!(!unknown.contains(&"hitcircle@2x.png"));
    assert!(!unknown.contains(&"default-5@2x.png"));
    assert!(!unknown.contains(&"normal-hitnormal.wav"));
    assert!(!unknown.contains(&"combobreak.mp3"));
    assert!(!unknown.contains(&"sliderb3@2x.png"));
    assert!(!unknown.contains(&"skin.ini"));
}
