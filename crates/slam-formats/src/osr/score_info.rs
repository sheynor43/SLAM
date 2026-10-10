//! The osu!lazer score-info block of `.osr` replays (versions >= 30000001).
//!
//! The block is an LZMA blob (same container as the frames) holding the JSON of
//! `LegacyReplaySoloScoreInfo`: online id, mods with their settings, hit statistics, client
//! version, rank, user id, score without mods and pauses. [`Replay::score_info`] parses it,
//! [`ScoreInfo::to_block`] produces it. The raw [`Replay::lazer_block`] stays the source of
//! truth; nothing here is applied automatically on decode or encode.
//!
//! Reading follows Newtonsoft's defaults as lazer uses them: a missing field takes its
//! initializer (`online_id` and `user_id` are -1, the collections are empty), `null` for a
//! reference-type field (`mods`, `statistics`, `maximum_statistics`, `client_version`,
//! `pauses`, `rank`, a mod's `settings`) means the default, and `null` or a wrong type for a
//! value-type field is an error, as in lazer where it fails the import. Unknown top-level
//! fields, unknown fields of a mod, unknown [`HitResult`] keys, unknown [`ScoreRank`] names and
//! setting values of any shape are kept verbatim so a read/write cycle loses nothing.
//!
//! Writing follows lazer's serializer: indented JSON with the properties in the order of
//! `LegacyReplaySoloScoreInfo`, omitting a property that equals its CLR default
//! (`online_id` and `user_id` when 0, `rank` and `total_score_without_mods` when `None`),
//! always writing the arrays, dictionaries and `client_version`, and `settings` of a mod only
//! when non-empty. Unknown fields come last.
//!
//! Deviation from lazer: lazer encodes the JSON with `ASCIIEncoding`, turning every non-ASCII
//! character into `?`. Here non-ASCII characters are written as `\uXXXX` escapes (UTF-16
//! surrogate pairs for astral characters), so the output is pure ASCII, lazer reads it back
//! identically and nothing is lost.
//!
//! Reading is stricter than Newtonsoft: enum names are matched exactly (a rank or statistics
//! key in another case, or given as an integer, becomes `Unknown` or an error), integer
//! fields do not accept numeric strings or `5.0`, invalid UTF-8 is an error rather than
//! U+FFFD, and comments or trailing commas are rejected. Lazer never writes any of these.
//! Nesting deeper than 128 levels is an error (Newtonsoft's `MaxDepth` is 64).
//!
//! Keys of settings and statistics are written verbatim. Lazer's serializer would snake_case
//! dictionary keys and looks settings up by snake_case names, so callers must supply
//! snake_case keys. [`HitResult::Unknown`] and [`ScoreRank::Unknown`] write back what was
//! read, but a block containing them is rejected by lazer (its enum parsing throws); that is
//! fine for re-writing files that already had them. On write, `extra` entries whose key is a
//! known field (or `acronym`/`settings` for a mod) are skipped. Duplicate keys in a JSON
//! object: the last value wins on read, as in Newtonsoft; duplicate keys in `statistics` or
//! `settings` given to the writer collapse the same way.

use serde_json::{Map, Number, Value};

use super::error::OsrError;
use super::lzma::{compress, decompress};
use super::model::Replay;

/// A validated JSON fragment, stored as compact text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawJson(String);

impl RawJson {
    /// Validates `text` as JSON and keeps it in compact form; `None` if it is not valid JSON.
    pub fn new(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        Some(Self::from_value(&value))
    }

    /// The compact JSON text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_value(value: &Value) -> Self {
        Self(value.to_string())
    }

    fn to_value(&self) -> Value {
        // Invariant: the text is always produced from a valid `Value`.
        serde_json::from_str(&self.0).expect("RawJson holds valid JSON")
    }
}

/// The value of one mod setting.
///
/// Enums are stored by lazer as integers, colours as hex strings.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    /// JSON `null` (an unset nullable bindable).
    Null,
    /// A boolean.
    Bool(bool),
    /// An integer that fits `i64`.
    Int(i64),
    /// Any other number. Non-finite values are written like Newtonsoft's default, as the
    /// strings `"NaN"`, `"Infinity"` and `"-Infinity"`, and read back as [`String`](Self::String).
    Float(f64),
    /// A string.
    String(String),
    /// An array, an object or an integer in `i64::MAX + 1 ..= u64::MAX`, kept as compact JSON.
    /// Number spelling inside is normalised (`1.10` becomes `1.1`); integers above `u64::MAX`
    /// become [`Float`](Self::Float) elsewhere (no arbitrary precision).
    Raw(RawJson),
}

impl SettingValue {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(b) => Self::Bool(*b),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Self::Int(i)
                } else if n.is_u64() {
                    Self::Raw(RawJson::from_value(value))
                } else {
                    Self::Float(n.as_f64().unwrap_or(0.0))
                }
            }
            Value::String(s) => Self::String(s.clone()),
            Value::Array(_) | Value::Object(_) => Self::Raw(RawJson::from_value(value)),
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(b) => Value::Bool(*b),
            Self::Int(i) => Value::from(*i),
            Self::Float(f) => match Number::from_f64(*f) {
                Some(n) => Value::Number(n),
                None if f.is_nan() => Value::String("NaN".into()),
                None if *f > 0.0 => Value::String("Infinity".into()),
                None => Value::String("-Infinity".into()),
            },
            Self::String(s) => Value::String(s.clone()),
            Self::Raw(raw) => raw.to_value(),
        }
    }
}

/// A mod of the score (`APIMod`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ApiMod {
    /// The mod acronym, e.g. `DT`.
    pub acronym: String,
    /// Settings in file order. Keys are written verbatim and must already be snake_case, which
    /// lazer expects (see the module docs).
    pub settings: Vec<(String, SettingValue)>,
    /// Unknown fields of the mod object, kept verbatim. Entries named `acronym` or `settings`
    /// are skipped on write.
    pub extra: Vec<(String, RawJson)>,
}

macro_rules! name_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)*
            /// A name this version does not know, kept as read. Writing it yields a block that
            /// lazer rejects.
            Unknown(String),
        }

        impl $name {
            /// The name used in the JSON.
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $text,)*
                    Self::Unknown(s) => s,
                }
            }

            /// Looks a JSON name up (exact match); anything else becomes `Unknown`.
            pub fn from_name(name: &str) -> Self {
                match name {
                    $($text => Self::$variant,)*
                    other => Self::Unknown(other.to_owned()),
                }
            }
        }
    };
}

name_enum! {
    /// A hit result, the key of the statistics dictionaries.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitResult.cs
    HitResult {
        /// `none`.
        None => "none",
        /// `miss`.
        Miss => "miss",
        /// `meh`.
        Meh => "meh",
        /// `ok`.
        Ok => "ok",
        /// `good`.
        Good => "good",
        /// `great`.
        Great => "great",
        /// `perfect`.
        Perfect => "perfect",
        /// `small_tick_miss`.
        SmallTickMiss => "small_tick_miss",
        /// `small_tick_hit`.
        SmallTickHit => "small_tick_hit",
        /// `large_tick_miss`.
        LargeTickMiss => "large_tick_miss",
        /// `large_tick_hit`.
        LargeTickHit => "large_tick_hit",
        /// `small_bonus`.
        SmallBonus => "small_bonus",
        /// `large_bonus`.
        LargeBonus => "large_bonus",
        /// `ignore_miss`.
        IgnoreMiss => "ignore_miss",
        /// `ignore_hit`.
        IgnoreHit => "ignore_hit",
        /// `combo_break`.
        ComboBreak => "combo_break",
        /// `slider_tail_hit`.
        SliderTailHit => "slider_tail_hit",
        /// `legacy_combo_increase`.
        LegacyComboIncrease => "legacy_combo_increase",
    }
}

name_enum! {
    /// The rank of the score.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/ScoreRank.cs
    ScoreRank {
        /// `F`.
        F => "F",
        /// `D`.
        D => "D",
        /// `C`.
        C => "C",
        /// `B`.
        B => "B",
        /// `A`.
        A => "A",
        /// `S`.
        S => "S",
        /// `SH` (silver S).
        SH => "SH",
        /// `X` (SS).
        X => "X",
        /// `XH` (silver SS).
        XH => "XH",
    }
}

/// The score-info JSON of a lazer replay (`LegacyReplaySoloScoreInfo`).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyReplaySoloScoreInfo.cs
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreInfo {
    /// Online score id, -1 when unknown (`online_id`).
    pub online_id: i64,
    /// The mods (`mods`).
    pub mods: Vec<ApiMod>,
    /// Hit counts in file order (`statistics`). Lazer drops entries with a zero count when it
    /// builds the block; [`to_block`](Self::to_block) writes whatever is here, so that is the
    /// caller's job. Keys are written verbatim; duplicates collapse (last wins).
    pub statistics: Vec<(HitResult, i32)>,
    /// Maximum hit counts in file order (`maximum_statistics`).
    pub maximum_statistics: Vec<(HitResult, i32)>,
    /// Version of the client that set the score (`client_version`).
    pub client_version: String,
    /// The rank, if stored (`rank`).
    pub rank: Option<ScoreRank>,
    /// User id, -1 when unknown (`user_id`).
    pub user_id: i32,
    /// Standardised score without mod multipliers (`total_score_without_mods`).
    pub total_score_without_mods: Option<i64>,
    /// Times of pauses in milliseconds (`pauses`).
    pub pauses: Vec<i32>,
    /// Unknown top-level fields, kept verbatim and in order. Entries whose key is a known field
    /// are skipped on write.
    pub extra: Vec<(String, RawJson)>,
}

impl Default for ScoreInfo {
    fn default() -> Self {
        Self {
            online_id: -1,
            mods: Vec::new(),
            statistics: Vec::new(),
            maximum_statistics: Vec::new(),
            client_version: String::new(),
            rank: None,
            user_id: -1,
            total_score_without_mods: None,
            pauses: Vec::new(),
            extra: Vec::new(),
        }
    }
}

fn field_error(field: impl Into<String>, expected: &'static str) -> OsrError {
    OsrError::InvalidScoreInfoField {
        field: field.into(),
        expected,
    }
}

/// `None` for a missing key or `null` (the default applies).
fn present<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    obj.get(key).filter(|v| !v.is_null())
}

fn read_i64(obj: &Map<String, Value>, key: &str, default: i64) -> Result<i64, OsrError> {
    match obj.get(key) {
        None => Ok(default),
        Some(v) => v.as_i64().ok_or_else(|| field_error(key, "an integer")),
    }
}

fn to_i32(v: &Value, field: impl FnOnce() -> String) -> Result<i32, OsrError> {
    v.as_i64()
        .and_then(|i| i32::try_from(i).ok())
        .ok_or_else(|| field_error(field(), "a 32-bit integer"))
}

fn read_string(
    obj: &Map<String, Value>,
    key: &str,
    path: impl FnOnce() -> String,
) -> Result<Option<String>, OsrError> {
    match present(obj, key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(field_error(path(), "a string")),
    }
}

fn read_counts(obj: &Map<String, Value>, key: &str) -> Result<Vec<(HitResult, i32)>, OsrError> {
    match present(obj, key) {
        None => Ok(Vec::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| Ok((HitResult::from_name(k), to_i32(v, || format!("{key}.{k}"))?)))
            .collect(),
        Some(_) => Err(field_error(key, "an object")),
    }
}

fn write_counts(counts: &[(HitResult, i32)]) -> Value {
    let mut map = Map::new();
    for (result, count) in counts {
        map.insert(result.as_str().to_owned(), Value::from(*count));
    }
    Value::Object(map)
}

fn read_mod(value: &Value, index: usize) -> Result<ApiMod, OsrError> {
    let path = || format!("mods[{index}]");
    let Value::Object(obj) = value else {
        return Err(field_error(path(), "an object"));
    };
    let acronym =
        read_string(obj, "acronym", || format!("{}.acronym", path()))?.unwrap_or_default();
    let settings = match present(obj, "settings") {
        None => Vec::new(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| (k.clone(), SettingValue::from_value(v)))
            .collect(),
        Some(_) => return Err(field_error(format!("{}.settings", path()), "an object")),
    };
    let extra = obj
        .iter()
        .filter(|(k, _)| k.as_str() != "acronym" && k.as_str() != "settings")
        .map(|(k, v)| (k.clone(), RawJson::from_value(v)))
        .collect();
    Ok(ApiMod {
        acronym,
        settings,
        extra,
    })
}

fn write_mod(m: &ApiMod) -> Value {
    let mut obj = Map::new();
    obj.insert("acronym".into(), Value::String(m.acronym.clone()));
    if !m.settings.is_empty() {
        let mut settings = Map::new();
        for (k, v) in &m.settings {
            settings.insert(k.clone(), v.to_value());
        }
        obj.insert("settings".into(), Value::Object(settings));
    }
    for (k, v) in &m.extra {
        if k == "acronym" || k == "settings" {
            continue;
        }
        obj.insert(k.clone(), v.to_value());
    }
    Value::Object(obj)
}

const KNOWN_FIELDS: [&str; 9] = [
    "online_id",
    "mods",
    "statistics",
    "maximum_statistics",
    "client_version",
    "rank",
    "user_id",
    "total_score_without_mods",
    "pauses",
];

/// Escapes every non-ASCII character as `\uXXXX`. Non-ASCII only occurs inside JSON strings.
fn escape_non_ascii(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

impl ScoreInfo {
    /// Parses the JSON text of the block. See the module docs for the rules.
    pub fn from_json(json: &str) -> Result<Self, OsrError> {
        let json = json.strip_prefix('\u{feff}').unwrap_or(json);
        let value: Value = serde_json::from_str(json)
            .map_err(|e| OsrError::InvalidScoreInfoJson(e.to_string()))?;
        let Value::Object(obj) = value else {
            return Err(OsrError::InvalidScoreInfoJson(
                "the top level is not an object".into(),
            ));
        };

        let mods = match present(&obj, "mods") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .enumerate()
                .map(|(i, m)| read_mod(m, i))
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(field_error("mods", "an array")),
        };
        let rank = match present(&obj, "rank") {
            None => None,
            Some(Value::String(s)) => Some(ScoreRank::from_name(s)),
            Some(_) => return Err(field_error("rank", "a string")),
        };
        let total_score_without_mods = match present(&obj, "total_score_without_mods") {
            None => None,
            Some(v) => Some(
                v.as_i64()
                    .ok_or_else(|| field_error("total_score_without_mods", "an integer"))?,
            ),
        };
        let pauses = match present(&obj, "pauses") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .enumerate()
                .map(|(i, v)| to_i32(v, || format!("pauses[{i}]")))
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(field_error("pauses", "an array")),
        };
        let user_id = match obj.get("user_id") {
            None => -1,
            Some(v) => to_i32(v, || "user_id".into())?,
        };
        let extra = obj
            .iter()
            .filter(|(k, _)| !KNOWN_FIELDS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), RawJson::from_value(v)))
            .collect();

        Ok(Self {
            online_id: read_i64(&obj, "online_id", -1)?,
            mods,
            statistics: read_counts(&obj, "statistics")?,
            maximum_statistics: read_counts(&obj, "maximum_statistics")?,
            client_version: read_string(&obj, "client_version", || "client_version".into())?
                .unwrap_or_default(),
            rank,
            user_id,
            total_score_without_mods,
            pauses,
            extra,
        })
    }

    /// Serializes to indented JSON (2 spaces) following lazer's field order and omission
    /// rules, see the module docs. The result is pure ASCII.
    pub fn to_json(&self) -> String {
        let mut obj = Map::new();
        if self.online_id != 0 {
            obj.insert("online_id".into(), Value::from(self.online_id));
        }
        obj.insert(
            "mods".into(),
            Value::Array(self.mods.iter().map(write_mod).collect()),
        );
        obj.insert("statistics".into(), write_counts(&self.statistics));
        obj.insert(
            "maximum_statistics".into(),
            write_counts(&self.maximum_statistics),
        );
        obj.insert(
            "client_version".into(),
            Value::String(self.client_version.clone()),
        );
        if let Some(rank) = &self.rank {
            obj.insert("rank".into(), Value::String(rank.as_str().to_owned()));
        }
        if self.user_id != 0 {
            obj.insert("user_id".into(), Value::from(self.user_id));
        }
        if let Some(score) = self.total_score_without_mods {
            obj.insert("total_score_without_mods".into(), Value::from(score));
        }
        obj.insert(
            "pauses".into(),
            Value::Array(self.pauses.iter().map(|p| Value::from(*p)).collect()),
        );
        for (k, v) in &self.extra {
            if KNOWN_FIELDS.contains(&k.as_str()) {
                continue;
            }
            obj.insert(k.clone(), v.to_value());
        }
        let text = serde_json::to_string_pretty(&Value::Object(obj))
            .expect("a JSON value always serializes");
        escape_non_ascii(&text)
    }

    /// Produces the lazer block for [`Replay::lazer_block`]: [`to_json`](Self::to_json),
    /// LZMA-compressed like the frames.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreEncoder.cs
    pub fn to_block(&self) -> Vec<u8> {
        compress(self.to_json().as_bytes())
    }
}

impl Replay {
    /// Parses the lazer score-info block. `None` when there is no block or it is empty (as
    /// in lazer); an error when it cannot be decompressed or its JSON is malformed.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreDecoder.cs
    pub fn score_info(&self) -> Result<Option<ScoreInfo>, OsrError> {
        let Some(block) = self.lazer_block.as_deref().filter(|b| !b.is_empty()) else {
            return Ok(None);
        };
        let raw = decompress(block)?;
        let text =
            String::from_utf8(raw).map_err(|e| OsrError::InvalidScoreInfoJson(e.to_string()))?;
        ScoreInfo::from_json(&text).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(text: &str) -> RawJson {
        RawJson::new(text).unwrap()
    }

    fn rich() -> ScoreInfo {
        ScoreInfo {
            online_id: 123_456_789_012,
            mods: vec![
                ApiMod {
                    acronym: "DT".into(),
                    settings: vec![
                        ("speed_change".into(), SettingValue::Float(1.1)),
                        ("adjust_pitch".into(), SettingValue::Bool(true)),
                        ("direction".into(), SettingValue::Int(2)),
                        ("ratio".into(), SettingValue::Float(0.3)),
                        ("seed".into(), SettingValue::Null),
                        ("colour".into(), SettingValue::String("#ff66aa".into())),
                        ("points".into(), SettingValue::Raw(raw("[1,2.5,\"x\"]"))),
                        ("big".into(), SettingValue::Raw(raw("18446744073709551615"))),
                    ],
                    extra: vec![],
                },
                ApiMod {
                    acronym: "ZZ".into(),
                    settings: vec![("weird_setting".into(), SettingValue::Raw(raw("{\"a\":[]}")))],
                    extra: vec![("flavour".into(), raw("\"sweet\""))],
                },
            ],
            statistics: vec![
                (HitResult::Great, 500),
                (HitResult::Unknown("future_result".into()), 3),
                (HitResult::Miss, 0),
            ],
            maximum_statistics: vec![(HitResult::Great, 503)],
            client_version: "2026.1005.0 \u{00fc}\u{1f600}".into(),
            rank: Some(ScoreRank::Unknown("SS+".into())),
            user_id: 42,
            total_score_without_mods: Some(987_654),
            pauses: vec![1000, 2500],
            extra: vec![("new_field".into(), raw("{\"x\":1}"))],
        }
    }

    #[test]
    fn round_trips_through_block() {
        let info = rich();
        let block = info.to_block();
        let json = String::from_utf8(decompress(&block).unwrap()).unwrap();
        assert!(json.is_ascii());
        assert!(json.contains("\\ud83d\\ude00"));
        assert_eq!(ScoreInfo::from_json(&json).unwrap(), info);
        let replay = Replay {
            lazer_block: Some(block),
            ..Replay::default()
        };
        assert_eq!(replay.score_info().unwrap(), Some(info));
    }

    #[test]
    fn reads_lazer_style_json() {
        let json = r#"{
  "online_id": 77,
  "mods": [
    {
      "acronym": "DT",
      "settings": {
        "speed_change": 1.5,
        "adjust_pitch": false
      }
    },
    {
      "acronym": "HD"
    }
  ],
  "statistics": {
    "great": 10,
    "small_tick_hit": 2
  },
  "maximum_statistics": {
    "great": 10,
    "small_tick_hit": 2
  },
  "client_version": "2026.1005.0",
  "rank": "SH",
  "user_id": 5,
  "total_score_without_mods": 900000,
  "pauses": []
}"#;
        let info = ScoreInfo::from_json(json).unwrap();
        assert_eq!(info.online_id, 77);
        assert_eq!(info.mods.len(), 2);
        assert_eq!(info.mods[0].acronym, "DT");
        assert_eq!(
            info.mods[0].settings,
            vec![
                ("speed_change".into(), SettingValue::Float(1.5)),
                ("adjust_pitch".into(), SettingValue::Bool(false)),
            ]
        );
        assert!(info.mods[1].settings.is_empty());
        assert_eq!(
            info.statistics,
            vec![(HitResult::Great, 10), (HitResult::SmallTickHit, 2)]
        );
        assert_eq!(info.rank, Some(ScoreRank::SH));
        assert_eq!(info.user_id, 5);
        assert_eq!(info.total_score_without_mods, Some(900_000));
        assert!(info.pauses.is_empty() && info.extra.is_empty());
        // Writing it back reproduces lazer's text.
        assert_eq!(info.to_json(), json);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let info = ScoreInfo::from_json("{}").unwrap();
        assert_eq!(info, ScoreInfo::default());
        assert_eq!(info.online_id, -1);
        assert_eq!(info.user_id, -1);
        let nulls = ScoreInfo::from_json(
            r#"{"mods":null,"statistics":null,"client_version":null,"rank":null,"pauses":null}"#,
        )
        .unwrap();
        assert_eq!(nulls, ScoreInfo::default());
    }

    #[test]
    fn omits_defaults_when_writing() {
        let zero = ScoreInfo {
            online_id: 0,
            user_id: 0,
            ..ScoreInfo::default()
        };
        let json = zero.to_json();
        assert!(!json.contains("online_id") && !json.contains("user_id"));
        assert!(!json.contains("rank") && !json.contains("total_score_without_mods"));
        assert!(json.contains("\"client_version\": \"\""));
        assert!(json.contains("\"pauses\": []"));
        let minus = ScoreInfo::default().to_json();
        assert!(minus.contains("\"online_id\": -1") && minus.contains("\"user_id\": -1"));
        // Omitted zeros read back as -1 (lazer behaves the same).
        assert_eq!(ScoreInfo::from_json(&json).unwrap().online_id, -1);
    }

    #[test]
    fn hit_result_and_rank_names() {
        assert_eq!(
            HitResult::from_name("legacy_combo_increase").as_str(),
            "legacy_combo_increase"
        );
        assert_eq!(
            HitResult::from_name("Great"),
            HitResult::Unknown("Great".into())
        );
        assert_eq!(ScoreRank::from_name("XH"), ScoreRank::XH);
        assert_eq!(ScoreRank::from_name("xh"), ScoreRank::Unknown("xh".into()));
    }

    fn replay_of(json: &[u8]) -> Replay {
        Replay {
            lazer_block: Some(compress(json)),
            ..Replay::default()
        }
    }

    #[test]
    fn floats_round_trip_bit_exactly() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut settings = Vec::new();
        while settings.len() < 3000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let f = f64::from_bits(state);
            if f.is_finite() {
                settings.push((format!("k{}", settings.len()), SettingValue::Float(f)));
            }
        }
        // Values in [1, 2) exercise the 17-digit case.
        for i in 0..1000u64 {
            let bits = 0x3ff0_0000_0000_0000 | (i.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 12);
            settings.push((format!("m{i}"), SettingValue::Float(f64::from_bits(bits))));
        }
        let info = ScoreInfo {
            mods: vec![ApiMod {
                acronym: "X".into(),
                settings,
                extra: vec![],
            }],
            ..ScoreInfo::default()
        };
        let replay = Replay {
            lazer_block: Some(info.to_block()),
            ..Replay::default()
        };
        let parsed = replay.score_info().unwrap().unwrap();
        assert_eq!(info.mods[0].settings.len(), parsed.mods[0].settings.len());
        for ((ka, a), (kb, b)) in info.mods[0].settings.iter().zip(&parsed.mods[0].settings) {
            assert_eq!(ka, kb);
            match (a, b) {
                (SettingValue::Float(a), SettingValue::Float(b)) => {
                    assert_eq!(a.to_bits(), b.to_bits());
                }
                other => panic!("{other:?}"),
            }
        }
        for text in [
            "1.5018626713492373e0",
            "1.2653852959177379",
            "0.1",
            "5e-324",
            "1.7976931348623157e308",
        ] {
            let json = format!(r#"{{"mods":[{{"acronym":"A","settings":{{"v":{text}}}}}]}}"#);
            let info = ScoreInfo::from_json(&json).unwrap();
            assert_eq!(
                info.mods[0].settings[0].1,
                SettingValue::Float(text.parse::<f64>().unwrap())
            );
        }
    }

    #[test]
    fn non_finite_floats_write_as_strings() {
        let info = ScoreInfo {
            mods: vec![ApiMod {
                acronym: "X".into(),
                settings: vec![
                    ("a".into(), SettingValue::Float(f64::NAN)),
                    ("b".into(), SettingValue::Float(f64::INFINITY)),
                    ("c".into(), SettingValue::Float(f64::NEG_INFINITY)),
                ],
                extra: vec![],
            }],
            ..ScoreInfo::default()
        };
        let json = info.to_json();
        assert!(json.contains("\"a\": \"NaN\""));
        assert!(json.contains("\"b\": \"Infinity\""));
        assert!(json.contains("\"c\": \"-Infinity\""));
        let back = ScoreInfo::from_json(&json).unwrap();
        let texts: Vec<_> = back.mods[0]
            .settings
            .iter()
            .map(|(_, v)| v.clone())
            .collect();
        assert_eq!(
            texts,
            ["NaN", "Infinity", "-Infinity"].map(|t| SettingValue::String(t.into()))
        );
    }

    #[test]
    fn bom_and_invalid_utf8() {
        let info = ScoreInfo::from_json("\u{feff}{\"user_id\": 3}").unwrap();
        assert_eq!(info.user_id, 3);
        assert!(matches!(
            replay_of(b"{\"client_version\":\"\xff\"}").score_info(),
            Err(OsrError::InvalidScoreInfoJson(_))
        ));
    }

    #[test]
    fn out_of_range_ids_and_deep_nesting() {
        for (json, field) in [
            (r#"{"online_id":18446744073709551615}"#, "online_id"),
            (r#"{"user_id":2147483648}"#, "user_id"),
        ] {
            match ScoreInfo::from_json(json) {
                Err(OsrError::InvalidScoreInfoField { field: f, .. }) => assert_eq!(f, field),
                other => panic!("{other:?}"),
            }
        }
        let deep = format!(r#"{{"x":{}{}}}"#, "[".repeat(200), "]".repeat(200));
        assert!(matches!(
            ScoreInfo::from_json(&deep),
            Err(OsrError::InvalidScoreInfoJson(_))
        ));
    }

    #[test]
    fn extra_collisions_skipped_and_duplicates_last_wins() {
        let info = ScoreInfo {
            extra: vec![("user_id".into(), raw("9")), ("keep".into(), raw("1"))],
            mods: vec![ApiMod {
                acronym: "HD".into(),
                settings: vec![],
                extra: vec![
                    ("acronym".into(), raw("\"zz\"")),
                    ("settings".into(), raw("1")),
                ],
            }],
            ..ScoreInfo::default()
        };
        let back = ScoreInfo::from_json(&info.to_json()).unwrap();
        assert_eq!(back.user_id, -1);
        assert_eq!(back.extra, vec![("keep".into(), raw("1"))]);
        assert_eq!(back.mods[0].acronym, "HD");
        assert!(back.mods[0].extra.is_empty() && back.mods[0].settings.is_empty());

        let dup =
            ScoreInfo::from_json(r#"{"user_id":1,"user_id":2,"statistics":{"great":1,"great":5}}"#)
                .unwrap();
        assert_eq!(dup.user_id, 2);
        assert_eq!(dup.statistics, vec![(HitResult::Great, 5)]);
    }

    #[test]
    fn raw_json_validates() {
        assert!(RawJson::new("{ \"a\" : 1 }").is_some_and(|r| r.as_str() == "{\"a\":1}"));
        assert!(RawJson::new("{").is_none());
    }

    #[test]
    fn score_info_none_without_block() {
        let mut replay = Replay::default();
        assert_eq!(replay.score_info(), Ok(None));
        replay.lazer_block = Some(Vec::new());
        assert_eq!(replay.score_info(), Ok(None));
    }

    #[test]
    fn score_info_errors() {
        let replay = |b: Vec<u8>| Replay {
            lazer_block: Some(b),
            ..Replay::default()
        };
        assert!(matches!(
            replay(vec![1, 2, 3]).score_info(),
            Err(OsrError::CompressedTooShort)
        ));
        let mut corrupt = compress(b"{}");
        corrupt.truncate(15);
        assert!(replay(corrupt).score_info().is_err());
        assert!(matches!(
            replay(compress(b"{not json")).score_info(),
            Err(OsrError::InvalidScoreInfoJson(_))
        ));
        assert!(matches!(
            replay(compress(b"[]")).score_info(),
            Err(OsrError::InvalidScoreInfoJson(_))
        ));
        for (json, field) in [
            (r#"{"online_id":"x"}"#, "online_id"),
            (r#"{"online_id":null}"#, "online_id"),
            (r#"{"user_id":1.5}"#, "user_id"),
            (r#"{"rank":5}"#, "rank"),
            (r#"{"mods":{}}"#, "mods"),
            (r#"{"mods":[{"acronym":3}]}"#, "mods[0].acronym"),
            (r#"{"statistics":{"great":"a"}}"#, "statistics.great"),
            (r#"{"pauses":[1,"x"]}"#, "pauses[1]"),
            (r#"{"client_version":1}"#, "client_version"),
        ] {
            match ScoreInfo::from_json(json) {
                Err(OsrError::InvalidScoreInfoField { field: f, .. }) => assert_eq!(f, field),
                other => panic!("{json}: {other:?}"),
            }
        }
    }
}
