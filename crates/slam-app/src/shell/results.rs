//! Turns a finished play into what the results screen shows.

use slam_osu::judgement::HitResult;
use slam_osu::mods::{GameplayMod as _, ModSet};
use slam_osu::scoring::ScoreRank;
use slam_play::PlayResults;
use slam_ui::i18n::{FluentArgs, Localizer};
use slam_ui::results::{
    Rank, ResultsData, ResultsExtra, ResultsMods, ResultsStat, StatKind, is_early, write_fixed2,
};
use slam_ui::song_select::ScoreRow;

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
    extra: ResultsExtra,
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
        extra,
    }
}

/// The rank of a listed score from its letters (`XH`, `X`, `SH`, `S`, `A`...); an
/// unknown one reads as a fail.
pub fn rank_of_letters(letters: [u8; 2]) -> Rank {
    match &letters {
        b"XH" => Rank::XH,
        [b'X', 0] => Rank::X,
        b"SH" => Rank::SH,
        [b'S', 0] => Rank::S,
        [b'A', 0] => Rank::A,
        [b'B', 0] => Rank::B,
        [b'C', 0] => Rank::C,
        [b'D', 0] => Rank::D,
        _ => Rank::F,
    }
}

/// The results of a saved local score `row` (a click on the song select's list). What the
/// file does not keep is absent: the hit error lines, the health line and the difficulty
/// details. A score saved without its perfect play's counts shows the four basic rows
/// and takes its own combo for the highest.
pub fn score_data(
    id: u64,
    title: String,
    row: &ScoreRow,
    info: &PlayInfo,
    date: &str,
    localizer: &Localizer,
) -> ResultsData {
    let mods = ResultsMods::new(row.mods.iter());
    let known = row.maximum_combo > 0;
    let beatmap_by = if info.creator.is_empty() {
        String::new()
    } else {
        format_text(
            localizer,
            "results-beatmap-by",
            &[("mapper", info.creator.clone())],
        )
    };
    let played_by = format_text(
        localizer,
        "results-played-by",
        &[
            ("player", row.player.as_str().to_owned()),
            ("date", date.to_owned()),
        ],
    );
    let mut details = Vec::new();
    if let Some(stars) = info.stars {
        details.push(format_text(
            localizer,
            "results-details-stars",
            &[("stars", format!("{stars:.2}"))],
        ));
    }
    // The mods are in the details' table.
    ResultsData {
        id,
        title,
        score: i64::try_from(row.total_score).unwrap_or(i64::MAX),
        accuracy: row.accuracy,
        rank: rank_of_letters(row.rank),
        stats: StatKind::ALL.map(|kind| ResultsStat {
            value: row.hits[kind.index()],
            maximum: row.maximums[kind.index()],
        }),
        combo: row.max_combo,
        max_combo: if known {
            row.maximum_combo
        } else {
            row.max_combo
        },
        mods,
        average_hit_error: None,
        average_hit_error_text: None,
        unstable_rate: None,
        extra: ResultsExtra {
            beatmap_by,
            played_by,
            health: Vec::new(),
            details,
        },
    }
}

fn format_text(localizer: &Localizer, key: &str, args: &[(&str, String)]) -> String {
    let mut fluent = FluentArgs::new();
    for (name, value) in args {
        fluent.set(*name, value.clone());
    }
    localizer.format(key, Some(&fluent)).into_owned()
}

/// What a play's results say of its beatmap besides the title, known when it starts.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayInfo {
    /// The mapper; empty when not known.
    pub creator: String,
    /// The star rating with the mods, when the song select had it.
    pub stars: Option<f32>,
    /// CS, AR, OD and HP as the mods make them (lazer's adjusted display difficulty).
    pub difficulty: Option<slam_osu::beatmap::Difficulty>,
    /// The mods' speed.
    pub rate: f64,
}

impl Default for PlayInfo {
    fn default() -> Self {
        PlayInfo {
            creator: String::new(),
            stars: None,
            difficulty: None,
            rate: 1.0,
        }
    }
}

/// A number with at most two decimals and no trailing zeros: `4`, `9.33`, `1.5`.
fn short_number(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

// Ported from McOsu db2add2: src/App/Osu/OsuRankingScreen.cpp (update: the tooltip's lines)
// Ported from McOsu db2add2: src/App/Osu/OsuScore.cpp (onScoreChange: hit error averages)
/// Stable's additions to the results (ADR-0161): the header's lines, the health line and
/// the details under the panel, localised. `offsets` are the play's hit offsets in ms.
#[allow(clippy::too_many_arguments)]
pub fn extra(
    info: &PlayInfo,
    player: &str,
    date: &str,
    health: Vec<f32>,
    offsets: &[f64],
    localizer: &Localizer,
) -> ResultsExtra {
    let text = |key: &str, args: &[(&str, String)]| {
        let mut fluent = FluentArgs::new();
        for (name, value) in args {
            fluent.set(*name, value.clone());
        }
        localizer.format(key, Some(&fluent)).into_owned()
    };
    let beatmap_by = if info.creator.is_empty() {
        String::new()
    } else {
        text("results-beatmap-by", &[("mapper", info.creator.clone())])
    };
    let played_by = text(
        "results-played-by",
        &[("player", player.to_owned()), ("date", date.to_owned())],
    );
    let mut details = Vec::new();
    if let Some(stars) = info.stars {
        details.push(text(
            "results-details-stars",
            &[("stars", format!("{stars:.2}"))],
        ));
    }
    if info.rate != 1.0 {
        details.push(text(
            "results-details-speed",
            &[("speed", short_number(info.rate))],
        ));
    }
    if let Some(d) = info.difficulty {
        details.push(text(
            "results-details-difficulty",
            &[
                ("cs", short_number(f64::from(d.circle_size))),
                ("ar", short_number(f64::from(d.approach_rate))),
                ("od", short_number(f64::from(d.overall_difficulty))),
                ("hp", short_number(f64::from(d.drain_rate))),
            ],
        ));
    }
    // The mods, the average error and the unstable rate are in the details' table.
    // McOsu's "Error: min - max avg": the averages of the early and of the late hits; an
    // exact hit counts on both sides, an empty side is 0. McOsu sums whole milliseconds
    // in `float`; these are the exact offsets.
    let mean = |early: bool| {
        let (sum, count) = offsets
            .iter()
            .filter(|&&o| if early { o <= 0.0 } else { o >= 0.0 })
            .fold((0.0, 0u32), |(s, n), &o| (s + o, n + 1));
        if count > 0 {
            sum / f64::from(count)
        } else {
            0.0
        }
    };
    if !offsets.is_empty() {
        let (early, late) = (mean(true), mean(false));
        details.push(text(
            "results-details-error",
            &[
                ("early", format!("{early:.2}")),
                ("late", format!("{late:+.2}")),
            ],
        ));
    }
    ResultsExtra {
        beatmap_by,
        played_by,
        health,
        details,
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
            ResultsExtra::default(),
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
            ResultsExtra::default(),
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
        let localizer = localizer();
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

    #[test]
    fn the_extras_word_the_header_and_the_details() {
        let info = PlayInfo {
            creator: "Jounzan".into(),
            stars: Some(5.234),
            difficulty: Some(slam_osu::beatmap::Difficulty {
                circle_size: 4.0,
                approach_rate: 9.333,
                overall_difficulty: 8.5,
                drain_rate: 6.0,
                slider_multiplier: 1.4,
                slider_tick_rate: 1.0,
            }),
            rate: 1.5,
        };
        let got = extra(
            &info,
            "Guest",
            "2026-11-20 21:55:12",
            vec![1.0, 0.5],
            &[-10.0, -20.0, 4.0],
            &localizer(),
        );
        assert_eq!(got.beatmap_by, "Beatmap by Jounzan");
        assert_eq!(got.played_by, "Played by Guest on 2026-11-20 21:55:12.");
        assert_eq!(got.health, [1.0, 0.5]);
        assert_eq!(
            got.details,
            [
                "Stars: 5.23",
                "Speed: 1.5x",
                "CS 4 · AR 9.33 · OD 8.5 · HP 6",
                "Error: -15.00 ms – +4.00 ms avg",
            ]
        );
        let unknown = extra_of_unknown_mapper();
        assert_eq!(unknown, "");
        // Exact hits count on both sides; an empty side is 0 (McOsu).
        let only_early = extra(
            &PlayInfo::default(),
            "Guest",
            "now",
            Vec::new(),
            &[-10.0, 0.0],
            &localizer(),
        );
        assert_eq!(only_early.details, ["Error: -5.00 ms – +0.00 ms avg"]);
    }

    fn extra_of_unknown_mapper() -> String {
        extra(
            &PlayInfo::default(),
            "Guest",
            "now",
            Vec::new(),
            &[],
            &localizer(),
        )
        .beatmap_by
    }
}
