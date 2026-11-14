//! Star rating of an osu!standard beatmap without mods (ADR-0134), computed by `rosu-pp`.
//!
//! The values are used for ordering and showing difficulties in the song list. They are
//! not part of gameplay, so nothing here is deterministic-critical; cached values must be
//! discarded when [`CALCULATOR_VERSION`] changes.

use std::io;
use std::path::Path;

/// The version of the calculator behind the values; part of every cache key. Keep in sync
/// with the pinned `rosu-pp` version in the workspace manifest.
pub const CALCULATOR_VERSION: &str = "rosu-pp 4.0.1";

/// Nomod star rating of the `.osu` file `bytes`. Maps `rosu-pp` flags as too suspicious to
/// calculate are an error, so a malicious file cannot stall the caller.
pub fn stars_from_bytes(bytes: &[u8]) -> io::Result<f32> {
    let map = rosu_pp::Beatmap::from_bytes(bytes)?;
    map.check_suspicion()
        .map_err(|sus| io::Error::new(io::ErrorKind::InvalidData, format!("{sus:?}")))?;
    let stars = rosu_pp::Difficulty::new().calculate(&map).stars();
    Ok(stars as f32)
}

/// Nomod star rating of the `.osu` file at `path`.
pub fn stars_from_path(path: &Path) -> io::Result<f32> {
    stars_from_bytes(&std::fs::read(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");

    #[test]
    fn matches_rosu_pp_directly() {
        let path = format!("{DATA}hitobject-combo-offset.osu");
        let map = rosu_pp::Beatmap::from_path(&path).unwrap();
        let expected = rosu_pp::Difficulty::new().calculate(&map).stars();
        let got = f64::from(stars_from_path(Path::new(&path)).unwrap());
        assert!((got - expected).abs() < 1e-4, "{got} vs {expected}");
        assert!(got > 0.0);
    }

    #[test]
    fn garbage_gives_no_nan() {
        if let Ok(stars) = stars_from_bytes(b"not a beatmap") {
            assert!(stars.is_finite());
        }
    }

    #[test]
    fn version_matches_manifest() {
        let manifest = include_str!("../../../Cargo.toml");
        let pinned = CALCULATOR_VERSION.strip_prefix("rosu-pp ").unwrap();
        assert!(manifest.contains(&format!("rosu-pp = \"={pinned}\"")));
    }
}
