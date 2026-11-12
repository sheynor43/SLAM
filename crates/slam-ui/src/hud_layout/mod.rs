//! HUD layouts: the data model, the built-in presets and the JSON format.
//!
//! A layout says where the elements of three screens go: the in-game HUD (`gameplay`), the
//! pause menu and fail window (`pause`) and the results screen (`results`). Each section is a
//! list of [`LayoutElement`]s. Positions are offsets in 640x480 screen units from an anchor
//! point of the screen to an origin point of the element (as in stable and the lazer HUD
//! JSON); rotation is in degrees. See ADR-0061 and ADR-0109.
//!
//! Unknown component kinds, element fields, properties and top-level fields are kept as they
//! are, so a layout written by a newer SLAM survives an edit in an older one.
//!
//! The built-in presets place the elements as the clients do:
//! - `stable` after McOsu `db2add2`: `src/App/Osu/OsuHUD.cpp` (`drawScore`, `drawAccuracy`,
//!   `drawCombo`, `drawScorebarBg`, `drawSkip`), offsets taken as screen units; the hit error
//!   meter as lazer's `LegacySkin` places it (bottom centre). The accuracy's 33 units below
//!   the top are the default skin's score digits at lazer's `LegacyScoreCounter` scale
//!   (46 × 0.96 lazer pixels) plus `LegacyAccuracyCounter`'s top margin of 9, as lazer's
//!   `LegacySkin` puts the accuracy below the score; a skin with taller digits overlaps.
//! - `lazer` after osu!lazer 2026.1005.0-lazer: `osu.Game/Skinning/ArgonSkin.cs` (the default
//!   skin's `MainHUDComponents`) and `osu.Game/Screens/Play/SkipOverlay.cs`, its 1024x768
//!   pixels converted to screen units (x 0.625).
//! - `slam` is `stable` for now, because the extra components of ADR-0109 (unstable rate,
//!   key counter, pp counter) are not drawn yet.
//!
//! A hit error meter element takes its style from the `style` property: `legacy` (the
//! default) or `bar` (ADR-0050).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

mod place;

pub use place::{SCREEN_UNITS, unit_scale};

/// The layout format version this build reads and writes.
pub const LAYOUT_VERSION: u32 = 1;

/// One of nine anchor points of a rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    TopCentre,
    TopRight,
    CentreLeft,
    Centre,
    CentreRight,
    BottomLeft,
    BottomCentre,
    BottomRight,
}

/// The kind of an element; unknown kinds are kept verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentKind {
    Score,
    Accuracy,
    Combo,
    Health,
    HitErrorMeter,
    SkipButton,
    SongProgress,
    UnstableRate,
    KeyCounter,
    PpCounter,
    ModDisplay,
    /// A kind this build does not know.
    Unknown(String),
}

impl ComponentKind {
    /// The name in the JSON file.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Score => "score",
            Self::Accuracy => "accuracy",
            Self::Combo => "combo",
            Self::Health => "health",
            Self::HitErrorMeter => "hit_error_meter",
            Self::SkipButton => "skip_button",
            Self::SongProgress => "song_progress",
            Self::UnstableRate => "unstable_rate",
            Self::KeyCounter => "key_counter",
            Self::PpCounter => "pp_counter",
            Self::ModDisplay => "mod_display",
            Self::Unknown(s) => s,
        }
    }

    fn from_name(name: &str) -> Self {
        match name {
            "score" => Self::Score,
            "accuracy" => Self::Accuracy,
            "combo" => Self::Combo,
            "health" => Self::Health,
            "hit_error_meter" => Self::HitErrorMeter,
            "skip_button" => Self::SkipButton,
            "song_progress" => Self::SongProgress,
            "unstable_rate" => Self::UnstableRate,
            "key_counter" => Self::KeyCounter,
            "pp_counter" => Self::PpCounter,
            "mod_display" => Self::ModDisplay,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

impl Serialize for ComponentKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ComponentKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Ok(Self::from_name(&name))
    }
}

fn one_pair() -> [f32; 2] {
    [1.0, 1.0]
}
fn zero_pair() -> [f32; 2] {
    [0.0, 0.0]
}
fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}
fn is_empty_map(m: &Map<String, Value>) -> bool {
    m.is_empty()
}

/// One placed component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutElement {
    pub component: ComponentKind,
    /// The point of the screen the offset is measured from.
    pub anchor: Anchor,
    /// The point of the element that sits at the anchor plus the offset.
    pub origin: Anchor,
    /// Offset in 640x480 screen units.
    pub position: [f32; 2],
    #[serde(default = "one_pair")]
    pub scale: [f32; 2],
    /// Degrees.
    #[serde(default)]
    pub rotation: f32,
    #[serde(default = "one")]
    pub alpha: f32,
    #[serde(default = "yes")]
    pub visible: bool,
    /// Perspective tilt; stored only, applied in M5.
    #[serde(default = "zero_pair")]
    pub perspective: [f32; 2],
    /// File name inside the layout's `sprites/` folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite: Option<String>,
    /// Component-specific settings.
    #[serde(default, skip_serializing_if = "is_empty_map")]
    pub properties: Map<String, Value>,
    /// Fields from a newer format that this build does not know.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl LayoutElement {
    /// An element with the anchor equal to the origin and every other field at its default.
    pub fn new(component: ComponentKind, anchor: Anchor, position: [f32; 2]) -> Self {
        Self {
            component,
            anchor,
            origin: anchor,
            position,
            scale: one_pair(),
            rotation: 0.0,
            alpha: 1.0,
            visible: true,
            perspective: zero_pair(),
            sprite: None,
            properties: Map::new(),
            extra: Map::new(),
        }
    }
}

/// A whole layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HudLayout {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub gameplay: Vec<LayoutElement>,
    #[serde(default)]
    pub pause: Vec<LayoutElement>,
    #[serde(default)]
    pub results: Vec<LayoutElement>,
    /// Top-level fields from a newer format that this build does not know.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Why a layout could not be read.
#[derive(Debug)]
pub enum LayoutError {
    Json(serde_json::Error),
    /// No integer `version` field.
    MissingVersion,
    /// The file is newer than [`LAYOUT_VERSION`].
    UnsupportedVersion(u32),
    /// A sprite name that is not a plain file name.
    InvalidSprite(String),
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "invalid layout JSON: {e}"),
            Self::MissingVersion => write!(f, "layout has no version"),
            Self::UnsupportedVersion(v) => {
                write!(
                    f,
                    "layout version {v} is newer than supported {LAYOUT_VERSION}"
                )
            }
            Self::InvalidSprite(s) => write!(f, "invalid sprite name `{s}`"),
        }
    }
}

impl std::error::Error for LayoutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for LayoutError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Whether `name` is a plain file name that cannot leave the sprites folder.
fn is_plain_file_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    !name.is_empty()
        && name != "."
        && !name.contains("..")
        && !name.contains(['/', '\\', '\0'])
        && !drive
}

/// Brings a layout of an older `version` up to [`LAYOUT_VERSION`].
fn migrate(value: Value, version: u32) -> Value {
    // Future migrations go here, one step per version:
    //   if version < 2 { value = v1_to_v2(value); }
    // Version 1 is current, so there is nothing to do yet.
    let _ = version;
    value
}

/// Parses and validates a layout.
pub fn from_json(text: &str) -> Result<HudLayout, LayoutError> {
    let value: Value = serde_json::from_str(text)?;
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .ok_or(LayoutError::MissingVersion)?;
    let version = u32::try_from(version).unwrap_or(u32::MAX);
    if version > LAYOUT_VERSION {
        return Err(LayoutError::UnsupportedVersion(version));
    }
    let value = migrate(value, version);
    let layout: HudLayout = serde_json::from_value(value)?;
    let sections = [&layout.gameplay, &layout.pause, &layout.results];
    for element in sections.into_iter().flatten() {
        if let Some(sprite) = &element.sprite
            && !is_plain_file_name(sprite)
        {
            return Err(LayoutError::InvalidSprite(sprite.clone()));
        }
    }
    Ok(layout)
}

/// Pretty JSON with a trailing newline.
pub fn to_json(layout: &HudLayout) -> String {
    let mut text = serde_json::to_string_pretty(layout).expect("a layout always serialises");
    text.push('\n');
    text
}

/// A preset that ships inside the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Stable,
    Lazer,
    Slam,
}

impl Builtin {
    pub const ALL: [Builtin; 3] = [Builtin::Stable, Builtin::Lazer, Builtin::Slam];

    /// The layout id, such as `builtin:stable`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Stable => "builtin:stable",
            Self::Lazer => "builtin:lazer",
            Self::Slam => "builtin:slam",
        }
    }

    pub fn from_id(id: &str) -> Option<Builtin> {
        Self::ALL.into_iter().find(|b| b.id() == id)
    }

    /// The preset layout.
    pub fn layout(self) -> HudLayout {
        let text = match self {
            Self::Stable => include_str!("../../assets/hud-layouts/stable.json"),
            Self::Lazer => include_str!("../../assets/hud-layouts/lazer.json"),
            Self::Slam => include_str!("../../assets/hud-layouts/slam.json"),
        };
        from_json(text).unwrap_or_else(|e| panic!("built-in layout {} is broken: {e}", self.id()))
    }
}

#[cfg(test)]
mod tests;
