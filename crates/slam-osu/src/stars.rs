//! Star rating and SS performance of an osu!standard beatmap (ADR-0134, ADR-0151), computed
//! by `rosu-pp`, without mods or with the mods of a [`RatingMods`].
//!
//! The values are used for ordering and showing difficulties in the song list. They are
//! not part of gameplay, so nothing here is deterministic-critical; cached values must be
//! discarded when [`CALCULATOR_VERSION`] changes.

use std::io;
use std::path::Path;

use crate::{Mod, ModSet};

/// The version of the calculator behind the values; part of every cache key. Keep in sync
/// with the pinned `rosu-pp` version in the workspace manifest.
pub const CALCULATOR_VERSION: &str = "rosu-pp 4.0.1";

/// A beatmap's star rating and the performance of an SS on it (ADR-0151).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rating {
    /// The star rating.
    pub stars: f32,
    /// The pp of an SS: full combo at 100 % accuracy.
    pub pp: f32,
}

/// What of a mod selection changes the star rating and the performance, in a form that is
/// `Copy` and compares by [`RatingMods::key`] (ADR-0151). `Default` is no mods.
#[derive(Debug, Clone, Copy)]
pub struct RatingMods {
    /// Stable's bitmask of the mods ([`ModSet::to_legacy`]), only the bits that change the
    /// rating ([`RATING_BITS`]).
    pub legacy: u32,
    /// Mods and settings without a stable bit: [`RatingMods::CLASSIC`] and the others.
    pub flags: u8,
    /// The gameplay rate; 0 means the mods' own (1 without rate mods).
    pub rate: f64,
    /// Difficulty Adjust's circle size, approach rate, overall difficulty and drain rate;
    /// NaN where the beatmap's value is kept.
    pub adjust: [f32; 4],
}

/// The stable bits that change the star rating or the performance in `rosu-pp`: NF, EZ,
/// TD, HD, HR, DT, RX, HT, NC, FL, SO, AP. Sudden Death, Perfect, Autoplay, Cinema,
/// ScoreV2 and Target do not.
pub const RATING_BITS: u32 = 1 | 2 | 4 | 8 | 16 | 64 | 128 | 256 | 512 | 1024 | 4096 | 8192;

impl RatingMods {
    /// Classic is selected.
    pub const CLASSIC: u8 = 1;
    /// Classic without its "no slider head accuracy" setting.
    pub const CLASSIC_HEAD_ACCURACY: u8 = 2;
    /// Traceable is selected.
    pub const TRACEABLE: u8 = 4;
    /// Hidden fades only the approach circles.
    pub const HIDDEN_APPROACH_ONLY: u8 = 8;

    /// No mods.
    pub const NONE: RatingMods = RatingMods {
        legacy: 0,
        flags: 0,
        rate: 0.0,
        adjust: [f32::NAN; 4],
    };

    /// What of `mods` changes the rating. Does not allocate.
    pub fn of(mods: &ModSet) -> RatingMods {
        let mut out = RatingMods::NONE;
        out.legacy = mods.to_legacy() & RATING_BITS;
        for m in mods.mods() {
            match m {
                Mod::Classic(settings) => {
                    out.flags |= RatingMods::CLASSIC;
                    if !settings.no_slider_head_accuracy {
                        out.flags |= RatingMods::CLASSIC_HEAD_ACCURACY;
                    }
                }
                Mod::Traceable => out.flags |= RatingMods::TRACEABLE,
                Mod::Hidden {
                    only_fade_approach_circles: true,
                } => out.flags |= RatingMods::HIDDEN_APPROACH_ONLY,
                Mod::DifficultyAdjust(da) => {
                    let value = |v: Option<f32>| v.unwrap_or(f32::NAN);
                    out.adjust = [
                        value(da.circle_size),
                        value(da.approach_rate),
                        value(da.overall_difficulty),
                        value(da.drain_rate),
                    ];
                }
                _ => {}
            }
        }
        let rate = mods.true_gameplay_rate();
        if rate != 1.0 {
            out.rate = rate;
        }
        out
    }

    /// The mods of a [`RatingMods::key`].
    pub fn from_key(key: [u64; 4]) -> RatingMods {
        let low = |v: u64| f32::from_bits(v as u32);
        let high = |v: u64| f32::from_bits((v >> 32) as u32);
        RatingMods {
            legacy: key[0] as u32,
            flags: (key[0] >> 32) as u8,
            rate: f64::from_bits(key[1]),
            adjust: [low(key[2]), high(key[2]), low(key[3]), high(key[3])],
        }
    }

    /// Whether these are no mods at all, so the nomod rating applies.
    pub fn is_none(&self) -> bool {
        self.key() == RatingMods::NONE.key()
    }

    /// The fields as plain bits: equal keys mean equal ratings. NaNs compare equal here.
    pub fn key(&self) -> [u64; 4] {
        let a = |i: usize| u64::from(self.adjust[i].to_bits());
        [
            u64::from(self.legacy) | (u64::from(self.flags) << 32),
            self.rate.to_bits(),
            a(0) | (a(1) << 32),
            a(2) | (a(3) << 32),
        ]
    }

    /// The mods as `rosu-pp` takes them, with the settings that change the rating.
    fn rosu(&self) -> rosu_pp::model::mods::GameMods {
        use rosu_pp::model::mods::rosu_mods::{
            GameMod, GameModIntermode, GameMode, GameModsIntermode,
        };

        let mut set = GameModsIntermode::from_bits(self.legacy);
        if self.flags & RatingMods::CLASSIC != 0 {
            set.insert(GameModIntermode::Classic);
        }
        if self.flags & RatingMods::TRACEABLE != 0 {
            set.insert(GameModIntermode::Traceable);
        }
        let mut lazer = set.with_mode(GameMode::Osu);
        for m in lazer.iter_mut() {
            match m {
                GameMod::HiddenOsu(hidden) => {
                    hidden.only_fade_approach_circles =
                        Some(self.flags & RatingMods::HIDDEN_APPROACH_ONLY != 0);
                }
                GameMod::ClassicOsu(classic) => {
                    classic.no_slider_head_accuracy =
                        Some(self.flags & RatingMods::CLASSIC_HEAD_ACCURACY == 0);
                }
                _ => {}
            }
        }
        lazer.into()
    }
}

impl Default for RatingMods {
    fn default() -> RatingMods {
        RatingMods::NONE
    }
}

impl PartialEq for RatingMods {
    fn eq(&self, other: &RatingMods) -> bool {
        self.key() == other.key()
    }
}

impl Eq for RatingMods {}

/// Nomod star rating of the `.osu` file `bytes`. Maps `rosu-pp` flags as too suspicious to
/// calculate are an error, so a malicious file cannot stall the caller.
pub fn stars_from_bytes(bytes: &[u8]) -> io::Result<f32> {
    Ok(rating_from_bytes(bytes, &RatingMods::NONE)?.stars)
}

/// The star rating and SS performance of the `.osu` file `bytes` with `mods`, by lazer's
/// formula. Suspicious maps are an error, as in [`stars_from_bytes`].
pub fn rating_from_bytes(bytes: &[u8], mods: &RatingMods) -> io::Result<Rating> {
    let map = rosu_pp::Beatmap::from_bytes(bytes)?;
    map.check_suspicion()
        .map_err(|sus| io::Error::new(io::ErrorKind::InvalidData, format!("{sus:?}")))?;
    let mut difficulty = rosu_pp::Difficulty::new().mods(mods.rosu()).lazer(true);
    if mods.rate > 0.0 {
        difficulty = difficulty.clock_rate(mods.rate);
    }
    let [cs, ar, od, hp] = mods.adjust;
    if !cs.is_nan() {
        difficulty = difficulty.cs(cs, false);
    }
    if !ar.is_nan() {
        difficulty = difficulty.ar(ar, false);
    }
    if !od.is_nan() {
        difficulty = difficulty.od(od, false);
    }
    if !hp.is_nan() {
        difficulty = difficulty.hp(hp, false);
    }
    let attributes = difficulty.calculate(&map);
    let stars = attributes.stars();
    // No hit counts given: a full combo of 300s.
    let pp = rosu_pp::Performance::new(attributes)
        .difficulty(difficulty)
        .lazer(true)
        .calculate()
        .pp();
    Ok(Rating {
        stars: stars as f32,
        pp: pp as f32,
    })
}

/// The largest `.osu` file rated; a bigger one counts as not computable (ADR-0135).
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Nomod star rating of the `.osu` file at `path`; a file over [`MAX_FILE_BYTES`] is an
/// error.
pub fn stars_from_path(path: &Path) -> io::Result<f32> {
    Ok(rating_from_path(path, &RatingMods::NONE)?.stars)
}

/// The star rating and SS performance of the `.osu` file at `path` with `mods`; a file
/// over [`MAX_FILE_BYTES`] is an error.
pub fn rating_from_path(path: &Path, mods: &RatingMods) -> io::Result<Rating> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "beatmap file too large",
        ));
    }
    let mut bytes = Vec::new();
    // The limit holds even if the file grows while it is read.
    io::Read::read_to_end(&mut io::Read::take(file, MAX_FILE_BYTES + 1), &mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "beatmap file too large",
        ));
    }
    rating_from_bytes(&bytes, mods)
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
    fn rating_matches_rosu_pp_with_mods() {
        let path = format!("{DATA}hitobject-combo-offset.osu");
        let map = rosu_pp::Beatmap::from_path(&path).unwrap();
        let bits = 8 | 64; // HD | DT;
        let attrs = rosu_pp::Difficulty::new()
            .mods(bits)
            .lazer(true)
            .calculate(&map);
        let pp = rosu_pp::Performance::new(attrs.clone())
            .mods(bits)
            .lazer(true)
            .calculate()
            .pp();
        let mods = ModSet::from_legacy(bits);
        let got = rating_from_path(Path::new(&path), &RatingMods::of(&mods)).unwrap();
        assert!((f64::from(got.stars) - attrs.stars()).abs() < 1e-4);
        assert!((f64::from(got.pp) - pp).abs() < 1e-2, "{} vs {pp}", got.pp);
        let nomod = rating_from_path(Path::new(&path), &RatingMods::NONE).unwrap();
        assert!(got.stars > nomod.stars && got.pp > nomod.pp && nomod.pp > 0.0);
    }

    #[test]
    fn rating_mods_keep_what_changes_the_rating() {
        assert!(RatingMods::of(&ModSet::default()).is_none());
        let dt = RatingMods::of(&ModSet::from_legacy(64));
        assert!(!dt.is_none());
        assert_eq!(dt.rate, 1.5);
        let classic = RatingMods::of(&ModSet::new(vec![Mod::from_acronym("CL")]).unwrap());
        assert!(classic.flags & RatingMods::CLASSIC != 0 && !classic.is_none());
        let mut da = Mod::from_acronym("DA");
        if let Mod::DifficultyAdjust(settings) = &mut da {
            settings.approach_rate = Some(9.5);
        }
        let da = RatingMods::of(&ModSet::new(vec![da]).unwrap());
        assert_eq!(da.adjust[1], 9.5);
        assert!(da.adjust[0].is_nan());
        assert_ne!(da.key(), RatingMods::NONE.key());
        assert_eq!(RatingMods::from_key(da.key()).key(), da.key());
        let both = RatingMods::of(
            &ModSet::new(vec![Mod::from_acronym("CL"), Mod::from_acronym("HR")]).unwrap(),
        );
        assert_eq!(RatingMods::from_key(both.key()), both);
    }

    #[test]
    fn settings_without_a_stable_bit_change_the_rating() {
        let path = format!("{DATA}hitobject-combo-offset.osu");
        let rate = |mods: Vec<Mod>| {
            let mods = RatingMods::of(&ModSet::new(mods).unwrap());
            rating_from_path(Path::new(&path), &mods).unwrap()
        };
        let nomod = rate(Vec::new());
        // Traceable has no stable bit; rosu-pp gives it a bonus.
        let traceable = rate(vec![Mod::Traceable]);
        assert_ne!(traceable.pp, nomod.pp);
        let hidden = |approach_only| Mod::Hidden {
            only_fade_approach_circles: approach_only,
        };
        assert_ne!(rate(vec![hidden(true)]).pp, rate(vec![hidden(false)]).pp);
        let mods = RatingMods::of(&ModSet::new(vec![hidden(true)]).unwrap());
        assert_ne!(
            mods.key(),
            RatingMods::of(&ModSet::new(vec![hidden(false)]).unwrap()).key()
        );
        // Bits that change nothing do not leave the nomod rating.
        assert!(RatingMods::of(&ModSet::new(vec![Mod::from_acronym("SD")]).unwrap()).is_none());
    }

    #[test]
    fn difficulty_overrides_change_the_rating() {
        let path = format!("{DATA}hitobject-combo-offset.osu");
        let mut mods = RatingMods::NONE;
        mods.adjust = [7.0, 10.0, 10.0, f32::NAN];
        let high = rating_from_path(Path::new(&path), &mods).unwrap();
        let nomod = rating_from_path(Path::new(&path), &RatingMods::NONE).unwrap();
        // The map has no [Difficulty] section: every value is 5.
        assert!(
            high.stars > nomod.stars && high.pp > nomod.pp,
            "{high:?} vs {nomod:?}"
        );
    }

    #[test]
    fn oversized_files_are_not_rated() {
        let path = std::env::temp_dir().join(format!("slam-stars-big-{}.osu", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        drop(file);
        let err = stars_from_path(&path).unwrap_err();
        let _ = std::fs::remove_file(&path);
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
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
