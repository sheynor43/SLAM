//! Element behaviour that depends on the skin's `Version` (`skin.ini`), in one table.
//!
//! For osu!standard gameplay, osu!lazer reads the version in exactly four places;
//! [`VersionRules`] has one field for each, so drawing code never compares versions itself.
//! Where lazer and stable (McOsu) disagree, lazer wins (ADR-0107).
//!
//! Like lazer, a rule reads the version either from the skin that provides the element
//! ([`VersionRules::of_skin`]) or from the skin chain, which is the first skin with a
//! version ([`VersionRules::of_chain`]); each field says which. With a chain lookup, an
//! element from the default skin follows the user skin's version.
//!
//! Lazer reads the version with `GetConfig<LegacySetting, decimal>(Version)`. Every legacy
//! skin answers it (`1.0` without a `Version` key, `2.7` for `latest`, a missing `skin.ini`
//! or the default skin), except a beatmap's skin, which ignores its version
//! ([`Skin::config_version`]). With no answer the value is `null`, and C# compares `null`
//! as false whichever way the comparison goes, so every rule is off. [`VersionRules::new`]
//! does the same for `None`.
//!
//! # Behaviour that does not depend on the version
//!
//! In lazer the following elements choose their look by which textures and `skin.ini` keys
//! exist, never by the version. They are listed so they are not added here by mistake:
//!
//! - Old versus new style spinner: `spinner-background` gives the old style, `spinner-top`
//!   without it the new style (`OsuLegacySkinTransformer.cs`, `SpinnerBody`). Stable (McOsu
//!   `OsuSpinner.cpp`) also draws the old style for any skin below version 2.0.
//! - Slider ball frames (`sliderb0`, `sliderb1`, … without a separator) and their frame
//!   length, which follows the slider velocity (`LegacySliderBall.cs`). McOsu uses a fixed
//!   50 frames per second.
//! - `AnimationFramerate`: applied to follow points, the follow circle and the health bar
//!   colour (`applyConfigFrameRate` in `OsuLegacySkinTransformer.cs` and
//!   `LegacyHealthDisplay.cs`, `getFrameLength` in `LegacySkinExtensions.cs`); other
//!   animations except the slider ball run at 60 frames per second.
//! - `@2x` textures, including digit fonts (`LegacySkin.GetTexture`).
//! - `HitCircleOverlayAboveNumber` (default `true`, `LegacyMainCirclePiece.cs`).
//! - Combo colours, the cursor, the follow circle and the approach circle.
//!
//! Stable also changes song select and the ranking screen by version (McOsu
//! `OsuUISongBrowserSongButton.cpp`, `OsuRankingScreen.cpp`,
//! `OsuUIRankingScreenRankingPanel.cpp`); those rules belong to those
//! screens when they are skinned.

use crate::{Skin, SkinChain, SkinVersion};

/// `2.0`, the first version without stable's slider score points.
const V2: SkinVersion = SkinVersion::new(2, 0);

/// What a skin's version changes in how its elements are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VersionRules {
    /// On a hit, the circle's number only fades out quickly (a quarter of the 240 ms fade)
    /// instead of scaling and fading with the circle. `Version > 1.0`.
    ///
    /// McOsu does not draw the number at all after a hit for these skins, on circles and
    /// slider heads. Read from the chain.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyMainCirclePiece.cs (updateStateTransforms)
    pub number_short_fade: bool,
    /// A missed judgement (not a missed tick or slider end) also falls down as it fades.
    /// `Version > 1.0`.
    ///
    /// McOsu moves it along a different curve (`lerp(-5, 40, t³)`, `OsuHitObject.cpp`).
    /// In both, an animated miss does not move whatever the version. Read from the chain.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyJudgementPieceOld.cs (PlayAnimation)
    pub miss_falls: bool,
    /// Slider ticks and ends are judged with stable's score points `sliderpoint30` (large
    /// ticks) and `sliderpoint10` (the rest), if the skin has them. `Version < 2.0`.
    ///
    /// McOsu does not draw score points. Read from each skin in turn, so the beatmap's skin
    /// never uses them.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs (getSliderPointTexture)
    pub slider_points: bool,
    /// Reverse arrows rock back and forth as they pulse. `Version <= 1.0`. Read from the
    /// chain, not from the skin that provides the arrow.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyReverseArrow.cs (load)
    pub reverse_arrow_rotates: bool,
}

impl VersionRules {
    /// The rules for a skin of `version`; `None` when no skin gives a version, where lazer
    /// compares `null` and every rule is off.
    pub fn new(version: Option<SkinVersion>) -> Self {
        let Some(v) = version else {
            return Self::default();
        };
        Self {
            number_short_fade: v > SkinVersion::V1,
            miss_falls: v > SkinVersion::V1,
            slider_points: v < V2,
            reverse_arrow_rotates: v <= SkinVersion::V1,
        }
    }

    /// The rules of one skin's own version ([`Skin::config_version`]).
    pub fn of_skin(skin: &Skin) -> Self {
        Self::new(skin.config_version())
    }

    /// The rules of the version a chain lookup gives: the first skin with a version, which
    /// skips the beatmap's skin.
    pub fn of_chain(skins: &SkinChain) -> Self {
        Self::new(skins.config(Skin::config_version).map(|f| f.value))
    }
}
