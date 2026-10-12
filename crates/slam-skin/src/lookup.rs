//! Texture, animation and sample lookup of one skin.

use std::path::PathBuf;

use crate::skin::Skin;

/// The frame length of legacy animations at 60 frames per second, in milliseconds.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs
pub const SIXTY_FRAME_TIME: f64 = 1000.0 / 60.0;

/// Extensions the texture loader tries after the name as given.
// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Textures/TextureLoaderStore.cs
const TEXTURE_EXTENSIONS: [&str; 2] = [".png", ".jpg"];

/// Extensions the sample store tries after the name as given: wav, mp3, then ogg like stable.
// Ported from osu-framework 2026.921.1: osu.Framework/Audio/Sample/SampleStore.cs
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/Skin.cs
const SAMPLE_EXTENSIONS: [&str; 3] = [".wav", ".mp3", ".ogg"];

/// A texture file found in a skin.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureFile {
    /// Path of the image file in the skin folder.
    pub path: PathBuf,
    /// Pixel density: 2.0 for an `@2x` file, 1.0 otherwise (lazer's `ScaleAdjust`).
    pub scale: f32,
}

/// How [`Skin::animation`] looks up frames.
///
/// There are no defaults in lazer; [`Default`] is a static, non-looping element with the
/// `-` separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationOptions<'a> {
    /// Look for frames `{name}{separator}{n}` before the static texture.
    pub animatable: bool,
    /// Whether the animation loops (kept in the result for the caller).
    pub looping: bool,
    /// Take the frame length from `AnimationFramerate` (`applyConfigFrameRate`).
    pub apply_config_frame_rate: bool,
    /// Text between the name and the frame index: `"-"` normally, `""` for `sliderb`.
    pub separator: &'a str,
}

impl Default for AnimationOptions<'_> {
    fn default() -> Self {
        Self {
            animatable: false,
            looping: false,
            apply_config_frame_rate: false,
            separator: "-",
        }
    }
}

/// The frames of an element: several for an animation, one for a static texture.
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    /// Frames in order.
    pub frames: Vec<TextureFile>,
    /// Time each frame is shown, in milliseconds. Only meaningful for two or more frames
    /// (lazer returns a plain sprite for one frame); [`SIXTY_FRAME_TIME`] then.
    pub frame_length_ms: f64,
    /// Copy of [`AnimationOptions::looping`].
    pub looping: bool,
}

impl Animation {
    /// Whether this is a single static texture.
    pub fn is_static(&self) -> bool {
        self.frames.len() == 1
    }
}

/// `Path.ChangeExtension(name, null)` + `"@2x"` + `Path.GetExtension(name)`.
fn two_times_name(name: &str) -> String {
    let file_start = name.rfind(['/', '\\']).map_or(0, |i| i + 1);
    match name[file_start..].rfind('.') {
        Some(dot) => {
            let dot = file_start + dot;
            let ext = &name[dot..];
            // `GetExtension` is empty for a trailing dot, `ChangeExtension` still drops it.
            let ext = if ext.len() > 1 { ext } else { "" };
            format!("{}@2x{ext}", &name[..dot])
        }
        None => format!("{name}@2x"),
    }
}

impl Skin {
    /// Looks up a texture like `LegacySkin.GetTexture`: `name@2x` first, then `name`, each
    /// tried as given and with `.png` and `.jpg` appended. `@2x` inside `name` is removed
    /// first, like stable does.
    ///
    /// Zero-length files are not indexed. Images are not decoded here: a file that fails to
    /// decode must be treated by the consumer as missing (lazer then continues with the 1x
    /// file and the next skin). [`crate::SkinChain`] cannot fall through for that case yet.
    pub fn texture(&self, name: &str) -> Option<TextureFile> {
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkin.cs
        let name = name.replace("@2x", "");
        if name.is_empty() {
            return None;
        }
        let high = two_times_name(&name);
        for (candidate, scale) in [(&high, 2.0), (&name, 1.0)] {
            if let Some(path) = self.find_with_extensions(candidate, &TEXTURE_EXTENSIONS) {
                return Some(TextureFile {
                    path: path.to_path_buf(),
                    scale,
                });
            }
        }
        None
    }

    /// Looks up an element's frames like `LegacySkinExtensions.GetAnimation`.
    ///
    /// With `animatable`, frames `{name}{separator}0`, `...1`, ... are collected until the
    /// first missing index; without any frame, or when not animatable, the static texture
    /// `name` is the only frame. `None` when the skin has neither.
    pub fn animation(&self, name: &str, options: AnimationOptions<'_>) -> Option<Animation> {
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs
        let mut frames = Vec::new();
        if options.animatable {
            while let Some(frame) =
                self.texture(&format!("{name}{}{}", options.separator, frames.len()))
            {
                frames.push(frame);
            }
        }
        if frames.is_empty() {
            frames.push(self.texture(name)?);
        }
        let frame_length_ms = if frames.len() > 1 {
            self.frame_length(options.apply_config_frame_rate, frames.len())
        } else {
            SIXTY_FRAME_TIME
        };
        Some(Animation {
            frames,
            frame_length_ms,
            looping: options.looping,
        })
    }

    /// Whether [`Skin::animation`] with these options would find something. Used to pick the
    /// provider of an element (lazer's `FindProvider` in `GetTextures`).
    pub fn provides_animation(&self, name: &str, options: AnimationOptions<'_>) -> bool {
        (options.animatable
            && self
                .texture(&format!("{name}{}0", options.separator))
                .is_some())
            || self.texture(name).is_some()
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs
    fn frame_length(&self, apply_config_frame_rate: bool, frame_count: usize) -> f64 {
        if apply_config_frame_rate {
            // `1000f / rate` is a single-precision division in lazer.
            return match self.animation_framerate() {
                Some(rate) if rate > 0 => f64::from(1000f32 / rate as f32),
                _ => f64::from(1000f32 / frame_count as f32),
            };
        }
        SIXTY_FRAME_TIME
    }

    /// Looks up a sample file like `LegacySkin.GetSample`: the name as given, then its last
    /// `/`-separated segment (`Gameplay/normal-hitnormal` -> `normal-hitnormal`), each tried
    /// as given and with `.wav`, `.mp3` and `.ogg` appended.
    ///
    /// Zero-length files are not indexed. A file that fails to decode must be treated by the
    /// consumer as missing; [`crate::SkinChain`] cannot fall through for that case yet.
    ///
    /// Mapping hit samples to names (banks, custom sets) is the caller's job.
    pub fn sample(&self, name: &str) -> Option<PathBuf> {
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkin.cs
        let last = name.rsplit('/').next().unwrap_or(name);
        [name, last]
            .into_iter()
            .find_map(|n| self.find_with_extensions(n, &SAMPLE_EXTENSIONS))
            .map(PathBuf::from)
    }

    /// Tries `name` as given, then with each extension appended
    /// (`ResourceStore.GetFilenames`).
    // Ported from osu-framework 2026.921.1: osu.Framework/IO/Stores/ResourceStore.cs
    fn find_with_extensions(&self, name: &str, extensions: &[&str]) -> Option<&std::path::Path> {
        if let Some(p) = self.file(name) {
            return Some(p);
        }
        extensions
            .iter()
            .find_map(|ext| self.file(&format!("{name}{ext}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_times_names() {
        assert_eq!(two_times_name("hitcircle"), "hitcircle@2x");
        assert_eq!(two_times_name("a.png"), "a@2x.png");
        assert_eq!(two_times_name("dir/a.b.jpg"), "dir/a.b@2x.jpg");
        assert_eq!(two_times_name("dir.x/a"), "dir.x/a@2x");
        assert_eq!(two_times_name("a."), "a@2x");
    }
}
