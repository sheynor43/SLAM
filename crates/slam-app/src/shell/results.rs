//! Turns a finished play into what the results screen shows.

use slam_osu::judgement::HitResult;
use slam_osu::mods::{GameplayMod as _, ModSet};
use slam_osu::scoring::ScoreRank;
use slam_play::PlayResults;
use slam_ui::results::{Rank, ResultsData, ResultsMods, ResultsStat, StatKind};

/// The judgement a row counts.
pub fn hit_result(kind: StatKind) -> HitResult {
    match kind {
        StatKind::Great => HitResult::Great,
        StatKind::Ok => HitResult::Ok,
        StatKind::Meh => HitResult::Meh,
        StatKind::Miss => HitResult::Miss,
        StatKind::SliderTick => HitResult::LargeTickHit,
        StatKind::SliderEnd => HitResult::SmallTickHit,
        StatKind::SliderTail => HitResult::SliderTailHit,
        StatKind::SpinnerBonus => HitResult::LargeBonus,
        StatKind::SpinnerSpin => HitResult::SmallBonus,
    }
}

pub fn rank(rank: ScoreRank) -> Rank {
    match rank {
        ScoreRank::F => Rank::F,
        ScoreRank::D => Rank::D,
        ScoreRank::C => Rank::C,
        ScoreRank::B => Rank::B,
        ScoreRank::A => Rank::A,
        ScoreRank::S => Rank::S,
        ScoreRank::SH => Rank::SH,
        ScoreRank::X => Rank::X,
        ScoreRank::XH => Rank::XH,
    }
}

/// The beatmap line of the results: "artist - title [difficulty]".
pub fn title(artist: &str, title: &str, version: &str) -> String {
    format!("{artist} - {title} [{version}]")
}

/// The mods of the results in lazer's display order (`AsOrdered`).
pub fn mods(mods: &ModSet) -> ResultsMods {
    ResultsMods::new(mods.ordered().map(|m| m.acronym()))
}

/// The results of play `id`.
pub fn results_data(id: u64, title: String, mods: ResultsMods, r: &PlayResults) -> ResultsData {
    ResultsData {
        id,
        title,
        score: r.score,
        accuracy: r.accuracy,
        rank: rank(r.rank),
        stats: StatKind::ALL.map(|kind| {
            let result = hit_result(kind);
            ResultsStat {
                value: r.statistics.count(result),
                maximum: r.maximum_statistics.count(result),
            }
        }),
        combo: r.statistics.max_combo,
        max_combo: r.maximum_statistics.max_combo,
        mods,
    }
}

#[cfg(test)]
mod tests {
    use slam_osu::judgement::Statistics;

    use super::*;

    #[test]
    fn mods_are_shown_in_lazers_order() {
        let set = slam_osu::ModSet::new(
            ["MR", "DT", "HD", "EZ"]
                .into_iter()
                .map(slam_osu::Mod::from_acronym)
                .collect(),
        )
        .unwrap();
        let shown = mods(&set);
        let shown: Vec<&str> = shown.iter().map(|a| a.as_str()).collect();
        assert_eq!(shown, ["EZ", "DT", "HD", "MR"]);
    }

    #[test]
    fn rows_count_their_judgement_and_the_perfect_plays_maximum() {
        let mut played = Statistics::default();
        let mut perfect = Statistics::default();
        for (result, hit, all) in [
            (HitResult::Great, 3, 4),
            (HitResult::Ok, 1, 0),
            (HitResult::Miss, 0, 0),
            (HitResult::LargeTickHit, 2, 5),
            (HitResult::SliderTailHit, 1, 1),
            (HitResult::SmallBonus, 7, 9),
            (HitResult::LargeBonus, 0, 2),
        ] {
            for _ in 0..hit {
                played.apply(result);
            }
            for _ in 0..all {
                perfect.apply(result);
            }
        }
        let results = PlayResults {
            score: 123_456,
            score_without_mods: 120_000,
            accuracy: 0.9,
            rank: ScoreRank::SH,
            statistics: played,
            maximum_statistics: perfect,
        };
        let data = results_data(7, "t".into(), ResultsMods::new(["HD"]), &results);
        assert_eq!((data.id, data.score, data.rank), (7, 123_456, Rank::SH));
        assert_eq!(
            (data.combo, data.max_combo),
            (played.max_combo, perfect.max_combo)
        );
        let stat = |kind: StatKind| data.stats[kind.index()];
        assert_eq!(
            stat(StatKind::Great),
            ResultsStat {
                value: 3,
                maximum: 4
            }
        );
        assert_eq!(stat(StatKind::Ok).value, 1);
        assert_eq!(
            stat(StatKind::SliderTick),
            ResultsStat {
                value: 2,
                maximum: 5
            }
        );
        assert_eq!(
            stat(StatKind::SliderTail),
            ResultsStat {
                value: 1,
                maximum: 1
            }
        );
        assert_eq!(
            stat(StatKind::SpinnerSpin),
            ResultsStat {
                value: 7,
                maximum: 9
            }
        );
        assert_eq!(
            stat(StatKind::SpinnerBonus),
            ResultsStat {
                value: 0,
                maximum: 2
            }
        );
        // A beatmap without small ticks does not list them.
        let rows: Vec<StatKind> = data.rows().map(|(kind, _)| kind).collect();
        assert!(!rows.contains(&StatKind::SliderEnd));
        assert!(rows.contains(&StatKind::SliderTail));
        assert_eq!(data.mods.iter().count(), 1);
    }

    #[test]
    fn every_rank_maps_to_its_own() {
        let pairs = [
            (ScoreRank::F, Rank::F),
            (ScoreRank::D, Rank::D),
            (ScoreRank::C, Rank::C),
            (ScoreRank::B, Rank::B),
            (ScoreRank::A, Rank::A),
            (ScoreRank::S, Rank::S),
            (ScoreRank::SH, Rank::SH),
            (ScoreRank::X, Rank::X),
            (ScoreRank::XH, Rank::XH),
        ];
        for (from, to) in pairs {
            assert_eq!(rank(from), to);
        }
    }
}
