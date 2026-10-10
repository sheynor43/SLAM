//! osu!standard gameplay rules: beatmap processing, sliders, hit windows, judgement, health, scoring, mods.

#![warn(missing_docs)]

pub mod beatmap;
pub mod control_points;
pub mod objects;
pub mod samples;

pub use beatmap::{Beatmap, BeatmapError, Difficulty};
