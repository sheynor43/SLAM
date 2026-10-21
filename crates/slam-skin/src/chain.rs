//! The beatmap's skin and the user skin in front of the default skin, with per-element
//! fallback.

use std::path::PathBuf;

use crate::lookup::{Animation, AnimationOptions, TextureFile};
use crate::skin::Skin;

/// Which skin of a [`SkinChain`] provided a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkinSource {
    /// The beatmap's folder (`LegacyBeatmapSkin`, [`Skin::load_beatmap`]).
    Beatmap,
    /// The user's skin.
    User,
    /// The bundled default skin.
    Default,
}

/// A lookup result together with the skin that provided it.
///
/// Which skin supplies configuration (version, settings) is decided per call site, as in
/// lazer; this crate does not force one rule:
///
/// - Drawing code that picks an element's provider (`FindProvider`, e.g. hit circle prefix,
///   spinner and cursor textures) interprets that element by its provider's `skin.ini`: use
///   [`SkinChain::skin`]`(found.source)`.
/// - Lookups made through the skin source (`ISkinSource`) read the first skin that has a value,
///   user skin first: `LegacyReverseArrow.cs` (`Version`; it additionally checks whether the
///   texture comes from the default skin), `DrawableSpinner.cs` (`SpinnerFrequencyModulate`),
///   `OsuCursor.cs` (`CursorExpand`). Use [`SkinChain::config`] for these.
#[derive(Debug, Clone, PartialEq)]
pub struct Found<T> {
    /// The skin the value came from.
    pub source: SkinSource,
    /// The value.
    pub value: T,
}

/// The skins used for gameplay, in lookup order: the beatmap's skin (if any), the user skin
/// (if any), then the default (ADR-0058).
///
/// Mirrors `SkinManager.lookupWithFallback`: every texture and sample falls back on its own,
/// so a user skin with `hitcircle` but no `hitcircleoverlay` shows the default overlay.
///
/// The beatmap layer holds what lazer's `BeatmapSkins` setting allows: textures and
/// configuration. Its combo colours (`BeatmapColours`) and samples (`BeatmapHitsounds`) are
/// gated separately and read from the beatmap's skin by their callers, so [`SkinChain::sample`]
/// skips this layer.
#[derive(Debug, Clone)]
pub struct SkinChain {
    beatmap: Option<Skin>,
    user: Option<Skin>,
    default: Skin,
}

impl SkinChain {
    /// Creates a chain without a beatmap skin.
    pub fn new(user: Option<Skin>, default: Skin) -> Self {
        Self {
            beatmap: None,
            user,
            default,
        }
    }

    /// The chain with `beatmap` ([`Skin::load_beatmap`]) as its first layer; `None` leaves the
    /// beatmap's elements out (`BeatmapSkins` off).
    pub fn with_beatmap(mut self, beatmap: Option<Skin>) -> Self {
        self.beatmap = beatmap;
        self
    }

    /// The skin behind a source. `User` falls back to the default when the chain has no user
    /// skin, `Beatmap` to the user skin (or the default) when it has no beatmap skin.
    pub fn skin(&self, source: SkinSource) -> &Skin {
        let beatmap = match source {
            SkinSource::Beatmap => self.beatmap.as_ref(),
            _ => None,
        };
        let user = match source {
            SkinSource::Beatmap | SkinSource::User => self.user.as_ref(),
            SkinSource::Default => None,
        };
        beatmap.or(user).unwrap_or(&self.default)
    }

    /// Skins in lookup order with their sources.
    pub fn sources(&self) -> impl Iterator<Item = (SkinSource, &Skin)> {
        self.beatmap
            .iter()
            .map(|s| (SkinSource::Beatmap, s))
            .chain(self.skin_sources())
    }

    /// The user and default skins in lookup order, without the beatmap's skin.
    pub fn skin_sources(&self) -> impl Iterator<Item = (SkinSource, &Skin)> {
        self.user
            .iter()
            .map(|s| (SkinSource::User, s))
            .chain(std::iter::once((SkinSource::Default, &self.default)))
    }

    /// Replaces the user skin.
    pub fn set_user(&mut self, user: Option<Skin>) {
        self.user = user;
    }

    /// The first skin for which `has` is true (`ISkinSource.FindProvider`).
    pub fn find_provider(&self, has: impl Fn(&Skin) -> bool) -> Option<SkinSource> {
        self.sources().find(|(_, s)| has(s)).map(|(src, _)| src)
    }

    /// The first non-`None` value of a configuration lookup, beatmap skin first. This is how
    /// lazer resolves a `GetConfig` call across skin layers.
    pub fn config<T>(&self, get: impl Fn(&Skin) -> Option<T>) -> Option<Found<T>> {
        self.sources()
            .find_map(|(source, s)| get(s).map(|value| Found { source, value }))
    }

    /// Texture lookup with per-texture fallback.
    pub fn texture(&self, name: &str) -> Option<Found<TextureFile>> {
        self.sources()
            .find_map(|(source, s)| s.texture(name).map(|value| Found { source, value }))
    }

    /// Animation lookup. The provider is the first skin with frame 0 or the static texture
    /// (`GetTextures` in lazer); all frames then come from that skin only.
    pub fn animation(&self, name: &str, options: AnimationOptions<'_>) -> Option<Found<Animation>> {
        let source = self.find_provider(|s| s.provides_animation(name, options))?;
        self.skin(source)
            .animation(name, options)
            .map(|value| Found { source, value })
    }

    /// Sample lookup with per-sample fallback through the user and default skins; the
    /// beatmap's samples follow their own rules (`slam-play::hitsounds`).
    pub fn sample(&self, name: &str) -> Option<Found<PathBuf>> {
        self.skin_sources()
            .find_map(|(source, s)| s.sample(name).map(|value| Found { source, value }))
    }
}
