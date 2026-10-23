//! A recorded play as a `.osr` replay, as lazer's `LegacyScoreEncoder` writes it.

use slam_formats::osr::{self, FrameData, GeneratedFrame, ScoreInfo, frames_from_timeline};

use crate::judgement::{HitResult, Statistics};
use crate::mods::ModSet;
use crate::scoring::ScoreRank;

use super::input::{Actions, InputFrame};

/// The replay version lazer writes (`LegacyScoreEncoder.LATEST_VERSION`, the default
/// `TotalScoreVersion` of a new score).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreEncoder.cs (LATEST_VERSION)
pub const LAZER_REPLAY_VERSION: i32 = 30_000_019;

/// The score of a finished play.
#[derive(Debug, Clone, Copy)]
pub struct PlayScore<'a> {
    /// The mods played with.
    pub mods: &'a ModSet,
    /// Counts and combo of the play.
    pub statistics: &'a Statistics,
    /// Counts of a perfect play.
    pub maximum_statistics: &'a Statistics,
    /// Total score with the mods' multiplier.
    pub total_score: i64,
    /// Total score without the mods' multiplier.
    pub total_score_without_mods: i64,
    /// The rank.
    pub rank: ScoreRank,
}

/// What a replay records besides the play.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayInfo {
    /// MD5 of the beatmap file, lowercase hex.
    pub beatmap_hash: String,
    /// The beatmap's format version: frames of versions below 5 are shifted by 24 ms.
    pub beatmap_format_version: i32,
    /// The player's name.
    pub player_name: String,
    /// When the play ended, as .NET `DateTime` ticks (UTC).
    pub date_ticks: i64,
    /// The client that recorded the play.
    pub client_version: String,
}

/// Legacy replay button bits of `actions` (`OsuReplayFrame.ToLegacy`): only the first
/// button bits, never the second ones.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Replays/OsuReplayFrame.cs (ToLegacy)
pub fn legacy_buttons(actions: Actions) -> i32 {
    // ReplayButtonState: Left1 = 1, Right1 = 2, Smoke = 16.
    let mut buttons = 0;
    if actions.contains(Actions::LEFT) {
        buttons |= 1;
    }
    if actions.contains(Actions::RIGHT) {
        buttons |= 2;
    }
    if actions.contains(Actions::SMOKE) {
        buttons |= 16;
    }
    buttons
}

/// The `.osr` replay of a play recorded as `frames`.
///
/// The replay hash is left out (lazer hashes a culture-formatted date; it is not checked on
/// import), as is the life bar graph, which lazer writes empty.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyScoreEncoder.cs (Encode)
pub fn replay_of_play(
    frames: &[InputFrame],
    score: &PlayScore<'_>,
    info: &ReplayInfo,
) -> osr::Replay {
    let timeline: Vec<GeneratedFrame> = frames
        .iter()
        .map(|f| GeneratedFrame {
            time: f.time,
            x: f.position.x,
            y: f.position.y,
            buttons: legacy_buttons(f.actions),
        })
        .collect();
    let count = |result: HitResult| wrap_u16(score.statistics.count(result));
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/ScoreInfoExtensions.cs (GetMaximumAchievableCombo)
    let maximum_combo: u32 = HitResult::ALL
        .iter()
        .filter(|r| r.affects_combo())
        .map(|&r| score.maximum_statistics.count(r))
        .sum();
    let block = ScoreInfo {
        mods: score.mods.to_api(),
        statistics: non_zero(score.statistics),
        maximum_statistics: non_zero(score.maximum_statistics),
        client_version: info.client_version.clone(),
        rank: Some(score.rank.to_score_key()),
        total_score_without_mods: (score.total_score_without_mods > 0)
            .then_some(score.total_score_without_mods),
        ..ScoreInfo::default()
    };
    osr::Replay {
        mode: 0,
        version: LAZER_REPLAY_VERSION,
        beatmap_hash: Some(info.beatmap_hash.clone()),
        player_name: Some(info.player_name.clone()),
        replay_hash: None,
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/ScoreInfoExtensions.cs (GetCount300, GetCount100, GetCount50, GetCountMiss)
        // osu!standard has no gekis or katus.
        count_300: count(HitResult::Great),
        count_100: count(HitResult::Ok),
        count_50: count(HitResult::Meh),
        count_geki: 0,
        count_katu: 0,
        count_miss: count(HitResult::Miss),
        // C#'s `(int)` and `(ushort)` casts of a long and an int wrap.
        total_score: score.total_score as i32,
        max_combo: score.statistics.max_combo as u16,
        perfect: score.statistics.max_combo == maximum_combo,
        mods: score.mods.to_legacy(),
        life_bar: Some(String::new()),
        date_ticks: info.date_ticks,
        frames: FrameData::Frames(frames_from_timeline(&timeline, info.beatmap_format_version)),
        // The end marker `-12345|0|0|0`.
        seed: Some(0),
        online_id: Some(-1),
        lazer_block: Some(block.to_block()),
        trailing: Vec::new(),
    }
}

/// `(ushort)` of a count, which wraps in C#.
fn wrap_u16(count: u32) -> u16 {
    count as u16
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/Legacy/LegacyReplaySoloScoreInfo.cs (FromScore)
/// Counts without the zeros, keyed for the score block.
fn non_zero(statistics: &Statistics) -> Vec<(osr::HitResult, i32)> {
    HitResult::ALL
        .iter()
        .map(|&r| (r, statistics.count(r)))
        .filter(|&(_, n)| n != 0)
        .map(|(r, n)| (r.to_score_key(), n as i32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::Mod;
    use slam_formats::osu::Vec2;

    fn statistics(counts: &[(HitResult, u32)], max_combo: u32) -> Statistics {
        let mut s = Statistics {
            max_combo,
            ..Statistics::default()
        };
        for &(r, n) in counts {
            s.counts[r.index()] = n;
        }
        s
    }

    fn info() -> ReplayInfo {
        ReplayInfo {
            beatmap_hash: "0123456789abcdef0123456789abcdef".into(),
            beatmap_format_version: 14,
            player_name: "player".into(),
            date_ticks: 638_000_000_000_000_000,
            client_version: "slam-test".into(),
        }
    }

    #[test]
    fn buttons_convert_to_the_first_bits() {
        assert_eq!(legacy_buttons(Actions::NONE), 0);
        assert_eq!(legacy_buttons(Actions::LEFT), 1);
        assert_eq!(legacy_buttons(Actions::RIGHT), 2);
        assert_eq!(
            legacy_buttons(Actions::LEFT | Actions::RIGHT | Actions::SMOKE),
            19
        );
        for buttons in 0..32 {
            let actions = Actions::from_legacy(buttons);
            assert_eq!(Actions::from_legacy(legacy_buttons(actions)), actions);
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Beatmaps/Formats/LegacyScoreEncoderTest.cs (TestScoreWithMissIsNotPerfect)
    #[test]
    fn a_play_with_a_miss_is_not_perfect() {
        let stats = statistics(&[(HitResult::Great, 2), (HitResult::Miss, 1)], 1);
        let max = statistics(&[(HitResult::Great, 3)], 3);
        let mods = ModSet::default();
        let score = PlayScore {
            mods: &mods,
            statistics: &stats,
            maximum_statistics: &max,
            total_score: 1000,
            total_score_without_mods: 1000,
            rank: ScoreRank::C,
        };
        let replay = replay_of_play(&[], &score, &info());
        assert!(!replay.perfect);
        let perfect = statistics(&[(HitResult::Great, 3)], 3);
        let score = PlayScore {
            statistics: &perfect,
            ..score
        };
        assert!(replay_of_play(&[], &score, &info()).perfect);
    }

    #[test]
    fn the_replay_round_trips_through_the_file() {
        let stats = statistics(
            &[
                (HitResult::Great, 7),
                (HitResult::Ok, 2),
                (HitResult::Meh, 1),
                (HitResult::Miss, 3),
                (HitResult::SliderTailHit, 4),
            ],
            9,
        );
        let max = statistics(&[(HitResult::Great, 13), (HitResult::SliderTailHit, 4)], 17);
        let mods = ModSet::new(vec![Mod::from_acronym("HD"), Mod::from_acronym("NC")]).unwrap();
        let score = PlayScore {
            mods: &mods,
            statistics: &stats,
            maximum_statistics: &max,
            total_score: 543_210,
            total_score_without_mods: 456_789,
            rank: ScoreRank::SH,
        };
        let frames = [
            InputFrame {
                time: -1000.0,
                position: Vec2::new(256.0, 192.0),
                actions: Actions::NONE,
            },
            InputFrame {
                time: 12.5,
                position: Vec2::new(100.25, 50.5),
                actions: Actions::LEFT,
            },
            InputFrame {
                time: 13.5,
                position: Vec2::new(101.0, 51.0),
                actions: Actions::LEFT | Actions::RIGHT,
            },
        ];
        let replay = replay_of_play(&frames, &score, &info());
        let decoded = osr::decode(&osr::encode(&replay)).unwrap().replay;

        assert_eq!(decoded.version, LAZER_REPLAY_VERSION);
        assert_eq!(
            decoded.beatmap_hash.as_deref(),
            Some(info().beatmap_hash.as_str())
        );
        assert_eq!(decoded.player_name.as_deref(), Some("player"));
        assert_eq!(
            (
                decoded.count_300,
                decoded.count_100,
                decoded.count_50,
                decoded.count_miss
            ),
            (7, 2, 1, 3)
        );
        assert_eq!((decoded.count_geki, decoded.count_katu), (0, 0));
        assert_eq!(decoded.total_score, 543_210);
        assert_eq!(decoded.max_combo, 9);
        assert!(!decoded.perfect);
        // HD | NC | DT.
        assert_eq!(decoded.mods, (1 << 3) | (1 << 9) | (1 << 6));
        assert_eq!(decoded.date_ticks, info().date_ticks);
        assert_eq!(decoded.seed, Some(0));

        // Times rounded half to even, as lazer writes them.
        let timeline = osr::lazer_timeline(decoded.frames(), 14);
        let got: Vec<(i64, f32, f32, i32)> = timeline
            .iter()
            .map(|f| (f.time, f.x, f.y, f.buttons))
            .collect();
        assert_eq!(
            got,
            [
                (-1000, 256.0, 192.0, 0),
                (12, 100.25, 50.5, 1),
                (14, 101.0, 51.0, 3),
            ]
        );

        let block = decoded.score_info().unwrap().unwrap();
        assert_eq!(block.mods, mods.to_api());
        assert_eq!(
            block.statistics,
            [
                (osr::HitResult::Miss, 3),
                (osr::HitResult::Meh, 1),
                (osr::HitResult::Ok, 2),
                (osr::HitResult::Great, 7),
                (osr::HitResult::SliderTailHit, 4),
            ]
        );
        assert_eq!(block.rank, Some(osr::ScoreRank::SH));
        assert_eq!(block.total_score_without_mods, Some(456_789));
        assert_eq!(block.client_version, "slam-test");
        assert_eq!((block.online_id, block.user_id), (-1, -1));
    }
}
