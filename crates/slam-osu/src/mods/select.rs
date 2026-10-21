//! Mod selection: lazer's mod types, the columns of the mod select overlay, the display order
//! and the way a new selection drops incompatible mods.

use super::{GameplayMod, Mod, ModSet};

/// Lazer's `ModType`, in its declaration order (the order [`ModSet::ordered`] sorts by).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModType {
    /// `DifficultyReduction`.
    DifficultyReduction,
    /// `DifficultyIncrease`.
    DifficultyIncrease,
    /// `Conversion`.
    Conversion,
    /// `Automation`.
    Automation,
    /// `Fun`: lazer's default for a mod that does not override `Type`.
    Fun,
    /// `System`.
    System,
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/OsuRuleset.cs (GetModsFor)
/// The acronyms of the built-in mods of a mod type, in lazer's order, with lazer's `MultiMod`s
/// flattened as the mod select overlay shows them. Mods SLAM does not implement are left out.
pub fn mods_for(ty: ModType) -> &'static [&'static str] {
    match ty {
        ModType::DifficultyReduction => &["EZ", "NF", "HT", "DC"],
        ModType::DifficultyIncrease => &["HR", "SD", "PF", "DT", "NC", "HD", "TC"],
        ModType::Conversion => &["DA", "CL", "MR"],
        ModType::Automation | ModType::Fun | ModType::System => &[],
    }
}

impl Mod {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/*.cs (Type)
    /// Lazer's type of this mod; a [`Mod::Other`] has the type lazer gives its acronym.
    pub fn mod_type(&self) -> ModType {
        use Mod::*;
        match self {
            NoFail | Easy { .. } | HalfTime(_) | Daycore { .. } => ModType::DifficultyReduction,
            HardRock
            | DoubleTime(_)
            | Nightcore { .. }
            | Hidden { .. }
            | Traceable
            | SuddenDeath { .. }
            | Perfect { .. } => ModType::DifficultyIncrease,
            Classic(_) | DifficultyAdjust(_) | Mirror { .. } => ModType::Conversion,
            Other { acronym, .. } => other_mod_type(acronym),
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/OsuRuleset.cs (GetModsFor), osu.Game/Rulesets/Mods/UnknownMod.cs
/// The type of an osu!standard mod SLAM does not implement; an acronym lazer does not know
/// becomes its `UnknownMod`, a [`ModType::System`] mod.
fn other_mod_type(acronym: &str) -> ModType {
    const TYPES: &[(ModType, &[&str])] = &[
        (ModType::DifficultyIncrease, &["FL", "BL", "ST", "AC"]),
        (ModType::Conversion, &["TP", "RD", "AL", "SG"]),
        (ModType::Automation, &["AT", "CN", "RX", "AP", "SO"]),
        (
            ModType::Fun,
            &[
                "TR", "WG", "SI", "GR", "DF", "WU", "WD", "BR", "AD", "MU", "NS", "MG", "RP", "AS",
                "FR", "BU", "SY", "DP", "BM",
            ],
        ),
        (ModType::System, &["TD", "SV2"]),
    ];
    TYPES
        .iter()
        .find(|(_, acronyms)| acronyms.iter().any(|a| a.eq_ignore_ascii_case(acronym)))
        .map_or(ModType::System, |&(ty, _)| ty)
}

/// The most mods [`ModSet::apply_selection`] tracks: the bit sets below are `u64`.
const MAX_SELECTION: usize = 63;

impl ModSet {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModExtensions.cs (AsOrdered)
    /// The mods in lazer's display order: by type, then by acronym (ordinal; lazer compares
    /// with the culture's comparer, which orders upper-case ASCII acronyms such as the
    /// built-in ones the same way). The sort is stable, as LINQ's `OrderBy`. Lazer sorts an
    /// unknown mod by its shown acronym (`XX??`), SLAM by the acronym as read. Does not
    /// allocate.
    pub fn ordered(&self) -> impl Iterator<Item = &Mod> + '_ {
        let key = |i: usize| (self.mods[i].mod_type(), self.mods[i].acronym(), i);
        let mut last: Option<usize> = None;
        std::iter::from_fn(move || {
            let next = (0..self.mods.len())
                .filter(|&i| last.is_none_or(|l| key(i) > key(l)))
                .min_by(|&a, &b| key(a).cmp(&key(b)))?;
            last = Some(next);
            Some(&self.mods[next])
        })
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Overlays/Mods/UserModSelectOverlay.cs (ComputeNewModsFromSelection)
    /// Replaces this selection with `selection` (the active mods of the overlay, in its
    /// order), the way lazer's user mod select does: every newly added mod drops the mods it
    /// conflicts with and moves to the end. Mods are compared by value, as lazer's `Except`.
    /// Does not allocate for built-in mods once the set has room for the selection.
    ///
    /// # Panics
    /// If `selection` has more than 63 mods, or two different mods with one acronym (the
    /// overlay holds one instance per mod; the result would break [`ModSet::new`]'s rule).
    pub fn apply_selection(&mut self, selection: &[Mod]) {
        assert!(selection.len() <= MAX_SELECTION, "too many mods selected");
        for (i, m) in selection.iter().enumerate() {
            assert!(
                !selection[..i]
                    .iter()
                    .any(|other| other != m && other.acronym().eq_ignore_ascii_case(m.acronym())),
                "two different mods with one acronym selected"
            );
        }
        let mut added = 0u64;
        // `newSelection.Except(oldSelection)` is distinct too.
        for (i, m) in selection.iter().enumerate() {
            if !self.mods.contains(m) && !selection[..i].contains(m) {
                added |= 1 << i;
            }
        }
        // `newSelection.Except(removedMods)`: removed mods are not in the new selection, so
        // this is the distinct new selection.
        self.mods.clear();
        for m in selection {
            if !self.mods.contains(m) {
                self.mods.push(m.clone());
            }
        }
        for (i, added_mod) in selection.iter().enumerate() {
            if added & (1 << i) == 0 {
                continue;
            }
            let invalid = invalid_mods(&self.mods, added_mod);
            let combination = |p: usize| self.mods.get(p).unwrap_or(added_mod);
            let is_invalid = |m: &Mod| {
                (0..=self.mods.len()).any(|p| invalid & (1 << p) != 0 && combination(p) == m)
            };
            let keep: u64 = (0..self.mods.len())
                .filter(|&p| !is_invalid(&self.mods[p]))
                .fold(0, |bits, p| bits | 1 << p);
            let mut p = 0;
            self.mods.retain(|_| {
                p += 1;
                keep & (1 << (p - 1)) != 0
            });
            self.mods.push(added_mod.clone());
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/ModUtils.cs (CheckCompatibleSet)
/// The positions `invalidMods` names in the combination of `mods` and then `added`: later
/// duplicates of a type and every mod another mod of the combination lists as incompatible.
/// A type is an acronym here: unlike lazer's `UnknownMod`s, two [`Mod::Other`]s with
/// different acronyms are not duplicates. Lazer skips a mod listing itself by reference
/// (`invalid == mod`), so the added mod, which is in the combination twice, flags its other
/// copy here when it lists its own type (rate mods); the duplicate check flags that copy
/// anyway, so the result is the same.
fn invalid_mods(mods: &[Mod], added: &Mod) -> u64 {
    let len = mods.len() + 1;
    let at = |p: usize| mods.get(p).unwrap_or(added);
    let mut invalid = 0u64;
    for i in 0..len {
        for j in i + 1..len {
            if at(i).acronym().eq_ignore_ascii_case(at(j).acronym()) {
                invalid |= 1 << j;
            }
        }
    }
    for i in 0..len {
        for j in 0..len {
            if i != j && at(i).lists_incompatible(at(j)) {
                invalid |= 1 << j;
            }
        }
    }
    invalid
}
