//! Data model of a decoded `.osr` replay.

use std::ops::{BitAnd, BitOr};

/// Number of .NET ticks (100 ns) per millisecond.
const TICKS_PER_MILLI: i64 = 10_000;
/// .NET ticks of the Unix epoch (1970-01-01T00:00:00Z) counted from 0001-01-01.
const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
/// Largest valid `DateTime` tick count (`DateTime.MaxValue.Ticks`).
pub(crate) const MAX_DATE_TICKS: i64 = 3_155_378_975_999_999_999;

/// Lowest version whose files carry an online score id (as an `i32`).
pub const VERSION_ONLINE_ID_I32: i32 = 20_121_008;
/// Lowest version whose online score id is an `i64`.
pub const VERSION_ONLINE_ID_I64: i32 = 20_140_721;
/// Lowest version (lazer) that appends the score-info block.
pub const VERSION_LAZER_BLOCK: i32 = 30_000_001;

/// Button bits of a replay frame (`ReplayButtonState`).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Replays/Legacy/ReplayButtonState.cs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ButtonState(pub i32);

impl ButtonState {
    /// No button pressed.
    pub const NONE: Self = Self(0);
    /// Left mouse button (M1).
    pub const LEFT1: Self = Self(1);
    /// Right mouse button (M2).
    pub const RIGHT1: Self = Self(2);
    /// Keyboard key mapped to M1 (K1).
    pub const LEFT2: Self = Self(4);
    /// Keyboard key mapped to M2 (K2).
    pub const RIGHT2: Self = Self(8);
    /// Smoke key.
    pub const SMOKE: Self = Self(16);

    /// Raw bits as stored in the file.
    pub const fn bits(self) -> i32 {
        self.0
    }

    /// Whether every bit of `other` is set in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for ButtonState {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitAnd for ButtonState {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

/// One frame exactly as stored in the frame text: a delta to the previous frame, no time
/// processing applied. Use [`lazer_timeline`](super::lazer_timeline) to get absolute times.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplayFrame {
    /// Milliseconds since the previous frame (negative values occur in real files).
    pub delta: i32,
    /// Cursor x (in mania: the pressed key bitmask in the low 20 bits).
    pub x: f32,
    /// Cursor y.
    pub y: f32,
    /// Raw button bits, see [`ButtonState`].
    pub buttons: i32,
}

impl ReplayFrame {
    /// The button bits as a [`ButtonState`].
    pub fn button_state(&self) -> ButtonState {
        ButtonState(self.buttons)
    }
}

/// The compressed frame array of a replay, keeping the distinctions needed to re-encode it.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum FrameData {
    /// The compressed array is null (length prefix -1).
    #[default]
    Null,
    /// The compressed array is present but zero bytes long.
    Empty,
    /// Compressed data, decompressed and parsed. May hold no frames (e.g. only a seed).
    Frames(Vec<ReplayFrame>),
}

impl FrameData {
    /// The parsed frames; empty for [`FrameData::Null`] and [`FrameData::Empty`].
    pub fn as_slice(&self) -> &[ReplayFrame] {
        match self {
            FrameData::Frames(f) => f,
            _ => &[],
        }
    }
}

/// A stable `.osr` replay.
///
/// Value fields are kept as stored (raw mods bitmask, raw life bar text, raw lazer block), so
/// that decoding and re-encoding loses nothing except the exact LZMA byte stream.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Replay {
    /// Game mode (0 osu, 1 taiko, 2 catch, 3 mania).
    pub mode: u8,
    /// Version stamp (`YYYYMMDD` for stable, `30000000+` for lazer).
    pub version: i32,
    /// MD5 of the beatmap file.
    pub beatmap_hash: Option<String>,
    /// Player name.
    pub player_name: Option<String>,
    /// MD5 of the replay (not verified).
    pub replay_hash: Option<String>,
    /// Number of 300s.
    pub count_300: u16,
    /// Number of 100s.
    pub count_100: u16,
    /// Number of 50s.
    pub count_50: u16,
    /// Number of gekis (max 300s in mania).
    pub count_geki: u16,
    /// Number of katus (200s in mania, 100s in catch drops).
    pub count_katu: u16,
    /// Number of misses.
    pub count_miss: u16,
    /// Total score.
    pub total_score: i32,
    /// Maximum combo.
    pub max_combo: u16,
    /// Whether the play was a full combo ("perfect").
    pub perfect: bool,
    /// Legacy mods bitmask, raw.
    pub mods: u32,
    /// Life bar graph text, raw (see [`Replay::life_bar_points`]).
    pub life_bar: Option<String>,
    /// Play date as .NET `DateTime` ticks (UTC), see [`Replay::unix_millis`].
    pub date_ticks: i64,
    /// Parsed frames.
    pub frames: FrameData,
    /// Value of the `-12345|0|0|seed` frame, if the file had one.
    pub seed: Option<i32>,
    /// Online score id as stored (raw; lazer maps 0 to -1); present only for versions >=
    /// 20121008.
    pub online_id: Option<i64>,
    /// Raw score-info block (versions >= 30000001; still compressed, not parsed). `None` is a
    /// null array.
    pub lazer_block: Option<Vec<u8>>,
    /// Everything after the known fields (e.g. stable's Target Practice accuracy double).
    pub trailing: Vec<u8>,
}

/// One point of the life bar graph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifeBarPoint {
    /// Time in milliseconds.
    pub time: i32,
    /// Health, 0..=1 in stable.
    pub hp: f64,
}

impl Replay {
    /// The parsed frames (empty if the compressed array was null or empty).
    pub fn frames(&self) -> &[ReplayFrame] {
        self.frames.as_slice()
    }

    /// Play date as Unix time in milliseconds (floored).
    pub fn unix_millis(&self) -> i64 {
        unix_millis_from_ticks(self.date_ticks)
    }

    /// Parses the life bar text (`time|hp,time|hp,...`), skipping malformed entries.
    ///
    /// Stable writes the time as an integer and the health as a double; the time is parsed as
    /// an integer first and as a float (rounded) second, health as a double. Entries with
    /// non-finite health are skipped.
    pub fn life_bar_points(&self) -> Vec<LifeBarPoint> {
        let Some(text) = &self.life_bar else {
            return Vec::new();
        };
        text.split(',')
            .filter_map(|entry| {
                let (t, h) = entry.split_once('|')?;
                let time = crate::osu::parsing::net_parse_i32(t).ok().or_else(|| {
                    let v = crate::osu::parsing::net_parse_f64(t).ok()?;
                    v.is_finite().then(|| v.round_ties_even() as i32)
                })?;
                let hp = crate::osu::parsing::net_parse_f64(h).ok()?;
                hp.is_finite().then_some(LifeBarPoint { time, hp })
            })
            .collect()
    }
}

/// Converts .NET ticks (UTC) to Unix milliseconds, flooring.
pub fn unix_millis_from_ticks(ticks: i64) -> i64 {
    ticks
        .saturating_sub(UNIX_EPOCH_TICKS)
        .div_euclid(TICKS_PER_MILLI)
}

/// Converts Unix milliseconds to .NET ticks (UTC), saturating.
pub fn ticks_from_unix_millis(millis: i64) -> i64 {
    millis
        .saturating_mul(TICKS_PER_MILLI)
        .saturating_add(UNIX_EPOCH_TICKS)
}
