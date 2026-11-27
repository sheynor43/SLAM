//! Daemon data model. `Settings` keeps the daemon's JSON tree and edits only
//! the fields SLAM exposes, so everything else round-trips unchanged (ADR-0166).

use std::path::PathBuf;

use serde_json::{Map, Number, Value};

use crate::Error;

/// Output mode type paths built into OTD 0.6.x.
pub const ABSOLUTE_MODE: &str = "OpenTabletDriver.Desktop.Output.AbsoluteMode";
pub const RELATIVE_MODE: &str = "OpenTabletDriver.Desktop.Output.RelativeMode";
pub const LINUX_ARTIST_MODE: &str = "OpenTabletDriver.Desktop.Output.LinuxArtistMode";

/// Built-in output modes offered on this platform.
pub fn builtin_output_modes() -> &'static [&'static str] {
    if cfg!(target_os = "linux") {
        &[ABSOLUTE_MODE, RELATIVE_MODE, LINUX_ARTIST_MODE]
    } else {
        &[ABSOLUTE_MODE, RELATIVE_MODE]
    }
}

/// Last segment of a .NET type path (`A.B.Smoothing` → `Smoothing`).
pub fn short_type_name(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// `GetApplicationInfo` subset.
#[derive(Debug, Clone, PartialEq)]
pub struct AppInfo {
    pub version: String,
    pub settings_file: PathBuf,
}

impl AppInfo {
    pub fn from_value(v: &Value) -> Result<Self, Error> {
        Ok(Self {
            version: str_field(v, "Version")?.to_owned(),
            settings_file: PathBuf::from(str_field(v, "SettingsFile")?),
        })
    }

    /// Only the 0.6.x protocol is supported (ADR-0166).
    pub fn is_supported(&self) -> bool {
        is_supported_version(&self.version)
    }
}

/// Whether a daemon version string speaks the supported 0.6.x protocol.
pub fn is_supported_version(version: &str) -> bool {
    let mut parts = version.trim().split(['.', '-', '+']);
    parts.next() == Some("0") && parts.next() == Some("6")
}

/// A connected tablet (`TabletReference` subset).
#[derive(Debug, Clone, PartialEq)]
pub struct Tablet {
    pub name: String,
    /// Digitizer size in millimetres.
    pub width_mm: f32,
    pub height_mm: f32,
}

/// Parses the `GetTablets` result.
pub fn parse_tablets(v: &Value) -> Result<Vec<Tablet>, Error> {
    let arr = v
        .as_array()
        .ok_or_else(|| Error::Protocol("GetTablets: expected an array".into()))?;
    arr.iter()
        .map(|t| {
            let props = field(t, "Properties")?;
            let digitizer = field(field(props, "Specifications")?, "Digitizer")?;
            Ok(Tablet {
                name: str_field(props, "Name")?.to_owned(),
                width_mm: f32_field(digitizer, "Width")?,
                height_mm: f32_field(digitizer, "Height")?,
            })
        })
        .collect()
}

/// An OTD area: size and centre, rotation in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Area {
    pub width: f32,
    pub height: f32,
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
}

/// Which area of `AbsoluteModeSettings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaKind {
    Tablet,
    Display,
}

impl AreaKind {
    fn key(self) -> &'static str {
        match self {
            Self::Tablet => "Tablet",
            Self::Display => "Display",
        }
    }
}

/// Boolean flags of `AbsoluteModeSettings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsoluteFlag {
    Clipping,
    AreaLimiting,
    LockAspectRatio,
}

impl AbsoluteFlag {
    fn key(self) -> &'static str {
        match self {
            Self::Clipping => "EnableClipping",
            Self::AreaLimiting => "EnableAreaLimiting",
            Self::LockAspectRatio => "LockAspectRatio",
        }
    }
}

/// Numeric fields of `RelativeModeSettings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativeField {
    XSensitivity,
    YSensitivity,
    Rotation,
}

impl RelativeField {
    fn key(self) -> &'static str {
        match self {
            Self::XSensitivity => "XSensitivity",
            Self::YSensitivity => "YSensitivity",
            Self::Rotation => "RelativeRotation",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AbsoluteView {
    pub display: Area,
    pub tablet: Area,
    pub clipping: bool,
    pub area_limiting: bool,
    pub lock_aspect_ratio: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelativeView {
    pub x_sensitivity: f32,
    pub y_sensitivity: f32,
    pub rotation: f32,
}

/// A plugin setting value, typed by its JSON token (the daemon sends no schema).
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    /// No value stored; the plugin uses its default.
    Unset,
    /// Strings, objects and arrays: shown, not edited.
    Other(String),
}

impl SettingValue {
    fn from_json(v: Option<&Value>) -> Self {
        match v {
            None | Some(Value::Null) => Self::Unset,
            Some(Value::Bool(b)) => Self::Bool(*b),
            Some(Value::Number(n)) => match n.as_i64() {
                Some(i) if !n.is_f64() => Self::Int(i),
                _ => Self::Float(n.as_f64().unwrap_or(0.0)),
            },
            Some(Value::String(s)) => Self::Other(s.clone()),
            Some(other) => Self::Other(other.to_string()),
        }
    }

    fn to_json(&self) -> Option<Value> {
        match self {
            Self::Float(f) => Number::from_f64(*f).map(Value::Number),
            Self::Int(i) => Some(Value::from(*i)),
            Self::Bool(b) => Some(Value::Bool(*b)),
            Self::Unset | Self::Other(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilterSettingView {
    pub property: String,
    pub value: SettingValue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilterView {
    /// .NET type path of the filter.
    pub path: String,
    pub enabled: bool,
    pub settings: Vec<FilterSettingView>,
}

/// A parsed, read-only view of one profile.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileView {
    pub tablet: String,
    pub output_mode: String,
    pub absolute: Option<AbsoluteView>,
    pub relative: Option<RelativeView>,
    pub filters: Vec<FilterView>,
}

impl ProfileView {
    pub fn is_absolute(&self) -> bool {
        self.output_mode != RELATIVE_MODE
    }
}

/// The daemon's settings (`GetSettings` result) with targeted edits.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    root: Value,
}

impl Settings {
    pub fn from_value(root: Value) -> Result<Self, Error> {
        let profiles = root
            .get("Profiles")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Protocol("settings: no Profiles array".into()))?;
        for p in profiles {
            str_field(p, "Tablet")?;
        }
        Ok(Self { root })
    }

    /// The JSON tree to send back with `SetSettings`.
    pub fn as_value(&self) -> &Value {
        &self.root
    }

    /// The `settings.json` text: 2-space indent, original field order.
    pub fn to_file_json(&self) -> String {
        // Serialising a `Value` cannot fail.
        serde_json::to_string_pretty(&self.root).unwrap_or_default()
    }

    fn profiles(&self) -> &[Value] {
        // Checked in `from_value`.
        self.root["Profiles"].as_array().map_or(&[], Vec::as_slice)
    }

    /// Index of the profile for a tablet name.
    pub fn profile_index(&self, tablet: &str) -> Option<usize> {
        self.profiles()
            .iter()
            .position(|p| p.get("Tablet").and_then(Value::as_str) == Some(tablet))
    }

    pub fn profile_count(&self) -> usize {
        self.profiles().len()
    }

    pub fn profile(&self, index: usize) -> Option<ProfileView> {
        let p = self.profiles().get(index)?;
        let absolute = p
            .get("AbsoluteModeSettings")
            .filter(|v| v.is_object())
            .map(|a| AbsoluteView {
                display: read_area(a.get("Display")),
                tablet: read_area(a.get("Tablet")),
                clipping: read_bool(a, AbsoluteFlag::Clipping.key()),
                area_limiting: read_bool(a, AbsoluteFlag::AreaLimiting.key()),
                lock_aspect_ratio: read_bool(a, AbsoluteFlag::LockAspectRatio.key()),
            });
        let relative = p
            .get("RelativeModeSettings")
            .filter(|v| v.is_object())
            .map(|r| RelativeView {
                x_sensitivity: read_f32(r, RelativeField::XSensitivity.key()),
                y_sensitivity: read_f32(r, RelativeField::YSensitivity.key()),
                rotation: read_f32(r, RelativeField::Rotation.key()),
            });
        let filters = p
            .get("Filters")
            .and_then(Value::as_array)
            .map(|fs| {
                fs.iter()
                    .map(|f| FilterView {
                        path: f
                            .get("Path")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        enabled: read_bool(f, "Enable"),
                        settings: f
                            .get("Settings")
                            .and_then(Value::as_array)
                            .map(|ss| {
                                ss.iter()
                                    .map(|s| FilterSettingView {
                                        property: s
                                            .get("Property")
                                            .and_then(Value::as_str)
                                            .unwrap_or("")
                                            .to_owned(),
                                        value: SettingValue::from_json(s.get("Value")),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(ProfileView {
            tablet: p
                .get("Tablet")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            output_mode: p
                .get("OutputMode")
                .and_then(|o| o.get("Path"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            absolute,
            relative,
            filters,
        })
    }

    /// The tablet-behaviour part of a profile as a preset (ADR-0176): output mode, absolute
    /// and relative settings and filters, without the tablet's name, bindings or anything
    /// else of the profile.
    pub fn profile_preset(&self, index: usize) -> Option<Value> {
        let p = self.profiles().get(index)?.as_object()?;
        let mut out = Map::new();
        for key in PRESET_KEYS {
            if let Some(v) = p.get(key) {
                out.insert(key.to_owned(), v.clone());
            }
        }
        Some(Value::Object(out))
    }

    /// Puts a preset into a profile. Output mode and the absolute and relative settings
    /// replace the profile's when the preset has them. Filters merge by path: a filter the
    /// profile has takes the preset's enable flag and the settings it has by name; filters
    /// the profile lacks are skipped (their plugin may not be installed) and counted in
    /// the result, and filters the preset lacks stay as they are.
    pub fn apply_profile_preset(&mut self, index: usize, preset: &Value) -> Result<usize, Error> {
        let preset = preset.as_object().ok_or(Error::NoSuchItem)?;
        let profile = self.profile_mut(index)?;
        for key in ["OutputMode", "AbsoluteModeSettings", "RelativeModeSettings"] {
            if let Some(v) = preset.get(key) {
                profile.insert(key.to_owned(), v.clone());
            }
        }
        let mut skipped = 0;
        let wanted = preset.get("Filters").and_then(Value::as_array);
        let mut ours = profile.get_mut("Filters").and_then(Value::as_array_mut);
        if let Some(wanted) = wanted {
            for want in wanted {
                let path = want.get("Path").and_then(Value::as_str);
                let found = ours.as_deref().and_then(|fs| {
                    fs.iter()
                        .position(|f| f.get("Path").and_then(Value::as_str) == path)
                });
                let (Some(at), Some(ours)) = (found, ours.as_deref_mut()) else {
                    skipped += 1;
                    continue;
                };
                merge_filter(&mut ours[at], want);
            }
        } else {
            return Ok(0);
        }
        Ok(skipped)
    }

    fn profile_mut(&mut self, index: usize) -> Result<&mut Map<String, Value>, Error> {
        self.root["Profiles"]
            .as_array_mut()
            .and_then(|ps| ps.get_mut(index))
            .and_then(Value::as_object_mut)
            .ok_or(Error::NoSuchItem)
    }

    fn absolute_mut(&mut self, index: usize) -> Result<&mut Map<String, Value>, Error> {
        self.profile_mut(index)?
            .get_mut("AbsoluteModeSettings")
            .and_then(Value::as_object_mut)
            .ok_or(Error::NoSuchItem)
    }

    pub fn set_area(&mut self, index: usize, kind: AreaKind, area: Area) -> Result<(), Error> {
        let abs = self.absolute_mut(index)?;
        let obj = abs
            .entry(kind.key())
            .or_insert_with(|| Value::Object(Map::new()));
        let obj = obj.as_object_mut().ok_or(Error::NoSuchItem)?;
        for (key, v) in [
            ("Width", area.width),
            ("Height", area.height),
            ("X", area.x),
            ("Y", area.y),
            ("Rotation", area.rotation),
        ] {
            obj.insert(key.into(), f32_json(v));
        }
        Ok(())
    }

    pub fn set_absolute_flag(
        &mut self,
        index: usize,
        flag: AbsoluteFlag,
        on: bool,
    ) -> Result<(), Error> {
        self.absolute_mut(index)?
            .insert(flag.key().into(), Value::Bool(on));
        Ok(())
    }

    pub fn set_relative(
        &mut self,
        index: usize,
        field: RelativeField,
        value: f32,
    ) -> Result<(), Error> {
        self.profile_mut(index)?
            .get_mut("RelativeModeSettings")
            .and_then(Value::as_object_mut)
            .ok_or(Error::NoSuchItem)?
            .insert(field.key().into(), f32_json(value));
        Ok(())
    }

    /// Switches the output mode type. Its own plugin settings are reset, as in
    /// OTD's UX when another mode is chosen.
    pub fn set_output_mode(&mut self, index: usize, path: &str) -> Result<(), Error> {
        let profile = self.profile_mut(index)?;
        let current = profile
            .get("OutputMode")
            .and_then(|o| o.get("Path"))
            .and_then(Value::as_str);
        if current == Some(path) {
            return Ok(());
        }
        let mut mode = Map::new();
        mode.insert("Path".into(), Value::String(path.to_owned()));
        mode.insert("Settings".into(), Value::Array(Vec::new()));
        mode.insert("Enable".into(), Value::Bool(true));
        profile.insert("OutputMode".into(), Value::Object(mode));
        Ok(())
    }

    fn filter_mut(
        &mut self,
        index: usize,
        filter: usize,
    ) -> Result<&mut Map<String, Value>, Error> {
        self.profile_mut(index)?
            .get_mut("Filters")
            .and_then(Value::as_array_mut)
            .and_then(|fs| fs.get_mut(filter))
            .and_then(Value::as_object_mut)
            .ok_or(Error::NoSuchItem)
    }

    pub fn set_filter_enabled(
        &mut self,
        index: usize,
        filter: usize,
        on: bool,
    ) -> Result<(), Error> {
        self.filter_mut(index, filter)?
            .insert("Enable".into(), Value::Bool(on));
        Ok(())
    }

    /// Sets a filter setting value. Only numbers and flags are editable, and
    /// the value keeps the JSON type the setting already has.
    pub fn set_filter_value(
        &mut self,
        index: usize,
        filter: usize,
        setting: usize,
        value: SettingValue,
    ) -> Result<(), Error> {
        let entry = self
            .filter_mut(index, filter)?
            .get_mut("Settings")
            .and_then(Value::as_array_mut)
            .and_then(|ss| ss.get_mut(setting))
            .and_then(Value::as_object_mut)
            .ok_or(Error::NoSuchItem)?;
        let current = SettingValue::from_json(entry.get("Value"));
        let same_kind = matches!(
            (&current, &value),
            (SettingValue::Float(_), SettingValue::Float(_))
                | (SettingValue::Int(_), SettingValue::Int(_))
                | (SettingValue::Bool(_), SettingValue::Bool(_))
        );
        if !same_kind {
            return Err(Error::WrongValueType);
        }
        let json = value.to_json().ok_or(Error::WrongValueType)?;
        entry.insert("Value".into(), json);
        Ok(())
    }
}

/// The profile keys a preset keeps.
const PRESET_KEYS: [&str; 4] = [
    "OutputMode",
    "AbsoluteModeSettings",
    "RelativeModeSettings",
    "Filters",
];

/// Copies the enable flag and the settings of `want` into `ours` (a filter of the same
/// path); settings `ours` has no entry for are left out.
fn merge_filter(ours: &mut Value, want: &Value) {
    let Some(ours) = ours.as_object_mut() else {
        return;
    };
    if let Some(on) = want.get("Enable") {
        ours.insert("Enable".into(), on.clone());
    }
    let Some(wanted) = want.get("Settings").and_then(Value::as_array) else {
        return;
    };
    let Some(settings) = ours.get_mut("Settings").and_then(Value::as_array_mut) else {
        return;
    };
    for entry in settings {
        let name = entry.get("Property").and_then(Value::as_str);
        let value = wanted
            .iter()
            .find(|w| w.get("Property").and_then(Value::as_str) == name)
            .and_then(|w| w.get("Value"));
        if let (Some(value), Some(entry)) = (value, entry.as_object_mut()) {
            entry.insert("Value".into(), value.clone());
        }
    }
}

/// `f32` as the shortest decimal that round-trips (`0.1f32` → `0.1`, not
/// `0.10000000149011612`), matching how Newtonsoft writes `float`.
fn f32_json(v: f32) -> Value {
    let v = if v.is_finite() { v } else { 0.0 };
    let shortest: f64 = format!("{v}").parse().unwrap_or(f64::from(v));
    Number::from_f64(shortest).map_or(Value::from(0), Value::Number)
}

fn read_area(v: Option<&Value>) -> Area {
    let Some(v) = v else {
        return Area::default();
    };
    Area {
        width: read_f32(v, "Width"),
        height: read_f32(v, "Height"),
        x: read_f32(v, "X"),
        y: read_f32(v, "Y"),
        rotation: read_f32(v, "Rotation"),
    }
}

fn read_f32(v: &Value, key: &str) -> f32 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0) as f32
}

fn read_bool(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a Value, Error> {
    v.get(key)
        .ok_or_else(|| Error::Protocol(format!("missing field {key}")))
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str, Error> {
    field(v, key)?
        .as_str()
        .ok_or_else(|| Error::Protocol(format!("field {key} is not a string")))
}

fn f32_field(v: &Value, key: &str) -> Result<f32, Error> {
    field(v, key)?
        .as_f64()
        .map(|f| f as f32)
        .ok_or_else(|| Error::Protocol(format!("field {key} is not a number")))
}
