//! The results screen: its update-side state and layout, input-agnostic (the application
//! maps its events to [`ResultsInput`]), and the pure functions that turn numbers into the
//! texts lazer shows. The draw side is [`view::ResultsView`].
//!
//! `slam-ui` does not know the rules crate, so the application converts a finished play
//! into [`ResultsData`] (counts per kind of judgement, rank, mods).

use std::fmt;
use std::sync::Arc;

use crate::anim::{Towards, ease_out_quad, ease_out_quint};
use crate::menu::Rect;
use crate::theme::Theme;

pub mod stable;
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
    /// The mean hit offset in ms, raw (not divided by the clock rate); negative is early.
    /// `None` when the play had no judged hit offsets.
    pub average_hit_error: Option<f64>,
    /// The average's row text ("1.23 ms early"), already localised and formatted: `Some`
    /// exactly when `average_hit_error` is.
    pub average_hit_error_text: Option<String>,
    /// The unstable rate as lazer computes it (ten times the standard deviation of the hit
    /// offsets in ms, already scaled by the clock rate); `None` without hit offsets.
    pub unstable_rate: Option<f64>,
    /// What stable's results add (ADR-0161).
    pub extra: ResultsExtra,
}

/// What stable's results show besides the counts (ADR-0161), already localised.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResultsExtra {
    /// "Beatmap by …"; empty when the mapper is not known.
    pub beatmap_by: String,
    /// "Played by … on …".
    pub played_by: String,
    /// Health over the playable time, 0..=1, evenly spaced; empty draws no line.
    pub health: Vec<f32>,
    /// The lines of the details under the panel, in order.
    pub details: Vec<String>,
}

impl ResultsData {
    /// Whether the play reached the highest possible combo (stable's "perfect" banner).
    pub fn full_combo(&self) -> bool {
        self.max_combo > 0 && self.combo >= self.max_combo
    }

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

/// Whether a hit error reads "early": decided on the raw sign, so that -0.001 (which
/// shows as 0.00) is still early and 0 is late, as lazer does.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Ranking/Statistics/AverageHitError.cs
pub fn is_early(offset: f64) -> bool {
    offset < 0.0
}

/// A fixed buffer for the digits of a formatted float.
struct Digits {
    bytes: [u8; 40],
    len: usize,
}

impl fmt::Write for Digits {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        self.bytes
            .get_mut(self.len..end)
            .ok_or(fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Writes `value` with two decimals and commas between thousands, as .NET's `N2` does in
/// the invariant culture ("1,234.57"). Rounds the exact decimal value of the float, so
/// 1.005 (a hair under) gives "1.00", and exact ties go away from zero (0.125 is "0.13");
/// a negative that rounds to zero keeps its sign ("-0.00"). Non-finite values: "NaN", "∞", "-∞". Magnitudes from 1e15 up are written
/// without commas.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Ranking/Statistics/UnstableRate.cs
pub fn write_fixed2(out: &mut impl fmt::Write, value: f64) -> fmt::Result {
    if value.is_nan() {
        return out.write_str("NaN");
    }
    if value.is_infinite() {
        return out.write_str(if value < 0.0 { "-∞" } else { "∞" });
    }
    if value.is_sign_negative() && value != 0.0 {
        out.write_char('-')?;
    }
    let magnitude = value.abs();
    if magnitude >= 1e15 {
        return write!(out, "{magnitude:.2}");
    }
    // Rust breaks an exact tie (x.125, x.375, ...) to even, .NET away from zero: nudge those
    // up. A tie needs an eighth that is odd, which a float below 1e15 holds exactly.
    let eighths = magnitude * 8.0;
    let magnitude = if eighths.fract() == 0.0 && eighths % 2.0 == 1.0 {
        magnitude + 0.001
    } else {
        magnitude
    };
    let mut digits = Digits {
        bytes: [0; 40],
        len: 0,
    };
    fmt::Write::write_fmt(&mut digits, format_args!("{magnitude:.2}"))?;
    let text = std::str::from_utf8(&digits.bytes[..digits.len]).map_err(|_| fmt::Error)?;
    let (whole, fraction) = text.split_once('.').ok_or(fmt::Error)?;
    write_grouped(out, whole.parse().map_err(|_| fmt::Error)?)?;
    write!(out, ".{fraction}")
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
    PointerMove {
        x: f32,
        y: f32,
    },
    PointerPress {
        x: f32,
        y: f32,
    },
    /// Scrolls by `delta` lines, positive down (stable look).
    Scroll {
        delta: f32,
    },
    /// Scrolls by `delta` pages, positive down.
    Page {
        delta: f32,
    },
    /// Scrolls to the top or the bottom.
    Edge {
        bottom: bool,
    },
}

/// Part of the scroll range one line of the wheel moves.
const SCROLL_LINE: f32 = 0.15;

/// How long a pull past the top takes to reach its furthest, in ns.
const OVERSCROLL_OUT_NS: u64 = 100_000_000;
/// How long the content then takes to spring back, in ns.
const OVERSCROLL_BACK_NS: u64 = 500_000_000;
/// The furthest the content is pulled past the top, as a part of the scroll range.
const MAX_OVERSCROLL: f32 = 0.75;

/// The content pulled down past the top by the wheel, springing back: stable lets the
/// results scroll up past the top, then eases them back (stable's results at 1920×1080: a
/// few quick lines pull it ~600 px down, back in about half a second).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Overscroll {
    /// Where the latest pull started from, as a part of the scroll range.
    from: f32,
    /// The furthest the latest pull goes.
    amount: f32,
    /// When the latest pull started.
    at_ns: u64,
}

impl Overscroll {
    /// How far the content is pulled down at `now_ns`, as a part of the scroll range.
    fn at(&self, now_ns: u64) -> f32 {
        if self.amount <= 0.0 {
            return 0.0;
        }
        let t = now_ns.saturating_sub(self.at_ns);
        if t < OVERSCROLL_OUT_NS {
            let out = ease_out_quint(t as f32 / OVERSCROLL_OUT_NS as f32);
            self.from + (self.amount - self.from) * out
        } else {
            let back = (t - OVERSCROLL_OUT_NS) as f32 / OVERSCROLL_BACK_NS as f32;
            self.amount * (1.0 - ease_out_quint(back.min(1.0)))
        }
    }

    /// Pulls `lines` of the wheel further than the pull under way is heading.
    fn pull(&mut self, lines: f32, now_ns: u64) {
        let now = self.at(now_ns);
        let rising = now_ns.saturating_sub(self.at_ns) < OVERSCROLL_OUT_NS;
        let base = if rising { self.amount } else { now };
        *self = Overscroll {
            from: now,
            amount: (base + lines * SCROLL_LINE).min(MAX_OVERSCROLL),
            at_ns: now_ns,
        };
    }
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
    /// The rows of the left column: score, accuracy, max combo, mods, average hit error,
    /// unstable rate.
    pub info: [Rect; 6],
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
    /// Pixels the content scrolls over its range; 0 when the screen does not scroll (only
    /// the stable look does, ADR-0161).
    pub scroll_area: f32,
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
        let mut info = [Rect::default(); 6];
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
            scroll_area: 0.0,
        }
    }

    /// This layout in the stable look (ADR-0158, ADR-0161): the skin's `ranking-retry` and
    /// `menu-back` are the buttons, `hint` is the scroll-down button, and the screen scrolls.
    pub fn stable(mut self, width: f32, height: f32, art: &stable::StableArt) -> ResultsLayout {
        let geometry = stable::geometry(width, height, art);
        self.buttons = [geometry.retry, geometry.back];
        self.hint = geometry.hint;
        self.scroll_area = geometry.scroll_area;
        self
    }

    /// [`ResultsLayout::new`], in the stable look when `stable` has the skin's image sizes.
    pub fn new_for(
        width: f32,
        height: f32,
        theme: &Theme,
        stable: Option<&stable::StableArt>,
    ) -> ResultsLayout {
        let layout = ResultsLayout::new(width, height, theme);
        match stable {
            Some(art) => layout.stable(width, height, art),
            None => layout,
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
        self.button_at_scrolled(x, y, 0.0)
    }

    /// [`button_at`](Self::button_at) with the content scrolled `scrolled` pixels up: in the
    /// stable look Retry scrolls with it, Back stays.
    pub fn button_at_scrolled(&self, x: f32, y: f32, scrolled: f32) -> Option<ResultsButton> {
        let stable = self.scroll_area > 0.0;
        ResultsButton::ALL
            .into_iter()
            .zip(&self.buttons)
            .find(|(button, rect)| {
                let up = if stable && *button == ResultsButton::Retry {
                    scrolled
                } else {
                    0.0
                };
                rect.contains(x, y + up)
            })
            .map(|(button, _)| button)
    }
}

/// What the draw side needs; cloning copies an `Arc`, no allocation.
#[derive(Clone, Debug)]
pub struct ResultsSnapshot {
    pub data: Arc<ResultsData>,
    pub hovered: Option<ResultsButton>,
    hover: [Towards; 2],
    scroll: Towards,
    target: f32,
    overscroll: Overscroll,
}

impl ResultsSnapshot {
    /// A snapshot at rest: nothing hovered, scrolled to the top.
    pub fn new(data: Arc<ResultsData>) -> ResultsSnapshot {
        ResultsSnapshot {
            data,
            hovered: None,
            hover: [Towards::default(); 2],
            scroll: Towards::default(),
            target: 0.0,
            overscroll: Overscroll::default(),
        }
    }

    /// This snapshot scrolled to `fraction` (0 top, 1 bottom) of the range, at rest.
    pub fn scrolled(mut self, fraction: f32) -> ResultsSnapshot {
        let fraction = fraction.clamp(0.0, 1.0);
        self.scroll = Towards::rest(fraction, 0);
        self.target = fraction;
        self
    }

    /// How far `button`'s highlight has come at `now_ns`, 0 ..= 1.
    pub fn hover_amount(&self, button: ResultsButton, now_ns: u64) -> f32 {
        self.hover[button as usize].at(now_ns, stable::HOVER_NS, ease_out_quad)
    }

    /// The scroll position at `now_ns`, 0 (top) ..= 1 (bottom of the range), below 0 while
    /// the content is pulled past the top.
    pub fn scroll_at(&self, now_ns: u64) -> f32 {
        scroll_at(&self.scroll, &self.overscroll, now_ns)
    }

    /// Where the scroll is heading, 0 ..= 1.
    pub fn scroll_target(&self) -> f32 {
        self.target
    }
}

/// The scroll position at `now_ns` of `scroll`, less the pull past the top.
fn scroll_at(scroll: &Towards, overscroll: &Overscroll, now_ns: u64) -> f32 {
    scroll
        .at(now_ns, stable::SCROLL_NS, ease_out_quint)
        .clamp(0.0, 1.0)
        - overscroll.at(now_ns)
}

/// The screen's state on the update thread.
#[derive(Clone, Debug, PartialEq)]
pub struct Results {
    data: Arc<ResultsData>,
    hovered: Option<ResultsButton>,
    hover: [Towards; 2],
    scroll: Towards,
    target: f32,
    overscroll: Overscroll,
}

impl Results {
    pub fn new(data: ResultsData) -> Results {
        Results {
            data: Arc::new(data),
            hovered: None,
            hover: [Towards::default(); 2],
            scroll: Towards::default(),
            target: 0.0,
            overscroll: Overscroll::default(),
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
            hover: self.hover,
            scroll: self.scroll,
            target: self.target,
            overscroll: self.overscroll,
        }
    }

    fn scroll_to(&mut self, target: f32, now_ns: u64) {
        self.target = target.clamp(0.0, 1.0);
        self.scroll
            .aim(self.target, now_ns, stable::SCROLL_NS, ease_out_quint);
    }

    fn point(&mut self, x: f32, y: f32, layout: &ResultsLayout, now_ns: u64) {
        let scrolled = scroll_at(&self.scroll, &self.overscroll, now_ns) * layout.scroll_area;
        self.hovered = layout.button_at_scrolled(x, y, scrolled);
        for button in ResultsButton::ALL {
            let to = if self.hovered == Some(button) {
                1.0
            } else {
                0.0
            };
            self.hover[button as usize].aim(to, now_ns, stable::HOVER_NS, ease_out_quad);
        }
    }

    /// Applies `input` at `now_ns`; `layout` places the buttons for the pointer. Does not
    /// allocate.
    pub fn input(
        &mut self,
        input: ResultsInput,
        layout: &ResultsLayout,
        now_ns: u64,
    ) -> Option<ResultsAction> {
        let scrolls = layout.scroll_area > 0.0;
        match input {
            ResultsInput::Retry => Some(ResultsAction::Retry),
            ResultsInput::Back => Some(ResultsAction::Back),
            ResultsInput::PointerMove { x, y } => {
                self.point(x, y, layout, now_ns);
                None
            }
            ResultsInput::PointerPress { x, y } => {
                if scrolls && self.target < 1.0 && layout.hint.contains(x, y) {
                    self.scroll_to(1.0, now_ns);
                    return None;
                }
                self.point(x, y, layout, now_ns);
                self.hovered.map(ResultsAction::from)
            }
            ResultsInput::Scroll { delta } if scrolls => {
                // Up at the top pulls the content past it, as stable does.
                if delta < 0.0 && self.target <= 0.0 {
                    self.overscroll.pull(-delta, now_ns);
                } else {
                    self.scroll_to(self.target + delta * SCROLL_LINE, now_ns);
                }
                None
            }
            ResultsInput::Page { delta } if scrolls => {
                self.scroll_to(self.target + delta, now_ns);
                None
            }
            ResultsInput::Edge { bottom } if scrolls => {
                self.scroll_to(if bottom { 1.0 } else { 0.0 }, now_ns);
                None
            }
            ResultsInput::Scroll { .. } | ResultsInput::Page { .. } | ResultsInput::Edge { .. } => {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
