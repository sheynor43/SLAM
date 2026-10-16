//! The results screen: its update-side state and layout, input-agnostic (the application
//! maps its events to [`ResultsInput`]), and the pure functions that turn numbers into the
//! texts lazer shows. The draw side is [`view::ResultsView`].
//!
//! `slam-ui` does not know the rules crate, so the application converts a finished play
//! into [`ResultsData`] (counts per kind of judgement, rank, mods).

use std::fmt;
use std::sync::Arc;

use crate::menu::Rect;
use crate::theme::Theme;

pub mod view;

pub use view::{ResultsLabels, ResultsView};

/// The longest mod list shown; further mods are dropped.
pub const MAX_MODS: usize = 8;
/// Letters of a mod acronym (lazer's are at most three).
pub const ACRONYM_LEN: usize = 3;

/// A mod's acronym, an ASCII string of up to [`ACRONYM_LEN`] letters; `Copy`, no heap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModAcronym([u8; ACRONYM_LEN]);

impl ModAcronym {
    /// The first [`ACRONYM_LEN`] ASCII letters of `text`; others are skipped.
    pub fn new(text: &str) -> ModAcronym {
        let mut bytes = [0; ACRONYM_LEN];
        for (slot, byte) in bytes.iter_mut().zip(text.bytes().filter(u8::is_ascii)) {
            *slot = byte;
        }
        ModAcronym(bytes)
    }

    pub fn as_str(&self) -> &str {
        let len = self.0.iter().position(|b| *b == 0).unwrap_or(ACRONYM_LEN);
        // The bytes are ASCII by construction.
        std::str::from_utf8(&self.0[..len]).unwrap_or("")
    }
}

/// The mods of a play, in the order they are shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResultsMods {
    acronyms: [ModAcronym; MAX_MODS],
    len: u8,
}

impl ResultsMods {
    /// The first [`MAX_MODS`] of `acronyms`.
    pub fn new<'a>(acronyms: impl IntoIterator<Item = &'a str>) -> ResultsMods {
        let mut mods = ResultsMods::default();
        for (slot, acronym) in mods.acronyms.iter_mut().zip(acronyms) {
            *slot = ModAcronym::new(acronym);
            mods.len += 1;
        }
        mods
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = &ModAcronym> {
        self.acronyms[..self.len as usize].iter()
    }
}

/// The rank of a play, as the rules crate names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rank {
    F,
    D,
    C,
    B,
    A,
    S,
    /// Silver S.
    SH,
    X,
    /// Silver SS.
    XH,
}

impl Rank {
    /// The letters lazer shows: X is "SS", and the silver ranks look like their plain ones.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Online/Leaderboards/DrawableRank.cs
    pub const fn letters(self) -> &'static str {
        match self {
            Rank::F => "F",
            Rank::D => "D",
            Rank::C => "C",
            Rank::B => "B",
            Rank::A => "A",
            Rank::S | Rank::SH => "S",
            Rank::X | Rank::XH => "SS",
        }
    }

    /// Whether the letters are drawn silver.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Online/Leaderboards/DrawableRank.cs
    pub const fn silver(self) -> bool {
        matches!(self, Rank::SH | Rank::XH)
    }
}

/// The kinds of judgement the results list, in lazer's display order: the order of
/// `HitResult`'s `Order` attributes (Great, Ok, Meh, Miss, then the slider and spinner
/// results).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HitResult.cs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatKind {
    Great,
    Ok,
    Meh,
    Miss,
    /// `LargeTickHit`.
    SliderTick,
    /// `SmallTickHit`.
    SliderEnd,
    /// `SliderTailHit`.
    SliderTail,
    /// `LargeBonus`.
    SpinnerBonus,
    /// `SmallBonus`.
    SpinnerSpin,
}

impl StatKind {
    pub const COUNT: usize = 9;
    /// Every kind, in display order.
    pub const ALL: [StatKind; StatKind::COUNT] = [
        StatKind::Great,
        StatKind::Ok,
        StatKind::Meh,
        StatKind::Miss,
        StatKind::SliderTick,
        StatKind::SliderEnd,
        StatKind::SliderTail,
        StatKind::SpinnerBonus,
        StatKind::SpinnerSpin,
    ];

    /// Position in [`StatKind::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Whether the row shows `value/maximum` and only when the beatmap has such results.
    pub const fn optional(self) -> bool {
        !matches!(
            self,
            StatKind::Great | StatKind::Ok | StatKind::Meh | StatKind::Miss
        )
    }
}

/// How often one kind of judgement happened, and how often it could have.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResultsStat {
    pub value: u32,
    pub maximum: u32,
}

impl ResultsStat {
    /// Whether the results list this row: always for the four basic results, else only
    /// when a perfect play has some.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Scoring/ScoreInfo.cs (GetStatisticsForDisplay)
    pub fn shown(self, kind: StatKind) -> bool {
        !kind.optional() || self.maximum > 0
    }
}

/// Everything the results show; built once per finished play (navigation).
#[derive(Clone, Debug, PartialEq)]
pub struct ResultsData {
    /// Names the data: the draw side re-lays its texts out when it changes.
    pub id: u64,
    /// The beatmap line, already formatted.
    pub title: String,
    pub score: i64,
    /// 0..=1.
    pub accuracy: f64,
    pub rank: Rank,
    /// Per kind, in [`StatKind::ALL`] order.
    pub stats: [ResultsStat; StatKind::COUNT],
    /// The highest combo reached and the highest possible.
    pub combo: u32,
    pub max_combo: u32,
    pub mods: ResultsMods,
}

impl ResultsData {
    /// The rows to list, in order.
    pub fn rows(&self) -> impl Iterator<Item = (StatKind, ResultsStat)> + '_ {
        StatKind::ALL
            .into_iter()
            .zip(self.stats)
            .filter(|(kind, stat)| stat.shown(*kind))
    }
}

/// Writes `accuracy` (0..=1) as lazer shows it: floored to four decimal digits, as a
/// percentage with two ("89.99%"), so that 89.9999% never reads as 90%.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/FormatUtils.cs (FormatAccuracy)
pub fn write_accuracy(out: &mut impl fmt::Write, accuracy: f64) -> fmt::Result {
    // `FloorToDecimalDigits(4)`: floor(value * 10^4) / 10^4. The percentage with two
    // decimals is then the integer count of 10^-4 steps, split at the hundreds.
    let accuracy = if accuracy.is_nan() { 0.0 } else { accuracy };
    let steps = (accuracy * 10_000.0).floor().clamp(0.0, 10_000.0) as u32;
    write!(out, "{}.{:02}%", steps / 100, steps % 100)
}

/// Writes `value` with commas between thousands ("1,234,567").
pub fn write_grouped(out: &mut impl fmt::Write, value: i64) -> fmt::Result {
    let digits = value.unsigned_abs();
    if value < 0 {
        out.write_char('-')?;
    }
    // u64 has at most 20 digits.
    let mut buffer = [0_u8; 20];
    let mut start = buffer.len();
    let mut rest = digits;
    loop {
        start -= 1;
        buffer[start] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    for (i, digit) in buffer[start..].iter().enumerate() {
        if i > 0 && (buffer.len() - start - i) % 3 == 0 {
            out.write_char(',')?;
        }
        out.write_char(*digit as char)?;
    }
    Ok(())
}

/// Writes a row's value: `value`, or `value/maximum` for the optional kinds.
pub fn write_stat(out: &mut impl fmt::Write, kind: StatKind, stat: ResultsStat) -> fmt::Result {
    if kind.optional() {
        write!(out, "{}/{}", stat.value, stat.maximum)
    } else {
        write!(out, "{}", stat.value)
    }
}

/// The two buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultsButton {
    Retry,
    Back,
}

impl ResultsButton {
    /// Left to right.
    pub const ALL: [ResultsButton; 2] = [ResultsButton::Retry, ResultsButton::Back];
}

/// What the screen understands; the application turns keys and mouse events into these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResultsInput {
    Retry,
    Back,
    PointerMove { x: f32, y: f32 },
    PointerPress { x: f32, y: f32 },
}

/// What the screen asks the application to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultsAction {
    Retry,
    Back,
}

impl From<ResultsButton> for ResultsAction {
    fn from(button: ResultsButton) -> ResultsAction {
        match button {
            ResultsButton::Retry => ResultsAction::Retry,
            ResultsButton::Back => ResultsAction::Back,
        }
    }
}

/// Where the parts of the screen are: the beatmap line on top, the score block on the left,
/// the judgement rows on the right, the buttons at the bottom. A pure function of the
/// window size and theme, shared by hit testing (update) and drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResultsLayout {
    pub title: Rect,
    pub rank: Rect,
    /// The rows of the left column: score, accuracy, max combo, mods.
    pub info: [Rect; 4],
    /// The rows of the right column, one per kind in [`StatKind::ALL`] order; the screen
    /// packs the shown ones from the top, see [`ResultsLayout::stat_row`].
    stats_top: f32,
    stats_x: f32,
    pub row_height: f32,
    pub panel_width: f32,
    pub buttons: [Rect; 2],
    pub hint: Rect,
    /// Pixels per reference unit.
    pub scale: f32,
}

impl ResultsLayout {
    pub fn new(width: f32, height: f32, theme: &Theme) -> ResultsLayout {
        let scale = theme.scale(height);
        let margin = theme.results_margin * scale;
        let row_height = theme.results_row_height * scale;
        let panel_width = theme.results_panel_width * scale;
        let gap = theme.results_column_gap * scale;
        let left = width / 2.0 - gap / 2.0 - panel_width;
        let right = width / 2.0 + gap / 2.0;
        let title = Rect {
            x: 0.0,
            y: margin,
            width,
            height: theme.results_title_size * scale * 1.25,
        };
        let top = title.y + title.height + margin;
        let rank = Rect {
            x: left,
            y: top,
            width: panel_width,
            height: theme.results_rank_size * scale,
        };
        let info_top = rank.y + rank.height + margin / 2.0;
        let mut info = [Rect::default(); 4];
        for (i, row) in info.iter_mut().enumerate() {
            *row = Rect {
                x: left,
                y: info_top + i as f32 * row_height,
                width: panel_width,
                height: row_height - 4.0 * scale,
            };
        }
        let (bw, bh) = (theme.button_width * scale, theme.button_height * scale);
        let spacing = theme.button_spacing * scale;
        let buttons_left = (width - 2.0 * bw - spacing) / 2.0;
        let button_y = height - margin - bh;
        let mut buttons = [Rect::default(); 2];
        for (i, button) in buttons.iter_mut().enumerate() {
            *button = Rect {
                x: buttons_left + i as f32 * (bw + spacing),
                y: button_y,
                width: bw,
                height: bh,
            };
        }
        let hint_height = theme.song_detail_size * scale * 1.5;
        ResultsLayout {
            title,
            rank,
            info,
            stats_top: top,
            stats_x: right,
            row_height,
            panel_width,
            buttons,
            hint: Rect {
                x: 0.0,
                y: button_y - hint_height - margin / 4.0,
                width,
                height: hint_height,
            },
            scale,
        }
    }

    /// The `n`th shown judgement row.
    pub fn stat_row(&self, n: usize) -> Rect {
        Rect {
            x: self.stats_x,
            y: self.stats_top + n as f32 * self.row_height,
            width: self.panel_width,
            height: self.row_height - 4.0 * self.scale,
        }
    }

    pub fn button_at(&self, x: f32, y: f32) -> Option<ResultsButton> {
        ResultsButton::ALL
            .into_iter()
            .zip(&self.buttons)
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(button, _)| button)
    }
}

/// What the draw side needs; cloning copies an `Arc`, no allocation.
#[derive(Clone, Debug)]
pub struct ResultsSnapshot {
    pub data: Arc<ResultsData>,
    pub hovered: Option<ResultsButton>,
}

/// The screen's state on the update thread.
#[derive(Clone, Debug, PartialEq)]
pub struct Results {
    data: Arc<ResultsData>,
    hovered: Option<ResultsButton>,
}

impl Results {
    pub fn new(data: ResultsData) -> Results {
        Results {
            data: Arc::new(data),
            hovered: None,
        }
    }

    /// A reference to the data, for the application to keep until the next navigation, so
    /// that the last one is never dropped by a snapshot buffer.
    pub fn share(&self) -> Arc<ResultsData> {
        Arc::clone(&self.data)
    }

    pub fn data(&self) -> &ResultsData {
        &self.data
    }

    pub fn snapshot(&self) -> ResultsSnapshot {
        ResultsSnapshot {
            data: Arc::clone(&self.data),
            hovered: self.hovered,
        }
    }

    /// Applies `input`; `layout` places the buttons for the pointer. Does not allocate.
    pub fn input(&mut self, input: ResultsInput, layout: &ResultsLayout) -> Option<ResultsAction> {
        match input {
            ResultsInput::Retry => Some(ResultsAction::Retry),
            ResultsInput::Back => Some(ResultsAction::Back),
            ResultsInput::PointerMove { x, y } => {
                self.hovered = layout.button_at(x, y);
                None
            }
            ResultsInput::PointerPress { x, y } => {
                self.hovered = layout.button_at(x, y);
                self.hovered.map(ResultsAction::from)
            }
        }
    }
}

#[cfg(test)]
mod tests;
