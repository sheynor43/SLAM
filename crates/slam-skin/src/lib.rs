//! Skin loading, element registry, layered lookup and fallbacks.
//!
//! A [`Skin`] is one skin folder: an index of its files (case-insensitive, `\` and `/` are
//! the same), the parsed `skin.ini` and lookups for textures, animations and samples that
//! follow osu!lazer's `LegacySkin`. A [`SkinChain`] puts a user skin in front of the bundled
//! default skin and falls back per texture and per sample.
//!
//! Lookups return paths of files, not decoded data: decoding and upload belong to the
//! renderer and the audio engine. An [`ImageCheck`] from the caller makes texture lookups
//! treat images that do not decode as missing (ADR-0110).

#![warn(missing_docs)]

mod chain;
mod lookup;
pub mod registry;
mod skin;
mod version_rules;

pub use chain::{Found, SkinChain, SkinSource};
pub use lookup::{Animation, AnimationOptions, SIXTY_FRAME_TIME, TextureFile};
pub use skin::{ImageCheck, LoadError, Skin};
pub use slam_formats::skin_ini::{Rgba, SkinVersion};
pub use version_rules::VersionRules;
