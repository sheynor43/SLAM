//! The table of skinnable osu!standard elements.
//!
//! This is the single source for the skin assembly menu and for telling used files from
//! unknown ones. It covers gameplay, HUD, pause and fail menus, countdown, results and the
//! menu elements that are not drawn during gameplay (listed so their files count as known).
//! Names are the union of osu!lazer's legacy skin code
//! (`osu.Game.Rulesets.Osu/Skinning/Legacy/*`, `osu.Game/Skinning/LegacySkin.cs`) and
//! McOsu's `OsuSkin.cpp`, checked against the bundled classic skin.
//!
//! # Version-dependent rules
//!
//! Which variant of an element is drawn can depend on the version of the skin that provides
//! it. The drawing code implements these rules; the places in lazer are:
//!
//! - Reverse arrow rotates only for `Version <= 1`:
//!   `osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyReverseArrow.cs` (also checks whether the
//!   texture comes from the default skin).
//! - Hit circle number: for `Version > 1.0` only a very short fade-out is applied on hit,
//!   older skins scale and fade it with the other pieces:
//!   `osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyMainCirclePiece.cs`.
//! - Missed judgement: for `Version > 1.0` the miss text also moves down:
//!   `osu.Game/Skinning/LegacyJudgementPieceOld.cs`.
//! - `sliderpoint10` / `sliderpoint30` are used for slider ticks and ends only when
//!   `Version < 2`: `OsuLegacySkinTransformer.cs` (`getSliderPointTexture`).
//! - Old versus new style spinner is chosen by which textures exist: `spinner-background`
//!   selects the old style, `spinner-top` without it the new style
//!   (`OsuLegacySkinTransformer.cs`, `SpinnerBody`).

use std::collections::{HashMap, HashSet};

/// Where an element appears in the skin assembly menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Hit circles, approach circles, combo numbers, follow points.
    HitCircle,
    /// Slider parts.
    Slider,
    /// Spinner parts.
    Spinner,
    /// Cursor and trail.
    Cursor,
    /// Hit result judgements.
    Judgement,
    /// Sample sets.
    Hitsounds,
    /// Score, combo and accuracy fonts, health bar, key overlay, in-game leaderboard.
    Hud,
    /// Pause and fail menus.
    GameplayMenus,
    /// Countdown and gameplay service elements (skip, warning arrows, section pass/fail).
    Countdown,
    /// Ranking screen.
    Results,
    /// Combo bursts, hit lighting, cursor ripples, smoke and particles, kiai star.
    Effects,
    /// Menu and song select elements, not drawn during gameplay (listed so their files count
    /// as known).
    Interface,
}

/// The glyph set of a font pattern: the `{glyph}` placeholder takes each of these in turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyphs {
    /// Digits `0` to `9` (hit circle numbers).
    Digits,
    /// Digits plus `comma`, `dot`, `percent`, `x` and `pp` (score, combo and accuracy fonts).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySpriteText.cs (`getLookupName`)
    Font,
    /// [`Glyphs::Font`] without `pp`: the legacy score entry font of the key overlay, which
    /// the classic skin ships without a `pp` glyph.
    EntryFont,
}

impl Glyphs {
    /// The glyph names substituted for `{glyph}`.
    pub const fn names(self) -> &'static [&'static str] {
        match self {
            Self::Digits => &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"],
            Self::Font => &[
                "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "comma", "dot", "percent", "x",
                "pp",
            ],
            Self::EntryFont => &[
                "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "comma", "dot", "percent", "x",
            ],
        }
    }
}

/// The font prefixes from `skin.ini` `[Fonts]`, used to expand font patterns. A prefix may
/// contain a folder (`fonts\default`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontPrefixes {
    /// `HitCirclePrefix`, default `default`.
    pub hit_circle: String,
    /// `ScorePrefix`, default `score`.
    pub score: String,
    /// `ComboPrefix`, default `score`.
    pub combo: String,
}

impl FontPrefixes {
    /// Prefixes from explicit values.
    pub fn new(
        hit_circle: impl Into<String>,
        score: impl Into<String>,
        combo: impl Into<String>,
    ) -> Self {
        Self {
            hit_circle: hit_circle.into(),
            score: score.into(),
            combo: combo.into(),
        }
    }
}

impl Default for FontPrefixes {
    /// `default`, `score`, `score`.
    fn default() -> Self {
        Self::new("default", "score", "score")
    }
}

/// One image of an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDef {
    /// File name without extension and `@2x`. Font patterns contain `{glyph}` and optionally
    /// `{HitCirclePrefix}`, `{ScorePrefix}` or `{ComboPrefix}` (replaced by the skin's prefix).
    pub name: &'static str,
    /// `Some(separator)` when the image can be an animation `{name}{separator}{n}`. The
    /// separator is `"-"` except for the slider ball (`""`).
    pub animation: Option<&'static str>,
    /// The glyph set when `name` is a font pattern.
    pub glyphs: Option<Glyphs>,
    /// Whether the bundled classic skin has this image.
    pub in_default_skin: bool,
}

impl TextureDef {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            animation: None,
            glyphs: None,
            in_default_skin: true,
        }
    }

    const fn animated(name: &'static str, separator: &'static str) -> Self {
        Self {
            animation: Some(separator),
            ..Self::new(name)
        }
    }

    const fn font(name: &'static str, glyphs: Glyphs) -> Self {
        Self {
            glyphs: Some(glyphs),
            ..Self::new(name)
        }
    }

    const fn missing_in_default(self) -> Self {
        Self {
            in_default_skin: false,
            ..self
        }
    }

    /// The concrete names of this image for the skin's font prefixes (one name unless a font
    /// pattern), with `/` as the folder separator.
    pub fn expand(&self, prefixes: &FontPrefixes) -> Vec<String> {
        let Some(glyphs) = self.glyphs else {
            return vec![self.name.to_owned()];
        };
        let fix = |p: &str| p.replace('\\', "/");
        let base = self
            .name
            .replace("{HitCirclePrefix}", &fix(&prefixes.hit_circle))
            .replace("{ScorePrefix}", &fix(&prefixes.score))
            .replace("{ComboPrefix}", &fix(&prefixes.combo));
        glyphs
            .names()
            .iter()
            .map(|g| base.replace("{glyph}", g))
            .collect()
    }
}

/// One sample of an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleDef {
    /// Sample name without extension.
    pub name: &'static str,
    /// Whether the bundled classic skin has this sample.
    pub in_default_skin: bool,
}

impl SampleDef {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            in_default_skin: true,
        }
    }

    const fn missing_in_default(self) -> Self {
        Self {
            in_default_skin: false,
            ..self
        }
    }
}

/// A customisable element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementDef {
    /// Stable identifier.
    pub id: &'static str,
    /// Menu category.
    pub category: Category,
    /// Images of the element.
    pub textures: &'static [TextureDef],
    /// Samples of the element.
    pub samples: &'static [SampleDef],
    /// `skin.ini` keys (settings and colours) the element reads.
    pub ini_keys: &'static [&'static str],
    /// Identifiers of other elements whose images or settings this one falls back to or
    /// shares.
    pub depends_on: &'static [&'static str],
}

const fn t(name: &'static str) -> TextureDef {
    TextureDef::new(name)
}

const fn anim(name: &'static str) -> TextureDef {
    TextureDef::animated(name, "-")
}

const fn s(name: &'static str) -> SampleDef {
    SampleDef::new(name)
}

macro_rules! hitsounds {
    ($set:literal) => {
        &[
            s(concat!($set, "-hitnormal")),
            s(concat!($set, "-hitwhistle")),
            s(concat!($set, "-hitfinish")),
            s(concat!($set, "-hitclap")),
            s(concat!($set, "-slidertick")),
            s(concat!($set, "-sliderslide")),
            s(concat!($set, "-sliderwhistle")),
        ]
    };
}

/// All skinnable elements.
pub const ELEMENTS: &[ElementDef] = &[
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyMainCirclePiece.cs
    ElementDef {
        id: "hitcircle",
        category: Category::HitCircle,
        // McOsu animates the overlay.
        textures: &[t("hitcircle"), anim("hitcircleoverlay")],
        samples: &[],
        ini_keys: &["HitCircleOverlayAboveNumber", "HitCircleOverlayAboveNumer"],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderHeadHitCircle.cs
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    ElementDef {
        id: "slider-start-circle",
        category: Category::HitCircle,
        textures: &[
            t("sliderstartcircle").missing_in_default(),
            anim("sliderstartcircleoverlay").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &["hitcircle"],
    },
    ElementDef {
        id: "slider-end-circle",
        category: Category::HitCircle,
        textures: &[
            t("sliderendcircle").missing_in_default(),
            anim("sliderendcircleoverlay").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &["hitcircle"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyApproachCircle.cs
    ElementDef {
        id: "approach-circle",
        category: Category::HitCircle,
        textures: &[t("approachcircle")],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySpriteText.cs
    ElementDef {
        id: "combo-numbers",
        category: Category::HitCircle,
        textures: &[TextureDef::font(
            "{HitCirclePrefix}-{glyph}",
            Glyphs::Digits,
        )],
        samples: &[],
        ini_keys: &["HitCirclePrefix", "HitCircleOverlap"],
        depends_on: &["hitcircle"],
    },
    ElementDef {
        id: "follow-point",
        category: Category::HitCircle,
        textures: &[anim("followpoint")],
        samples: &[],
        ini_keys: &["AnimationFramerate"],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBall.cs
    ElementDef {
        id: "slider-ball",
        category: Category::Slider,
        textures: &[
            TextureDef::animated("sliderb", ""),
            t("sliderb-nd"),
            t("sliderb-spec"),
        ],
        samples: &[],
        ini_keys: &["SliderBall", "AllowSliderBallTint", "SliderBallFlip"],
        depends_on: &[],
    },
    ElementDef {
        id: "slider-follow-circle",
        category: Category::Slider,
        textures: &[anim("sliderfollowcircle")],
        samples: &[],
        ini_keys: &["AnimationFramerate"],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyReverseArrow.cs
    ElementDef {
        id: "reverse-arrow",
        category: Category::Slider,
        textures: &[t("reversearrow")],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    ElementDef {
        id: "slider-score-point",
        category: Category::Slider,
        textures: &[t("sliderscorepoint")],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBody.cs
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp (`slidergradient`)
    ElementDef {
        id: "slider-body",
        category: Category::Slider,
        textures: &[t("slidergradient").missing_in_default()],
        samples: &[],
        ini_keys: &["SliderBorder", "SliderTrackOverride"],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyOldStyleSpinner.cs
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp (`circularmetre`)
    ElementDef {
        id: "spinner-old-style",
        category: Category::Spinner,
        textures: &[
            t("spinner-background").missing_in_default(),
            t("spinner-circle"),
            t("spinner-metre").missing_in_default(),
            t("circularmetre").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &["SpinnerBackground", "SpinnerNoBlink"],
        depends_on: &["spinner-common"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyNewStyleSpinner.cs
    ElementDef {
        id: "spinner-new-style",
        category: Category::Spinner,
        textures: &[
            t("spinner-glow"),
            t("spinner-bottom"),
            t("spinner-top"),
            t("spinner-middle"),
            t("spinner-middle2"),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &["spinner-common"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySpinner.cs
    ElementDef {
        id: "spinner-common",
        category: Category::Spinner,
        textures: &[
            t("spinner-approachcircle"),
            t("spinner-clear"),
            t("spinner-spin"),
            t("spinner-rpm"),
            // Neither lazer (osu!standard) nor McOsu draws it; the classic skin ships it.
            t("spinner-warning"),
        ],
        samples: &[],
        ini_keys: &["SpinnerFrequencyModulate"],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursor.cs
    ElementDef {
        id: "cursor",
        category: Category::Cursor,
        textures: &[t("cursor"), t("cursormiddle")],
        samples: &[],
        ini_keys: &["CursorRotate", "CursorExpand", "CursorCentre"],
        depends_on: &[],
    },
    ElementDef {
        id: "cursor-trail",
        category: Category::Cursor,
        textures: &[t("cursortrail")],
        samples: &[],
        ini_keys: &["CursorTrailRotate"],
        depends_on: &["cursor"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkin.cs
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    ElementDef {
        id: "judgements",
        category: Category::Judgement,
        textures: &[
            anim("hit0"),
            anim("hit50"),
            anim("hit50g").missing_in_default(),
            anim("hit50k").missing_in_default(),
            anim("hit100"),
            anim("hit100g").missing_in_default(),
            anim("hit100k"),
            anim("hit300"),
            anim("hit300g"),
            anim("hit300k"),
            t("particle50"),
            t("particle100"),
            t("particle300"),
            anim("slidertickmiss"),
            anim("sliderendmiss"),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs
    ElementDef {
        id: "slider-point-judgements",
        category: Category::Judgement,
        textures: &[
            t("sliderpoint10").missing_in_default(),
            t("sliderpoint30").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    ElementDef {
        id: "hitsounds-normal",
        category: Category::Hitsounds,
        textures: &[],
        samples: hitsounds!("normal"),
        ini_keys: &["LayeredHitSounds"],
        depends_on: &[],
    },
    ElementDef {
        id: "hitsounds-soft",
        category: Category::Hitsounds,
        textures: &[],
        samples: hitsounds!("soft"),
        ini_keys: &["LayeredHitSounds"],
        depends_on: &[],
    },
    ElementDef {
        id: "hitsounds-drum",
        category: Category::Hitsounds,
        textures: &[],
        samples: hitsounds!("drum"),
        ini_keys: &["LayeredHitSounds"],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSpinner.cs
    ElementDef {
        id: "spinner-sounds",
        category: Category::Hitsounds,
        textures: &[],
        samples: &[
            s("spinnerspin"),
            s("spinnerbonus"),
            s("spinnerbonus-max").missing_in_default(),
        ],
        ini_keys: &["SpinnerFrequencyModulate"],
        depends_on: &[],
    },
    ElementDef {
        id: "combo-break",
        category: Category::Hitsounds,
        textures: &[],
        samples: &[s("combobreak")],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModNightcore.cs
    ElementDef {
        id: "nightcore",
        category: Category::Hitsounds,
        textures: &[],
        samples: &[
            s("nightcore-kick"),
            s("nightcore-clap"),
            s("nightcore-hat"),
            s("nightcore-finish"),
        ],
        ini_keys: &[],
        depends_on: &[],
    },
    // --- HUD ---------------------------------------------------------------------------
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySpriteText.cs
    ElementDef {
        id: "score-font",
        category: Category::Hud,
        textures: &[TextureDef::font("{ScorePrefix}-{glyph}", Glyphs::Font)],
        samples: &[],
        ini_keys: &["ScorePrefix", "ScoreOverlap"],
        depends_on: &[],
    },
    // ComboPrefix defaults to the score font.
    ElementDef {
        id: "combo-font",
        category: Category::Hud,
        textures: &[TextureDef::font("{ComboPrefix}-{glyph}", Glyphs::Font)],
        samples: &[],
        ini_keys: &["ComboPrefix", "ComboOverlap"],
        depends_on: &["score-font"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyHealthDisplay.cs
    ElementDef {
        id: "health-bar",
        category: Category::Hud,
        textures: &[
            anim("scorebar-bg"),
            anim("scorebar-colour"),
            anim("scorebar-marker"),
            anim("scorebar-ki").missing_in_default(),
            anim("scorebar-kidanger").missing_in_default(),
            anim("scorebar-kidanger2").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &["AnimationFramerate"],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyKeyCounter.cs
    // The key text uses the fixed `scoreentry` font (`LegacyFont.ScoreEntry`).
    ElementDef {
        id: "key-overlay",
        category: Category::Hud,
        textures: &[
            anim("inputoverlay-background"),
            anim("inputoverlay-key"),
            TextureDef::font("scoreentry-{glyph}", Glyphs::EntryFont),
        ],
        samples: &[],
        ini_keys: &["InputOverlayText"],
        depends_on: &[],
    },
    // The stable in-game leaderboard; the explosions are also used by lazer's catch ruleset.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Catch/Skinning/Legacy/LegacyHitExplosion.cs
    ElementDef {
        id: "leaderboard",
        category: Category::Hud,
        textures: &[t("scoreboard-explosion-1"), t("scoreboard-explosion-2")],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // --- Pause and fail menus ----------------------------------------------------------
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs
    ElementDef {
        id: "pause-menu",
        category: Category::GameplayMenus,
        textures: &[
            t("pause-overlay").missing_in_default(),
            t("pause-continue").missing_in_default(),
            t("pause-retry").missing_in_default(),
            t("pause-back").missing_in_default(),
            t("pause-replay").missing_in_default(),
        ],
        samples: &[
            s("pause-loop").missing_in_default(),
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/PlayerLoader.cs
            s("pause-retry-click").missing_in_default(),
        ],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    ElementDef {
        id: "fail-menu",
        category: Category::GameplayMenus,
        textures: &[t("fail-background").missing_in_default()],
        samples: &[s("failsound")],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/DelayedResumeOverlay.cs
    ElementDef {
        id: "unpause",
        category: Category::GameplayMenus,
        textures: &[t("unpause").missing_in_default()],
        samples: &[s("resume-countdown").missing_in_default()],
        ini_keys: &[],
        depends_on: &[],
    },
    // --- Countdown and service elements -----------------------------------------------
    // The classic skin ships `count` and the images; the per-step stable sounds are not in it.
    ElementDef {
        id: "countdown",
        category: Category::Countdown,
        textures: &[t("ready"), t("count3"), t("count2"), t("count1"), t("go")],
        samples: &[
            s("count"),
            s("readys").missing_in_default(),
            s("count3s").missing_in_default(),
            s("count2s").missing_in_default(),
            s("count1s").missing_in_default(),
            s("gos").missing_in_default(),
        ],
        ini_keys: &[],
        depends_on: &[],
    },
    ElementDef {
        id: "skip",
        category: Category::Countdown,
        textures: &[anim("play-skip").missing_in_default()],
        samples: &[],
        ini_keys: &["AnimationFramerate"],
        depends_on: &[],
    },
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    ElementDef {
        id: "warning-arrows",
        category: Category::Countdown,
        textures: &[anim("play-warningarrow")],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp (sounds)
    ElementDef {
        id: "section-pass-fail",
        category: Category::Countdown,
        textures: &[anim("section-pass"), anim("section-fail")],
        samples: &[s("sectionpass"), s("sectionfail")],
        ini_keys: &[],
        depends_on: &[],
    },
    // --- Results ----------------------------------------------------------------------
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Online/Leaderboards/DrawableRank.cs
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyRankDisplay.cs
    ElementDef {
        id: "ranking-grades",
        category: Category::Results,
        textures: &[
            t("ranking-XH"),
            t("ranking-X"),
            t("ranking-SH"),
            t("ranking-S"),
            t("ranking-A"),
            t("ranking-B"),
            t("ranking-C"),
            t("ranking-D"),
            anim("ranking-XH-small"),
            anim("ranking-X-small"),
            anim("ranking-SH-small"),
            anim("ranking-S-small"),
            anim("ranking-A-small"),
            anim("ranking-B-small"),
            anim("ranking-C-small"),
            anim("ranking-D-small"),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    ElementDef {
        id: "ranking-panel",
        category: Category::Results,
        textures: &[
            t("ranking-panel").missing_in_default(),
            t("ranking-graph").missing_in_default(),
            t("ranking-title").missing_in_default(),
            t("ranking-maxcombo").missing_in_default(),
            t("ranking-accuracy").missing_in_default(),
            anim("ranking-perfect").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &["ranking-grades"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Ranking/ResultsScreen.cs
    ElementDef {
        id: "results-sounds",
        category: Category::Results,
        textures: &[],
        samples: &[s("applause")],
        ini_keys: &[],
        depends_on: &[],
    },
    // --- Effects ----------------------------------------------------------------------
    // The classic skin ships `comboburst`; `comboburst-{n}` frames count as the animation.
    ElementDef {
        id: "combo-burst",
        category: Category::Effects,
        textures: &[anim("comboburst")],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/SkinnableLighting.cs
    ElementDef {
        id: "hit-lighting",
        category: Category::Effects,
        textures: &[t("lighting")],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    ElementDef {
        id: "cursor-smoke",
        category: Category::Effects,
        textures: &[t("cursor-smoke")],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    ElementDef {
        id: "cursor-ripple",
        category: Category::Effects,
        textures: &[t("cursor-ripple").missing_in_default()],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorParticles.cs
    ElementDef {
        id: "cursor-particles",
        category: Category::Effects,
        textures: &[t("star2")],
        samples: &[],
        ini_keys: &["StarBreakAdditive"],
        depends_on: &[],
    },
    // --- Interface (not drawn during gameplay) -----------------------------------------
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    ElementDef {
        id: "song-select-buttons",
        category: Category::Interface,
        textures: &[
            t("selection-mode"),
            t("selection-mode-over"),
            t("selection-mods"),
            t("selection-mods-over"),
            t("selection-random"),
            t("selection-random-over"),
            t("selection-options"),
            t("selection-options-over"),
            t("songselect-top").missing_in_default(),
            t("songselect-bottom").missing_in_default(),
            t("menu-button-background").missing_in_default(),
            t("button-left").missing_in_default(),
            t("button-middle").missing_in_default(),
            t("button-right").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    ElementDef {
        id: "mod-icons",
        category: Category::Interface,
        textures: &[
            anim("selection-mod-easy").missing_in_default(),
            anim("selection-mod-nofail").missing_in_default(),
            anim("selection-mod-halftime").missing_in_default(),
            anim("selection-mod-hardrock").missing_in_default(),
            anim("selection-mod-suddendeath").missing_in_default(),
            anim("selection-mod-perfect").missing_in_default(),
            anim("selection-mod-doubletime").missing_in_default(),
            anim("selection-mod-nightcore").missing_in_default(),
            anim("selection-mod-daycore").missing_in_default(),
            anim("selection-mod-hidden").missing_in_default(),
            anim("selection-mod-flashlight").missing_in_default(),
            anim("selection-mod-relax").missing_in_default(),
            anim("selection-mod-relax2").missing_in_default(),
            anim("selection-mod-spunout").missing_in_default(),
            anim("selection-mod-autoplay").missing_in_default(),
            anim("selection-mod-nightmare").missing_in_default(),
            anim("selection-mod-target").missing_in_default(),
            anim("selection-mod-scorev2").missing_in_default(),
            anim("selection-mod-touchdevice").missing_in_default(),
            anim("selection-mod-cinema").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // The classic skin ships these; neither reference draws `levelbar*`, `user-bg` or `star3`
    // in osu!standard gameplay (`star` and `user-bg` are song select and profile elements).
    ElementDef {
        id: "profile-bar",
        category: Category::Interface,
        textures: &[
            t("levelbar"),
            t("levelbar-bg"),
            t("user-bg"),
            t("star"),
            t("star3"),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Editor selection highlight; neither reference draws it in gameplay.
    ElementDef {
        id: "editor-selection",
        category: Category::Interface,
        textures: &[t("hitcircleselect")],
        samples: &[],
        ini_keys: &[],
        depends_on: &["hitcircle"],
    },
    // Stable menu and level-up lighting; lazer draws `lightingL`/`lightingN` only in the mania
    // ruleset and neither reference draws them in osu!standard.
    ElementDef {
        id: "legacy-lighting",
        category: Category::Interface,
        textures: &[anim("lightingL"), t("lightingN")],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    ElementDef {
        id: "back-button",
        category: Category::Interface,
        textures: &[
            anim("menu-back").missing_in_default(),
            t("back-button-layer"),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/OsuCheckbox.cs
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/ScreenshotManager.cs
    ElementDef {
        id: "menu-sounds",
        category: Category::Interface,
        textures: &[],
        samples: &[
            s("menuback"),
            s("menuclick"),
            s("click-short"),
            s("click-short-confirm"),
            s("menuhit").missing_in_default(),
            s("check-on").missing_in_default(),
            s("check-off").missing_in_default(),
            s("shutter").missing_in_default(),
        ],
        ini_keys: &[],
        depends_on: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Backgrounds/SkinBackground.cs
    // Ported from McOsu db2add20ea291f6f3b6d022fcd4eba100a5bd161: src/App/Osu/OsuSkin.cpp
    ElementDef {
        id: "menu-background",
        category: Category::Interface,
        textures: &[
            t("menu-background").missing_in_default(),
            t("loading-spinner").missing_in_default(),
            t("beatmapimport-spinner").missing_in_default(),
            t("circle-empty").missing_in_default(),
            t("circle-full").missing_in_default(),
            t("seektriangle").missing_in_default(),
            t("user-icon").missing_in_default(),
            t("backgroundcube").missing_in_default(),
            t("skybox").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &[],
        depends_on: &[],
    },
];

/// The element with this identifier.
pub fn element(id: &str) -> Option<&'static ElementDef> {
    ELEMENTS.iter().find(|e| e.id == id)
}

/// Image extensions the texture loader accepts.
const TEXTURE_EXTENSIONS: [&str; 2] = [".png", ".jpg"];
/// Sample extensions the sample store accepts.
const SAMPLE_EXTENSIONS: [&str; 3] = [".wav", ".mp3", ".ogg"];

fn strip_digits(s: &str) -> Option<&str> {
    let trimmed = s.trim_end_matches(|c: char| c.is_ascii_digit());
    (trimmed.len() != s.len()).then_some(trimmed)
}

/// The file names the registry uses for one set of font prefixes, built once so that a whole
/// skin folder can be checked without expanding every pattern per file.
#[derive(Debug, Clone)]
pub struct KnownFiles {
    /// Lowercased image names without extension and `@2x`.
    images: HashSet<String>,
    /// Lowercased animation base names and their frame separator.
    animations: HashMap<String, &'static str>,
    /// Sample names without extension.
    samples: HashSet<&'static str>,
}

impl KnownFiles {
    /// The known names for these prefixes. Font glyphs of the default prefixes are included
    /// too, because a skin with a custom prefix still falls back to them.
    pub fn new(prefixes: &FontPrefixes) -> Self {
        let defaults = FontPrefixes::default();
        let mut images = HashSet::new();
        let mut animations = HashMap::new();
        let mut samples = HashSet::new();
        for e in ELEMENTS {
            for tex in e.textures {
                let mut names = tex.expand(prefixes);
                if tex.glyphs.is_some() {
                    names.extend(tex.expand(&defaults));
                }
                for name in names {
                    let name = name.to_lowercase();
                    if let Some(sep) = tex.animation {
                        animations.insert(name.clone(), sep);
                    }
                    images.insert(name);
                }
            }
            samples.extend(e.samples.iter().map(|s| s.name));
        }
        Self {
            images,
            animations,
            samples,
        }
    }

    /// Whether a file (lowercased relative path with `/`) is used by a registry element. Image
    /// files match with or without `@2x`; animation frames and sample sets with a custom bank
    /// number (`normal-hitnormal2.wav`) count as used.
    pub fn contains(&self, key: &str) -> bool {
        // Without a known extension a file still counts when its whole name matches exactly
        // (the loaders try the name as given first).
        let (stem, check_image, check_sample) =
            if let Some(s) = TEXTURE_EXTENSIONS.iter().find_map(|e| key.strip_suffix(e)) {
                (s, true, false)
            } else if let Some(s) = SAMPLE_EXTENSIONS.iter().find_map(|e| key.strip_suffix(e)) {
                (s, false, true)
            } else {
                (key, true, true)
            };
        let image = check_image && {
            let stem = stem.strip_suffix("@2x").unwrap_or(stem);
            self.images.contains(stem)
                || strip_digits(stem).is_some_and(|base| {
                    self.animations.get(base) == Some(&"")
                        || base
                            .strip_suffix('-')
                            .is_some_and(|b| self.animations.get(b) == Some(&"-"))
                })
        };
        image
            || (check_sample
                && (self.samples.contains(stem)
                    || strip_digits(stem).is_some_and(|s| self.samples.contains(s))))
    }
}

/// Whether a file (lowercased relative path with `/`) is used by a registry element, given
/// the skin's font prefixes. Builds a [`KnownFiles`]; to check many files, build it once.
pub fn is_known_file(key: &str, prefixes: &FontPrefixes) -> bool {
    KnownFiles::new(prefixes).contains(key)
}
