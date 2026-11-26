//! Feedback of a search box while the player types (ADR-0167): the newest character fades
//! and slides in, and an erase gives the text a small pop. It only observes the text, so
//! the query and the screens' input stay as they were.

use slam_render::Color;
use slam_text::{Layout, PositionedGlyph};

use crate::anim::ease_out_cubic;
use crate::theme::Theme;

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
    since_ns: u64,
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
    pub(crate) const fn new() -> TypeFeedback {
        TypeFeedback {
            chars: None,
            change: Change::None,
            since_ns: 0,
        }
    }

    /// Notes the text shown at `now_ns`. The first text seen starts nothing.
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
            self.since_ns = now_ns;
        }
        self.chars = Some(chars);
    }

    /// The pose at `now_ns`, `scale` the window height over the reference height.
    pub(crate) fn pose(&self, now_ns: u64, theme: &Theme, scale: f32) -> TypePose {
        let elapsed = now_ns.saturating_sub(self.since_ns);
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
    fn an_unchanged_text_starts_nothing() {
        let mut f = TypeFeedback::new();
        f.observe("ab", 0);
        f.observe("ab", MS);
        assert_eq!(f.pose(MS, &Theme::default(), 1.0), TypePose::SETTLED);
    }
}
