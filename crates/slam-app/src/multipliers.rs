//! Score multipliers shown in the song select: of one mod, of the selection and of a saved
//! score's mods. The numbers come from `slam-osu`; the interface gets plain floats
//! (ADR-0032).

use slam_osu::mods::{MultiplierContext, MultiplierVersion};
use slam_osu::{Difficulty, GameplayMod, Mod, ModSet};

/// The difficulty a multiplier is computed against when the beatmap is not known: only
/// Difficulty Adjust looks at it.
pub const NEUTRAL_DIFFICULTY: Difficulty = Difficulty {
    drain_rate: 5.0,
    circle_size: 5.0,
    overall_difficulty: 5.0,
    approach_rate: 5.0,
    slider_multiplier: 1.4,
    slider_tick_rate: 1.0,
};

/// The difficulty of a scanned file, for the multiplier of Difficulty Adjust.
pub fn difficulty_of(entry: &crate::songs::SongFile) -> Difficulty {
    Difficulty {
        drain_rate: entry.drain_rate,
        circle_size: entry.circle_size,
        overall_difficulty: entry.overall_difficulty,
        approach_rate: entry.approach_rate,
        ..NEUTRAL_DIFFICULTY
    }
}

/// The mod of a tile with its default settings, made once so that the update tick does not
/// build mods (building one allocates).
pub fn default_mod(acronym: &str) -> &'static Mod {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static MODS: OnceLock<HashMap<&'static str, Mod>> = OnceLock::new();
    let mods = MODS.get_or_init(|| {
        crate::shell::mod_columns()
            .iter()
            .flat_map(|column| column.iter())
            .map(|&acronym| (acronym, Mod::from_acronym(acronym)))
            .collect()
    });
    mods.get(acronym).unwrap_or(&NO_MOD)
}

static NO_MOD: Mod = Mod::NoFail;

fn context(difficulty: &Difficulty) -> MultiplierContext<'_> {
    MultiplierContext {
        version: MultiplierVersion::for_total_score_version(None),
        difficulty_without_mods: difficulty,
    }
}

/// The multiplier of one mod, with its settings.
pub fn mod_multiplier(m: &Mod, difficulty: &Difficulty) -> f64 {
    m.score_multiplier(&context(difficulty))
}

/// The multiplier of a selection.
pub fn set_multiplier(mods: &ModSet, difficulty: &Difficulty) -> f64 {
    mods.score_multiplier(&context(difficulty))
}

/// The multiplier of a saved score's mods, by acronym with their default settings (the
/// records keep only the acronyms); 1 when the set is not valid.
pub fn acronyms_multiplier<'a>(acronyms: impl IntoIterator<Item = &'a str>) -> f64 {
    let mods = acronyms.into_iter().map(Mod::from_acronym).collect();
    ModSet::new(mods).map_or(1.0, |set| set_multiplier(&set, &NEUTRAL_DIFFICULTY))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_mods_score_one_and_double_time_more() {
        assert_eq!(acronyms_multiplier([]), 1.0);
        assert!(acronyms_multiplier(["DT"]) > 1.0);
        assert!(acronyms_multiplier(["EZ"]) < 1.0);
        let both = acronyms_multiplier(["HD", "DT"]);
        assert_eq!(
            both,
            acronyms_multiplier(["HD"]) * acronyms_multiplier(["DT"])
        );
    }

    #[test]
    fn duplicate_acronyms_count_as_one() {
        assert_eq!(acronyms_multiplier(["DT", "DT"]), 1.0);
    }

    #[test]
    fn a_tile_shows_its_mods_own_multiplier() {
        let dt = Mod::from_acronym("DT");
        assert_eq!(
            mod_multiplier(&dt, &NEUTRAL_DIFFICULTY),
            acronyms_multiplier(["DT"])
        );
    }
}
