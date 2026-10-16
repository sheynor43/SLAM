//! Reading and writing `slam.toml`.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use slam_engine::MAX_UPDATE_HZ;
use slam_engine::limiter::MIN_HZ;
use toml::{Table, Value};

use super::model::{BindingEntry, DrawRate, Settings, Timing};

/// The schema version this build writes.
pub const CURRENT_VERSION: i64 = 1;

/// How loading went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadStatus {
    /// No file; defaults.
    Missing,
    /// Parsed without a complaint.
    Ok,
    /// Parsed, but some fields were invalid or unknown and fell back to defaults.
    Recovered,
    /// Not TOML at all; defaults. The file must not be overwritten.
    Broken,
}

/// The result of [`load`].
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub settings: Settings,
    /// One line per problem, naming the field.
    pub warnings: Vec<String>,
    pub status: LoadStatus,
}

/// Upgrades a table from schema version `from_version` to [`CURRENT_VERSION`]. Version 1 is
/// the first, so there is nothing to do yet.
pub fn migrate(_table: &mut Table, _from_version: i64) {}

/// Reads settings from `path`; never fails.
pub fn load(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Loaded {
                settings: Settings::default(),
                warnings: Vec::new(),
                status: LoadStatus::Missing,
            };
        }
        Err(e) => return broken(format!("cannot read {}: {e}", path.display())),
    };
    match text.parse::<Table>() {
        Ok(table) => from_table(table),
        Err(e) => broken(format!("{} is not valid TOML: {e}", path.display())),
    }
}

fn broken(warning: String) -> Loaded {
    tracing::warn!("{warning}");
    Loaded {
        settings: Settings::default(),
        warnings: vec![warning],
        status: LoadStatus::Broken,
    }
}

/// Parses settings from TOML text; see [`load`] for the rules.
pub fn from_toml_str(text: &str) -> Loaded {
    match text.parse::<Table>() {
        Ok(table) => from_table(table),
        Err(e) => broken(format!("not valid TOML: {e}")),
    }
}

struct Ctx {
    warnings: Vec<String>,
}

impl Ctx {
    fn warn(&mut self, message: String) {
        tracing::warn!("settings: {message}");
        self.warnings.push(message);
    }

    /// Takes a sub-table; reports unknown keys left in it.
    fn section(&mut self, table: &mut Table, name: &str) -> Table {
        match table.remove(name) {
            None => Table::new(),
            Some(Value::Table(t)) => t,
            Some(other) => {
                self.warn(format!(
                    "{name}: expected a table, got {}",
                    other.type_str()
                ));
                Table::new()
            }
        }
    }

    fn unknown(&mut self, table: &Table, section: &str) {
        for key in table.keys() {
            let name = if section.is_empty() {
                key.clone()
            } else {
                format!("{section}.{key}")
            };
            self.warn(format!("{name}: unknown key, ignored"));
        }
    }

    /// Reads field `key` into `slot`; keeps `slot` and warns when invalid.
    fn field<T: DeserializeOwned>(
        &mut self,
        table: &mut Table,
        section: &str,
        key: &str,
        slot: &mut T,
        check: impl Fn(&T) -> Result<(), String>,
    ) {
        let Some(value) = table.remove(key) else {
            return;
        };
        let name = format!("{section}.{key}");
        match value.try_into::<T>() {
            Ok(v) => match check(&v) {
                Ok(()) => *slot = v,
                Err(reason) => self.warn(format!("{name}: {reason}, using the default")),
            },
            Err(e) => self.warn(format!("{name}: {}, using the default", e.message())),
        }
    }

    /// Like [`Ctx::field`] for an optional string or path.
    fn opt<T: DeserializeOwned>(
        &mut self,
        table: &mut Table,
        section: &str,
        key: &str,
        slot: &mut Option<T>,
    ) {
        let Some(value) = table.remove(key) else {
            return;
        };
        match value.try_into::<T>() {
            Ok(v) => *slot = Some(v),
            Err(e) => self.warn(format!(
                "{section}.{key}: {}, using the default",
                e.message()
            )),
        }
    }

    /// Reads a float field that may be written as an integer.
    fn float(
        &mut self,
        table: &mut Table,
        section: &str,
        key: &str,
        slot: &mut f64,
        range: std::ops::RangeInclusive<f64>,
    ) {
        if let Some(Value::Integer(i)) = table.get(key) {
            let f = Value::Float(*i as f64);
            table.insert(key.to_string(), f);
        }
        self.field(table, section, key, slot, |v: &f64| {
            if range.contains(v) {
                Ok(())
            } else {
                Err(format!(
                    "{v} is outside [{}, {}]",
                    range.start(),
                    range.end()
                ))
            }
        });
    }
}

fn from_table(mut table: Table) -> Loaded {
    let mut cx = Ctx {
        warnings: Vec::new(),
    };
    let mut settings = Settings::default();

    let mut version = 1;
    if let Some(value) = table.remove("version") {
        match value {
            Value::Integer(v) if v >= 1 => version = v,
            other => cx.warn(format!(
                "version: expected a positive integer, got {other}, assuming 1"
            )),
        }
    }
    if version > CURRENT_VERSION {
        cx.warn(format!(
            "version: {version} is newer than {CURRENT_VERSION}; reading what is understood"
        ));
    } else {
        migrate(&mut table, version);
    }

    let mut paths = cx.section(&mut table, "paths");
    cx.opt(&mut paths, "paths", "songs", &mut settings.paths.songs);
    cx.unknown(&paths, "paths");

    let mut audio = cx.section(&mut table, "audio");
    cx.opt(&mut audio, "audio", "backend", &mut settings.audio.backend);
    cx.opt(&mut audio, "audio", "device", &mut settings.audio.device);
    cx.field(
        &mut audio,
        "audio",
        "buffer_frames",
        &mut settings.audio.buffer_frames,
        |v: &u32| {
            if *v >= 1 {
                Ok(())
            } else {
                Err("must be at least 1".to_string())
            }
        },
    );
    cx.float(
        &mut audio,
        "audio",
        "master_volume",
        &mut settings.audio.master_volume,
        0.0..=1.0,
    );
    cx.float(
        &mut audio,
        "audio",
        "music_volume",
        &mut settings.audio.music_volume,
        0.0..=1.0,
    );
    cx.float(
        &mut audio,
        "audio",
        "hitsound_volume",
        &mut settings.audio.hitsound_volume,
        0.0..=1.0,
    );
    cx.float(
        &mut audio,
        "audio",
        "effects_volume",
        &mut settings.audio.effects_volume,
        0.0..=1.0,
    );
    cx.unknown(&audio, "audio");

    let mut timing = cx.section(&mut table, "timing");
    cx.float(
        &mut timing,
        "timing",
        "global_offset_ms",
        &mut settings.timing.global_offset_ms,
        Timing::OFFSET_RANGE,
    );
    cx.unknown(&timing, "timing");

    let mut frame = cx.section(&mut table, "frame");
    cx.float(
        &mut frame,
        "frame",
        "update_hz",
        &mut settings.frame.update_hz,
        MIN_HZ..=MAX_UPDATE_HZ,
    );
    if let Some(value) = frame.remove("draw") {
        match DrawRate::from_value(&value) {
            Ok(rate) => settings.frame.draw = rate,
            Err(e) => cx.warn(format!("frame.draw: {e}, using the default")),
        }
    }
    cx.unknown(&frame, "frame");

    let mut gameplay = cx.section(&mut table, "gameplay");
    cx.float(
        &mut gameplay,
        "gameplay",
        "dim",
        &mut settings.gameplay.dim,
        0.0..=1.0,
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "beatmap_colours",
        &mut settings.gameplay.beatmap_colours,
        |_| Ok(()),
    );
    cx.field(
        &mut gameplay,
        "gameplay",
        "mouse_buttons",
        &mut settings.gameplay.mouse_buttons,
        |_| Ok(()),
    );
    cx.unknown(&gameplay, "gameplay");

    let mut input = cx.section(&mut table, "input");
    if let Some(value) = input.remove("bindings") {
        match value {
            Value::Array(items) => {
                let mut entries = Vec::new();
                let mut skipped = 0;
                for (i, item) in items.into_iter().enumerate() {
                    let name = format!("input.bindings[{i}]");
                    match item.try_into::<RawBinding>() {
                        Ok(raw) => {
                            let entry = BindingEntry {
                                input: raw.input,
                                action: raw.action,
                            };
                            if entry.to_binding().is_some() {
                                entries.push(entry);
                            } else {
                                skipped += 1;
                                cx.warn(format!(
                                    "{name}: unknown input \"{}\" or action \"{}\", entry skipped",
                                    entry.input, entry.action
                                ));
                            }
                        }
                        Err(e) => {
                            skipped += 1;
                            cx.warn(format!("{name}: {}, entry skipped", e.message()));
                        }
                    }
                }
                if entries.is_empty() && skipped > 0 {
                    cx.warn("input.bindings: no valid entry, using the defaults".to_string());
                } else {
                    settings.input.bindings = entries;
                }
            }
            other => cx.warn(format!(
                "input.bindings: expected an array, got {}, using the default",
                other.type_str()
            )),
        }
    }
    cx.unknown(&input, "input");

    cx.unknown(&table, "");

    let status = if cx.warnings.is_empty() {
        LoadStatus::Ok
    } else {
        LoadStatus::Recovered
    };
    Loaded {
        settings,
        warnings: cx.warnings,
        status,
    }
}

#[derive(serde::Deserialize)]
struct RawBinding {
    input: String,
    action: String,
}

/// Serializes settings with the `version` key first.
pub fn to_toml_string(settings: &Settings) -> Result<String, toml::ser::Error> {
    #[derive(serde::Serialize)]
    struct File<'a> {
        version: i64,
        #[serde(flatten)]
        settings: &'a Settings,
    }
    toml::to_string(&File {
        version: CURRENT_VERSION,
        settings,
    })
}

/// Writes settings atomically: a temporary file in the same directory, flushed to disk,
/// then renamed over `path`.
pub fn save(path: &Path, settings: &Settings) -> io::Result<()> {
    let text = to_toml_string(settings).map_err(io::Error::other)?;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp: PathBuf = path.to_path_buf();
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    tmp.set_file_name(name);
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Loads settings; when the file is missing, writes the defaults. A broken or recovered
/// file is never overwritten.
pub fn load_or_create(path: &Path) -> Loaded {
    let loaded = load(path);
    if loaded.status == LoadStatus::Missing
        && let Err(e) = save(path, &loaded.settings)
    {
        tracing::warn!("settings: cannot write {}: {e}", path.display());
    }
    loaded
}
