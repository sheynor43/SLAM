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
            for name in tex.expand(&skin.font_prefixes()) {
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
        for sample in element.samples.iter().filter(|s| s.in_default_skin) {
            if skin.sample(sample.name).is_none() {
                missing.push(sample.name.to_owned());
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
            let options = AnimationOptions {
                animatable: tex.animation.is_some(),
                separator: tex.animation.unwrap_or("-"),
                ..AnimationOptions::default()
            };
            assert!(
                skin.animation(tex.name, options).is_none(),
                "{} exists",
                tex.name
            );
        }
        for sample in element.samples.iter().filter(|s| !s.in_default_skin) {
            assert!(skin.sample(sample.name).is_none(), "{} exists", sample.name);
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
fn default_skin_has_no_unknown_files() {
    let skin = Skin::load_default(default_skin_dir()).unwrap();
    assert_eq!(skin.unknown_files(), Vec::<&str>::new());
}
