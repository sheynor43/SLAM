//! Client for the OpenTabletDriver 0.6.x daemon (ADR-0166): reads tablets and
//! settings and applies edits over the daemon's JSON-RPC pipe. All I/O runs on
//! a background worker; the UI talks to it through non-blocking channels.

pub mod framing;
pub mod model;
pub mod rpc;
pub mod transport;
pub mod worker;

pub use model::{
    AbsoluteFlag, AbsoluteView, AppInfo, Area, AreaKind, FilterSettingView, FilterView,
    ProfileView, RelativeField, RelativeView, SettingValue, Settings, Tablet, builtin_output_modes,
    short_type_name,
};
pub use worker::{Command, Event, OtdService, Snapshot};

/// Errors of daemon operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("daemon error {code}: {message}")]
    Remote { code: i64, message: String },
    #[error("no such profile, filter or setting")]
    NoSuchItem,
    #[error("value type does not match the setting")]
    WrongValueType,
}
