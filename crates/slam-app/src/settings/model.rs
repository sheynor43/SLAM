//! The settings model and its defaults.

use std::path::PathBuf;

use serde::{Serialize, Serializer};
use slam_audio::StreamConfig;
use slam_engine::EngineConfig;
use slam_engine::limiter::{InvalidRate, LimiterMode, MAX_HZ, MIN_HZ};
use slam_play::BackgroundSettings;
use slam_play::input::{Binding, Bindings, MAX_BINDINGS};

use super::keys;

/// The draw frame rate: unlimited or a fixed rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrawRate {
    Unlimited,
    Hz(f64),
}

impl DrawRate {
    /// The limiter mode for this rate.
    pub fn to_limiter(self) -> LimiterMode {
        match self {
            DrawRate::Unlimited => LimiterMode::Unlimited,
            DrawRate::Hz(hz) => LimiterMode::Hz(hz),
        }
    }

    /// Checks a fixed rate against the limiter's range.
    pub fn validate(self) -> Result<DrawRate, InvalidRate> {
        match self {
            DrawRate::Hz(hz) if !(MIN_HZ..=MAX_HZ).contains(&hz) => Err(InvalidRate),
            other => Ok(other),
        }
    }
}

impl From<LimiterMode> for DrawRate {
    fn from(mode: LimiterMode) -> DrawRate {
        match mode {
            LimiterMode::Unlimited => DrawRate::Unlimited,
            LimiterMode::Hz(hz) => DrawRate::Hz(hz),
        }
    }
}

impl Serialize for DrawRate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            DrawRate::Unlimited => serializer.serialize_str("unlimited"),
            DrawRate::Hz(hz) => serializer.serialize_f64(*hz),
        }
    }
}

impl DrawRate {
    /// Reads a TOML value: the string `unlimited` (any case) or a number.
    pub fn from_value(value: &toml::Value) -> Result<DrawRate, String> {
        let rate = match value {
            toml::Value::String(s) if s.eq_ignore_ascii_case("unlimited") => DrawRate::Unlimited,
            toml::Value::String(s) => {
                return Err(format!("expected \"unlimited\" or a number, got \"{s}\""));
            }
            toml::Value::Float(f) => DrawRate::Hz(*f),
            toml::Value::Integer(i) => DrawRate::Hz(*i as f64),
            other => {
                return Err(format!(
                    "expected \"unlimited\" or a number, got {}",
                    other.type_str()
                ));
            }
        };
        rate.validate().map_err(|e| format!("{e}"))
    }
}

/// Library locations.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Paths {
    /// The `Songs` directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub songs: Option<PathBuf>,
}

/// Audio output and volumes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Audio {
    /// Backend name; validated when applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Output device id; `None` follows the system default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub buffer_frames: u32,
    pub master_volume: f64,
    pub music_volume: f64,
}

impl Default for Audio {
    fn default() -> Audio {
        Audio {
            backend: None,
            device: None,
            buffer_frames: StreamConfig::default().buffer_frames,
            master_volume: 1.0,
            music_volume: 1.0,
        }
    }
}

/// Timing offsets.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Timing {
    /// Global audio offset in milliseconds, in [`Timing::OFFSET_RANGE`].
    pub global_offset_ms: f64,
}

impl Timing {
    /// Allowed global offset, milliseconds.
    pub const OFFSET_RANGE: std::ops::RangeInclusive<f64> = -500.0..=500.0;
}

/// Update and draw rates.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Frame {
    pub update_hz: f64,
    pub draw: DrawRate,
}

impl Default for Frame {
    fn default() -> Frame {
        let engine = EngineConfig::default();
        Frame {
            update_hz: engine.update_hz,
            draw: engine.draw.into(),
        }
    }
}

/// Gameplay options.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Gameplay {
    /// Background dim, 0 ..= 1.
    pub dim: f64,
    pub beatmap_colours: bool,
    pub mouse_buttons: bool,
}

impl Default for Gameplay {
    fn default() -> Gameplay {
        Gameplay {
            dim: BackgroundSettings::default().dim_level,
            beatmap_colours: true,
            mouse_buttons: true,
        }
    }
}

/// One key binding as written in the file.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BindingEntry {
    /// A key or mouse button name, see [`keys`].
    pub input: String,
    /// `left`, `right` or `smoke`.
    pub action: String,
}

impl BindingEntry {
    /// The entry for a binding.
    pub fn from_binding(binding: &Binding) -> Option<BindingEntry> {
        Some(BindingEntry {
            input: keys::input_name(binding.input),
            action: keys::action_name(binding.action)?.to_string(),
        })
    }

    /// The binding, `None` if the input or action name is unknown.
    pub fn to_binding(&self) -> Option<Binding> {
        Some(Binding {
            input: keys::input_from_name(&self.input)?,
            action: keys::action_from_name(&self.action)?,
        })
    }
}

/// Input settings.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InputSettings {
    pub bindings: Vec<BindingEntry>,
}

impl Default for InputSettings {
    fn default() -> InputSettings {
        InputSettings {
            bindings: Bindings::default()
                .as_slice()
                .iter()
                .filter_map(BindingEntry::from_binding)
                .collect(),
        }
    }
}

/// All user settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Settings {
    pub paths: Paths,
    pub audio: Audio,
    pub timing: Timing,
    pub frame: Frame,
    pub gameplay: Gameplay,
    pub input: InputSettings,
}

impl Settings {
    /// The gameplay key bindings; entries that do not parse are skipped and bindings past
    /// the capacity are dropped, both with a warning.
    pub fn bindings(&self) -> Bindings {
        let mut list = Vec::with_capacity(self.input.bindings.len());
        for entry in &self.input.bindings {
            match entry.to_binding() {
                Some(binding) => list.push(binding),
                None => tracing::warn!(
                    input = %entry.input,
                    action = %entry.action,
                    "input.bindings: unknown input or action, entry skipped"
                ),
            }
        }
        if list.len() > MAX_BINDINGS {
            tracing::warn!(
                count = list.len(),
                max = MAX_BINDINGS,
                "input.bindings: too many bindings, the rest are dropped"
            );
        }
        Bindings::new(&list)
    }
}
