//! Turns a finished play into what the results screen shows.

use slam_osu::judgement::HitResult;
use slam_osu::mods::{GameplayMod as _, ModSet};
use slam_osu::scoring::ScoreRank;
use slam_play::PlayResults;
use slam_ui::i18n::{FluentArgs, Localizer};
use slam_ui::results::{
    Rank, ResultsData, ResultsMods, ResultsStat, StatKind, is_early, write_fixed2,
};

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

/// The average hit error as lazer words it: "1.23 ms early" or "1.23 ms late", by the raw
/// sign of `offset`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Ranking/Statistics/AverageHitError.cs
pub fn average_hit_error_text(localizer: &Localizer, offset: f64) -> String {
    let mut ms = String::new();
    let _ = write_fixed2(&mut ms, offset.abs());
    let mut args = FluentArgs::new();
    args.set("ms", ms);
    let key = if is_early(offset) {
        "results-hit-error-early"
    } else {
        "results-hit-error-late"
    };
    localizer.format(key, Some(&args)).into_owned()
}

/// The results of play `id`.
pub fn results_data(
    id: u64,
    title: String,
    mods: ResultsMods,
    r: &PlayResults,
    localizer: &Localizer,
) -> ResultsData {
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
        average_hit_error: r.hit_errors.average,
        average_hit_error_text: r
            .hit_errors
            .average
            .map(|offset| average_hit_error_text(localizer, offset)),
        unstable_rate: r.hit_errors.unstable_rate,
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
            hit_errors: Default::default(),
        };
        let data = results_data(
            7,
            "t".into(),
            ResultsMods::new(["HD"]),
            &results,
            &localizer(),
        );
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
        // No hit offsets: nothing to show.
        assert_eq!(data.average_hit_error, None);
        assert_eq!(data.average_hit_error_text, None);
        assert_eq!(data.unstable_rate, None);
    }

    fn localizer() -> Localizer {
        Localizer::builtin().unwrap()
    }

    #[test]
    fn hit_errors_reach_the_results_with_their_text() {
        let results = PlayResults {
            score: 0,
            score_without_mods: 0,
            accuracy: 1.0,
            rank: ScoreRank::X,
            statistics: Statistics::default(),
            maximum_statistics: Statistics::default(),
            hit_errors: slam_osu::hit_events::HitErrorSummary {
                unstable_rate: Some(123.456),
                average: Some(-1.234),
                median: Some(-9.0),
                hits: 10,
            },
        };
        let data = results_data(
            1,
            "t".into(),
            ResultsMods::default(),
            &results,
            &localizer(),
        );
        assert_eq!(data.average_hit_error, Some(-1.234));
        assert_eq!(data.unstable_rate, Some(123.456));
        assert_eq!(
            data.average_hit_error_text.as_deref(),
            Some("1.23 ms early")
        );
    }

    #[test]
    fn the_average_reads_early_or_late_by_its_sign() {
        let mut localizer = localizer();
        let cases = [
            (-1.234, "1.23 ms early"),
            (1.234, "1.23 ms late"),
            (0.0, "0.00 ms late"),
            (-0.001, "0.00 ms early"),
            (-1234.567, "1,234.57 ms early"),
            (25.0, "25.00 ms late"),
        ];
        for (offset, expected) in cases {
            assert_eq!(average_hit_error_text(&localizer, offset), expected);
        }
        localizer.set_language("ru").unwrap();
        assert_eq!(average_hit_error_text(&localizer, -1.5), "1.50 мс раньше");
        assert_eq!(average_hit_error_text(&localizer, 2.0), "2.00 мс позже");
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
