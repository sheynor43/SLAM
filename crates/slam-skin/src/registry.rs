//! The table of gameplay elements a skin can customise in M2.
//!
//! This is the single source for the skin assembly menu and for telling used files from
//! unknown ones. Names are taken from osu!lazer's legacy skin code
//! (`osu.Game.Rulesets.Osu/Skinning/Legacy/*`, `osu.Game/Skinning/LegacySkin.cs`) and checked
//! against the bundled classic skin.
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

/// Where an element appears in the skin assembly menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Hit circles, approach circles, combo numbers, follow points, hit lighting.
    HitCircle,
    /// Slider parts.
    Slider,
    /// Spinner parts.
    Spinner,
    /// Cursor, trail, smoke.
    Cursor,
    /// Hit result judgements.
    Judgement,
    /// Sample sets.
    Hitsounds,
}

/// One image of an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDef {
    /// File name without extension and `@2x`. Pattern names contain `{HitCirclePrefix}`
    /// (replaced by the skin's prefix) and `{n}` (a digit 0 to 9).
    pub name: &'static str,
    /// `Some(separator)` when the image can be an animation `{name}{separator}{n}`. The
    /// separator is `"-"` except for the slider ball (`""`).
    pub animation: Option<&'static str>,
    /// Whether `name` is a pattern.
    pub pattern: bool,
    /// Whether the bundled classic skin has this image.
    pub in_default_skin: bool,
}

impl TextureDef {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            animation: None,
            pattern: false,
            in_default_skin: true,
        }
    }

    const fn animated(name: &'static str, separator: &'static str) -> Self {
        Self {
            animation: Some(separator),
            ..Self::new(name)
        }
    }

    const fn pattern(name: &'static str) -> Self {
        Self {
            pattern: true,
            ..Self::new(name)
        }
    }

    const fn missing_in_default(self) -> Self {
        Self {
            in_default_skin: false,
            ..self
        }
    }

    /// The concrete names of this image for a `HitCirclePrefix` (one name unless a pattern),
    /// with `/` as the folder separator.
    pub fn expand(&self, hit_circle_prefix: &str) -> Vec<String> {
        if !self.pattern {
            return vec![self.name.to_owned()];
        }
        let base = self
            .name
            .replace("{HitCirclePrefix}", &hit_circle_prefix.replace('\\', "/"));
        (0..=9)
            .map(|n| base.replace("{n}", &n.to_string()))
            .collect()
    }
}

/// A customisable gameplay element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementDef {
    /// Stable identifier.
    pub id: &'static str,
    /// Menu category.
    pub category: Category,
    /// Images of the element.
    pub textures: &'static [TextureDef],
    /// Sample names without extension.
    pub samples: &'static [&'static str],
    /// `skin.ini` keys (settings and colours) the element reads.
    pub ini_keys: &'static [&'static str],
}

const fn t(name: &'static str) -> TextureDef {
    TextureDef::new(name)
}

const fn anim(name: &'static str) -> TextureDef {
    TextureDef::animated(name, "-")
}

macro_rules! hitsounds {
    ($set:literal) => {
        &[
            concat!($set, "-hitnormal"),
            concat!($set, "-hitwhistle"),
            concat!($set, "-hitfinish"),
            concat!($set, "-hitclap"),
            concat!($set, "-slidertick"),
            concat!($set, "-sliderslide"),
            concat!($set, "-sliderwhistle"),
        ]
    };
}

/// All M2 gameplay elements.
pub const ELEMENTS: &[ElementDef] = &[
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyMainCirclePiece.cs
    ElementDef {
        id: "hitcircle",
        category: Category::HitCircle,
        textures: &[t("hitcircle"), t("hitcircleoverlay")],
        samples: &[],
        ini_keys: &["HitCircleOverlayAboveNumber", "HitCircleOverlayAboveNumer"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderHeadHitCircle.cs
    ElementDef {
        id: "slider-start-circle",
        category: Category::HitCircle,
        textures: &[
            t("sliderstartcircle").missing_in_default(),
            t("sliderstartcircleoverlay").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &[],
    },
    ElementDef {
        id: "slider-end-circle",
        category: Category::HitCircle,
        textures: &[
            t("sliderendcircle").missing_in_default(),
            t("sliderendcircleoverlay").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyApproachCircle.cs
    ElementDef {
        id: "approach-circle",
        category: Category::HitCircle,
        textures: &[t("approachcircle")],
        samples: &[],
        ini_keys: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySpriteText.cs
    ElementDef {
        id: "combo-numbers",
        category: Category::HitCircle,
        textures: &[TextureDef::pattern("{HitCirclePrefix}-{n}")],
        samples: &[],
        ini_keys: &["HitCirclePrefix", "HitCircleOverlap"],
    },
    ElementDef {
        id: "follow-point",
        category: Category::HitCircle,
        textures: &[anim("followpoint")],
        samples: &[],
        ini_keys: &["AnimationFramerate"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/SkinnableLighting.cs
    ElementDef {
        id: "hit-lighting",
        category: Category::HitCircle,
        textures: &[t("lighting")],
        samples: &[],
        ini_keys: &[],
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
    },
    ElementDef {
        id: "slider-follow-circle",
        category: Category::Slider,
        textures: &[anim("sliderfollowcircle")],
        samples: &[],
        ini_keys: &["AnimationFramerate"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyReverseArrow.cs
    ElementDef {
        id: "reverse-arrow",
        category: Category::Slider,
        textures: &[t("reversearrow")],
        samples: &[],
        ini_keys: &[],
    },
    ElementDef {
        id: "slider-score-point",
        category: Category::Slider,
        textures: &[t("sliderscorepoint")],
        samples: &[],
        ini_keys: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBody.cs
    ElementDef {
        id: "slider-body",
        category: Category::Slider,
        textures: &[],
        samples: &[],
        ini_keys: &["SliderBorder", "SliderTrackOverride"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyOldStyleSpinner.cs
    ElementDef {
        id: "spinner-old-style",
        category: Category::Spinner,
        textures: &[
            t("spinner-background").missing_in_default(),
            t("spinner-circle"),
            t("spinner-metre").missing_in_default(),
        ],
        samples: &[],
        ini_keys: &["SpinnerBackground", "SpinnerNoBlink"],
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
        ],
        samples: &[],
        ini_keys: &["SpinnerFrequencyModulate"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursor.cs
    ElementDef {
        id: "cursor",
        category: Category::Cursor,
        textures: &[t("cursor"), t("cursormiddle")],
        samples: &[],
        ini_keys: &["CursorRotate", "CursorExpand", "CursorCentre"],
    },
    ElementDef {
        id: "cursor-trail",
        category: Category::Cursor,
        textures: &[t("cursortrail")],
        samples: &[],
        ini_keys: &["CursorTrailRotate"],
    },
    ElementDef {
        id: "cursor-smoke",
        category: Category::Cursor,
        textures: &[t("cursor-smoke")],
        samples: &[],
        ini_keys: &[],
    },
    ElementDef {
        id: "cursor-ripple",
        category: Category::Cursor,
        textures: &[t("cursor-ripple").missing_in_default()],
        samples: &[],
        ini_keys: &[],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorParticles.cs
    ElementDef {
        id: "cursor-particles",
        category: Category::Cursor,
        textures: &[t("star2")],
        samples: &[],
        ini_keys: &["StarBreakAdditive"],
    },
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkin.cs
    ElementDef {
        id: "judgements",
        category: Category::Judgement,
        textures: &[
            anim("hit0"),
            anim("hit50"),
            anim("hit100"),
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
    },
    ElementDef {
        id: "hitsounds-normal",
        category: Category::Hitsounds,
        textures: &[],
        samples: hitsounds!("normal"),
        ini_keys: &["LayeredHitSounds"],
    },
    ElementDef {
        id: "hitsounds-soft",
        category: Category::Hitsounds,
        textures: &[],
        samples: hitsounds!("soft"),
        ini_keys: &["LayeredHitSounds"],
    },
    ElementDef {
        id: "hitsounds-drum",
        category: Category::Hitsounds,
        textures: &[],
        samples: hitsounds!("drum"),
        ini_keys: &["LayeredHitSounds"],
    },
    ElementDef {
        id: "spinner-sounds",
        category: Category::Hitsounds,
        textures: &[],
        samples: &["spinnerspin", "spinnerbonus"],
        ini_keys: &["SpinnerFrequencyModulate"],
    },
    ElementDef {
        id: "combo-break",
        category: Category::Hitsounds,
        textures: &[],
        samples: &["combobreak"],
        ini_keys: &[],
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

/// Whether a file (lowercased relative path with `/`) is used by a registry element, given
/// the skin's `HitCirclePrefix`. Image files match with or without `@2x`; animation frames and
/// sample sets with a custom bank number (`normal-hitnormal2.wav`) count as used.
pub fn is_known_file(key: &str, hit_circle_prefix: &str) -> bool {
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

    ELEMENTS.iter().any(|e| {
        let image = check_image && {
            let stem = stem.strip_suffix("@2x").unwrap_or(stem);
            e.textures.iter().any(|tex| {
                let prefix = hit_circle_prefix.to_lowercase();
                tex.expand(&prefix).iter().any(|name| {
                    let name = name.to_lowercase();
                    stem == name
                        || tex.animation.is_some_and(|sep| {
                            stem.strip_prefix(name.as_str())
                                .and_then(|r| r.strip_prefix(sep))
                                .is_some_and(|r| {
                                    !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit())
                                })
                        })
                })
            })
        };
        image
            || (check_sample
                && e.samples
                    .iter()
                    .any(|s| stem == *s || strip_digits(stem) == Some(*s)))
    })
}
