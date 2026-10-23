//! One skin folder: file index, `skin.ini` and configuration lookups.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use slam_formats::osu::parsing;
use slam_formats::skin_ini::{self, DEFAULT_COMBO_COLOURS, Rgba, SkinIni, SkinVersion, Warning};
use thiserror::Error;

use crate::registry;

/// A skin could not be loaded.
#[derive(Debug, Error)]
pub enum LoadError {
    /// The skin path is not a directory.
    #[error("{0} is not a directory")]
    NotADirectory(PathBuf),
    /// A directory or file of the skin could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

/// A loaded skin folder.
///
/// Files are indexed by their lowercased relative path with `/` separators, so lookups are
/// case-insensitive and accept both kinds of slash, like lazer's `RealmBackedResourceStore`.
/// The skin stores only paths; files are read by whoever consumes the lookup results.
#[derive(Debug, Clone)]
pub struct Skin {
    root: PathBuf,
    files: HashMap<String, PathBuf>,
    ini: SkinIni,
    has_ini: bool,
    warnings: Vec<Warning>,
    /// Loaded by [`Skin::load_beatmap`]: lookups follow `LegacyBeatmapSkin`.
    beatmap: bool,
}

/// `ToStandardisedPath().ToLowerInvariant()`.
fn index_key(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> LoadError {
    let path = path.to_path_buf();
    move |source| LoadError::Io { path, source }
}

/// Collects all files below `dir` into `files`; later entries of the sorted walk replace
/// earlier ones that differ only by case.
fn index_dir(
    root: &Path,
    dir: &Path,
    files: &mut HashMap<String, PathBuf>,
) -> Result<(), LoadError> {
    let mut entries = fs::read_dir(dir)
        .map_err(io_error(dir))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_error(dir))?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            tracing::warn!(path = %path.display(), "skipping unreadable skin entry");
            continue;
        };
        if file_type.is_dir() {
            // An unreadable subfolder is treated as absent, like missing files in lazer.
            if let Err(e) = index_dir(root, &path, files) {
                tracing::warn!("skipping unreadable skin folder: {e}");
            }
            continue;
        }
        // Symlinks are followed for files only, so a link cannot make the walk loop.
        let is_file = file_type.is_file() || (file_type.is_symlink() && path.is_file());
        // Zero-length files cannot be valid images or samples; lazer's loaders return null
        // for them and the lookup continues, so they are left out of the index.
        let non_empty = is_file && fs::metadata(&path).is_ok_and(|m| m.len() > 0);
        if non_empty && let Ok(rel) = path.strip_prefix(root) {
            files.insert(index_key(&rel.to_string_lossy()), path);
        }
    }
    Ok(())
}

/// The configuration of a skin without a readable `skin.ini`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/Skin.cs
fn missing_ini() -> SkinIni {
    SkinIni {
        version: SkinVersion::LATEST,
        is_latest: true,
        ..SkinIni::default()
    }
}

impl Skin {
    /// Loads the skin in `dir`: indexes all its files and parses `skin.ini`.
    ///
    /// A missing or unreadable `skin.ini` means the latest version, like lazer's `Skin` constructor. Parse
    /// warnings are logged and kept in [`Skin::warnings`].
    pub fn load(dir: impl AsRef<Path>) -> Result<Self, LoadError> {
        Self::load_with(dir.as_ref(), None)
    }

    /// Loads a beatmap folder as the beatmap's skin (`LegacyBeatmapSkin`): all files of the
    /// folder are indexed and the configuration is read from the `.osu` file's bytes, which
    /// lazer decodes as a `skin.ini`.
    ///
    /// Textures are looked up without `@2x` sprites ([`Skin::texture`]) and the configuration has
    /// no version ([`Skin::config_version`]), as in stable and lazer.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyBeatmapSkin.cs
    pub fn load_beatmap(dir: impl AsRef<Path>, osu: &[u8]) -> Result<Self, LoadError> {
        Self::load_with(dir.as_ref(), Some(osu))
    }

    /// Indexes `root` and reads the configuration from `config`, or from the folder's
    /// `skin.ini` when it is `None`.
    fn load_with(root: &Path, config: Option<&[u8]>) -> Result<Self, LoadError> {
        if !root.is_dir() {
            return Err(LoadError::NotADirectory(root.to_path_buf()));
        }
        let mut files = HashMap::new();
        index_dir(root, root, &mut files)?;

        let (ini, has_ini, warnings) = match config {
            Some(bytes) => {
                let decoded = skin_ini::decode(bytes);
                (decoded.skin, true, decoded.warnings)
            }
            None => match files.get("skin.ini") {
                Some(path) => match fs::read(path) {
                    Ok(bytes) => {
                        let decoded = skin_ini::decode(&bytes);
                        (decoded.skin, true, decoded.warnings)
                    }
                    Err(e) => {
                        tracing::warn!(path = %path.display(), "cannot read skin.ini: {e}");
                        (missing_ini(), false, Vec::new())
                    }
                },
                None => (missing_ini(), false, Vec::new()),
            },
        };
        // A beatmap's own lines are not skin settings; only real skin.ini warnings are logged.
        if config.is_none() {
            for w in &warnings {
                tracing::warn!(skin = %root.display(), "skin.ini: {w}");
            }
        }
        let skin = Self {
            root: root.to_path_buf(),
            files,
            ini,
            has_ini,
            warnings,
            beatmap: config.is_some(),
        };
        tracing::debug!(
            skin = %root.display(),
            files = skin.files.len(),
            unknown = skin.unknown_files().len(),
            "skin loaded"
        );
        Ok(skin)
    }

    /// Loads the bundled classic skin and applies the configuration lazer sets in code.
    ///
    /// The osu-resources skin has no `skin.ini`; `DefaultLegacySkin` supplies the slider ball
    /// colour, the combo colours, slider ball tinting and the latest version.
    pub fn load_default(dir: impl AsRef<Path>) -> Result<Self, LoadError> {
        let mut skin = Self::load(dir)?;
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/DefaultLegacySkin.cs
        skin.ini
            .set_custom_colour("SliderBall", Rgba::rgb(2, 170, 255));
        skin.ini.combo_colours = DEFAULT_COMBO_COLOURS.to_vec();
        skin.ini.settings.set("AllowSliderBallTint", "true");
        skin.ini.version = SkinVersion::LATEST;
        skin.ini.is_latest = true;
        Ok(skin)
    }

    /// The skin folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The parsed `skin.ini` (defaults when the file is missing).
    pub fn ini(&self) -> &SkinIni {
        &self.ini
    }

    /// Whether a configuration was read: the folder's `skin.ini`, or the `.osu` file of a
    /// beatmap skin.
    pub fn has_ini(&self) -> bool {
        self.has_ini
    }

    /// Problems found while parsing `skin.ini`.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Number of indexed files.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// The file at a relative path, case-insensitively and with either slash.
    pub fn file(&self, relative: &str) -> Option<&Path> {
        self.files.get(&index_key(relative)).map(PathBuf::as_path)
    }

    /// Indexed files (lowercased relative paths) that are neither `skin.ini` nor matched by
    /// the element [`registry`], sorted. Most menu and ranking files are unknown in M2.
    pub fn unknown_files(&self) -> Vec<&str> {
        let prefix = self.hit_circle_prefix();
        let mut unknown: Vec<&str> = self
            .files
            .keys()
            .map(String::as_str)
            .filter(|k| *k != "skin.ini" && !registry::is_known_file(k, prefix))
            .collect();
        unknown.sort_unstable();
        unknown
    }

    // --- configuration -------------------------------------------------------------------

    /// A raw `skin.ini` value by exact key.
    pub fn setting(&self, key: &str) -> Option<&str> {
        self.ini.settings.get(key)
    }

    /// A boolean setting like lazer's `genericLookup<bool>`: `true`/`false` in any case, or an
    /// integer where non-zero is true (some skins use 2). `None` when absent or unparsable.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkin.cs
    pub fn setting_bool(&self, key: &str) -> Option<bool> {
        let value = self.setting(key)?.trim();
        if value.eq_ignore_ascii_case("true") {
            Some(true)
        } else if value.eq_ignore_ascii_case("false") {
            Some(false)
        } else {
            parsing::net_parse_i32(value).ok().map(|v| v != 0)
        }
    }

    /// A `float` setting parsed with the invariant culture; `None` when absent or invalid.
    pub fn setting_f32(&self, key: &str) -> Option<f32> {
        parsing::net_parse_f32(self.setting(key)?).ok()
    }

    /// An `int` setting; `None` when absent or invalid.
    pub fn setting_i32(&self, key: &str) -> Option<i32> {
        parsing::net_parse_i32(self.setting(key)?).ok()
    }

    /// The skin version (`GetConfig<LegacySetting, decimal>(Version)`).
    pub fn version(&self) -> SkinVersion {
        self.ini.version
    }

    /// The version a configuration lookup gives (`LegacySetting.Version`): `None` for a
    /// beatmap's skin, which ignores beatmap-level versions (`LegacyBeatmapSkin.GetConfig`).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyBeatmapSkin.cs (GetConfig)
    pub fn config_version(&self) -> Option<SkinVersion> {
        (!self.beatmap).then_some(self.ini.version)
    }

    /// Whether this is a beatmap's skin ([`Skin::load_beatmap`]).
    pub fn is_beatmap(&self) -> bool {
        self.beatmap
    }

    /// Whether the version was `latest`.
    pub fn is_latest_version(&self) -> bool {
        self.ini.is_latest
    }

    /// The combo colours: the skin's own, else the default four
    /// (`SkinConfiguration.ComboColours`).
    pub fn combo_colours(&self) -> &[Rgba] {
        if self.ini.combo_colours.is_empty() {
            &DEFAULT_COMBO_COLOURS
        } else {
            &self.ini.combo_colours
        }
    }

    /// A custom colour by key (`SkinCustomColourLookup`).
    pub fn custom_colour(&self, name: &str) -> Option<Rgba> {
        self.ini.custom_colour(name)
    }

    /// `[Fonts] HitCirclePrefix`, default `default`. May contain a folder (`fonts\default`);
    /// `texture` resolves it like any name.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs
    pub fn hit_circle_prefix(&self) -> &str {
        self.setting("HitCirclePrefix").unwrap_or("default")
    }

    /// `HitCircleOverlap`, default -2. Positive values bring digits closer together.
    pub fn hit_circle_overlap(&self) -> f32 {
        self.setting_f32("HitCircleOverlap").unwrap_or(-2.0)
    }

    /// `ScorePrefix`, default `score`.
    pub fn score_prefix(&self) -> &str {
        self.setting("ScorePrefix").unwrap_or("score")
    }

    /// `ScoreOverlap`, default 0.
    pub fn score_overlap(&self) -> f32 {
        self.setting_f32("ScoreOverlap").unwrap_or(0.0)
    }

    /// `ComboPrefix`, default `score`.
    pub fn combo_prefix(&self) -> &str {
        self.setting("ComboPrefix").unwrap_or("score")
    }

    /// `ComboOverlap`, default 0.
    pub fn combo_overlap(&self) -> f32 {
        self.setting_f32("ComboOverlap").unwrap_or(0.0)
    }

    /// `AnimationFramerate`; `None` when absent. Frame lengths treat values `<= 0` like absent.
    pub fn animation_framerate(&self) -> Option<i32> {
        self.setting_i32("AnimationFramerate")
    }

    /// `CursorRotate`, default true.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursor.cs
    pub fn cursor_rotate(&self) -> bool {
        self.setting_bool("CursorRotate").unwrap_or(true)
    }

    /// `CursorCentre`, default true.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursor.cs
    pub fn cursor_centre(&self) -> bool {
        self.setting_bool("CursorCentre").unwrap_or(true)
    }

    /// `CursorExpand`, default true.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/OsuCursor.cs
    pub fn cursor_expand(&self) -> bool {
        self.setting_bool("CursorExpand").unwrap_or(true)
    }

    /// `CursorTrailRotate`, default true.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorTrail.cs
    pub fn cursor_trail_rotate(&self) -> bool {
        self.setting_bool("CursorTrailRotate").unwrap_or(true)
    }

    /// `HitCircleOverlayAboveNumber`, default true. The misspelt `HitCircleOverlayAboveNumer`
    /// of old skins is the fallback when the correct key is absent or invalid.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs
    pub fn hit_circle_overlay_above_number(&self) -> bool {
        self.setting_bool("HitCircleOverlayAboveNumber")
            .or_else(|| self.setting_bool("HitCircleOverlayAboveNumer"))
            .unwrap_or(true)
    }

    /// `SliderBallFlip`. osu!lazer's osu! ruleset never reads this key, so there is no
    /// default; stable flips the ball when it is true.
    pub fn slider_ball_flip(&self) -> Option<bool> {
        self.setting_bool("SliderBallFlip")
    }

    /// `AllowSliderBallTint`: tint the slider ball with the combo colour. Only an explicit
    /// true enables it.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBall.cs
    pub fn allow_slider_ball_tint(&self) -> bool {
        self.setting_bool("AllowSliderBallTint") == Some(true)
    }

    /// `SpinnerFrequencyModulate`, default true.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs
    pub fn spinner_frequency_modulate(&self) -> bool {
        self.setting_bool("SpinnerFrequencyModulate")
            .unwrap_or(true)
    }

    /// `SpinnerNoBlink`, default false (the old-style spinner blinks unless it is true).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyOldStyleSpinner.cs
    pub fn spinner_no_blink(&self) -> bool {
        self.setting_bool("SpinnerNoBlink") == Some(true)
    }

    /// `LayeredHitSounds`, default true: only an explicit false silences layered samples.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinTransformer.cs
    pub fn layered_hit_sounds(&self) -> bool {
        self.setting_bool("LayeredHitSounds") != Some(false)
    }

    /// `SliderBorder`. No default in the skin: lazer's slider body uses white.
    /// Ported from `LegacySliderBody.cs` (`?? Color4.White`).
    pub fn slider_border(&self) -> Option<Rgba> {
        self.custom_colour("SliderBorder")
    }

    /// `SliderTrackOverride`. No default in the skin: lazer's slider body uses the combo
    /// colour (`LegacySliderBody.cs`).
    pub fn slider_track_override(&self) -> Option<Rgba> {
        self.custom_colour("SliderTrackOverride")
    }

    /// `SliderBall` colour, default white (`LegacySliderBall.cs`).
    pub fn slider_ball_colour(&self) -> Rgba {
        self.custom_colour("SliderBall")
            .unwrap_or(Rgba::rgb(255, 255, 255))
    }

    /// `SpinnerBackground` colour, default (100, 100, 100) (`LegacyOldStyleSpinner.cs`).
    pub fn spinner_background(&self) -> Rgba {
        self.custom_colour("SpinnerBackground")
            .unwrap_or(Rgba::rgb(100, 100, 100))
    }

    /// `StarBreakAdditive` colour, default (255, 182, 193) (`LegacyCursorParticles.cs`).
    pub fn star_break_additive(&self) -> Rgba {
        self.custom_colour("StarBreakAdditive")
            .unwrap_or(Rgba::rgb(255, 182, 193))
    }
}
