//! Feedback of a search box while the player types (ADR-0167): the newest character fades
//! and slides in, and an erase gives the text a small pop. It only observes the text, so
//! the query and the screens' input stay as they were.

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::shape::ShapeBatch;
use slam_text::{Layout, PositionedGlyph};

use crate::anim::ease_out_cubic;
use crate::menu::Rect;
use crate::theme::Theme;
use crate::widgets::{Look, draw};

/// What changed in the text last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    None,
    Typed,
    Erased,
}

/// Tracks the length of a search text and what its last change was.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TypeFeedback {
    chars: Option<usize>,
    change: Change,
    /// When the text last changed (or the field was reset); `None` before any of that.
    since_ns: Option<u64>,
}

/// How to draw the text now: the newest character's opacity and offset, and the pop of the
/// whole text (all offsets in target pixels, up is negative).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TypePose {
    pub(crate) last_alpha: f32,
    pub(crate) last_offset: f32,
    pub(crate) text_offset: f32,
}

impl TypePose {
    pub(crate) const SETTLED: TypePose = TypePose {
        last_alpha: 1.0,
        last_offset: 0.0,
        text_offset: 0.0,
    };
}

/// Fraction of a `span_ms` animation done after `elapsed_ns`, in `0 ..= 1`.
fn fraction(elapsed_ns: u64, span_ms: f32) -> f32 {
    if span_ms <= 0.0 {
        return 1.0;
    }
    (elapsed_ns as f32 / 1.0e6 / span_ms).clamp(0.0, 1.0)
}

impl TypeFeedback {
    /// A field that starts empty: the first character typed is a change.
    pub(crate) const fn new() -> TypeFeedback {
        TypeFeedback {
            chars: Some(0),
            change: Change::None,
            since_ns: None,
        }
    }

    /// Notes the text shown at `now_ns`. After [`TypeFeedback::reset`] the first text seen
    /// starts nothing.
    pub(crate) fn observe(&mut self, text: &str, now_ns: u64) {
        let chars = text.chars().count();
        if let Some(before) = self.chars
            && chars != before
        {
            self.change = if chars > before {
                Change::Typed
            } else {
                Change::Erased
            };
            self.since_ns = Some(now_ns);
        }
        self.chars = Some(chars);
    }

    /// Forgets the text and starts the caret's blink at `now_ns`, on.
    pub(crate) fn reset(&mut self, now_ns: u64) {
        *self = TypeFeedback {
            chars: None,
            change: Change::None,
            since_ns: Some(now_ns),
        };
    }

    /// How visible the text caret is at `now_ns`, 0 to 1 (ADR-0173, ADR-0177): solid for
    /// `caret_solid_ms` after the text changed, then blinking with soft edges
    /// (`caret_blink_ms` on and off, `caret_fade_ms` per fade). A field out of use shows
    /// none: `filled` fields (text in them, or being edited) always count as used, an
    /// empty one only for `caret_active_ms` after its last change.
    pub(crate) fn caret_alpha(&self, now_ns: u64, theme: &Theme, filled: bool) -> f32 {
        let Some(since) = self.since_ns else {
            return if filled { 1.0 } else { 0.0 };
        };
        let elapsed = now_ns.saturating_sub(since) as f32 / 1.0e6;
        let fade = theme.caret_fade_ms.max(1.0);
        let presence = if filled {
            1.0
        } else {
            1.0 - ((elapsed - theme.caret_active_ms) / fade).clamp(0.0, 1.0)
        };
        if presence <= 0.0 {
            return 0.0;
        }
        let idle = if self.change == Change::None {
            elapsed
        } else if elapsed < theme.caret_solid_ms {
            return presence;
        } else {
            elapsed - theme.caret_solid_ms
        };
        let blink = theme.caret_blink_ms;
        if blink <= 0.0 {
            return presence;
        }
        let fade = fade.min(blink);
        let t = idle % (2.0 * blink);
        let smooth = |x: f32| x * x * (3.0 - 2.0 * x);
        let blinking = if t < blink - fade {
            1.0
        } else if t < blink {
            1.0 - smooth((t - (blink - fade)) / fade)
        } else if t < 2.0 * blink - fade {
            0.0
        } else {
            smooth((t - (2.0 * blink - fade)) / fade)
        };
        presence * blinking
    }

    /// The pose at `now_ns`, `scale` the window height over the reference height.
    pub(crate) fn pose(&self, now_ns: u64, theme: &Theme, scale: f32) -> TypePose {
        let elapsed = now_ns.saturating_sub(self.since_ns.unwrap_or(now_ns));
        match self.change {
            Change::None => TypePose::SETTLED,
            Change::Typed => {
                let t = ease_out_cubic(fraction(elapsed, theme.type_in_ms));
                TypePose {
                    last_alpha: t,
                    last_offset: (1.0 - t) * theme.type_slide * scale,
                    text_offset: 0.0,
                }
            }
            Change::Erased => {
                let t = ease_out_cubic(fraction(elapsed, theme.erase_pop_ms));
                TypePose {
                    text_offset: -(1.0 - t) * theme.erase_pop * scale,
                    ..TypePose::SETTLED
                }
            }
        }
    }
}

/// Where a text caret goes: a thin bar at `x`, centred on `y`, `line_height` being the
/// text's height (ADR-0173).
pub(crate) fn caret_rect(x: f32, y: f32, line_height: f32, theme: &Theme, scale: f32) -> Rect {
    let height = line_height * theme.caret_height;
    Rect {
        x,
        y: y - height / 2.0,
        width: (theme.caret_width * scale).max(1.0),
        height,
    }
}

/// Draws a text caret (see [`caret_rect`]); `opacity` is the layer's times the caret's
/// own ([`TypeFeedback::caret_alpha`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_caret(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    x: f32,
    y: f32,
    line_height: f32,
    opacity: f32,
    theme: &Theme,
    scale: f32,
) {
    let rect = caret_rect(x, y, line_height, theme, scale);
    let colour = Color {
        a: theme.caret.a * opacity,
        ..theme.caret
    };
    draw(shapes, white, &rect, colour, Look::Pill, theme, scale);
}

/// The glyph style of `layout` under `pose`: the newest glyph (the last cluster) has its own
/// opacity and offset, the whole text the pop.
pub(crate) fn styler(
    layout: &Layout,
    colour: Color,
    pose: TypePose,
) -> impl Fn(&PositionedGlyph) -> (Color, [f32; 2]) {
    let last = layout.glyphs().last().map(|g| g.cluster);
    move |g| {
        if last == Some(g.cluster) {
            (
                Color {
                    a: colour.a * pose.last_alpha,
                    ..colour
                },
                [0.0, pose.text_offset + pose.last_offset],
            )
        } else {
            (colour, [0.0, pose.text_offset])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn typing_slides_the_new_character_in_and_settles() {
        let theme = Theme::default();
        let mut f = TypeFeedback::new();
        f.reset(0);
        f.observe("a", 0);
        assert_eq!(f.pose(0, &theme, 1.0), TypePose::SETTLED, "the first text");
        f.observe("ab", 10 * MS);
        let start = f.pose(10 * MS, &theme, 1.0);
        assert_eq!(start.last_alpha, 0.0);
        assert!(start.last_offset > 0.0);
        let mid = f.pose(70 * MS, &theme, 1.0);
        assert!(mid.last_alpha > 0.0 && mid.last_alpha < 1.0);
        assert_eq!(f.pose(210 * MS, &theme, 1.0), TypePose::SETTLED);
    }

    #[test]
    fn erasing_pops_the_whole_text() {
        let theme = Theme::default();
        let mut f = TypeFeedback::new();
        f.observe("abc", 0);
        f.observe("ab", MS);
        let pose = f.pose(MS, &theme, 2.0);
        assert!(pose.text_offset < 0.0);
        assert_eq!(pose.last_alpha, 1.0);
        assert_eq!(f.pose(501 * MS, &theme, 2.0), TypePose::SETTLED);
    }

    #[test]
    fn the_caret_is_solid_after_a_key_then_blinks_softly() {
        let theme = Theme::default();
        let mut f = TypeFeedback::new();
        f.observe("a", 0);
        f.observe("ab", 100 * MS);
        let blink = theme.caret_blink_ms as u64;
        for t in [100, 300, 599] {
            assert_eq!(f.caret_alpha(t * MS, &theme, true), 1.0, "{t}");
        }
        let start = 100 + theme.caret_solid_ms as u64;
        assert_eq!(f.caret_alpha(start * MS, &theme, true), 1.0);
        // Half way through the fade-out it is neither on nor off.
        let mid = start + blink - theme.caret_fade_ms as u64 / 2;
        let a = f.caret_alpha(mid * MS, &theme, true);
        assert!(a > 0.1 && a < 0.9, "{a}");
        assert_eq!(f.caret_alpha((start + blink + 5) * MS, &theme, true), 0.0);
        assert_eq!(
            f.caret_alpha((start + 2 * blink + 5) * MS, &theme, true),
            1.0
        );
    }

    #[test]
    fn an_empty_idle_field_shows_no_caret() {
        let theme = Theme::default();
        let mut f = TypeFeedback::new();
        assert_eq!(f.caret_alpha(MS, &theme, false), 0.0, "never used");
        // With text in it, it counts as used.
        assert!(f.caret_alpha(MS, &theme, true) > 0.0);
        // Erased to empty: the caret stays for a while, then fades away.
        f.observe("a", 0);
        f.observe("", 10 * MS);
        assert_eq!(f.caret_alpha(11 * MS, &theme, false), 1.0);
        let gone = 10 + (theme.caret_active_ms + theme.caret_fade_ms) as u64 + 1;
        for t in [gone, gone + 700, gone + 5_000] {
            assert_eq!(f.caret_alpha(t * MS, &theme, false), 0.0, "{t}");
        }
        // Typing wakes it.
        f.observe("x", (gone + 6_000) * MS);
        assert_eq!(f.caret_alpha((gone + 6_001) * MS, &theme, false), 1.0);
    }

    #[test]
    fn a_reset_field_blinks_from_the_start() {
        let theme = Theme::default();
        let mut f = TypeFeedback::new();
        f.reset(0);
        let blink = theme.caret_blink_ms as u64;
        assert_eq!(f.caret_alpha(MS, &theme, true), 1.0);
        assert_eq!(f.caret_alpha((blink + 1) * MS, &theme, true), 0.0);
        assert_eq!(f.caret_alpha((2 * blink + 1) * MS, &theme, true), 1.0);
    }

    #[test]
    fn an_unchanged_text_starts_nothing() {
        let mut f = TypeFeedback::new();
        f.reset(0);
        f.observe("ab", 0);
        f.observe("ab", MS);
        assert_eq!(f.pose(MS, &Theme::default(), 1.0), TypePose::SETTLED);
    }
}
