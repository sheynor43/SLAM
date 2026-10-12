//! A user skin in front of the default skin, with per-element fallback.

use std::path::PathBuf;

use crate::lookup::{Animation, AnimationOptions, TextureFile};
use crate::skin::Skin;

/// Which skin of a [`SkinChain`] provided a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkinSource {
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

/// The skins used for gameplay, in lookup order: the user skin (if any), then the default.
///
/// Mirrors `SkinManager.lookupWithFallback`: every texture and sample falls back on its own,
/// so a user skin with `hitcircle` but no `hitcircleoverlay` shows the default overlay.
#[derive(Debug, Clone)]
pub struct SkinChain {
    user: Option<Skin>,
    default: Skin,
}

impl SkinChain {
    /// Creates a chain.
    pub fn new(user: Option<Skin>, default: Skin) -> Self {
        Self { user, default }
    }

    /// The skin behind a source. `User` falls back to the default when the chain has no user
    /// skin.
    pub fn skin(&self, source: SkinSource) -> &Skin {
        match (source, &self.user) {
            (SkinSource::User, Some(user)) => user,
            _ => &self.default,
        }
    }

    /// Skins in lookup order with their sources.
    pub fn sources(&self) -> impl Iterator<Item = (SkinSource, &Skin)> {
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

    /// The first non-`None` value of a configuration lookup, user skin first. This is how
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

    /// Sample lookup with per-sample fallback.
    pub fn sample(&self, name: &str) -> Option<Found<PathBuf>> {
        self.sources()
            .find_map(|(source, s)| s.sample(name).map(|value| Found { source, value }))
    }
}
