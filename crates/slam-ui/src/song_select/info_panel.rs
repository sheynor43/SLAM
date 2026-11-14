//! The info panel on the left of the song select: the selected difficulty's name, its
//! statistics as labelled bars and counts, and the beatmap's best local scores (ADR-0067,
//! ADR-0125). Texts are written into reserved buffers when the data changes, so drawing
//! does not allocate once warm.

use std::fmt::Write as _;

use slam_render::Color;

use super::view::{DetailLabels, TEXT_CAPACITY, fit};
use super::{
    BeatmapDetails, DetailStats, LocalScores, MAX_LOCAL_SCORES, ScoreRow, SongSelectLayout,
    SongSelectSnapshot,
};
use crate::menu::Rect;
use crate::menu::view::{Line, Typesetter};
use crate::theme::Theme;
use crate::widgets::Look;

/// What the panel's values are made of; asked for up front.
const CHARSET: &str = "0123456789.:-()\u{b7} ,%x/ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Separates the details of one line.
const SEPARATOR: &str = "  \u{b7}  ";

/// Difficulty values are drawn as bars out of this.
const BAR_MAXIMUM: f32 = 10.0;

/// Where the panel's parts are, inside [`SongSelectLayout::info`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InfoLayout {
    pub name: Rect,
    pub artist: Rect,
    /// CS, AR, OD, HP.
    pub bars: [BarLayout; 4],
    /// Length and BPM.
    pub stats: Rect,
    /// Circle, slider and spinner counts.
    pub counts: Rect,
    pub scores_title: Rect,
    /// The rows of the score list, from the first.
    pub rows: [Rect; MAX_LOCAL_SCORES],
    /// How many of `rows` end above the panel's bottom.
    pub row_count: usize,
    /// The "no local scores" line.
    pub empty: Rect,
}

/// One difficulty bar: its name, the track and the number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarLayout {
    pub label: Rect,
    pub track: Rect,
    pub value: Rect,
}

impl InfoLayout {
    pub fn new(info: &Rect, theme: &Theme, scale: f32) -> InfoLayout {
        let detail = theme.song_detail_size * scale;
        let gap = theme.song_text_pad * scale / 2.0;
        let y = std::cell::Cell::new(info.y);
        let next = |height: f32| {
            let rect = Rect {
                x: info.x,
                y: y.get(),
                width: info.width,
                height,
            };
            y.set(y.get() + height);
            rect
        };
        let name = next(theme.song_title_size * scale * 1.4);
        let artist = next(detail * 1.7);
        y.set(y.get() + gap);
        let bar_height = detail * 1.8;
        let label_width = (detail * 2.8).min(info.width / 4.0);
        let value_width = (detail * 3.2).min(info.width / 4.0);
        let bars = std::array::from_fn(|_| {
            let row = next(bar_height);
            let track_height = (theme.song_bar_height * scale).min(row.height);
            BarLayout {
                label: Rect {
                    width: label_width,
                    ..row
                },
                track: Rect {
                    x: row.x + label_width + gap,
                    y: row.y + (row.height - track_height) / 2.0,
                    width: (row.width - label_width - value_width - 2.0 * gap).max(0.0),
                    height: track_height,
                },
                value: Rect {
                    x: row.x + row.width - value_width,
                    width: value_width,
                    ..row
                },
            }
        });
        y.set(y.get() + gap);
        let stats = next(detail * 1.8);
        let counts = next(detail * 1.8);
        y.set(y.get() + gap);
        let scores_title = next(detail * 2.0);
        let pitch = theme.song_score_row_height * scale;
        let row_gap = theme.song_row_gap * scale;
        let top = y.get();
        let rows = std::array::from_fn(|i| Rect {
            x: info.x,
            y: top + i as f32 * pitch,
            width: info.width,
            height: pitch - row_gap,
        });
        let bottom = info.y + info.height;
        let row_count = (((bottom - top) / pitch).floor().max(0.0) as usize).min(MAX_LOCAL_SCORES);
        InfoLayout {
            name,
            artist,
            bars,
            stats,
            counts,
            scores_title,
            rows,
            row_count,
            empty: Rect {
                height: detail * 1.8,
                ..rows[0]
            },
        }
    }
}

/// The texts of one score row.
struct ScoreLines {
    rank: Line,
    score: Line,
    accuracy: Line,
    extra: Line,
}

impl ScoreLines {
    fn new() -> ScoreLines {
        let line = || {
            let mut line = Line::new("");
            line.text.reserve(TEXT_CAPACITY);
            line
        };
        ScoreLines {
            rank: line(),
            score: line(),
            accuracy: line(),
            extra: line(),
        }
    }

    fn lines(&self) -> [&Line; 4] {
        [&self.rank, &self.score, &self.accuracy, &self.extra]
    }
}

/// The draw side of the info panel.
pub(super) struct InfoPanel {
    labels: DetailLabels,
    /// CS, AR, OD, HP.
    bar_labels: [Line; 4],
    scores_title: Line,
    no_scores: Line,
    charset: Line,
    name: Line,
    artist: Line,
    /// The selection and list generation the name and artist are written for.
    name_for: Option<(u64, Option<(usize, usize)>)>,
    values: [Line; 4],
    stats: Line,
    counts: Line,
    details_for: Option<Option<BeatmapDetails>>,
    scores: Vec<ScoreLines>,
    scores_for: Option<Option<LocalScores>>,
}

impl InfoPanel {
    pub(super) fn new(labels: &DetailLabels) -> InfoPanel {
        let line = || {
            let mut line = Line::new("");
            line.text.reserve(TEXT_CAPACITY);
            line
        };
        InfoPanel {
            labels: labels.clone(),
            bar_labels: [
                Line::new(&labels.circle_size),
                Line::new(&labels.approach_rate),
                Line::new(&labels.overall_difficulty),
                Line::new(&labels.drain_rate),
            ],
            scores_title: Line::new(&labels.scores),
            no_scores: Line::new(&labels.no_scores),
            charset: Line::new(CHARSET),
            name: line(),
            artist: line(),
            name_for: None,
            values: std::array::from_fn(|_| line()),
            stats: line(),
            counts: line(),
            details_for: None,
            scores: (0..MAX_LOCAL_SCORES).map(|_| ScoreLines::new()).collect(),
            scores_for: None,
        }
    }

    /// Lays the static texts out for `scale` and forgets what the others showed.
    pub(super) fn lay_out(&mut self, type_setter: &mut Typesetter, theme: &Theme, scale: f32) {
        let detail = theme.song_detail_size * scale;
        for line in self.bar_labels.iter_mut().chain([
            &mut self.scores_title,
            &mut self.no_scores,
            &mut self.charset,
        ]) {
            type_setter.lay_out(line, detail);
        }
        self.name_for = None;
        self.details_for = None;
        self.scores_for = None;
    }

    /// Every line, for asking the glyph cache.
    pub(super) fn lines(&self) -> impl Iterator<Item = &Line> {
        self.bar_labels
            .iter()
            .chain(&self.values)
            .chain([
                &self.scores_title,
                &self.no_scores,
                &self.charset,
                &self.name,
                &self.artist,
                &self.stats,
                &self.counts,
            ])
            .chain(self.scores.iter().flat_map(ScoreLines::lines))
    }

    /// Writes the texts that differ from the ones shown. `ellipsis` is the width of the
    /// ellipsis at the title and detail sizes.
    pub(super) fn refresh(
        &mut self,
        type_setter: &mut Typesetter,
        theme: &Theme,
        layout: &SongSelectLayout,
        ellipsis: [f32; 2],
        snapshot: &SongSelectSnapshot,
    ) {
        let scale = layout.scale;
        let (title, detail) = (
            theme.song_title_size * scale,
            theme.song_detail_size * scale,
        );
        let parts = InfoLayout::new(&layout.info, theme, scale);
        let key = (snapshot.generation, snapshot.selected);
        if self.name_for != Some(key) {
            self.name_for = Some(key);
            self.name.text.clear();
            self.artist.text.clear();
            let set = snapshot
                .selected
                .and_then(|(s, d)| Some((snapshot.list.sets.get(s)?, d)));
            if let Some((set, difficulty)) = set {
                if let Some(d) = set.difficulties.get(difficulty) {
                    self.name.text.push_str(&d.version);
                }
                let _ = write!(self.artist.text, "{} - {}", set.artist, set.title);
            }
            fit(
                type_setter,
                &mut self.name,
                title,
                parts.name.width,
                ellipsis[0],
            );
            fit(
                type_setter,
                &mut self.artist,
                detail,
                parts.artist.width,
                ellipsis[1],
            );
        }
        if self.details_for != Some(snapshot.details) {
            self.details_for = Some(snapshot.details);
            self.write_details(snapshot.details.as_ref());
            for line in [&mut self.stats, &mut self.counts] {
                fit(type_setter, line, detail, parts.stats.width, ellipsis[1]);
            }
            for line in &mut self.values {
                type_setter.lay_out(line, detail);
            }
        }
        let scores = snapshot.details.and_then(|d| d.scores);
        if self.scores_for != Some(scores) {
            self.scores_for = Some(scores);
            let rows = scores.map(|s| s.rows).unwrap_or_default();
            let gap = theme.song_text_pad * scale;
            let avail = (parts.rows[0].width - title * 2.2 - gap).max(0.0);
            for (lines, row) in self.scores.iter_mut().zip(rows) {
                write_score(lines, row.as_ref());
                type_setter.lay_out(&mut lines.rank, title);
                type_setter.lay_out(&mut lines.accuracy, detail);
                let accuracy = lines.accuracy.layout.width();
                fit(
                    type_setter,
                    &mut lines.score,
                    detail,
                    (avail - accuracy - gap).max(0.0),
                    ellipsis[1],
                );
                fit(type_setter, &mut lines.extra, detail, avail, ellipsis[1]);
            }
        }
    }

    fn write_details(&mut self, details: Option<&BeatmapDetails>) {
        self.stats.text.clear();
        self.counts.text.clear();
        for line in &mut self.values {
            line.text.clear();
        }
        let Some(d) = details else { return };
        if let Some(s) = d.stats {
            write_length_bpm(&mut self.stats.text, &self.labels, &s);
            write_counts(&mut self.counts.text, &self.labels, &s);
        }
        let values = [
            d.circle_size,
            d.approach_rate,
            d.overall_difficulty,
            d.drain_rate,
        ];
        for (line, value) in self.values.iter_mut().zip(values) {
            push_difficulty(&mut line.text, value);
        }
    }

    /// Draws the panel. `rect` fills, `left` draws a line left-aligned at `(x, centre_y)`.
    pub(super) fn push(
        &self,
        layout: &SongSelectLayout,
        theme: &Theme,
        snapshot: &SongSelectSnapshot,
        rect: &mut impl FnMut(&Rect, Color, Look),
        left: &mut impl FnMut(&Line, f32, f32, Color),
    ) {
        let scale = layout.scale;
        let parts = InfoLayout::new(&layout.info, theme, scale);
        let centre = |r: &Rect| r.y + r.height / 2.0;
        left(&self.name, parts.name.x, centre(&parts.name), theme.text);
        left(
            &self.artist,
            parts.artist.x,
            centre(&parts.artist),
            theme.song_hint_text,
        );
        let Some(details) = snapshot.details else {
            return;
        };
        let values = [
            details.circle_size,
            details.approach_rate,
            details.overall_difficulty,
            details.drain_rate,
        ];
        for (i, bar) in parts.bars.iter().enumerate() {
            left(
                &self.bar_labels[i],
                bar.label.x,
                centre(&bar.label),
                theme.song_hint_text,
            );
            rect(&bar.track, theme.song_bar_track_fill, Look::Pill);
            let fraction = (values[i] / BAR_MAXIMUM).clamp(0.0, 1.0);
            rect(
                &Rect {
                    width: bar.track.width * fraction,
                    ..bar.track
                },
                theme.widget_accent_fill,
                Look::Pill,
            );
            let value = &self.values[i];
            left(
                value,
                bar.value.x + bar.value.width - value.layout.width(),
                centre(&bar.value),
                theme.text,
            );
        }
        left(&self.stats, parts.stats.x, centre(&parts.stats), theme.text);
        left(
            &self.counts,
            parts.counts.x,
            centre(&parts.counts),
            theme.text,
        );
        left(
            &self.scores_title,
            parts.scores_title.x,
            centre(&parts.scores_title),
            theme.song_hint_text,
        );
        let Some(scores) = details.scores else { return };
        if scores.rows[0].is_none() {
            left(
                &self.no_scores,
                parts.empty.x,
                centre(&parts.empty),
                theme.disabled_text,
            );
            return;
        }
        let pad = theme.song_text_pad * scale;
        let rank_width = theme.song_title_size * scale * 2.2;
        for ((area, row), lines) in parts
            .rows
            .iter()
            .zip(&scores.rows)
            .zip(&self.scores)
            .take(parts.row_count)
        {
            if row.is_none() {
                break;
            }
            rect(area, theme.song_search_fill, Look::Round);
            let (top, bottom) = (area.y + area.height * 0.3, area.y + area.height * 0.72);
            left(&lines.rank, area.x + pad / 2.0, centre(area), theme.text);
            let x = area.x + rank_width;
            left(&lines.score, x, top, theme.text);
            left(
                &lines.accuracy,
                area.x + area.width - pad / 2.0 - lines.accuracy.layout.width(),
                top,
                theme.text,
            );
            left(&lines.extra, x, bottom, theme.song_hint_text);
        }
    }
}

/// Writes one score row's texts; empty ones for `None`.
fn write_score(lines: &mut ScoreLines, row: Option<&ScoreRow>) {
    for line in [
        &mut lines.rank,
        &mut lines.score,
        &mut lines.accuracy,
        &mut lines.extra,
    ] {
        line.text.clear();
    }
    let Some(row) = row else { return };
    for &b in row.rank.iter().take_while(|&&b| b != 0) {
        lines.rank.text.push(char::from(b));
    }
    push_grouped(&mut lines.score.text, row.total_score);
    push_accuracy(&mut lines.accuracy.text, row.accuracy);
    let _ = write!(lines.extra.text, "{}x", row.max_combo);
    let mut mods = row.mods.iter().peekable();
    if mods.peek().is_some() {
        lines.extra.text.push_str(SEPARATOR);
        for (i, acronym) in mods.enumerate() {
            if i > 0 {
                lines.extra.text.push(' ');
            }
            lines.extra.text.push_str(acronym);
        }
    }
    lines.extra.text.push_str(SEPARATOR);
    push_date(&mut lines.extra.text, row.date_ms);
}

/// `1,234,567`.
fn push_grouped(text: &mut String, value: u64) {
    let mut digits = [0u8; 20];
    let mut n = 0;
    let mut rest = value;
    loop {
        digits[n] = b'0' + (rest % 10) as u8;
        n += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    for (i, &d) in digits[..n].iter().rev().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            text.push(',');
        }
        text.push(char::from(d));
    }
}

/// `98.45%`: cut, not rounded, to two decimals, so that 99.996% does not read 100.00%.
fn push_accuracy(text: &mut String, accuracy: f64) {
    let hundredths = (accuracy.clamp(0.0, 1.0) * 10_000.0 + 1e-7).floor() as u32;
    let _ = write!(text, "{}.{:02}%", hundredths / 100, hundredths % 100);
}

/// `2026-11-14 18:30` in UTC (the local time zone needs the system's database).
fn push_date(text: &mut String, unix_ms: i64) {
    let seconds = unix_ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let in_day = seconds.rem_euclid(86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let _ = write!(
        text,
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        in_day / 3600,
        in_day / 60 % 60
    );
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Select/BeatmapTitleWedge.cs (updateLengthAndBpmStatistics)
/// "Length 03:25  ·  BPM 150-200 (180)".
fn write_length_bpm(text: &mut String, labels: &DetailLabels, s: &DetailStats) {
    let _ = write!(text, "{} ", labels.length);
    push_duration(text, s.length_ms);
    let _ = write!(text, "{SEPARATOR}{} ", labels.bpm);
    let _ = if s.bpm_minimum == s.bpm_maximum {
        write!(text, "{}", s.bpm_minimum)
    } else {
        write!(
            text,
            "{}-{} ({})",
            s.bpm_minimum, s.bpm_maximum, s.bpm_most_common
        )
    };
}

/// "Circles 512  ·  Sliders 300  ·  Spinners 2".
fn write_counts(text: &mut String, labels: &DetailLabels, s: &DetailStats) {
    for (i, (label, count)) in [
        (&labels.circles, s.circles),
        (&labels.sliders, s.sliders),
        (&labels.spinners, s.spinners),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            text.push_str(SEPARATOR);
        }
        let _ = write!(text, "{label} {count}");
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Extensions/TimeDisplayExtensions.cs (ToFormattedDuration)
/// `mm:ss`, `hh:mm:ss` from an hour, `dd:hh:mm:ss` from a day; seconds cut, as `TimeSpan`.
fn push_duration(text: &mut String, ms: u64) {
    let seconds = ms / 1000;
    let (days, hours, minutes, seconds) = (
        seconds / 86_400,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
    );
    let _ = if days > 0 {
        write!(text, "{days:02}:{hours:02}:{minutes:02}:{seconds:02}")
    } else if hours > 0 {
        write!(text, "{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        write!(text, "{minutes:02}:{seconds:02}")
    };
}

/// Formats into a fixed buffer, so that reading a float's digits does not allocate.
struct StackText {
    bytes: [u8; 32],
    len: usize,
}

impl std::fmt::Write for StackText {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len + s.len();
        let slot = self.bytes.get_mut(self.len..end).ok_or(std::fmt::Error)?;
        slot.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// A difficulty value as lazer's `float.ToString("0.##")`: .NET rounds the float to 7
/// significant digits first, then those digits to two decimals half away from zero, and
/// drops trailing zeros.
fn push_difficulty(text: &mut String, value: f32) {
    let mut digits = StackText {
        bytes: [0; 32],
        len: 0,
    };
    // "d.dddddde-N": seven significant digits, correctly rounded.
    if !value.is_finite() || write!(digits, "{:.6e}", value.abs()).is_err() {
        return;
    }
    let written = std::str::from_utf8(&digits.bytes[..digits.len]).unwrap_or("");
    let Some((mantissa, exponent)) = written.split_once('e') else {
        return;
    };
    let Ok(exponent) = exponent.parse::<i32>() else {
        return;
    };
    let significand = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .fold(0_i64, |n, d| n * 10 + i64::from(d - b'0'));
    // The value is `significand * 10^(exponent - 6)`; in hundredths, `10^(exponent - 4)`.
    let shift = exponent - 4;
    let hundredths = if shift >= 0 {
        significand.saturating_mul(10_i64.saturating_pow(shift as u32))
    } else if shift > -18 {
        let divisor = 10_i64.pow((-shift) as u32);
        let (q, r) = (significand / divisor, significand % divisor);
        q + i64::from(2 * r >= divisor)
    } else {
        0
    };
    if value < 0.0 && hundredths != 0 {
        text.push('-');
    }
    let (whole, part) = (hundredths / 100, hundredths % 100);
    let _ = write!(text, "{whole}");
    match part {
        0 => {}
        p if p % 10 == 0 => {
            let _ = write!(text, ".{}", p / 10);
        }
        p => {
            let _ = write!(text, ".{p:02}");
        }
    }
}

#[cfg(test)]
#[path = "info_panel_tests.rs"]
mod tests;
