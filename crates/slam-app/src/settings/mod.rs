//! User settings: the TOML file, its model, key names and the data directories (ADR-0013).
//!
//! Loading never fails: a missing or broken file yields defaults, and an invalid field falls
//! back to its default on its own, with a warning naming it.

pub mod dirs;
pub mod file;
pub mod keys;
pub mod model;
pub mod saver;

pub use dirs::{Dirs, Os};
pub use file::{CURRENT_VERSION, LoadStatus, Loaded, load, load_or_create, save};
pub use model::{
    Audio, BINDING_SLOTS, BindingEntry, DrawRate, Frame, Gameplay, InputSettings, Paths, Settings,
    Timing,
};

/// File name of the settings file inside the config directory.
pub const FILE_NAME: &str = "slam.toml";

#[cfg(test)]
mod tests;
