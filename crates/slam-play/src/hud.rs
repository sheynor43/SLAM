//! A stand-in HUD until the HUD components exist (M5): score, accuracy and combo as plain
//! numbers, health as a bar, the skip button and the click-to-resume target.
//!
//! Draw side only. Numbers are formatted into fixed buffers and laid out again only when
//! they change; once warm ([`HudView::new`]), nothing here allocates.
//!
//! Only the numbers follow lazer. Unlike its counters, values show at once (no rolling),
//! sizes and places are this module's own, and the resume target is a square.

use std::fmt::{self, Write as _};
use std::sync::Arc;

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_text::{
    FamilyId, FontRegistry, GlyphCache, GlyphKey, Layout, LayoutContext, TextStats, TextStyle,
    UI_FAMILY, push_layout,
};

use crate::playfield::PlayfieldLayout;
use crate::session::{HudValues, PlaySnapshot, PlayState, RESUME_CURSOR_SIZE};

/// Window height the HUD's sizes are given at; it scales with the window.
const REFERENCE_HEIGHT: f32 = 768.0;
const MARGIN: f32 = 10.0;
const WEIGHT: u16 = 700;
const SCORE_SIZE: f32 = 40.0;
const ACCURACY_SIZE: f32 = 24.0;
const COMBO_SIZE: f32 = 40.0;
const SKIP_SIZE: f32 = 24.0;
const HEALTH_BAR: [f32; 2] = [300.0, 8.0];
const SKIP_BUTTON: [f32; 2] = [180.0, 48.0];

const TEXT: Color = Color::WHITE;
const SHADE: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.5,
};
const RESUME: Color = Color {
    r: 1.0,
    g: 0.6,
    b: 0.2,
    a: 0.6,
};

/// Every character the numbers use, as long as the longest number text: laying it out
/// once settles the layout buffers, and its glyphs are the ones to warm.
const WARM_TEXT: &str = "0123456789.%x-0123456789";
/// The numbers without a letter: a shape plan is made per script, and digits alone have
/// none (the combo's `x` is Latin).
const WARM_DIGITS: &str = "0123456789.%-";

/// Bytes of a number's text.
const NUMBER_TEXT_LEN: usize = WARM_TEXT.len();

/// The text of a number, formatted without allocating.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct NumberText {
    bytes: [u8; NUMBER_TEXT_LEN],
    len: usize,
}

impl NumberText {
    fn new() -> NumberText {
        NumberText {
            bytes: [0; NUMBER_TEXT_LEN],
            len: 0,
        }
    }

    pub fn as_str(&self) -> &str {
        // Only whole `&str`s are written.
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or_default()
    }
}

impl fmt::Debug for NumberText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Write for NumberText {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        let dest = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        dest.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Formats with `write!`; a text too long for the buffer keeps what fit (never for the
/// numbers below).
fn number_text(args: fmt::Arguments<'_>) -> NumberText {
    let mut text = NumberText::new();
    let _ = text.write_fmt(args);
    text
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/GameplayScoreCounter.cs (RequiredDisplayDigits)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/ScoreCounter.cs (FormatCount)
/// The score with leading zeros to six digits (standardised scoring).
pub fn format_score(score: i64) -> NumberText {
    number_text(format_args!("{score:06}"))
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Utils/FormatUtils.cs (FormatAccuracy, FloorToDecimalDigits)
/// Accuracy (0..=1) as a percentage with two decimals, rounded down: 89.99999 % never
/// shows as 90 %. Always with a point and no space; lazer formats in the UI culture.
pub fn format_accuracy(accuracy: f64) -> NumberText {
    let floored = (accuracy * 10_000.0).floor() / 10_000.0;
    // .NET's "0.00%" rounds the hundredths of a percent half away from zero; after the
    // floor this only takes back the representation error. NaN and below 0 give 0.
    let hundredths = (floored * 10_000.0).round() as u64;
    number_text(format_args!(
        "{}.{:02}%",
        hundredths / 100,
        hundredths % 100
    ))
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/DefaultComboCounter.cs (FormatCount)
pub fn format_combo(combo: u32) -> NumberText {
    number_text(format_args!("{combo}x"))
}

/// A number laid out again when it changes.
#[derive(Debug, Clone)]
struct Counter {
    size: f32,
    /// The value the layout shows; `None` lays it out on the next update.
    shown: Option<u64>,
    text: NumberText,
    layout: Layout,
}

impl Counter {
    fn new(size: f32) -> Counter {
        Counter {
            size,
            shown: None,
            text: NumberText::new(),
            layout: Layout::new(),
        }
    }
}

/// Text sizes and positions of one window size.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Frame {
    width: f32,
    height: f32,
    scale: f32,
}

/// The draw side of the stand-in HUD.
pub struct HudView {
    registry: Arc<FontRegistry>,
    family: FamilyId,
    context: LayoutContext,
    frame: Frame,
    score: Counter,
    accuracy: Counter,
    combo: Counter,
    skip_label: String,
    skip: Layout,
    /// [`WARM_TEXT`] and [`WARM_DIGITS`] laid out: the glyphs to warm (each script may
    /// shape to other glyphs).
    warm: [Layout; 2],
}

impl std::fmt::Debug for HudView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HudView")
            .field("frame", &self.frame)
            .field("score", &self.score.text)
            .field("accuracy", &self.accuracy.text)
            .field("combo", &self.combo.text)
            .field("skip_label", &self.skip_label)
            .finish_non_exhaustive()
    }
}

impl HudView {
    /// A HUD with fonts from `registry` and the skip button's localised `skip_label`.
    /// Lays out every character it will show, so that later layouts do not allocate:
    /// call it off the hot paths.
    pub fn new(registry: Arc<FontRegistry>, skip_label: &str) -> HudView {
        // An unknown family lays out nothing; the built-in registry has the UI family.
        let family = registry.family(UI_FAMILY).unwrap_or(FamilyId(u32::MAX));
        let mut hud = HudView {
            registry,
            family,
            context: LayoutContext::new(),
            frame: Frame {
                width: 0.0,
                height: 0.0,
                scale: 0.0,
            },
            score: Counter::new(SCORE_SIZE),
            accuracy: Counter::new(ACCURACY_SIZE),
            combo: Counter::new(COMBO_SIZE),
            skip_label: skip_label.to_owned(),
            skip: Layout::new(),
            warm: [Layout::new(), Layout::new()],
        };
        let style = hud.style(1.0);
        for counter in [&mut hud.score, &mut hud.accuracy, &mut hud.combo] {
            hud.context
                .layout_into(&hud.registry, WARM_DIGITS, &style, &mut counter.layout);
            hud.context
                .layout_into(&hud.registry, WARM_TEXT, &style, &mut counter.layout);
        }
        let [text, digits] = &mut hud.warm;
        hud.context
            .layout_into(&hud.registry, WARM_TEXT, &style, text);
        hud.context
            .layout_into(&hud.registry, WARM_DIGITS, &style, digits);
        hud.context
            .layout_into(&hud.registry, &hud.skip_label, &style, &mut hud.skip);
        hud
    }

    fn style(&self, size: f32) -> TextStyle {
        TextStyle::new(self.family, WEIGHT, size)
    }

    /// Asks `glyphs` for every glyph the HUD shows. Returns how many are not ready yet;
    /// repeat with [`GlyphCache::process`] in between until none are, before the play
    /// starts.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        self.warm
            .iter()
            .chain([&self.skip])
            .flat_map(Layout::glyphs)
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), slam_text::Glyph::Pending))
            .count()
    }

    /// Lays out what changed for the HUD `values` on a window of `width` × `height`
    /// pixels. Allocation-free.
    pub fn update(&mut self, width: f32, height: f32, values: &HudValues) {
        let frame = Frame {
            width,
            height,
            scale: height / REFERENCE_HEIGHT,
        };
        if frame != self.frame {
            self.frame = frame;
            for counter in [&mut self.score, &mut self.accuracy, &mut self.combo] {
                counter.shown = None;
            }
            let style = self.style(SKIP_SIZE * frame.scale);
            self.context
                .layout_into(&self.registry, &self.skip_label, &style, &mut self.skip);
        }
        let score = values.score.max(0) as u64;
        // Hundredths of a percent as shown, so that a change of a smaller digit does not
        // lay out the same text again.
        let accuracy = format_accuracy(values.accuracy);
        let accuracy_key = accuracy
            .as_str()
            .bytes()
            .filter(u8::is_ascii_digit)
            .fold(0u64, |key, d| key * 10 + u64::from(d - b'0'));
        let scale = self.frame.scale;
        let mut set = |counter: &mut Counter, key: u64, text: NumberText| {
            if counter.shown == Some(key) {
                return;
            }
            counter.shown = Some(key);
            counter.text = text;
            let style = TextStyle::new(self.family, WEIGHT, counter.size * scale);
            self.context.layout_into(
                &self.registry,
                counter.text.as_str(),
                &style,
                &mut counter.layout,
            );
        };
        set(&mut self.score, score, format_score(values.score));
        set(&mut self.accuracy, accuracy_key, accuracy);
        set(
            &mut self.combo,
            u64::from(values.combo),
            format_combo(values.combo),
        );
    }

    /// The texts of the counters as last laid out: score, accuracy, combo.
    pub fn texts(&self) -> [&str; 3] {
        [
            self.score.text.as_str(),
            self.accuracy.text.as_str(),
            self.combo.text.as_str(),
        ]
    }

    /// Writes the HUD's rectangles for `snapshot`, tinting `white` (an opaque white
    /// image): the health bar, the skip button and the click-to-resume target placed by
    /// the playfield `layout`. Allocation-free once `batch` has room.
    pub fn push_shapes(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        snapshot: &PlaySnapshot,
        white: AtlasRegion,
    ) {
        let Frame {
            width,
            height,
            scale,
        } = self.frame;
        let mut rect = |position: [f32; 2], size: [f32; 2], colour: Color| {
            let mut sprite = Sprite::new(white.texture, white.uv, position, size);
            sprite.colour = colour;
            batch.push(&sprite);
        };
        let margin = MARGIN * scale;
        let bar = [HEALTH_BAR[0] * scale, HEALTH_BAR[1] * scale];
        rect([margin, margin], bar, SHADE);
        let health = snapshot.health.clamp(0.0, 1.0) as f32;
        rect([margin, margin], [bar[0] * health, bar[1]], TEXT);

        if snapshot.skip_available {
            let (centre, size) = self.skip_button(width, height, scale);
            rect(
                [centre[0] - size[0] / 2.0, centre[1] - size[1] / 2.0],
                size,
                SHADE,
            );
        }
        if snapshot.state == PlayState::Resuming {
            let centre = layout.to_window(snapshot.resume_target);
            let side = RESUME_CURSOR_SIZE * layout.scale();
            rect(
                [centre.x - side / 2.0, centre.y - side / 2.0],
                [side, side],
                RESUME,
            );
        }
    }

    /// Writes the HUD's text for `snapshot` into a batch drawn with the MSDF shader.
    /// Glyphs not ready yet are skipped and counted. Allocation-free once `batch` has
    /// room.
    pub fn push_text(
        &self,
        batch: &mut SpriteBatch,
        snapshot: &PlaySnapshot,
        glyphs: &mut GlyphCache,
    ) -> TextStats {
        let Frame {
            width,
            height,
            scale,
        } = self.frame;
        let margin = MARGIN * scale;
        let mut stats = TextStats::default();
        let mut text = |layout: &Layout, position: [f32; 2]| {
            let s = push_layout(batch, layout, position, TEXT, glyphs);
            stats.drawn += s.drawn;
            stats.pending += s.pending;
        };
        let score = &self.score.layout;
        text(score, [width - margin - score.width(), margin]);
        let accuracy = &self.accuracy.layout;
        text(
            accuracy,
            [width - margin - accuracy.width(), margin + score.height()],
        );
        let combo = &self.combo.layout;
        text(combo, [margin, height - margin - combo.height()]);
        if snapshot.skip_available {
            let (centre, _) = self.skip_button(width, height, scale);
            let skip = &self.skip;
            text(
                skip,
                [
                    centre[0] - skip.width() / 2.0,
                    centre[1] - skip.height() / 2.0,
                ],
            );
        }
        stats
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/SkipOverlay.cs (Position)
    /// Centre and size of the skip button: at 70 % of the height, as lazer's overlay.
    fn skip_button(&self, width: f32, height: f32, scale: f32) -> ([f32; 2], [f32; 2]) {
        (
            [width / 2.0, height * 0.7],
            [SKIP_BUTTON[0] * scale, SKIP_BUTTON[1] * scale],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_has_six_digits_at_least() {
        assert_eq!(format_score(0).as_str(), "000000");
        assert_eq!(format_score(1234).as_str(), "001234");
        assert_eq!(format_score(1_000_000).as_str(), "1000000");
        assert_eq!(format_score(i64::MAX).as_str(), "9223372036854775807");
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/NonVisual/FormatUtilsTest.cs (TestAccuracyFormatting)
    #[test]
    fn accuracy_is_rounded_down_to_two_decimals() {
        for (accuracy, text) in [
            (0.0, "0.00%"),
            (0.01, "1.00%"),
            (0.9899, "98.99%"),
            (0.989999, "98.99%"),
            (0.99, "99.00%"),
            (0.9999, "99.99%"),
            (0.999999, "99.99%"),
            (1.0, "100.00%"),
        ] {
            assert_eq!(format_accuracy(accuracy).as_str(), text, "{accuracy}");
        }
    }

    #[test]
    fn accuracy_out_of_range_does_not_panic() {
        assert_eq!(format_accuracy(f64::NAN).as_str(), "0.00%");
        assert_eq!(format_accuracy(-0.5).as_str(), "0.00%");
    }

    #[test]
    fn combo_has_an_x() {
        assert_eq!(format_combo(0).as_str(), "0x");
        assert_eq!(format_combo(u32::MAX).as_str(), "4294967295x");
    }

    #[test]
    fn counters_lay_out_their_values() {
        let mut hud = HudView::new(Arc::new(FontRegistry::with_builtin()), "Skip");
        hud.update(
            1280.0,
            720.0,
            &HudValues {
                score: 12_345,
                accuracy: 0.98765,
                combo: 42,
            },
        );
        assert_eq!(hud.texts(), ["012345", "98.76%", "42x"]);
        assert_eq!(hud.score.layout.glyphs().len(), 6);
        assert_eq!(hud.combo.layout.glyphs().len(), 3);
        // Text scales with the window height.
        let font_size = hud.score.layout.font_size();
        assert!((font_size - SCORE_SIZE * 720.0 / REFERENCE_HEIGHT).abs() < 1e-4);
        hud.update(1280.0, 1536.0, &HudValues::default());
        assert_eq!(hud.texts(), ["000000", "100.00%", "0x"]);
        assert!((hud.score.layout.font_size() - SCORE_SIZE * 2.0).abs() < 1e-4);
    }
}
