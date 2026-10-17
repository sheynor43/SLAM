//! The beatmap offset as gameplay changes it with hotkeys, and the panel that shows it
//! (lazer's `BeatmapOffsetControl` and `GameplayOffsetControl`).

use std::fmt;

use crate::circles::Easing;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/PlayerSettings/BeatmapOffsetControl.cs (Current, OnPressed)
/// The lowest beatmap offset, milliseconds.
pub const MIN_MS: f64 = -50.0;
/// The highest beatmap offset, milliseconds.
pub const MAX_MS: f64 = 50.0;
/// The offset's precision, milliseconds.
const PRECISION_MS: f64 = 0.1;
/// What one press of a hotkey changes, milliseconds.
pub const STEP_MS: f64 = 1.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/SubmittingPlayer.cs (AllowCriticalSettingsAdjustment)
/// How long after the gameplay start the offset can change outside breaks, milliseconds.
pub const ADJUST_WINDOW_MS: f64 = 10_000.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayOffsetControl.cs (hideOp, PopIn, PopOut)
/// How long the panel stays after the last change, nanoseconds.
pub const HIDE_DELAY_NS: u64 = 500_000_000;
/// How long the panel takes to appear and to go, nanoseconds.
pub const FADE_NS: u64 = 500_000_000;
/// How far right of its place the hidden panel is, in HUD units.
pub const HIDDEN_SHIFT: f32 = 100.0;

/// `offset` moved by `delta`, as lazer's `BindableDouble` takes a new value: clamped, then
/// rounded half to even to the precision as a decimal (the double nearest the decimal).
// Ported from osu-framework 2026.921.1: osu.Framework/Bindables/BindableNumber.cs (setValue)
pub fn adjusted(offset: f64, delta: f64) -> f64 {
    let value = (offset + delta).clamp(MIN_MS, MAX_MS);
    let steps = 1.0 / PRECISION_MS;
    (value * steps).round_ties_even() / steps
}

/// Whether the offset may change at gameplay time `time` (lazer's
/// `AllowCriticalSettingsAdjustment` of a submitting player): within [`ADJUST_WINDOW_MS`]
/// of the gameplay start or in a break. Not while paused, which the caller checks.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/SubmittingPlayer.cs (AllowCriticalSettingsAdjustment)
pub fn may_adjust(time: f64, gameplay_start_time: f64, is_break_time: bool) -> bool {
    is_break_time || time - gameplay_start_time <= ADJUST_WINDOW_MS
}

/// The offset panel's visibility (lazer's `GameplayOffsetControl`): shown on each change,
/// hidden [`HIDE_DELAY_NS`] after the last one. Its shift right is always
/// `HIDDEN_SHIFT * (1 - alpha)`: appearing and going move both together from where they
/// are.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OffsetPanel {
    /// The offset shown, milliseconds.
    pub offset_ms: f64,
    /// Engine time the panel last began to appear; none before the first change.
    shown_ns: Option<u64>,
    /// Its alpha then.
    from: f32,
    /// Engine time it begins to go.
    hide_ns: u64,
}

impl OffsetPanel {
    /// The panel of a play that starts with `offset_ms`: hidden.
    pub fn new(offset_ms: f64) -> OffsetPanel {
        OffsetPanel {
            offset_ms,
            ..OffsetPanel::default()
        }
    }

    /// The offset changed to `offset_ms` at engine time `time_ns`.
    pub fn changed(&mut self, offset_ms: f64, time_ns: u64) {
        self.offset_ms = offset_ms;
        // `Show` does nothing while visible; only the hide moves.
        if self.shown_ns.is_none() || time_ns >= self.hide_ns {
            self.from = self.alpha(time_ns);
            self.shown_ns = Some(time_ns);
        }
        self.hide_ns = time_ns + HIDE_DELAY_NS;
    }

    /// Hides the panel at once (a new play).
    pub fn reset(&mut self) {
        *self = OffsetPanel::new(self.offset_ms);
    }

    /// The panel's alpha at engine time `now_ns`, 0..=1.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayOffsetControl.cs (PopIn, PopOut)
    pub fn alpha(&self, now_ns: u64) -> f32 {
        let Some(shown) = self.shown_ns else {
            return 0.0;
        };
        let appearing = |at: u64| {
            let t = progress(at.saturating_sub(shown));
            self.from + (1.0 - self.from) * Easing::OutQuint.apply(t) as f32
        };
        if now_ns < self.hide_ns {
            appearing(now_ns)
        } else {
            let t = progress(now_ns - self.hide_ns);
            appearing(self.hide_ns) * (1.0 - Easing::InQuint.apply(t) as f32)
        }
    }

    /// The panel's shift right at engine time `now_ns`, in HUD units.
    pub fn shift(&self, now_ns: u64) -> f32 {
        HIDDEN_SHIFT * (1.0 - self.alpha(now_ns))
    }
}

fn progress(elapsed_ns: u64) -> f64 {
    (elapsed_ns as f64 / FADE_NS as f64).min(1.0)
}

/// Which text the panel shows (lazer's `GetOffsetExplanatoryText`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsetWording {
    /// Just the offset: it shows as zero.
    Zero,
    /// Objects appear earlier: the offset is positive.
    Earlier,
    /// Objects appear later: the offset is negative.
    Later,
}

/// The digits of `offset_ms` (at most one decimal, no trailing zeros) written into `out`,
/// and which wording goes with them. The point is always `.`; lazer uses the UI
/// culture's.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/PlayerSettings/BeatmapOffsetControl.cs (GetOffsetExplanatoryText)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Extensions/NumberFormattingExtensions.cs (ToStandardFormattedString)
pub fn format_offset(
    offset_ms: f64,
    out: &mut impl fmt::Write,
) -> Result<OffsetWording, fmt::Error> {
    let tenths = (offset_ms * 10.0).round_ties_even();
    if tenths == 0.0 || !tenths.is_finite() {
        out.write_char('0')?;
        return Ok(OffsetWording::Zero);
    }
    if tenths % 10.0 == 0.0 {
        write!(out, "{}", tenths / 10.0)?;
    } else {
        write!(out, "{:.1}", tenths / 10.0)?;
    }
    Ok(if offset_ms > 0.0 {
        OffsetWording::Earlier
    } else {
        OffsetWording::Later
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn text(offset: f64) -> (String, OffsetWording) {
        let mut s = String::new();
        let wording = format_offset(offset, &mut s).unwrap();
        (s, wording)
    }

    #[test]
    fn steps_are_clamped_to_fifty_ms() {
        assert_eq!(adjusted(0.0, -STEP_MS), -1.0);
        assert_eq!(adjusted(49.5, STEP_MS), 50.0);
        assert_eq!(adjusted(50.0, STEP_MS), 50.0);
        assert_eq!(adjusted(-50.0, -STEP_MS), -50.0);
    }

    #[test]
    fn adjusting_is_allowed_early_or_in_breaks() {
        assert!(may_adjust(9_000.0, -1_000.0, false));
        assert!(!may_adjust(9_000.1, -1_000.0, false));
        assert!(may_adjust(60_000.0, -1_000.0, true));
    }

    #[test]
    fn offsets_format_as_lazer_does() {
        assert_eq!(text(0.0), ("0".into(), OffsetWording::Zero));
        // Lazer's `TestNegativeZero`.
        assert_eq!(text(-0.0001), ("0".into(), OffsetWording::Zero));
        assert_eq!(text(-1.0), ("-1".into(), OffsetWording::Later));
        assert_eq!(
            text(3.0000000000000004),
            ("3".into(), OffsetWording::Earlier)
        );
        assert_eq!(text(12.5), ("12.5".into(), OffsetWording::Earlier));
        assert_eq!(text(-0.7), ("-0.7".into(), OffsetWording::Later));
    }

    #[test]
    fn the_panel_hides_500_ms_after_the_last_change() {
        let mut panel = OffsetPanel::new(0.0);
        assert_eq!(panel.alpha(0), 0.0);
        assert_eq!(panel.shift(0), HIDDEN_SHIFT);
        panel.changed(-1.0, 1000 * MS);
        assert_eq!(panel.alpha(1000 * MS), 0.0);
        assert!(panel.alpha(1250 * MS) > 0.9);
        // A change while shown keeps it appearing and moves the hide.
        panel.changed(-2.0, 1400 * MS);
        assert_eq!(panel.alpha(1500 * MS), 1.0);
        assert_eq!(panel.shift(1500 * MS), 0.0);
        assert_eq!(panel.alpha(1900 * MS), 1.0);
        assert!(panel.alpha(2000 * MS) < 1.0);
        assert_eq!(panel.alpha(2400 * MS), 0.0);
        assert_eq!(panel.offset_ms, -2.0);
    }

    #[test]
    fn a_change_while_going_appears_from_where_it_is() {
        let mut panel = OffsetPanel::new(0.0);
        panel.changed(1.0, 0);
        let going = panel.alpha(900 * MS);
        assert!(going > 0.0 && going < 1.0);
        panel.changed(2.0, 900 * MS);
        assert_eq!(panel.alpha(900 * MS), going);
        assert!(panel.alpha(1000 * MS) > going);
        panel.reset();
        assert_eq!(panel.alpha(1000 * MS), 0.0);
        assert_eq!(panel.offset_ms, 2.0);
    }
}
